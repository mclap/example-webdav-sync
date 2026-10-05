# AGENTS.md

Guidance for agents working on the WebDAV Sync project.

## Project Overview

Bidirectional directory sync service via WebDAV in Rust. Watches local directory and remote WebDAV server, transfers files between them, uses newest file on conflicts.

## Architecture

```
src/
├── main.rs      — entry point, argument parsing (clap)
├── lib.rs       — module exports
├── config.rs    — config loading and validation (TOML)
├── webdav.rs    — WebDAV client (PROPFIND, GET, PUT, DELETE, MKCOL, MOVE)
├── local.rs     — local filesystem operations
├── metadata.rs  — file metadata for fast comparison
├── sync.rs      — sync logic and conflict resolution
├── setup.rs     — interactive setup (dialoguer)
├── status.rs    — service status display
└── state.rs     — sync state persistence

tests/
├── fixtures/
│   └── wsgidav_server.rs  — wsgidav start/stop fixture
└── integration.rs          — integration tests
```

## Key Decisions

### Fast Diff Detection
Compares by **size + modification time** (no full file hashing):
```rust
pub fn differs_from(&self, other: &FileMetadata) -> bool {
    if self.size != other.size { return true; }
    self.modified.timestamp() != other.modified.timestamp()
}
```

### Conflict Resolution
When files differ, the one with newer `mtime` is selected:
```rust
if local_meta.is_newer_than(&remote_meta) {
    // Upload local to server
} else {
    // Download remote
}
```

### Change Monitoring
- **inotify** via `notify` + `notify-debouncer-mini` (500ms debounce)
- Periodic full sync on schedule

## Testing

### Running Tests
```bash
# All tests
cargo test

# Unit tests only
cargo test --lib

# Integration tests only
cargo test --test integration
```

### Requirements for Integration Tests
```bash
pip install wsgidav cheroot
```

### Test Coverage
- **Unit tests (32)**: metadata, local (glob), webdav (XML parsing), sync (compute_actions)
- **Integration tests (9)**: full sync cycle with real WebDAV server

## Code Style

### General
- **Edition**: 2021
- **Formatting**: `cargo fmt`
- **Linting**: `cargo clippy -- -D warnings`
- **Errors**: `anyhow` for propagation, `thiserror` for definitions

### Naming
- Structs: `PascalCase` (`WebDavClient`, `FileMetadata`)
- Functions/variables: `snake_case`
- Constants: `SCREAMING_SNAKE_CASE`

### Logging
- `tracing` with levels: `error`, `warn`, `info`, `debug`
- Default: `info`, verbose via `RUST_LOG=debug`

### Async
- `tokio` as runtime
- All I/O operations are async
- For parallel operations — `futures::join!` or `tokio::join!`

## Configuration

### Paths
- Config: `~/.config/webdav-sync/config.toml`
- State: `~/.config/webdav-sync/state.toml`
- Systemd: `~/.config/systemd/user/webdav-sync.service`

### Config Example
```toml
local_dir = "/home/user/my-files"
webdav_url = "https://cloud.example.com/dav/sync/"
username = "user"
password = "secret"
sync_interval_secs = 60
timeout_secs = 30
max_retries = 3
exclude = ["*.tmp", ".git", ".DS_Store"]
```

## CLI Commands

```bash
webdav-sync setup          # Interactive setup
webdav-sync status         # Service status
webdav-sync -c config.toml # Continuous sync
webdav-sync -c config.toml --once  # One-time sync
```

## Important Notes

### WebDAV Compatibility
- XML parser supports namespaces `D:`, `ns0:`, and no namespace
- Record saved on closing `response` tag
- Directories skipped (paths ending with `/`)

### Security
- Password stored in plaintext in config (use keyring for production)
- Basic Auth is base64 (not encrypted) — use HTTPS

### Performance
- File comparison without reading content (metadata only)
- Debounce to prevent multiple syncs
- Parallel processing of multiple files

## Common Tasks

### Add New Sync Action
1. Add variant to `SyncAction` (src/sync.rs)
2. Handle in `execute_action()`
3. Add test in `sync::tests`

### Add New Config Field
1. Add to `Config` (src/config.rs)
2. Update `config.example.toml`
3. Update `setup.rs` for interactive input

### Add Integration Test
1. Use `TestWebDavServer::start()`
2. Create test config via `test_config()`
3. Call `perform_full_sync()` and verify result

## Dependencies

### Main
- `tokio` — async runtime
- `reqwest` — HTTP client (rustls-tls)
- `quick-xml` — WebDAV XML parsing
- `notify` — filesystem monitoring
- `dialoguer` — interactive input
- `toml` — configuration
- `chrono` — dates and time
- `sha2` — hashing

### Dev
- `tempfile` — temporary directories
- `uuid` — unique identifier generation
