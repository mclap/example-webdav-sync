use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Record of a sync operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRecord {
    pub rel_path: String,
    pub direction: SyncDirection,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncDirection {
    Upload,
    Download,
    ConflictResolved,
}

/// Sync state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncState {
    pub last_sync: Option<DateTime<Utc>>,
    pub recent_files: Vec<SyncRecord>,
}

impl SyncState {
    /// Load state from file
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let state: SyncState = toml::from_str(&content)?;
        Ok(state)
    }

    /// Save state to file
    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Add sync record
    pub fn add_record(&mut self, rel_path: String, direction: SyncDirection) {
        self.last_sync = Some(Utc::now());
        self.recent_files.push(SyncRecord {
            rel_path,
            direction,
            timestamp: Utc::now(),
        });
        if self.recent_files.len() > 50 {
            self.recent_files.remove(0);
        }
    }
}

/// Get state file path
pub fn state_file_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("webdav-sync");
    config_dir.join("state.toml")
}
