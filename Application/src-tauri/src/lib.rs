//! Library surface. Exists so `src-tauri/tests/` can exercise the core against
//! a real socket (Instruction.md section 8), and so `main.rs` stays wiring only.

pub mod api_client;
pub mod auth;
pub mod core;
pub mod poller;
pub mod state;

use std::time::Duration;

use api_client::Config;
use core::{LogEntry, LogKind, Shared, Snapshot};
use state::{Command, Event, Failure};
use tauri::Manager;

#[tauri::command]
fn get_snapshot(shared: tauri::State<'_, Shared>) -> Snapshot {
    shared.snapshot()
}

#[tauri::command]
fn get_log(shared: tauri::State<'_, Shared>) -> Vec<LogEntry> {
    shared.entries()
}

/// Dispatch one command. Exactly once - never retried (rule 5).
#[tauri::command]
async fn send_command(shared: tauri::State<'_, Shared>, command: Command) -> Result<(), String> {
    if shared.is_pending() {
        return Err("a command is already in flight".to_string());
    }

    // Rule 1: this moves the UI to Pending, NOT to the new value.
    shared.apply(Event::CommandRequested(command));
    shared.log(LogKind::CommandSent, format!("GET {}", command.path()), None);

    let client = shared.client();
    let username = shared.username();

    // keyring is blocking; keep it off the async runtime's critical path.
    let creds = tokio::task::spawn_blocking(move || auth::load(&username))
        .await
        .map_err(|e| format!("credential lookup panicked: {e}"))?;

    let creds = match creds {
        Ok(c) => c,
        Err(e) => {
            shared.apply(Event::CommandFailed {
                command,
                error: Failure::DeviceError(e.clone()),
            });
            shared.log(LogKind::CommandFail, e.clone(), None);
            return Err(e);
        }
    };

    match client.send_command(command, creds.as_ref()).await {
        Ok(outcome) => {
            shared.apply(Event::CommandSucceeded {
                command,
                latency_ms: outcome.latency_ms,
            });
            shared.log(
                LogKind::CommandOk,
                format!("{} confirmed", command.path()),
                Some(outcome.latency_ms),
            );
            Ok(())
        }
        Err(error) => {
            let message = error.message();
            shared.apply(Event::CommandFailed { command, error });
            shared.log(LogKind::CommandFail, message.clone(), None);
            Err(message)
        }
    }
}

/// Rule 6: always reachable, never gated on connection or pending state.
///
/// NOTE: this cancels the app's in-flight request. It does NOT stop the air
/// conditioner - the API exposes no safe "off". `/Power` is a toggle, so firing
/// it as a stop could switch the unit ON. See README, "Unverified / blocked".
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

#[tauri::command]
fn update_settings(
    shared: tauri::State<'_, Shared>,
    base_url: String,
    poll_interval_ms: u64,
    command_timeout_ms: u64,
    probe_timeout_ms: u64,
    stale_after_ms: u64,
) -> Result<(), String> {
    // Fail fast on a URL we cannot build a probe address from.
    api_client::host_port(&base_url)?;

    if command_timeout_ms == 0 || probe_timeout_ms == 0 || poll_interval_ms == 0 {
        return Err("timeouts and poll interval must be greater than zero".to_string());
    }

    shared.set_config(Config {
        base_url: base_url.clone(),
        timeouts: api_client::Timeouts {
            command: Duration::from_millis(command_timeout_ms),
            probe: Duration::from_millis(probe_timeout_ms),
        },
        poll_interval: Duration::from_millis(poll_interval_ms),
        stale_after: Duration::from_millis(stale_after_ms),
    })?;

    shared.log(LogKind::Info, format!("target set to {base_url}"), None);
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

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let config = Config::from_env();
            let shared = Shared::new(app.handle().clone(), config.clone())?;

            shared.log(
                LogKind::Info,
                format!(
                    "target {} - polling every {}ms",
                    config.base_url,
                    config.poll_interval.as_millis()
                ),
                None,
            );

            app.manage(shared.clone());
            poller::spawn(shared);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_log,
            send_command,
            abort,
            update_settings,
            save_credentials,
            clear_credentials,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
