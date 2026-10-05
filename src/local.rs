use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::config::Config;
use crate::metadata::{local_metadata, FileMetadata};

/// Scan local directory for files and empty directories
pub fn scan_local(dir: &Path, config: &Config) -> anyhow::Result<Vec<FileMetadata>> {
    let mut files = Vec::new();
    let mut all_dirs: Vec<(PathBuf, String)> = Vec::new();

    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_excluded(&e.path().to_string_lossy(), &config.exclude))
    {
        let entry = entry?;
        let path = entry.path();
        let rel_path = path.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");

        if is_excluded(&rel_path, &config.exclude) {
            continue;
        }

        if entry.file_type().is_dir() {
            all_dirs.push((path.to_path_buf(), rel_path));
            continue;
        }

        match local_metadata(path, &rel_path) {
            Ok(meta) => files.push(meta),
            Err(e) => {
                tracing::warn!("Failed to get metadata for {}: {}", path.display(), e);
            }
        }
    }

    // Find empty directories (no files inside)
    for (dir_path, rel_path) in all_dirs {
        if is_empty_dir(&dir_path) {
            match local_metadata(&dir_path, &rel_path) {
                Ok(meta) => files.push(meta),
                Err(e) => {
                    tracing::warn!(
                        "Failed to get metadata for dir {}: {}",
                        dir_path.display(),
                        e
                    );
                }
            }
        }
    }

    Ok(files)
}

/// Check if directory is empty (no files inside, recursively)
fn is_empty_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_file() {
                    return false;
                }
                if entry_path.is_dir() && !is_empty_dir(&entry_path) {
                    return false;
                }
            }
            true
        }
        Err(_) => false,
    }
}

/// Check if path matches exclusion patterns
fn is_excluded(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| {
        if glob_match(p, path) {
            return true;
        }
        if path.starts_with(p.trim_end_matches('/')) {
            return true;
        }
        false
    })
}

/// Simple glob pattern matching
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" {
        return !text.contains('/');
    }
    glob_match_inner(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_inner(p: &[u8], t: &[u8]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }

    match p[0] {
        b'*' => {
            if p.len() >= 2 && p[1] == b'*' {
                let rest = &p[2..];
                let rest = if !rest.is_empty() && rest[0] == b'/' {
                    &rest[1..]
                } else {
                    rest
                };
                for i in 0..=t.len() {
                    if glob_match_inner(rest, &t[i..]) {
                        return true;
                    }
                }
                false
            } else {
                let rest = &p[1..];
                for i in 0..=t.len() {
                    if i > 0 && t[i - 1] == b'/' {
                        break;
                    }
                    if glob_match_inner(rest, &t[i..]) {
                        return true;
                    }
                }
                false
            }
        }
        b'?' => {
            if t.is_empty() || t[0] == b'/' {
                false
            } else {
                glob_match_inner(&p[1..], &t[1..])
            }
        }
        c => {
            if t.is_empty() || t[0] != c {
                false
            } else {
                glob_match_inner(&p[1..], &t[1..])
            }
        }
    }
}

/// Ensure directory exists
pub fn ensure_dir(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

/// Remove file
pub fn remove_file(path: &Path) -> anyhow::Result<()> {
    std::fs::remove_file(path)?;
    Ok(())
}

/// Remove empty directory
pub fn remove_dir(path: &Path) -> anyhow::Result<()> {
    std::fs::remove_dir(path)?;
    Ok(())
}

/// Atomic file write
pub fn write_file_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Get state file path
pub fn state_file_path(config: &Config) -> PathBuf {
    config.local_dir.join(".webdav-sync-state.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_exact() {
        assert!(glob_match("file.txt", "file.txt"));
        assert!(!glob_match("file.txt", "other.txt"));
    }

    #[test]
    fn test_glob_match_star() {
        assert!(glob_match("*.txt", "file.txt"));
        assert!(glob_match("*.txt", "a.txt"));
        assert!(!glob_match("*.txt", "file.rs"));
        assert!(!glob_match("*.txt", "dir/file.txt"));
    }

    #[test]
    fn test_glob_match_double_star() {
        assert!(glob_match("**/*.txt", "file.txt"));
        assert!(glob_match("**/*.txt", "dir/file.txt"));
        assert!(glob_match("**/*.txt", "a/b/c/file.txt"));
        assert!(!glob_match("**/*.txt", "file.rs"));
    }

    #[test]
    fn test_glob_match_question_mark() {
        assert!(glob_match("file?.txt", "file1.txt"));
        assert!(glob_match("file?.txt", "fileA.txt"));
        assert!(!glob_match("file?.txt", "file10.txt"));
        assert!(!glob_match("file?.txt", "file.txt"));
    }

    #[test]
    fn test_glob_match_directory_prefix() {
        assert!(glob_match("build/*", "build/output.txt"));
        assert!(glob_match("build/*", "build/subdir"));
        assert!(!glob_match("build/*", "src/build/output.txt"));
    }

    #[test]
    fn test_is_excluded() {
        let patterns = vec![".git".to_string(), "*.tmp".to_string(), "build".to_string()];

        assert!(is_excluded(".git", &patterns));
        assert!(is_excluded(".git/config", &patterns));
        assert!(is_excluded("file.tmp", &patterns));
        assert!(is_excluded("build", &patterns));
        assert!(is_excluded("build/output.txt", &patterns));
        assert!(!is_excluded("src/main.rs", &patterns));
        assert!(!is_excluded("README.md", &patterns));
    }

    #[test]
    fn test_is_excluded_empty_patterns() {
        let patterns: Vec<String> = vec![];
        assert!(!is_excluded("anything.txt", &patterns));
    }

    #[test]
    fn test_glob_match_edge_cases() {
        assert!(glob_match("*", "file.txt"));
        assert!(!glob_match("*", "dir/file.txt"));
        assert!(glob_match("**", "anything"));
        assert!(glob_match("**", "a/b/c"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "something"));
    }

    #[test]
    fn test_is_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let empty_dir = temp.path().join("empty");
        std::fs::create_dir(&empty_dir).unwrap();
        assert!(is_empty_dir(&empty_dir));

        let non_empty_dir = temp.path().join("non_empty");
        std::fs::create_dir(&non_empty_dir).unwrap();
        std::fs::write(non_empty_dir.join("file.txt"), "content").unwrap();
        assert!(!is_empty_dir(&non_empty_dir));

        assert!(!is_empty_dir(temp.path()));
    }
}
