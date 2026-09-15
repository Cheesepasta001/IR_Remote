//! Background alarm scheduler. Runs in the core and pushes updates to the
//! frontend as Tauri events, exactly like the reachability poller - the
//! frontend never schedules anything itself.
//!
//! All the decisions live in `alarm.rs`, which is pure. This module only
//! supplies the clock, performs the send, and records what happened.

use std::time::Duration;

use tokio::time::{interval, MissedTickBehavior};

use crate::alarm::{self, Due, Outcome, GRACE_MINUTES};
use crate::core::{dispatch_command, LogKind, Shared};

/// How often to look for a due alarm. Comfortably inside the grace window, so
/// a tick that slips still lands on time.
const TICK: Duration = Duration::from_secs(15);

pub fn spawn(shared: Shared) {
    // Android hands alarms to AlarmManager, which fires with the app dead.
    // Running this loop as well would send every IR code twice.
    if crate::android_alarm::native_scheduler() {
        return;
    }

    tauri::async_runtime::spawn(async move {
        let mut ticker = interval(TICK);
        // A laptop waking from sleep must not fire a burst of catch-up ticks.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            run_once(&shared).await;
        }
    });
}

async fn run_once(shared: &Shared) {
    let now = shared.local_now();
    let alarms = shared.alarms();

    for (id, decision) in alarm::due(&alarms, now, GRACE_MINUTES) {
        let Some(a) = alarms.iter().find(|a| a.id == id) else {
            continue;
        };
        let clock = format!("{:02}:{:02}", a.hour, a.minute);

        // Consume it for today first. Whatever happens next, it must not be
        // retried: every command this device accepts actuates hardware.
        let Some(command) = shared.claim_alarm(id, now.day) else {
            continue;
        };

        match decision {
            Due::Skip => {
                shared.log(
                    LogKind::Info,
                    format!(
                        "alarm {clock} missed - the app was not running at the time, so {} was not sent",
                        command.path()
                    ),
                    None,
                );
                shared.record_alarm_outcome(id, Outcome::Missed);
            }

            Due::Fire => {
                shared.log(
                    LogKind::Info,
                    format!("alarm {clock} firing {}", command.path()),
                    None,
                );

                match dispatch_command(shared, command).await {
                    Ok(()) => shared.record_alarm_outcome(id, Outcome::Fired),
                    Err(e) => {
                        // Recorded as Failed, never Fired: the UI must not claim
                        // a command reached the device when it did not.
                        shared.log(
                            LogKind::CommandFail,
                            format!("alarm {clock} send failed: {e} (not retried)"),
                            None,
                        );
                        shared.record_alarm_outcome(id, Outcome::Failed);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tick_fits_inside_the_grace_window() {
        // If the scheduler ticked less often than the grace window, an alarm
        // could be judged "missed" simply because nobody looked in time.
        let grace = Duration::from_secs(GRACE_MINUTES as u64 * 60);
        assert!(
            TICK < grace,
            "tick {TICK:?} must be shorter than the grace window {grace:?}"
        );
    }
}
