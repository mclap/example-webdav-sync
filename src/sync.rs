use anyhow::Context;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::local;
use crate::metadata::FileMetadata;
use crate::state::{state_db_path, SyncDirection, SyncState};
use crate::webdav::WebDavClient;

/// Sync result
#[derive(Debug, Default)]
pub struct SyncResult {
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted_local: usize,
    pub deleted_remote: usize,
    pub conflicts_resolved: usize,
    pub errors: Vec<String>,
}

/// Sync action
#[derive(Debug, Clone)]
enum SyncAction {
    /// Upload local file to server
    Upload(String),
    /// Download file from server
    Download(String),
    /// Conflict — needs resolution
    Conflict(String, FileMetadata, FileMetadata),
    /// Create directory on server
    CreateDir(String),
    /// Create directory locally
    CreateLocalDir(String),
}

/// Syncer
pub struct Syncer {
    config: Config,
    client: WebDavClient,
    debounce_tx: mpsc::Sender<()>,
    debounce_rx: mpsc::Receiver<()>,
}

impl Syncer {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let client = WebDavClient::new(&config)?;
        let (tx, rx) = mpsc::channel(100);

        Ok(Self {
            config,
            client,
            debounce_tx: tx,
            debounce_rx: rx,
        })
    }

    /// Run sync in background
    pub async fn run(self) -> anyhow::Result<()> {
        let Syncer {
            config,
            client,
            debounce_tx,
            mut debounce_rx,
        } = self;

        let watcher_handle = {
            let tx = debounce_tx.clone();
            let local_dir = config.local_dir.clone();
            tokio::spawn(async move {
                if let Err(e) = watch_local(tx, &local_dir).await {
                    tracing::error!("Watcher error: {}", e);
                }
            })
        };

        let sync_handle = {
            let config = config.clone();
            let client = client.clone();
            let state_path = state_db_path();
            tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(Duration::from_secs(config.sync_interval_secs));
                loop {
                    interval.tick().await;
                    tracing::info!("Running scheduled sync...");
                    if let Err(e) = perform_full_sync(&config, &client, &state_path).await {
                        tracing::error!("Scheduled sync error: {}", e);
                    }
                }
            })
        };

        let debounce_handle = {
            let config = config.clone();
            let client = client.clone();
            let state_path = state_db_path();
            tokio::spawn(async move {
                while debounce_rx.recv().await.is_some() {
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    tracing::info!("Changes detected, running sync...");
                    if let Err(e) = perform_full_sync(&config, &client, &state_path).await {
                        tracing::error!("Sync error: {}", e);
                    }
                }
            })
        };

        tracing::info!("Initial sync...");
        perform_full_sync(&config, &client, &state_db_path()).await?;

        let _ = tokio::join!(watcher_handle, sync_handle, debounce_handle);

        Ok(())
    }
}

