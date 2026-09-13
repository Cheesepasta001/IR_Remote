//! Background reachability polling. Runs in the core and pushes updates to the
//! frontend as Tauri events - the frontend never polls (Instruction.md section 5).
//!
//! What this polls is a TCP handshake, not an HTTP endpoint. The device exposes
//! no readable state and every HTTP path it serves fires an IR code, so an HTTP
//! poll would toggle the air conditioner on every interval. See
//! `ApiClient::probe`.

use std::time::Duration;

use tokio::time::{interval, MissedTickBehavior};

use crate::core::{LogKind, Shared};
use crate::state::Event;

/// Bounded, backed-off retry for the probe. Permitted by rule 5 only because a
/// TCP connect actuates nothing - it is genuinely idempotent.
const PROBE_ATTEMPTS: u32 = 3;
const PROBE_BACKOFF_BASE: Duration = Duration::from_millis(200);

pub fn spawn(shared: Shared) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = interval(shared.poll_interval());
        // If a probe overruns the interval, do not then fire a burst of
        // catch-up probes at a device that serves one client at a time.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;

            let configured = shared.poll_interval();
            if configured != ticker.period() {
                ticker = interval(configured);
                ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            }

            // Do not contend with a command in flight: the ESP32 accepts one
            // client at a time, so probing now would just make the command
            // time out.
            if shared.is_pending() {
                continue;
            }

            run_probe(&shared).await;
        }
    });
}

async fn run_probe(shared: &Shared) {
    let client = shared.client();
    let mut last_error = String::new();

    for attempt in 0..PROBE_ATTEMPTS {
        match client.probe().await {
            Ok(latency_ms) => {
                shared.apply(Event::ProbeSucceeded);
                shared.log_probe(LogKind::ProbeOk, "reachable".to_string(), Some(latency_ms));
                return;
            }
            Err(e) => {
                last_error = e;
                if attempt + 1 < PROBE_ATTEMPTS {
                    // Exponential backoff: 200ms, 400ms.
                    let wait = PROBE_BACKOFF_BASE * 2u32.pow(attempt);
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }

    shared.apply(Event::ProbeFailed(last_error.clone()));
    shared.log_probe(LogKind::ProbeFail, last_error, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_budget_is_bounded() {
        // Rule 5: retries must be bounded. An unbounded probe loop would hammer
        // a device that is legitimately off.
        assert!(PROBE_ATTEMPTS >= 1 && PROBE_ATTEMPTS <= 5);
    }

    #[test]
    fn backoff_grows() {
        let first = PROBE_BACKOFF_BASE * 2u32.pow(0);
        let second = PROBE_BACKOFF_BASE * 2u32.pow(1);
        assert!(second > first, "backoff must actually back off");
    }
}
