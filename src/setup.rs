use crate::config::Config;
use dialoguer::{Confirm, Input, Password};
use std::path::PathBuf;

/// Default config file path
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("webdav-sync")
        .join("webdav-sync.toml")
}

/// Interactive setup mode
pub fn run_setup() -> anyhow::Result<()> {
    println!("=== WebDAV Sync Setup ===\n");

    let local_dir: String = Input::new()
        .with_prompt("Local directory to sync")
        .default(dirs::home_dir().unwrap_or_default().display().to_string())
        .interact_text()?;

    let local_dir = PathBuf::from(&local_dir);
    if !local_dir.exists() {
        println!("Directory does not exist: {}", local_dir.display());
        if Confirm::new()
            .with_prompt("Create directory?")
            .default(true)
            .interact()?
        {
            std::fs::create_dir_all(&local_dir)?;
        } else {
            anyhow::bail!("Directory does not exist");
        }
    }

    let webdav_url: String = Input::new()
        .with_prompt("WebDAV URL (e.g., https://example.com/dav/sync/)")
        .interact_text()?;

    let username: String = Input::new()
        .with_prompt("Username")
        .allow_empty(true)
        .interact_text()?;

    let password: String = Password::new()
        .with_prompt("Password")
        .allow_empty_password(true)
        .interact()?;

    let sync_interval: u64 = Input::new()
        .with_prompt("Sync interval (seconds)")
        .default(60)
        .interact_text()?;

    let exclude_input: String = Input::new()
        .with_prompt("Exclusions (comma-separated glob patterns)")
        .default(".git, *.tmp, .DS_Store".to_string())
        .interact_text()?;

    let exclude: Vec<String> = exclude_input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let config = Config {
        local_dir,
        webdav_url,
        username,
        password,
        sync_interval_secs: sync_interval,
        timeout_secs: 30,
        max_retries: 3,
        exclude,
        verbose: false,
    };

    let config_path = default_config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let toml_content = toml::to_string_pretty(&config)?;
    std::fs::write(&config_path, toml_content)?;

    println!("\nConfiguration saved: {}", config_path.display());

    if Confirm::new()
        .with_prompt("Create systemd user service for auto-start?")
        .default(true)
        .interact()?
    {
        create_systemd_service()?;
    }

    Ok(())
}

/// Create systemd user service
fn create_systemd_service() -> anyhow::Result<()> {
    let service_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("systemd")
        .join("user");

    std::fs::create_dir_all(&service_dir)?;

    let service_path = service_dir.join("webdav-sync.service");

    let exe_path =
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/local/bin/webdav-sync"));

    let service_content = format!(
        "[Unit]\n\
         Description=WebDAV Sync Service\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} --config {}\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe_path.display(),
        default_config_path().display()
    );

    std::fs::write(&service_path, service_content)?;

    println!("Systemd service created: {}", service_path.display());
    println!("\nTo enable auto-start, run:");
    println!("  systemctl --user daemon-reload");
    println!("  systemctl --user enable --now webdav-sync.service");

    Ok(())
}
