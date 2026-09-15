//! Shared core state: the reducer's current value, the request log, the alarm
//! schedule, and the HTTP client. Lives here rather than in `main.rs` so that
//! `main.rs` stays wiring only, per Instruction.md section 8.
//!
//! Nothing in here decides device behaviour - that is `state.rs` - and nothing
//! decides scheduling - that is `alarm.rs`. This is the seam that holds those
//! pure pieces, applies timestamps, persists what must survive a restart, and
//! pushes snapshots to the frontend.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::alarm::{self, Alarm, LocalNow, Outcome, MAX_ALARMS};
use crate::api_client::{ApiClient, BaseUrlSource, Config};
use crate::auth;
use crate::state::{reduce, AppState, Command, Event, Failure};

/// The log pane is gone from the UI, but the core keeps a short ring buffer:
/// it is still the only record of what was sent when something goes wrong.
const LOG_CAPACITY: usize = 100;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogKind {
    CommandSent,
    CommandOk,
    CommandFail,
    ProbeOk,
    ProbeFail,
    Info,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub seq: u64,
    pub at_ms: u64,
    pub kind: LogKind,
    pub message: String,
    pub latency_ms: Option<u64>,
}

/// An alarm plus the derived fields the UI needs, so the view does no
/// scheduling arithmetic of its own.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlarmView {
    #[serde(flatten)]
    pub alarm: Alarm,
    pub minutes_until_next: Option<u32>,
}

/// What the frontend renders. It holds no URL-building logic and no credential -
/// `has_credential` is a boolean, never the secret (rule 7).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub state: AppState,
    pub now_ms: u64,
    pub age_ms: Option<u64>,
    pub is_stale: bool,
    pub base_url: String,
    pub base_url_source: BaseUrlSource,
    pub poll_interval_ms: u64,
    pub command_timeout_ms: u64,
    pub probe_timeout_ms: u64,
    pub stale_after_ms: u64,
    pub username: String,
    pub has_credential: bool,
    /// Rule 6. Always true, and published so the view does not decide it.
    pub can_abort: bool,
    pub alarms: Vec<AlarmView>,
    pub tz_offset_minutes: i32,
}

struct Inner {
    client: ApiClient,
    state: AppState,
    log: VecDeque<LogEntry>,
    seq: u64,
    username: String,
    has_credential: bool,
    base_url_source: BaseUrlSource,
    alarms: Vec<Alarm>,
    next_alarm_id: u64,
    /// Minutes to ADD to UTC for local time. Supplied by the view, because the
    /// standard library has no timezone database. Until it arrives, 0 (UTC).
    tz_offset_minutes: i32,
    alarms_path: PathBuf,
}

#[derive(Clone)]
pub struct Shared {
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
}

