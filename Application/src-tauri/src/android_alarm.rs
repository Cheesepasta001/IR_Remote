//! Hands alarm scheduling to Android's AlarmManager.
//!
//! ## Why this exists
//!
//! `scheduler.rs` runs a tokio timer, which is fine on desktop where the
//! process lives as long as the window is open. Android suspends and kills
//! backgrounded processes and defers work in Doze, so that timer stops firing
//! the moment the app leaves the screen. `AlarmManager.setExactAndAllowWhileIdle`
//! is the only thing that wakes the device at a wall-clock time.
//!
//! So on Android the Rust scheduler stands down entirely and the Kotlin
//! `AlarmReceiver` performs the send - see `ANDROID.md`. The cost is that the
//! request rules (exactly once, explicit timeout, confirmation required) exist
//! in two places and must be kept in step.
//!
//! On every other platform this module is a no-op and the tokio scheduler owns
//! alarms as before.

#[cfg(target_os = "android")]
pub use android::*;

#[cfg(not(target_os = "android"))]
pub use other::*;

/// True when Android's AlarmManager owns the schedule, so the tokio scheduler
/// must not also fire - two schedulers would actuate the hardware twice.
pub const fn native_scheduler() -> bool {
    cfg!(target_os = "android")
}

#[cfg(target_os = "android")]
mod android {
    use serde::{Deserialize, Serialize};
    use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
    use tauri::{AppHandle, Manager, Runtime};

    const PLUGIN_IDENTIFIER: &str = "com.seongjae.ir_remote";
    const PLUGIN_CLASS: &str = "AlarmPlugin";

    struct AlarmBridge<R: Runtime>(PluginHandle<R>);

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct SyncArgs {
        alarms_path: String,
        base_url: String,
        command_timeout_ms: u64,
    }

    #[derive(Deserialize)]
    struct SyncResponse {
        /// False when the user has withheld the exact-alarm permission, in
        /// which case Android may batch the alarm by minutes.
        #[serde(default)]
        exact: bool,
    }

    /// Register the Kotlin side. Called from the Tauri builder.
    pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
        Builder::new("ir-alarm")
            .setup(|app, api| {
                let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, PLUGIN_CLASS)?;
                app.manage(AlarmBridge(handle));
                Ok(())
            })
            .build()
    }

    /// Push the current alarms and target address to AlarmManager.
    ///
    /// Returns whether EXACT alarms are permitted, so the UI can say that a
    /// schedule may drift instead of promising a minute it cannot keep.
    pub fn sync<R: Runtime>(
        app: &AppHandle<R>,
        alarms_path: &std::path::Path,
        base_url: &str,
        command_timeout_ms: u64,
    ) -> Result<bool, String> {
        let bridge = app
            .try_state::<AlarmBridge<R>>()
            .ok_or_else(|| "the Android alarm plugin is not registered".to_string())?;

        bridge
            .0
            .run_mobile_plugin::<SyncResponse>(
                "sync",
                SyncArgs {
                    alarms_path: alarms_path.to_string_lossy().into_owned(),
                    base_url: base_url.to_string(),
                    command_timeout_ms,
                },
            )
            .map(|r| r.exact)
            .map_err(|e| format!("alarm sync failed: {e}"))
    }

    /// Open the system screen where exact-alarm permission is granted.
    pub fn request_exact_permission<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
        let bridge = app
            .try_state::<AlarmBridge<R>>()
            .ok_or_else(|| "the Android alarm plugin is not registered".to_string())?;

        bridge
            .0
            .run_mobile_plugin::<serde_json::Value>("requestExactPermission", ())
            .map(|_| ())
            .map_err(|e| format!("could not open the exact-alarm setting: {e}"))
    }
}

#[cfg(not(target_os = "android"))]
mod other {
    use tauri::plugin::{Builder, TauriPlugin};
    use tauri::{AppHandle, Runtime};

    /// A plugin that does nothing, so `run()` reads the same on every platform.
    pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
        Builder::new("ir-alarm").build()
    }

    /// Desktop keeps its tokio scheduler; there is nothing to hand off to.
    /// Reports `true` because desktop timers are exact.
    pub fn sync<R: Runtime>(
        _app: &AppHandle<R>,
        _alarms_path: &std::path::Path,
        _base_url: &str,
        _command_timeout_ms: u64,
    ) -> Result<bool, String> {
        Ok(true)
    }

    pub fn request_exact_permission<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_scheduler_owns_alarms() {
        // Two schedulers firing the same alarm would send the IR code twice.
        // Desktop: tokio owns it. Android: AlarmManager owns it. Never both.
        assert_eq!(native_scheduler(), cfg!(target_os = "android"));
    }
}
