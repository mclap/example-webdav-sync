use crate::state::{state_db_path, SyncState};
use std::process::Command;

/// Show current daemon status
pub fn show_status() -> anyhow::Result<()> {
    println!("=== WebDAV Sync Status ===\n");

    let service_active = check_systemd_service();
    print_service_status(&service_active);

    let state_path = state_db_path();
    let state = SyncState::open(&state_path)?;
    print_sync_state(&state);

    Ok(())
}

/// Check systemd service status
fn check_systemd_service() -> ServiceStatus {
    let output = Command::new("systemctl")
        .args(["--user", "is-active", "webdav-sync.service"])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match stdout.trim() {
                "active" => ServiceStatus::Active,
                "inactive" => ServiceStatus::Inactive,
                "failed" => ServiceStatus::Failed,
                _ => ServiceStatus::Unknown,
            }
        }
        Err(_) => ServiceStatus::Unknown,
    }
}

fn print_service_status(status: &ServiceStatus) {
    let (icon, text) = match status {
        ServiceStatus::Active => ("[ACTIVE]", "Active"),
        ServiceStatus::Inactive => ("[INACTIVE]", "Inactive"),
        ServiceStatus::Failed => ("[FAILED]", "Failed"),
        ServiceStatus::Unknown => ("[UNKNOWN]", "Unknown"),
    };
    println!("Service: {} {}\n", icon, text);
}

fn print_sync_state(state: &SyncState) {
    println!("--- Sync State ---");

    match state.last_sync() {
        Some(ts) => println!("Last sync: {}", ts.format("%Y-%m-%d %H:%M:%S")),
        None => println!("Sync has not been performed yet"),
    }

    let records = state.recent_records(20);
    if records.is_empty() {
        println!("\nNo sync records.");
        return;
    }

    println!("\nRecently synced files:");
    println!("{:<40} {:<15} {}", "File", "Direction", "Time");
    println!("{}", "-".repeat(70));

    for record in records.iter() {
        let direction = match record.direction {
            crate::state::SyncDirection::Upload => "Upload",
            crate::state::SyncDirection::Download => "Download",
            crate::state::SyncDirection::ConflictResolved => "Conflict",
        };
        let time = record.timestamp.format("%H:%M:%S");
        println!(
            "{:<40} {:<15} {}",
            truncate(&record.rel_path, 38),
            direction,
            time
        );
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() > max_len {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

#[derive(Debug)]
enum ServiceStatus {
    Active,
    Inactive,
    Failed,
    Unknown,
}