/// Wall-clock milliseconds. The frontend compares against `Date.now()`, so the
/// two must share this base.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Shared {
    pub fn new(app: AppHandle, config: Config, base_url_source: BaseUrlSource) -> Result<Self, String> {
        let stale_after_ms = config.stale_after.as_millis() as u64;
        let client = ApiClient::new(config)?;
        let state = AppState {
            stale_after_ms,
            ..AppState::default()
        };

        let alarms_path = app
            .path()
            .app_config_dir()
            .map(|d| d.join("alarms.json"))
            .unwrap_or_else(|_| PathBuf::from("alarms.json"));

        let alarms = load_alarms(&alarms_path);
        let next_alarm_id = alarms.iter().map(|a| a.id).max().unwrap_or(0) + 1;

        Ok(Shared {
            app,
            inner: Arc::new(Mutex::new(Inner {
                client,
                state,
                log: VecDeque::with_capacity(LOG_CAPACITY),
                seq: 0,
                username: String::new(),
                has_credential: false,
                base_url_source,
                alarms,
                next_alarm_id,
                tz_offset_minutes: 0,
                alarms_path,
            })),
        })
    }

    /// A cheap clone so callers never hold the lock across an await.
    pub fn client(&self) -> ApiClient {
        self.inner.lock().unwrap().client.clone()
    }

    pub fn poll_interval(&self) -> Duration {
        self.inner.lock().unwrap().client.config().poll_interval
    }

    pub fn is_pending(&self) -> bool {
        self.inner.lock().unwrap().state.is_pending()
    }

    pub fn username(&self) -> String {
        self.inner.lock().unwrap().username.clone()
    }

    pub fn set_credential_presence(&self, username: String, present: bool) {
        {
            let mut g = self.inner.lock().unwrap();
            g.username = username;
            g.has_credential = present;
        }
        self.emit_snapshot();
    }

    // ----------------------------------------------------------- clock ----

    pub fn set_tz_offset(&self, minutes: i32) {
        let changed = {
            let mut g = self.inner.lock().unwrap();
            let changed = g.tz_offset_minutes != minutes;
            g.tz_offset_minutes = minutes;
            changed
        };
        if changed {
            self.emit_snapshot();
        }
    }

    pub fn local_now(&self) -> LocalNow {
        let offset = self.inner.lock().unwrap().tz_offset_minutes;
        alarm::local_now(now_ms(), offset)
    }

    // ---------------------------------------------------------- alarms ----

    pub fn alarms(&self) -> Vec<Alarm> {
        self.inner.lock().unwrap().alarms.clone()
    }

    pub fn add_alarm(&self, hour: u8, minute: u8, command: Command) -> Result<u64, String> {
        if !alarm::valid_time(hour, minute) {
            return Err(format!("{hour:02}:{minute:02} is not a valid time"));
        }
        let id = {
            let mut g = self.inner.lock().unwrap();
            if g.alarms.len() >= MAX_ALARMS {
                return Err(format!("at most {MAX_ALARMS} alarms"));
            }
            let id = g.next_alarm_id;
            g.next_alarm_id += 1;
            g.alarms.push(Alarm::new(id, hour, minute, command));
            g.alarms.sort_by_key(|a| a.target_minute());
            id
        };
        self.persist_alarms();
        self.log(
            LogKind::Info,
            format!("alarm added: {hour:02}:{minute:02} sends {}", command.path()),
            None,
        );
        self.emit_snapshot();
        Ok(id)
    }

    pub fn set_alarm_enabled(&self, id: u64, enabled: bool) -> Result<(), String> {
        {
            let mut g = self.inner.lock().unwrap();
            let a = g
                .alarms
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or_else(|| format!("no alarm {id}"))?;
            a.enabled = enabled;
            // Re-arming clears today's resolution so it can still fire today.
            if enabled {
                a.last_day = None;
                a.last_outcome = None;
            }
        }
        self.persist_alarms();
        self.emit_snapshot();
        Ok(())
    }

    pub fn remove_alarm(&self, id: u64) -> Result<(), String> {
        {
            let mut g = self.inner.lock().unwrap();
            let before = g.alarms.len();
            g.alarms.retain(|a| a.id != id);
            if g.alarms.len() == before {
                return Err(format!("no alarm {id}"));
            }
        }
        self.persist_alarms();
        self.emit_snapshot();
        Ok(())
    }

    /// Consume this alarm for today and hand back the command it would send.
    ///
    /// Claiming happens BEFORE the send, so a failure cannot cause a retry -
    /// every command here actuates hardware. No outcome is recorded yet: what
    /// happened is only known once the send returns, and claiming a success
    /// early is exactly the optimistic lie rule 1 forbids.
    pub fn claim_alarm(&self, id: u64, day: i64) -> Option<Command> {
        let command = {
            let mut g = self.inner.lock().unwrap();
            let a = g.alarms.iter_mut().find(|a| a.id == id)?;
            a.last_day = Some(day);
            a.last_outcome = None;
            Some(a.command)
        };
        self.persist_alarms();
        self.emit_snapshot();
        command
    }

    /// Record what actually happened to a claimed alarm.
    ///
    /// `last_fired_ms` moves only on [`Outcome::Fired`], so the UI can never say
    /// "last sent" about a command the device did not confirm.
    pub fn record_alarm_outcome(&self, id: u64, outcome: Outcome) {
        {
            let mut g = self.inner.lock().unwrap();
            let Some(a) = g.alarms.iter_mut().find(|a| a.id == id) else {
                return;
            };
            a.last_outcome = Some(outcome);
            if outcome == Outcome::Fired {
                a.last_fired_ms = Some(now_ms());
            }
        }
        self.persist_alarms();
        self.emit_snapshot();
    }

    fn persist_alarms(&self) {
        let (path, alarms) = {
            let g = self.inner.lock().unwrap();
            (g.alarms_path.clone(), g.alarms.clone())
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(&alarms) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    // Not fatal: alarms still work this session, they just will
                    // not survive a restart. Say so rather than failing quietly.
                    self.log(
                        LogKind::Info,
                        format!("could not save alarms to {}: {e}", path.display()),
                        None,
                    );
                }
            }
            Err(e) => self.log(LogKind::Info, format!("could not encode alarms: {e}"), None),
        }
    }

    // ------------------------------------------------------------- log ----

    /// Feed an event through the pure reducer and publish the result.
    pub fn apply(&self, event: Event) {
        let at = now_ms();
        {
            let mut g = self.inner.lock().unwrap();
            g.state = reduce(&g.state, event, at);
        }
        self.emit_snapshot();
    }

    pub fn log(&self, kind: LogKind, message: String, latency_ms: Option<u64>) {
        let entry = {
            let mut g = self.inner.lock().unwrap();
            g.seq += 1;
            let entry = LogEntry {
                seq: g.seq,
                at_ms: now_ms(),
                kind,
                message,
                latency_ms,
            };
            g.log.push_back(entry.clone());
            while g.log.len() > LOG_CAPACITY {
                g.log.pop_front();
            }
            entry
        };
        let _ = self.app.emit("log", entry);
    }

    /// A probe result. Split out so a device that is simply off does not fill
    /// the buffer with identical failure lines - repeats are collapsed.
    pub fn log_probe(&self, kind: LogKind, message: String, latency_ms: Option<u64>) {
        let is_repeat = {
            let g = self.inner.lock().unwrap();
            match g.log.back() {
                Some(last) => {
                    matches!(
                        (last.kind, kind),
                        (LogKind::ProbeFail, LogKind::ProbeFail)
                            | (LogKind::ProbeOk, LogKind::ProbeOk)
                    ) && last.message == message
                }
                None => false,
            }
        };
        if is_repeat {
            return;
        }
        self.log(kind, message, latency_ms);
    }

    pub fn entries(&self) -> Vec<LogEntry> {
        self.inner.lock().unwrap().log.iter().cloned().collect()
    }

    // -------------------------------------------------------- snapshot ----

    pub fn snapshot(&self) -> Snapshot {
        let g = self.inner.lock().unwrap();
        let now = now_ms();
        let local = alarm::local_now(now, g.tz_offset_minutes);
        let config = g.client.config();

        Snapshot {
            age_ms: g.state.age_ms(now),
            is_stale: g.state.is_stale(now),
            state: g.state.clone(),
            now_ms: now,
            base_url: config.base_url.clone(),
            base_url_source: g.base_url_source,
            poll_interval_ms: config.poll_interval.as_millis() as u64,
            command_timeout_ms: config.timeouts.command.as_millis() as u64,
            probe_timeout_ms: config.timeouts.probe.as_millis() as u64,
            stale_after_ms: g.state.stale_after_ms,
            username: g.username.clone(),
            has_credential: g.has_credential,
            can_abort: g.state.can_abort(),
            tz_offset_minutes: g.tz_offset_minutes,
            alarms: g
                .alarms
                .iter()
                .map(|a| AlarmView {
                    minutes_until_next: alarm::minutes_until_next(a, local),
                    alarm: a.clone(),
                })
                .collect(),
        }
    }

    pub fn emit_snapshot(&self) {
        let _ = self.app.emit("snapshot", self.snapshot());
    }

    pub fn set_config(&self, config: Config) -> Result<(), String> {
        {
            let mut g = self.inner.lock().unwrap();
            g.state.stale_after_ms = config.stale_after.as_millis() as u64;
            g.client.set_config(config)?;
        }
        self.emit_snapshot();
        Ok(())
    }
}

fn load_alarms(path: &PathBuf) -> Vec<Alarm> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<Alarm>>(&text) {
        Ok(mut alarms) => {
            alarms.truncate(MAX_ALARMS);
            // A stored `last_day` from a previous run is kept: it is what stops
            // an alarm firing twice if the app is restarted the same day.
            alarms.sort_by_key(|a| a.target_minute());
            alarms
        }
        Err(e) => {
            eprintln!("ignoring unreadable {}: {e}", path.display());
            Vec::new()
        }
    }
}

/// Send exactly one command. Shared by the UI's `send_command` and the alarm
/// scheduler so both obey the same rules: pending first, never optimistic,
/// never retried.
pub async fn dispatch_command(shared: &Shared, command: Command) -> Result<(), String> {
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
