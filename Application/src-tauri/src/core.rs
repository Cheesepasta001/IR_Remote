//! Shared core state: the reducer's current value, the request log, and the
//! HTTP client. Lives here rather than in `main.rs` so that `main.rs` stays
//! wiring only, per Instruction.md section 8.
//!
//! Nothing in here makes a decision about device behaviour - that is `state.rs`.
//! This is the seam that holds the pure state machine, applies timestamps, and
//! pushes snapshots to the frontend.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::api_client::{ApiClient, Config};
use crate::state::{reduce, AppState, Event};

/// Section 6.5: the log pane keeps roughly the last 100 entries.
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
    pub poll_interval_ms: u64,
    pub command_timeout_ms: u64,
    pub probe_timeout_ms: u64,
    pub stale_after_ms: u64,
    pub username: String,
    pub has_credential: bool,
    /// Rule 6. Always true, and published so the view does not decide it.
    pub can_abort: bool,
}

struct Inner {
    client: ApiClient,
    state: AppState,
    log: VecDeque<LogEntry>,
    seq: u64,
    username: String,
    has_credential: bool,
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
    pub fn new(app: AppHandle, config: Config) -> Result<Self, String> {
        let stale_after_ms = config.stale_after.as_millis() as u64;
        let client = ApiClient::new(config)?;
        let state = AppState {
            stale_after_ms,
            ..AppState::default()
        };

        Ok(Shared {
            app,
            inner: Arc::new(Mutex::new(Inner {
                client,
                state,
                log: VecDeque::with_capacity(LOG_CAPACITY),
                seq: 0,
                username: String::new(),
                has_credential: false,
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
        let mut g = self.inner.lock().unwrap();
        g.username = username;
        g.has_credential = present;
        drop(g);
        self.emit_snapshot();
    }

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
    /// the pane with identical failure lines - repeats are collapsed.
    pub fn log_probe(&self, kind: LogKind, message: String, latency_ms: Option<u64>) {
        let is_repeat = {
            let g = self.inner.lock().unwrap();
            match g.log.back() {
                Some(last) => {
                    matches!(
                        (last.kind, kind),
                        (LogKind::ProbeFail, LogKind::ProbeFail) | (LogKind::ProbeOk, LogKind::ProbeOk)
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

    pub fn snapshot(&self) -> Snapshot {
        let g = self.inner.lock().unwrap();
        let now = now_ms();
        let config = g.client.config();
        Snapshot {
            age_ms: g.state.age_ms(now),
            is_stale: g.state.is_stale(now),
            state: g.state.clone(),
            now_ms: now,
            base_url: config.base_url.clone(),
            poll_interval_ms: config.poll_interval.as_millis() as u64,
            command_timeout_ms: config.timeouts.command.as_millis() as u64,
            probe_timeout_ms: config.timeouts.probe.as_millis() as u64,
            stale_after_ms: g.state.stale_after_ms,
            username: g.username.clone(),
            has_credential: g.has_credential,
            can_abort: g.state.can_abort(),
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
