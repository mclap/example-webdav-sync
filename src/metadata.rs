use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// File metadata for fast comparison
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMetadata {
    /// Relative path from sync root
    pub rel_path: String,
    /// File size in bytes
    pub size: u64,
    /// Last modification time (UTC)
    pub modified: DateTime<Utc>,
    /// ETag from WebDAV (if available)
    pub etag: Option<String>,
    /// Content hash (computed when needed)
    pub content_hash: Option<String>,
}

impl FileMetadata {
    /// Fast comparison: size + modification time
    /// Returns true if files potentially differ
    pub fn differs_from(&self, other: &FileMetadata) -> bool {
        if self.size != other.size {
            return true;
        }
        // Compare with second-level precision (WebDAV may have lower precision)
        let self_ts = self.modified.timestamp();
        let other_ts = other.modified.timestamp();
        self_ts != other_ts
    }

    /// Determines which file is newer
    pub fn is_newer_than(&self, other: &FileMetadata) -> bool {
        self.modified > other.modified
    }
}

/// Get local file metadata
pub fn local_metadata(path: &Path, rel_path: &str) -> anyhow::Result<FileMetadata> {
    let meta = std::fs::metadata(path)?;
    let modified: DateTime<Utc> = meta.modified()?.into();

    Ok(FileMetadata {
        rel_path: rel_path.to_string(),
        size: meta.len(),
        modified,
        etag: None,
        content_hash: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_meta(size: u64, timestamp: i64) -> FileMetadata {
        FileMetadata {
            rel_path: "test.txt".to_string(),
            size,
            modified: Utc.timestamp_opt(timestamp, 0).unwrap(),
            etag: None,
            content_hash: None,
        }
    }

    #[test]
    fn test_differs_same_file() {
        let a = make_meta(100, 1700000000);
        let b = make_meta(100, 1700000000);
        assert!(!a.differs_from(&b));
    }

    #[test]
    fn test_differs_different_size() {
        let a = make_meta(100, 1700000000);
        let b = make_meta(200, 1700000000);
        assert!(a.differs_from(&b));
    }

    #[test]
    fn test_differs_different_time() {
        let a = make_meta(100, 1700000000);
        let b = make_meta(100, 1700000001);
        assert!(a.differs_from(&b));
    }

    #[test]
    fn test_is_newer_than() {
        let older = make_meta(100, 1700000000);
        let newer = make_meta(100, 1700000001);
        assert!(newer.is_newer_than(&older));
        assert!(!older.is_newer_than(&newer));
    }

    #[test]
    fn test_is_newer_equal() {
        let a = make_meta(100, 1700000000);
        let b = make_meta(100, 1700000000);
        assert!(!a.is_newer_than(&b));
        assert!(!b.is_newer_than(&a));
    }
}