/// Watch local directory for changes
async fn watch_local(tx: mpsc::Sender<()>, dir: &std::path::Path) -> anyhow::Result<()> {
    use notify::RecursiveMode;
    use notify_debouncer_mini::new_debouncer;

    let mut debouncer = new_debouncer(
        Duration::from_millis(200),
        move |res: notify_debouncer_mini::DebounceEventResult| {
            if res.is_ok() {
                let _ = tx.blocking_send(());
            }
        },
    )?;

    debouncer.watcher().watch(dir, RecursiveMode::Recursive)?;

    tracing::info!("Watcher started for: {}", dir.display());

    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

/// Perform full sync
pub async fn perform_full_sync(
    config: &Config,
    client: &WebDavClient,
    state_path: &std::path::Path,
) -> anyhow::Result<SyncResult> {
    let start = Instant::now();
    let mut result = SyncResult::default();

    let mut state = SyncState::open(&state_path.to_path_buf())?;

    tracing::debug!("Scanning local directory...");
    let local_files = local::scan_local(&config.local_dir, config)?;
    let local_map: HashMap<String, FileMetadata> = local_files
        .iter()
        .map(|f| (f.rel_path.clone(), f.clone()))
        .collect();

    tracing::debug!("Fetching file list from WebDAV...");
    let remote_files = client.list_files().await?;
    let remote_map: HashMap<String, FileMetadata> = remote_files
        .iter()
        .map(|f| (f.rel_path.clone(), f.clone()))
        .collect();

    let actions = compute_actions(&local_map, &remote_map, &state);
    tracing::info!(
        "Computed actions: {} (upload: {}, download: {}, conflicts: {}, mkdir: {})",
        actions.len(),
        actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Upload(_)))
            .count(),
        actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Download(_)))
            .count(),
        actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Conflict(_, _, _)))
            .count(),
        actions
            .iter()
            .filter(|a| matches!(a, SyncAction::CreateDir(_) | SyncAction::CreateLocalDir(_)))
            .count(),
    );

    for action in actions {
        match execute_action(action, config, client, &mut result, &mut state).await {
            Ok(_) => {}
            Err(e) => {
                tracing::error!("Action execution error: {}", e);
                result.errors.push(e.to_string());
            }
        }
    }

    state.save(&state_path.to_path_buf())?;

    let elapsed = start.elapsed();
    tracing::info!(
        "Sync completed in {:?}: uploaded {}, downloaded {}, deleted local {}, deleted remote {}, conflicts {}, errors {}",
        elapsed,
        result.uploaded,
        result.downloaded,
        result.deleted_local,
        result.deleted_remote,
        result.conflicts_resolved,
        result.errors.len()
    );

    Ok(result)
}

