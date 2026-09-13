//! Pure state machine. NO I/O: no HTTP, no clock reads, no filesystem.
//!
//! Every transition is `reduce(state, event, now_ms) -> state`. The caller
//! supplies the timestamp, which is what makes staleness, connection
//! transitions and pending-command logic testable without a device.

use serde::Serialize;

/// How long a reachability observation stays fresh before the UI must grey it.
pub const DEFAULT_STALE_AFTER_MS: u64 = 5_000;

/// The four commands from Instruction.md section 1.
///
/// Every one of these fires an IR code at the air conditioner, so none of them
/// is idempotent: `Power` is a toggle, and the temperature commands step.
/// See [`Command::is_idempotent`] - nothing here may ever be auto-retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub enum Command {
    Power,
    Silent,
    LowTemp,
    HighTemp,
}

impl Command {
    /// The path on the device, exactly as the firmware matches it.
    pub fn path(&self) -> &'static str {
        match self {
            Command::Power => "/Power",
            Command::Silent => "/Silent",
            Command::LowTemp => "/Low_Temp",
            Command::HighTemp => "/High_Temp",
        }
    }

    /// Instruction.md rule 5: retries only on idempotent requests.
    ///
    /// No command on this device qualifies. Each one emits an IR code that
    /// mutates physical state, and the API documents no idempotency. A retry
    /// would toggle the unit twice or step the temperature twice.
    pub fn is_idempotent(&self) -> bool {
        false
    }
}

/// Why a request failed. Kept separate from a free-text string so the reducer
/// can distinguish "device unreachable" from "device answered with an error" -
/// only the former is a connection transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "camelCase")]
pub enum Failure {
    /// Could not reach the device at all: refused, timed out, DNS, reset.
    Unreachable(String),
    /// The device answered, but not with success.
    DeviceError(String),
    /// The device answered with something that is not the agreed contract.
    BadResponse(String),
    /// The user cancelled it.
    Aborted,
}

impl Failure {
    fn proves_reachable(&self) -> bool {
        matches!(self, Failure::DeviceError(_) | Failure::BadResponse(_))
    }

    pub fn message(&self) -> String {
        match self {
            Failure::Unreachable(d) => format!("unreachable: {d}"),
            Failure::DeviceError(d) => format!("device error: {d}"),
            Failure::BadResponse(d) => format!("bad response: {d}"),
            Failure::Aborted => "cancelled".to_string(),
        }
    }
}

/// Connection is a first-class state (rule 3), never an error toast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Connection {
    /// Nothing has been observed yet - startup, before the first probe lands.
    Unknown,
    /// The device answered at `last_seen_ms`.
    Online { last_seen_ms: u64 },
    /// The device did not answer. `last_seen_ms` is the last time it did, if ever.
    Offline {
        since_ms: u64,
        reason: String,
        last_seen_ms: Option<u64>,
    },
}

/// Rule 1: a command is never optimistically applied. It goes Pending and stays
/// there until the device confirms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CommandState {
    Idle,
    Pending {
        command: Command,
        started_ms: u64,
    },
    Succeeded {
        command: Command,
        at_ms: u64,
        latency_ms: u64,
    },
    Failed {
        command: Command,
        at_ms: u64,
        error: Failure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub connection: Connection,
    pub command: CommandState,
    pub stale_after_ms: u64,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            connection: Connection::Unknown,
            command: CommandState::Idle,
            stale_after_ms: DEFAULT_STALE_AFTER_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A reachability probe succeeded.
    ProbeSucceeded,
    /// A reachability probe failed.
    ProbeFailed(String),
    /// The user asked for a command. Rejected if one is already pending.
    CommandRequested(Command),
    CommandSucceeded {
        command: Command,
        latency_ms: u64,
    },
    CommandFailed {
        command: Command,
        error: Failure,
    },
    /// The user hit the cancel control. Always accepted (rule 6).
    AbortRequested,
}

impl AppState {
    /// Age of the newest thing we actually know about the device.
    pub fn last_seen_ms(&self) -> Option<u64> {
        match &self.connection {
            Connection::Unknown => None,
            Connection::Online { last_seen_ms } => Some(*last_seen_ms),
            Connection::Offline { last_seen_ms, .. } => *last_seen_ms,
        }
    }

