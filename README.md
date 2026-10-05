# WebDAV Sync

Bidirectional directory sync service via WebDAV in Rust.

## Features

- **Bidirectional sync** — automatic file upload and download
- **Fast diff detection** — compares by size and modification time (no full hashing)
- **Conflict resolution** — uses the newest file when conflicts occur
- **Real-time monitoring** — tracks changes via inotify
- **Periodic sync** — full check on schedule
- **Retry logic** — automatic retries on network errors
- **Exclusions** — glob pattern support for file exclusion

## Build

```bash
cargo build --release
```

## Configuration

Copy `config.example.toml` to `config.toml` and fill in:

```toml
local_dir = "/home/user/my-files"
webdav_url = "https://cloud.example.com/dav/sync/"
username = "user"
password = "secret"
sync_interval_secs = 60
```

## Usage

```bash
# Interactive setup (creates config and systemd service)
./target/release/webdav-sync setup

# Show service status and recent syncs
./target/release/webdav-sync status

# Continuous sync (with change monitoring)
./target/release/webdav-sync -c config.toml

# One-time sync
./target/release/webdav-sync -c config.toml --once

# Verbose logging
RUST_LOG=debug ./target/release/webdav-sync -c config.toml
```

## Auto-start via systemd

After running `webdav-sync setup`, a user service is created:

```bash
# Enable auto-start
systemctl --user daemon-reload
systemctl --user enable --now webdav-sync.service

# Check status
systemctl --user status webdav-sync.service

# Stop
systemctl --user stop webdav-sync.service
```

## How It Works

1. **Scan** — get file list locally and from WebDAV (PROPFIND)
2. **Compare** — fast comparison by size and modification time
3. **Actions**:
   - File only local → upload to server
   - File only remote → download
   - File exists everywhere but differs → conflict
4. **Conflict resolution** — select file with newer modification time

## Project Structure

```
src/
├── main.rs      — entry point, argument parsing
├── config.rs    — config loading and validation
├── webdav.rs    — WebDAV client (PROPFIND, GET, PUT, DELETE, MKCOL)
├── local.rs     — local filesystem operations
├── metadata.rs  — file metadata for fast comparison
└── sync.rs      — sync logic and conflict resolution
```

## License

MIT
