use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Local directory to sync
    pub local_dir: PathBuf,
    /// WebDAV server URL (e.g., https://example.com/dav/sync/)
    pub webdav_url: String,
    /// WebDAV username
    pub username: String,
    /// WebDAV password
    pub password: String,
    /// Full sync interval in seconds
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64,
    /// HTTP request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Number of retry attempts on errors
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Exclusion patterns (glob patterns relative to root)
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Enable verbose logging
    #[serde(default)]
    pub verbose: bool,
}

fn default_sync_interval() -> u64 {
    60
}

fn default_timeout() -> u64 {
    30
}

fn default_retries() -> u32 {
    3
}

impl Config {
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.local_dir.exists() {
            anyhow::bail!(
                "Local directory does not exist: {}",
                self.local_dir.display()
            );
        }
        if !self.local_dir.is_dir() {
            anyhow::bail!("Path is not a directory: {}", self.local_dir.display());
        }
        if self.webdav_url.is_empty() {
            anyhow::bail!("WebDAV URL cannot be empty");
        }
        Ok(())
    }
}