/// Compute required actions
fn compute_actions(
    local: &HashMap<String, FileMetadata>,
    remote: &HashMap<String, FileMetadata>,
    state: &SyncState,
) -> Vec<SyncAction> {
    let mut actions = Vec::new();
    let all_keys: HashSet<&String> = local.keys().chain(remote.keys()).collect();

    for key in all_keys {
        if key.is_empty() || key == "/" {
            continue;
        }
        match (local.get(key), remote.get(key)) {
            (Some(local_meta), None) => {
                if local_meta.is_dir {
                    actions.push(SyncAction::CreateDir(key.clone()));
                } else {
                    actions.push(SyncAction::Upload(key.clone()));
                }
            }
            (None, Some(remote_meta)) => {
                if remote_meta.is_dir {
                    actions.push(SyncAction::CreateLocalDir(key.clone()));
                } else {
                    actions.push(SyncAction::Download(key.clone()));
                }
            }
            (Some(local_meta), Some(remote_meta)) => {
                if local_meta.differs_from(remote_meta) {
                    // Check if already synced via state
                    if let Some(last) = state.get_synced_file(key) {
                        if !last.differs_from(local_meta) && !last.differs_from(remote_meta) {
                            continue;
                        }
                    }
                    actions.push(SyncAction::Conflict(
                        key.clone(),
                        local_meta.clone(),
                        remote_meta.clone(),
                    ));
                }
            }
            _ => {}
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn make_meta(rel_path: &str, size: u64, timestamp: i64) -> FileMetadata {
        FileMetadata {
            rel_path: rel_path.to_string(),
            size,
            modified: Utc.timestamp_opt(timestamp, 0).unwrap(),
            etag: None,
            content_hash: None,
            is_dir: false,
        }
    }

    fn test_state() -> SyncState {
        let temp = tempfile::tempdir().unwrap();
        SyncState::open(&temp.path().join("test.db")).unwrap()
    }

    #[test]
    fn test_compute_actions_upload_new_local_file() {
        let mut local = HashMap::new();
        local.insert(
            "new_file.txt".to_string(),
            make_meta("new_file.txt", 100, 1700000000),
        );

        let remote = HashMap::new();
        let state = test_state();

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::Upload(p) if p == "new_file.txt"));
    }

    #[test]
    fn test_compute_actions_download_new_remote_file() {
        let local = HashMap::new();
        let mut remote = HashMap::new();
        remote.insert(
            "new_file.txt".to_string(),
            make_meta("new_file.txt", 100, 1700000000),
        );

        let state = test_state();

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::Download(p) if p == "new_file.txt"));
    }

    #[test]
    fn test_compute_actions_no_changes() {
        let mut local = HashMap::new();
        local.insert(
            "file.txt".to_string(),
            make_meta("file.txt", 100, 1700000000),
        );

        let mut remote = HashMap::new();
        remote.insert(
            "file.txt".to_string(),
            make_meta("file.txt", 100, 1700000000),
        );

        let state = test_state();

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 0);
    }

    #[test]
    fn test_compute_actions_conflict() {
        let mut local = HashMap::new();
        local.insert(
            "file.txt".to_string(),
            make_meta("file.txt", 100, 1700000000),
        );

        let mut remote = HashMap::new();
        remote.insert(
            "file.txt".to_string(),
            make_meta("file.txt", 200, 1700000001),
        );

        let state = test_state();

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::Conflict(_, _, _)));
    }

    #[test]
    fn test_compute_actions_multiple_files() {
        let mut local = HashMap::new();
        local.insert(
            "local_only.txt".to_string(),
            make_meta("local_only.txt", 100, 1700000000),
        );
        local.insert(
            "same.txt".to_string(),
            make_meta("same.txt", 100, 1700000000),
        );
        local.insert(
            "conflict.txt".to_string(),
            make_meta("conflict.txt", 100, 1700000000),
        );

        let mut remote = HashMap::new();
        remote.insert(
            "remote_only.txt".to_string(),
            make_meta("remote_only.txt", 100, 1700000000),
        );
        remote.insert(
            "same.txt".to_string(),
            make_meta("same.txt", 100, 1700000000),
        );
        remote.insert(
            "conflict.txt".to_string(),
            make_meta("conflict.txt", 200, 1700000001),
        );

        let state = test_state();

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 3);

        let uploads: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Upload(_)))
            .collect();
        let downloads: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Download(_)))
            .collect();
        let conflicts: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, SyncAction::Conflict(_, _, _)))
            .collect();

        assert_eq!(uploads.len(), 1);
        assert_eq!(downloads.len(), 1);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn test_compute_actions_empty() {
        let local = HashMap::new();
        let remote = HashMap::new();
        let state = test_state();
        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 0);
    }

    #[test]
    fn test_conflict_resolution_local_newer() {
        let local = make_meta("file.txt", 100, 1700000002);
        let remote = make_meta("file.txt", 100, 1700000001);

        assert!(local.is_newer_than(&remote));
    }

    #[test]
    fn test_conflict_resolution_remote_newer() {
        let local = make_meta("file.txt", 100, 1700000001);
        let remote = make_meta("file.txt", 100, 1700000002);

        assert!(remote.is_newer_than(&local));
    }

    #[test]
    fn test_diff_detection_by_size() {
        let local = make_meta("file.txt", 100, 1700000000);
        let remote = make_meta("file.txt", 200, 1700000000);

        assert!(local.differs_from(&remote));
    }

    #[test]
    fn test_diff_detection_by_time() {
        let local = make_meta("file.txt", 100, 1700000000);
        let remote = make_meta("file.txt", 100, 1700000001);

        assert!(local.differs_from(&remote));
    }

    #[test]
    fn test_no_diff_same_file() {
        let local = make_meta("file.txt", 100, 1700000000);
        let remote = make_meta("file.txt", 100, 1700000000);

        assert!(!local.differs_from(&remote));
    }

    #[test]
    fn test_compute_actions_create_dir() {
        let mut local = HashMap::new();
        let mut dir_meta = make_meta("emptydir", 0, 1700000000);
        dir_meta.is_dir = true;
        local.insert("emptydir".to_string(), dir_meta);

        let remote = HashMap::new();
        let state = test_state();

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::CreateDir(p) if p == "emptydir"));
    }

    #[test]
    fn test_compute_actions_create_local_dir() {
        let local = HashMap::new();
        let mut remote = HashMap::new();
        let mut dir_meta = make_meta("remotedir", 0, 1700000000);
        dir_meta.is_dir = true;
        remote.insert("remotedir".to_string(), dir_meta);

        let state = test_state();

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::CreateLocalDir(p) if p == "remotedir"));
    }

    #[test]
    fn test_compute_actions_skip_already_synced() {
        let mut local = HashMap::new();
        local.insert(
            "file.txt".to_string(),
            make_meta("file.txt", 100, 1700000000),
        );

        let mut remote = HashMap::new();
        remote.insert(
            "file.txt".to_string(),
            make_meta("file.txt", 100, 1700000000),
        );

        let state = test_state();
        state.upsert_synced_file(&make_meta("file.txt", 100, 1700000000));

        let actions = compute_actions(&local, &remote, &state);
        assert_eq!(actions.len(), 0);
    }
}