    /// Rule 2: every displayed value carries its age.
    pub fn age_ms(&self, now_ms: u64) -> Option<u64> {
        self.last_seen_ms().map(|t| now_ms.saturating_sub(t))
    }

    /// Rule 2: past the threshold the display must go visibly stale rather than
    /// keep showing a number that looks live.
    pub fn is_stale(&self, now_ms: u64) -> bool {
        match self.age_ms(now_ms) {
            None => true,
            Some(age) => age > self.stale_after_ms,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.command, CommandState::Pending { .. })
    }

    /// Rule 6: the cancel control is never gated on connection or pending state.
    pub fn can_abort(&self) -> bool {
        true
    }
}

/// A sighting replaces whatever came before it: once the device answers, the
/// previous connection state carries no information worth keeping.
fn mark_seen(now_ms: u64) -> Connection {
    Connection::Online {
        last_seen_ms: now_ms,
    }
}

fn mark_lost(connection: &Connection, now_ms: u64, reason: String) -> Connection {
    match connection {
        // Already offline: keep the original `since` so the banner shows how
        // long it has been down, not how long since the last failed retry.
        Connection::Offline {
            since_ms,
            last_seen_ms,
            ..
        } => Connection::Offline {
            since_ms: *since_ms,
            reason,
            last_seen_ms: *last_seen_ms,
        },
        Connection::Online { last_seen_ms } => Connection::Offline {
            since_ms: now_ms,
            reason,
            last_seen_ms: Some(*last_seen_ms),
        },
        Connection::Unknown => Connection::Offline {
            since_ms: now_ms,
            reason,
            last_seen_ms: None,
        },
    }
}

