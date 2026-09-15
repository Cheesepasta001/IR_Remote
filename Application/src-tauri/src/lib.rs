//! Library surface. Exists so `src-tauri/tests/` can exercise the core against
//! a real socket (Instruction.md section 8), and so `main.rs` stays wiring only.

pub mod alarm;
pub mod android_alarm;
pub mod api_client;
pub mod auth;
pub mod core;
pub mod poller;
pub mod scheduler;
pub mod state;

use std::time::Duration;

use api_client::Config;
use core::{dispatch_command, LogEntry, LogKind, Shared, Snapshot};
use state::{Command, Event};
use tauri::Manager;

#[tauri::command]
fn get_snapshot(shared: tauri::State<'_, Shared>) -> Snapshot {
    shared.snapshot()
}

/// The log pane is no longer in the UI, but the core still records every
/// request. This stays so the history is reachable when something goes wrong.
#[tauri::command]
fn get_log(shared: tauri::State<'_, Shared>) -> Vec<LogEntry> {
    shared.entries()
}

/// Dispatch one command. Exactly once - never retried (rule 5).
#[tauri::command]
async fn send_command(shared: tauri::State<'_, Shared>, command: Command) -> Result<(), String> {
    dispatch_command(&shared, command).await
}

/// Cancel the app's in-flight request.
///
/// No longer surfaced as a button, but kept because it is the only way to clear
/// a pending command, and because it does NOT command the device: the API
/// exposes no safe "off", and `/Power` is a toggle.
#[tauri::command]
fn abort(shared: tauri::State<'_, Shared>) -> Result<(), String> {
    shared.apply(Event::AbortRequested);
    shared.log(
        LogKind::Info,
        "cancelled in-flight request (device not commanded)".to_string(),
        None,
    );
    Ok(())
}

/// The view reports its UTC offset; the core does all the scheduling.
/// Rust's standard library has no timezone database, so this is environment
/// data the view happens to know, not a scheduling decision it makes.
#[tauri::command]
fn set_tz_offset(shared: tauri::State<'_, Shared>, minutes: i32) -> Result<(), String> {
    if !(-14 * 60..=14 * 60).contains(&minutes) {
        return Err(format!("{minutes} is not a plausible UTC offset"));
    }
    shared.set_tz_offset(minutes);
    Ok(())
}

#[tauri::command]
fn add_alarm(
    shared: tauri::State<'_, Shared>,
    hour: u8,
    minute: u8,
    command: Command,
) -> Result<u64, String> {
    shared.add_alarm(hour, minute, command)
}

#[tauri::command]
fn set_alarm_enabled(
    shared: tauri::State<'_, Shared>,
    id: u64,
    enabled: bool,
) -> Result<(), String> {
    shared.set_alarm_enabled(id, enabled)
}

#[tauri::command]
fn remove_alarm(shared: tauri::State<'_, Shared>, id: u64) -> Result<(), String> {
    shared.remove_alarm(id)
}

/// Open Android's exact-alarm permission screen. A no-op elsewhere.
#[tauri::command]
fn request_exact_alarm_permission(app: tauri::AppHandle) -> Result<(), String> {
    android_alarm::request_exact_permission(&app)
}

/// Change which device the app talks to. Android only - see `Shared::set_base_url`.
#[tauri::command]
fn set_base_url(shared: tauri::State<'_, Shared>, base_url: String) -> Result<(), String> {
    shared.set_base_url(&base_url)
}

/// Runtime tunables. The base URL is NOT here: it comes from the environment or
/// `.env` at startup, so there is one source of truth for which device this is.
#[tauri::command]
fn update_settings(
    shared: tauri::State<'_, Shared>,
    poll_interval_ms: u64,
    command_timeout_ms: u64,
    probe_timeout_ms: u64,
    stale_after_ms: u64,
) -> Result<(), String> {
    if command_timeout_ms == 0 || probe_timeout_ms == 0 || poll_interval_ms == 0 {
        return Err("timeouts and poll interval must be greater than zero".to_string());
    }

    let base_url = shared.snapshot().base_url;
    shared.set_config(Config {
        base_url,
        timeouts: api_client::Timeouts {
            command: Duration::from_millis(command_timeout_ms),
            probe: Duration::from_millis(probe_timeout_ms),
        },
        poll_interval: Duration::from_millis(poll_interval_ms),
        stale_after: Duration::from_millis(stale_after_ms),
    })?;

    shared.log(LogKind::Info, "settings updated".to_string(), None);
    Ok(())
}

/// The password travels IN only. Nothing here ever sends one back.
#[tauri::command]
async fn save_credentials(
    shared: tauri::State<'_, Shared>,
    username: String,
    password: String,
) -> Result<(), String> {
    let u = username.clone();
    tokio::task::spawn_blocking(move || auth::store(&u, &password))
        .await
        .map_err(|e| format!("keychain write panicked: {e}"))??;

    shared.set_credential_presence(username.clone(), true);
    shared.log(
        LogKind::Info,
        format!("credential stored in OS keychain for {username}"),
        None,
    );
    Ok(())
}

#[tauri::command]
async fn clear_credentials(
    shared: tauri::State<'_, Shared>,
    username: String,
) -> Result<(), String> {
    let u = username.clone();
    tokio::task::spawn_blocking(move || auth::clear(&u))
        .await
        .map_err(|e| format!("keychain delete panicked: {e}"))??;

    shared.set_credential_presence(String::new(), false);
    shared.log(LogKind::Info, "credential cleared".to_string(), None);
    Ok(())
}

/// Entry point for every platform. On Android and iOS the generated native
/// project calls this through `mobile_entry_point`; on desktop `main.rs` calls
/// it directly.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(android_alarm::plugin())
        .setup(|app| {
            let shared = Shared::new(app.handle().clone())?;
            let snap = shared.snapshot();

            shared.log(
                LogKind::Info,
                format!(
                    "target {} (from {:?}) - polling every {}ms",
                    snap.base_url, snap.base_url_source, snap.poll_interval_ms
                ),
                None,
            );

            app.manage(shared.clone());
            poller::spawn(shared.clone());
            scheduler::spawn(shared);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_log,
            send_command,
            abort,
            set_tz_offset,
            set_base_url,
            request_exact_alarm_permission,
            add_alarm,
            set_alarm_enabled,
            remove_alarm,
            update_settings,
            save_credentials,
            clear_credentials,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