/// Execute sync action
async fn execute_action(
    action: SyncAction,
    config: &Config,
    client: &WebDavClient,
    result: &mut SyncResult,
    state: &mut SyncState,
) -> anyhow::Result<()> {
    match action {
        SyncAction::Upload(rel_path) => {
            let local_path = config.local_dir.join(&rel_path);
            let data = tokio::fs::read(&local_path)
                .await
                .with_context(|| format!("Read file: {}", rel_path))?;

            client.upload(&rel_path, data.into()).await?;
            result.uploaded += 1;
            tracing::info!("Uploaded: {}", rel_path);

            if let Ok(meta) = crate::metadata::local_metadata(&local_path, &rel_path) {
                state.upsert_synced_file(&meta);
            }
            state.add_record(rel_path, SyncDirection::Upload);
        }

        SyncAction::Download(rel_path) => {
            let data = client.download(&rel_path).await?;
            let local_path = config.local_dir.join(&rel_path);
            local::write_file_atomic(&local_path, &data)?;
            result.downloaded += 1;
            tracing::info!("Downloaded: {}", rel_path);

            if let Ok(meta) = crate::metadata::local_metadata(&local_path, &rel_path) {
                state.upsert_synced_file(&meta);
            }
            state.add_record(rel_path, SyncDirection::Download);
        }

        SyncAction::Conflict(rel_path, local_meta, remote_meta) => {
            if local_meta.is_newer_than(&remote_meta) {
                let local_path = config.local_dir.join(&rel_path);
                let data = tokio::fs::read(&local_path)
                    .await
                    .with_context(|| format!("Read file: {}", rel_path))?;

                client.upload(&rel_path, data.into()).await?;
                result.conflicts_resolved += 1;
                tracing::info!(
                    "Conflict resolved (local newer): {} (local: {}, remote: {})",
                    rel_path,
                    local_meta.modified,
                    remote_meta.modified
                );

                if let Ok(meta) = crate::metadata::local_metadata(&local_path, &rel_path) {
                    state.upsert_synced_file(&meta);
                }
                state.add_record(rel_path, SyncDirection::ConflictResolved);
            } else {
                let data = client.download(&rel_path).await?;
                let local_path = config.local_dir.join(&rel_path);
                local::write_file_atomic(&local_path, &data)?;
                result.conflicts_resolved += 1;
                tracing::info!(
                    "Conflict resolved (remote newer): {} (local: {}, remote: {})",
                    rel_path,
                    local_meta.modified,
                    remote_meta.modified
                );

                if let Ok(meta) = crate::metadata::local_metadata(&local_path, &rel_path) {
                    state.upsert_synced_file(&meta);
                }
                state.add_record(rel_path, SyncDirection::ConflictResolved);
            }
        }

        SyncAction::CreateDir(rel_path) => {
            client.create_dir(&rel_path).await?;
            result.uploaded += 1;
            tracing::info!("Created directory on server: {}", rel_path);

            let local_path = config.local_dir.join(&rel_path);
            if let Ok(meta) = crate::metadata::local_metadata(&local_path, &rel_path) {
                state.upsert_synced_file(&meta);
            }
            state.add_record(rel_path, SyncDirection::Upload);
        }

        SyncAction::CreateLocalDir(rel_path) => {
            let local_path = config.local_dir.join(&rel_path);
            local::ensure_dir(&local_path)?;
            result.downloaded += 1;
            tracing::info!("Created local directory: {}", rel_path);

            if let Ok(meta) = crate::metadata::local_metadata(&local_path, &rel_path) {
                state.upsert_synced_file(&meta);
            }
            state.add_record(rel_path, SyncDirection::Download);
        }
    }

    Ok(())
}
