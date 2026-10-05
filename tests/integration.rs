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

#[tokio::test]
async fn test_upload_new_file() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    std::fs::write(config.local_dir.join("hello.txt"), "hello world").unwrap();

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.uploaded, 1);
    assert_eq!(result.downloaded, 0);

    assert!(server.remote_file_exists("hello.txt"));
    assert_eq!(
        server.read_remote_file("hello.txt").unwrap(),
        b"hello world"
    );
}

#[tokio::test]
async fn test_download_new_file() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    server.create_remote_file("remote.txt", b"remote content");

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.uploaded, 0);
    assert_eq!(result.downloaded, 1);

    assert_eq!(
        std::fs::read(config.local_dir.join("remote.txt")).unwrap(),
        b"remote content"
    );
}

#[tokio::test]
async fn test_no_sync_when_identical() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    let content = b"identical content";
    std::fs::write(config.local_dir.join("same.txt"), content).unwrap();
    server.create_remote_file("same.txt", content);

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.uploaded, 0);
    assert_eq!(result.downloaded, 0);
    assert_eq!(result.conflicts_resolved, 0);
}

#[tokio::test]
async fn test_conflict_local_newer() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    server.create_remote_file("conflict.txt", b"old content");

    tokio::time::sleep(Duration::from_millis(1100)).await;

    std::fs::write(config.local_dir.join("conflict.txt"), b"new content").unwrap();

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.conflicts_resolved, 1);

    assert_eq!(
        server.read_remote_file("conflict.txt").unwrap(),
        b"new content"
    );
}

#[tokio::test]
async fn test_conflict_remote_newer() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    std::fs::write(config.local_dir.join("conflict.txt"), b"old content").unwrap();

    tokio::time::sleep(Duration::from_millis(1100)).await;

    server.create_remote_file("conflict.txt", b"new content");

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.conflicts_resolved, 1);

    assert_eq!(
        std::fs::read(config.local_dir.join("conflict.txt")).unwrap(),
        b"new content"
    );
}

#[tokio::test]
async fn test_nested_directories() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    std::fs::create_dir_all(config.local_dir.join("sub1/sub2")).unwrap();
    std::fs::create_dir_all(config.local_dir.join("sub2")).unwrap();

    std::fs::write(config.local_dir.join("sub1/file1.txt"), b"content1").unwrap();
    std::fs::write(config.local_dir.join("sub2/file2.txt"), b"content2").unwrap();

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.uploaded, 2);

    assert!(server.remote_file_exists("sub1/file1.txt"));
    assert!(server.remote_file_exists("sub2/file2.txt"));
}

#[tokio::test]
async fn test_exclude_patterns() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let mut config = test_config(local_dir.path(), &server);
    config.exclude = vec!["*.tmp".to_string(), ".git".to_string()];

    std::fs::write(config.local_dir.join("keep.txt"), b"keep").unwrap();
    std::fs::write(config.local_dir.join("skip.tmp"), b"skip").unwrap();
    std::fs::create_dir_all(config.local_dir.join(".git")).unwrap();
    std::fs::write(config.local_dir.join(".git/config"), b"git config").unwrap();

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.uploaded, 1);
    assert!(server.remote_file_exists("keep.txt"));
    assert!(!server.remote_file_exists("skip.tmp"));
    assert!(!server.remote_file_exists(".git/config"));
}

#[tokio::test]
async fn test_multiple_files_mixed_operations() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    std::fs::write(config.local_dir.join("local_only.txt"), b"local").unwrap();

    server.create_remote_file("remote_only.txt", b"remote");

    let content = b"same";
    std::fs::write(config.local_dir.join("same.txt"), content).unwrap();
    server.create_remote_file("same.txt", content);

    let client = WebDavClient::new(&config).unwrap();
    let result = perform_full_sync(&config, &client).await.unwrap();

    assert_eq!(result.uploaded, 1);
    assert_eq!(result.downloaded, 1);
    assert_eq!(result.conflicts_resolved, 0);

    assert!(server.remote_file_exists("local_only.txt"));
    assert!(std::fs::read(config.local_dir.join("remote_only.txt")).is_ok());
}

#[tokio::test]
async fn test_idempotent_sync() {
    let server = TestWebDavServer::start();
    let local_dir = TempDir::new().unwrap();
    let config = test_config(local_dir.path(), &server);

    std::fs::write(config.local_dir.join("file.txt"), b"content").unwrap();

    let client = WebDavClient::new(&config).unwrap();

    let result1 = perform_full_sync(&config, &client).await.unwrap();
    assert_eq!(result1.uploaded, 1);

    let result2 = perform_full_sync(&config, &client).await.unwrap();
    assert_eq!(result2.uploaded, 0);
    assert_eq!(result2.downloaded, 0);
    assert_eq!(result2.conflicts_resolved, 0);
}