/// The single transition function. Pure: same inputs, same output, always.
pub fn reduce(state: &AppState, event: Event, now_ms: u64) -> AppState {
    let mut next = state.clone();

    match event {
        Event::ProbeSucceeded => {
            next.connection = mark_seen(now_ms);
        }

        Event::ProbeFailed(reason) => {
            next.connection = mark_lost(&state.connection, now_ms, reason);
        }

        Event::CommandRequested(command) => {
            // One command at a time. A second request while one is in flight is
            // dropped rather than queued - queuing would fire IR codes the user
            // may no longer want.
            if !state.is_pending() {
                next.command = CommandState::Pending {
                    command,
                    started_ms: now_ms,
                };
            }
        }

        Event::CommandSucceeded {
            command,
            latency_ms,
        } => {
            // Only apply if it answers the command actually in flight. A late
            // reply to an aborted command must not resurrect it.
            if let CommandState::Pending { command: p, .. } = state.command {
                if p == command {
                    next.command = CommandState::Succeeded {
                        command,
                        at_ms: now_ms,
                        latency_ms,
                    };
                    // A successful command is itself proof of reachability.
                    next.connection = mark_seen(now_ms);
                }
            }
        }

        Event::CommandFailed { command, error } => {
            if let CommandState::Pending { command: p, .. } = state.command {
                if p == command {
                    // The device answering with an error still proves it is there.
                    next.connection = if error.proves_reachable() {
                        mark_seen(now_ms)
                    } else if matches!(error, Failure::Aborted) {
                        state.connection.clone()
                    } else {
                        mark_lost(&state.connection, now_ms, error.message())
                    };
                    next.command = CommandState::Failed {
                        command,
                        at_ms: now_ms,
                        error,
                    };
                }
            }
        }

        Event::AbortRequested => {
            // Always honoured, whatever the connection or pending state.
            if let CommandState::Pending { command, .. } = state.command {
                next.command = CommandState::Failed {
                    command,
                    at_ms: now_ms,
                    error: Failure::Aborted,
                };
            }
        }
    }

    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(cmd: Command, at: u64) -> AppState {
        reduce(&AppState::default(), Event::CommandRequested(cmd), at)
    }

    #[test]
    fn starts_unknown_and_stale() {
        let s = AppState::default();
        assert_eq!(s.connection, Connection::Unknown);
        assert_eq!(s.command, CommandState::Idle);
        assert_eq!(s.age_ms(1_000), None);
        // Nothing observed yet must not read as fresh.
        assert!(s.is_stale(0));
    }

    #[test]
    fn probe_success_connects_and_sets_age() {
        let s = reduce(&AppState::default(), Event::ProbeSucceeded, 1_000);
        assert_eq!(s.connection, Connection::Online { last_seen_ms: 1_000 });
        assert_eq!(s.age_ms(1_400), Some(400));
        assert!(!s.is_stale(1_400));
    }

    #[test]
    fn value_goes_stale_past_the_threshold() {
        let s = reduce(&AppState::default(), Event::ProbeSucceeded, 1_000);
        assert!(!s.is_stale(1_000 + DEFAULT_STALE_AFTER_MS));
        assert!(s.is_stale(1_000 + DEFAULT_STALE_AFTER_MS + 1));
    }

    #[test]
    fn disconnect_keeps_last_seen_and_pins_since() {
        let online = reduce(&AppState::default(), Event::ProbeSucceeded, 1_000);
        let lost = reduce(&online, Event::ProbeFailed("refused".into()), 3_000);
        assert_eq!(
            lost.connection,
            Connection::Offline {
                since_ms: 3_000,
                reason: "refused".into(),
                last_seen_ms: Some(1_000),
            }
        );
        // A second failure must not move `since` - the outage started at 3000.
        let still = reduce(&lost, Event::ProbeFailed("refused again".into()), 9_000);
        match still.connection {
            Connection::Offline {
                since_ms,
                last_seen_ms,
                ..
            } => {
                assert_eq!(since_ms, 3_000);
                assert_eq!(last_seen_ms, Some(1_000));
            }
            other => panic!("expected Offline, got {other:?}"),
        }
        // And the age still refers to the last real sighting.
        assert_eq!(still.age_ms(9_000), Some(8_000));
        assert!(still.is_stale(9_000));
    }

    #[test]
    fn reconnect_clears_offline() {
        let lost = reduce(&AppState::default(), Event::ProbeFailed("boom".into()), 500);
        let back = reduce(&lost, Event::ProbeSucceeded, 2_000);
        assert_eq!(back.connection, Connection::Online { last_seen_ms: 2_000 });
    }

    #[test]
    fn command_goes_pending_never_optimistic() {
        let s = pending(Command::Power, 100);
        // Rule 1: the state is Pending. There is no field anywhere claiming the
        // device is now on.
        assert_eq!(
            s.command,
            CommandState::Pending {
                command: Command::Power,
                started_ms: 100
            }
        );
        assert!(s.is_pending());
    }

    #[test]
    fn pending_blocks_a_second_command() {
        let s = pending(Command::Power, 100);
        let s2 = reduce(&s, Event::CommandRequested(Command::Silent), 150);
        assert_eq!(
            s2.command,
            CommandState::Pending {
                command: Command::Power,
                started_ms: 100
            },
            "a second command must not displace the one in flight"
        );
    }

    #[test]
    fn confirmation_is_required_before_success_shows() {
        let s = pending(Command::Power, 100);
        let done = reduce(
            &s,
            Event::CommandSucceeded {
                command: Command::Power,
                latency_ms: 42,
            },
            142,
        );
        assert_eq!(
            done.command,
            CommandState::Succeeded {
                command: Command::Power,
                at_ms: 142,
                latency_ms: 42
            }
        );
        // Confirming also proves the device is reachable.
        assert_eq!(done.connection, Connection::Online { last_seen_ms: 142 });
    }

    #[test]
    fn reply_for_a_different_command_is_ignored() {
        let s = pending(Command::Power, 100);
        let confused = reduce(
            &s,
            Event::CommandSucceeded {
                command: Command::Silent,
                latency_ms: 10,
            },
            110,
        );
        assert!(confused.is_pending(), "stale reply must not settle Power");
    }

    #[test]
    fn unreachable_failure_disconnects() {
        let online = reduce(&AppState::default(), Event::ProbeSucceeded, 50);
        let s = reduce(&online, Event::CommandRequested(Command::Power), 100);
        let failed = reduce(
            &s,
            Event::CommandFailed {
                command: Command::Power,
                error: Failure::Unreachable("timed out".into()),
            },
            3_100,
        );
        assert!(matches!(failed.connection, Connection::Offline { .. }));
        assert!(matches!(failed.command, CommandState::Failed { .. }));
    }

    #[test]
    fn device_error_keeps_the_connection_online() {
        let s = pending(Command::Power, 100);
        let failed = reduce(
            &s,
            Event::CommandFailed {
                command: Command::Power,
                error: Failure::DeviceError("500 ir send failed".into()),
            },
            200,
        );
        // It answered, so it is demonstrably reachable even though it said no.
        assert_eq!(failed.connection, Connection::Online { last_seen_ms: 200 });
    }

    #[test]
    fn malformed_response_is_not_success_but_proves_reachable() {
        let s = pending(Command::Power, 100);
        let failed = reduce(
            &s,
            Event::CommandFailed {
                command: Command::Power,
                error: Failure::BadResponse("invalid json".into()),
            },
            180,
        );
        assert!(matches!(failed.command, CommandState::Failed { .. }));
        assert_eq!(failed.connection, Connection::Online { last_seen_ms: 180 });
    }

    #[test]
    fn abort_settles_a_pending_command() {
        let s = pending(Command::Power, 100);
        let stopped = reduce(&s, Event::AbortRequested, 250);
        assert_eq!(
            stopped.command,
            CommandState::Failed {
                command: Command::Power,
                at_ms: 250,
                error: Failure::Aborted
            }
        );
        assert!(!stopped.is_pending());
    }

    #[test]
    fn abort_is_available_in_every_state() {
        // Rule 6: never disabled. Not while disconnected, not while pending.
        let disconnected = reduce(&AppState::default(), Event::ProbeFailed("down".into()), 10);
        assert!(disconnected.can_abort());

        let busy = pending(Command::Power, 100);
        assert!(busy.can_abort());

        assert!(AppState::default().can_abort());

        // Aborting while disconnected is a no-op on state, never a panic.
        let s = reduce(&disconnected, Event::AbortRequested, 20);
        assert_eq!(s.command, CommandState::Idle);
    }

    #[test]
    fn late_reply_after_abort_does_not_resurrect() {
        let s = pending(Command::Power, 100);
        let stopped = reduce(&s, Event::AbortRequested, 250);
        let late = reduce(
            &stopped,
            Event::CommandSucceeded {
                command: Command::Power,
                latency_ms: 900,
            },
            1_000,
        );
        assert_eq!(
            late.command, stopped.command,
            "a reply arriving after cancel must not flip the UI to success"
        );
    }

    #[test]
    fn abort_does_not_fabricate_a_connection() {
        let s = pending(Command::Power, 100);
        let stopped = reduce(&s, Event::AbortRequested, 250);
        assert_eq!(
            stopped.connection,
            Connection::Unknown,
            "cancelling locally tells us nothing about the device"
        );
    }

    #[test]
    fn no_command_is_retryable() {
        for c in [
            Command::Power,
            Command::Silent,
            Command::LowTemp,
            Command::HighTemp,
        ] {
            assert!(
                !c.is_idempotent(),
                "{c:?} fires an IR code; auto-retry would actuate twice"
            );
        }
    }

    #[test]
    fn paths_match_the_firmware() {
        assert_eq!(Command::Power.path(), "/Power");
        assert_eq!(Command::Silent.path(), "/Silent");
        assert_eq!(Command::LowTemp.path(), "/Low_Temp");
        assert_eq!(Command::HighTemp.path(), "/High_Temp");
    }

    #[test]
    fn reducer_is_pure() {
        let before = pending(Command::Power, 100);
        let snapshot = before.clone();
        let _ = reduce(&before, Event::ProbeSucceeded, 500);
        let _ = reduce(&before, Event::AbortRequested, 600);
        assert_eq!(before, snapshot, "reduce must not mutate its input");
    }
}
