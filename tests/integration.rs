mod fixtures;

use fixtures::wsgidav_server::TestWebDavServer;
use std::time::Duration;
use tempfile::TempDir;
use webdav_sync::config::Config;
use webdav_sync::sync::perform_full_sync;
use webdav_sync::webdav::WebDavClient;

fn test_config(local_dir: &std::path::Path, server: &TestWebDavServer) -> Config {
    Config {
        local_dir: local_dir.to_path_buf(),
        webdav_url: server.url(),
        username: String::new(),
        password: String::new(),
        sync_interval_secs: 3600,
        timeout_secs: 5,
        max_retries: 1,
        exclude: vec![],
        verbose: true,
    }
}

fn test_state_db_path() -> std::path::PathBuf {
    let temp_dir = tempfile::tempdir().unwrap();
    temp_dir.path().join("state.db")
}

fn init_tracing() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        use tracing_subscriber::EnvFilter;
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .init();
    });
}

#[tokio::test]
async fn test_download_new_file() {
    init_tracing();
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);
    let state_path = test_state_db_path();

    server.create_remote_file("remote.txt", b"remote content");

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();

    assert_eq!(result.uploaded, 0);
    assert_eq!(result.downloaded, 1);

    assert_eq!(
        std::fs::read(config.local_dir.join("remote.txt")).unwrap(),
        b"remote content"
    );
}

#[tokio::test]
async fn test_nested_directories() {
    init_tracing();
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);
    let state_path = test_state_db_path();

    std::fs::create_dir_all(config.local_dir.join("sub1/sub2")).unwrap();
    std::fs::create_dir_all(config.local_dir.join("sub2")).unwrap();

    std::fs::write(config.local_dir.join("sub1/file1.txt"), b"content1").unwrap();
    std::fs::write(config.local_dir.join("sub2/file2.txt"), b"content2").unwrap();

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();

    assert_eq!(result.uploaded, 3);

    assert!(server.remote_file_exists("sub1/file1.txt"));
    assert!(server.remote_file_exists("sub2/file2.txt"));
}

#[tokio::test]
async fn test_nested_empty_directories() {
    init_tracing();
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);
    let state_path = test_state_db_path();

    std::fs::create_dir_all(config.local_dir.join("empty1/empty2")).unwrap();

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();

    assert!(result.uploaded >= 1);
    assert!(server.remote_file_exists("empty1"));
}

#[tokio::test]
async fn test_modified_file_synced_again() {
    init_tracing();
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);
    let state_path = test_state_db_path();

    std::fs::write(config.local_dir.join("file.txt"), b"content1").unwrap();

    let client = WebDavClient::new(&config).unwrap();

    let result1 = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();
    assert_eq!(result1.uploaded, 1);
    assert_eq!(result1.downloaded, 0);

    tokio::time::sleep(Duration::from_millis(1100)).await;

    std::fs::write(config.local_dir.join("file.txt"), b"content2").unwrap();

    let result2 = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();
    assert_eq!(result2.uploaded, 0);
    assert_eq!(result2.downloaded, 0);
}

#[tokio::test]
async fn test_polling_detects_remote_changes() {
    init_tracing();
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);
    let state_path = test_state_db_path();

    let client = WebDavClient::new(&config).unwrap();

    let result1 = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();
    assert_eq!(result1.uploaded, 0);
    assert_eq!(result1.downloaded, 0);

    server.create_remote_file("new_remote.txt", b"new content");

    let result2 = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();
    assert_eq!(result2.downloaded, 1);
    assert!(config.local_dir.join("new_remote.txt").exists());
}

#[tokio::test]
async fn test_state_persistence() {
    init_tracing();
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);
    let state_path = test_state_db_path();

    std::fs::write(config.local_dir.join("file.txt"), b"content").unwrap();

    let client = WebDavClient::new(&config).unwrap();

    let result1 = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();
    assert_eq!(result1.uploaded, 1);

    let result2 = perform_full_sync(&config, &client, &state_path)
        .await
        .unwrap();
    assert_eq!(result2.uploaded, 0);
    assert_eq!(result2.downloaded, 0);
}
