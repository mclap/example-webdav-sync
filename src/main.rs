use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;
use webdav_sync::config::Config;
use webdav_sync::setup;
use webdav_sync::state::{state_file_path, SyncState};
use webdav_sync::status;
use webdav_sync::sync::{perform_full_sync, Syncer};
use webdav_sync::webdav::WebDavClient;

#[derive(Parser, Debug)]
#[command(
    name = "webdav-sync",
    about = "Bidirectional directory sync via WebDAV"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Run one sync and exit
    #[arg(short, long)]
    once: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Interactive setup
    Setup,
    /// Show service status
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Setup) => {
            setup::run_setup()?;
            return Ok(());
        }
        Some(Commands::Status) => {
            status::show_status()?;
            return Ok(());
        }
        None => {}
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let config_path = cli.config.unwrap_or_else(setup::default_config_path);

    if !config_path.exists() {
        eprintln!("Error: Config file not found: {}", config_path.display());
        eprintln!("Run 'webdav-sync setup' to create one.");
        std::process::exit(1);
    }

    let config = Config::load(&config_path)?;
    tracing::info!(
        "Sync: {} <-> {}",
        config.local_dir.display(),
        config.webdav_url
    );

    let state_path = state_file_path();
    let mut state = SyncState::load(&state_path)?;

    if cli.once {
        let client = WebDavClient::new(&config)?;
        let result = perform_full_sync(&config, &client).await?;

        update_state(&mut state, &result);
        state.save(&state_path)?;

        tracing::info!(
            "Sync completed: uploaded {}, downloaded {}, conflicts {}, errors {}",
            result.uploaded,
            result.downloaded,
            result.conflicts_resolved,
            result.errors.len()
        );
    } else {
        let syncer = Syncer::new(config)?;
        syncer.run().await?;
    }

    Ok(())
}

fn update_state(_state: &mut SyncState, result: &webdav_sync::sync::SyncResult) {
    let _ = result;
}
