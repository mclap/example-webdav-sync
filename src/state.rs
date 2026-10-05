use crate::metadata::FileMetadata;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::path::PathBuf;

/// Record of a sync operation
#[derive(Debug, Clone)]
pub struct SyncRecord {
    pub rel_path: String,
    pub direction: SyncDirection,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum SyncDirection {
    Upload,
    Download,
    ConflictResolved,
}

/// Sync state backed by SQLite
pub struct SyncState {
    conn: Connection,
}

impl SyncState {
    /// Open or create state database
    pub fn open(path: &PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS synced_files (
                rel_path TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                modified TEXT NOT NULL,
                is_dir INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS sync_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                rel_path TEXT NOT NULL,
                direction TEXT NOT NULL,
                timestamp TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    /// Load state from file (compatibility with old API)
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        Self::open(path)
    }

    /// Save state to file (no-op for SQLite, kept for API compatibility)
    pub fn save(&self, _path: &PathBuf) -> anyhow::Result<()> {
        Ok(())
    }

    /// Add sync record
    pub fn add_record(&mut self, rel_path: String, direction: SyncDirection) {
        let dir = match direction {
            SyncDirection::Upload => "upload",
            SyncDirection::Download => "download",
            SyncDirection::ConflictResolved => "conflict",
        };
        let _ = self.conn.execute(
            "INSERT INTO sync_records (rel_path, direction, timestamp) VALUES (?1, ?2, ?3)",
            (&rel_path, dir, Utc::now().to_rfc3339()),
        );
    }

    /// Get synced file metadata
    pub fn get_synced_file(&self, rel_path: &str) -> Option<FileMetadata> {
        self.conn
            .query_row(
                "SELECT size, modified, is_dir FROM synced_files WHERE rel_path = ?1",
                [rel_path],
                |row| {
                    let size: i64 = row.get(0)?;
                    let modified: String = row.get(1)?;
                    let is_dir: i64 = row.get(2)?;
                    Ok(FileMetadata {
                        rel_path: rel_path.to_string(),
                        size: size as u64,
                        modified: DateTime::parse_from_rfc3339(&modified)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                        etag: None,
                        content_hash: None,
                        is_dir: is_dir != 0,
                    })
                },
            )
            .ok()
    }

    /// Insert or update synced file
    pub fn upsert_synced_file(&self, meta: &FileMetadata) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO synced_files (rel_path, size, modified, is_dir) VALUES (?1, ?2, ?3, ?4)",
            (
                &meta.rel_path,
                meta.size as i64,
                meta.modified.to_rfc3339(),
                if meta.is_dir { 1 } else { 0 },
            ),
        );
    }

    /// Remove synced file
    pub fn remove_synced_file(&self, rel_path: &str) {
        let _ = self
            .conn
            .execute("DELETE FROM synced_files WHERE rel_path = ?1", [rel_path]);
    }

    /// Get last sync time
    pub fn last_sync(&self) -> Option<DateTime<Utc>> {
        self.conn
            .query_row("SELECT MAX(timestamp) FROM sync_records", [], |row| {
                let ts: String = row.get(0)?;
                Ok(DateTime::parse_from_rfc3339(&ts)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()))
            })
            .ok()
    }

    /// Get recent sync records
    pub fn recent_records(&self, limit: usize) -> Vec<SyncRecord> {
        let mut stmt = match self.conn.prepare(
            "SELECT rel_path, direction, timestamp FROM sync_records ORDER BY id DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([limit], |row| {
            let rel_path: String = row.get(0)?;
            let direction: String = row.get(1)?;
            let timestamp: String = row.get(2)?;
            Ok((rel_path, direction, timestamp))
        });
        match rows {
            Ok(rows) => rows
                .flatten()
                .filter_map(|(rel_path, direction, timestamp)| {
                    let direction = match direction.as_str() {
                        "upload" => SyncDirection::Upload,
                        "download" => SyncDirection::Download,
                        _ => SyncDirection::ConflictResolved,
                    };
                    Some(SyncRecord {
                        rel_path,
                        direction,
                        timestamp: DateTime::parse_from_rfc3339(&timestamp)
                            .ok()?
                            .with_timezone(&Utc),
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// Get state database path
pub fn state_db_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("webdav-sync");
    config_dir.join("state.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_persistence() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("test.db");

        // Create state and add data
        {
            let mut state = SyncState::open(&db_path).unwrap();
            state.add_record("test.txt".to_string(), SyncDirection::Upload);
            state.upsert_synced_file(&FileMetadata {
                rel_path: "test.txt".to_string(),
                size: 100,
                modified: Utc::now(),
                etag: None,
                content_hash: None,
                is_dir: false,
            });
        }

        // Reopen and verify data persists
        {
            let state = SyncState::open(&db_path).unwrap();
            let meta = state.get_synced_file("test.txt").unwrap();
            assert_eq!(meta.size, 100);
            assert!(!meta.is_dir);

            let records = state.recent_records(10);
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].rel_path, "test.txt");
        }
    }
}
