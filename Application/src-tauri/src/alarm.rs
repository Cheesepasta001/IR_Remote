//! Daily alarm scheduling. Pure: no I/O, no clock reads, no filesystem.
//!
//! The caller supplies the current local time, exactly as `state.rs` takes a
//! timestamp, so every rule below is testable without waiting for a clock.
//!
//! ## What an alarm can and cannot promise
//!
//! The device exposes a single `/Power` **toggle** and reports no state, so an
//! alarm cannot mean "turn on". It means "send this command at this time". If
//! the unit was already running when a Power alarm fires, it switches off, and
//! nothing in the app can detect that. The UI says "Send Power at 07:00" rather
//! than "Turn on at 07:00" for that reason. Discrete on/off IR codes in the
//! firmware are what would make this idempotent - see README.
//!
//! ## Missed alarms are skipped, never fired late
//!
//! A desktop app cannot wake itself. If the app was closed through an alarm's
//! time, that alarm resolves as [`Outcome::Missed`] and is not sent. Firing a
//! Power toggle hours late is worse than not firing it at all.

use serde::{Deserialize, Serialize};

use crate::state::Command;

/// How late an alarm may fire and still count as on time. The scheduler ticks
/// far more often than this; the window only covers a tick that was delayed.
pub const GRACE_MINUTES: u32 = 2;

/// Upper bound on stored alarms, so a stuck UI cannot grow the file forever.
pub const MAX_ALARMS: usize = 32;

/// What `due()` decides should happen to an alarm right now. A decision, not a
/// result - the result is [`Outcome`], recorded once the send has been tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Its time is now (within the grace window): send it.
    Fire,
    /// Its time passed unobserved: consume it for today without sending.
    Skip,
}

/// What actually happened, as recorded against the alarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    /// The command was sent and the device confirmed it.
    Fired,
    /// It was sent at the right time but the device never confirmed it.
    /// Distinct from `Fired` on purpose: the UI must not claim it was delivered.
    Failed,
    /// Its time passed while the app was not running, so it was skipped.
    Missed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alarm {
    pub id: u64,
    pub hour: u8,
    pub minute: u8,
    pub enabled: bool,
    pub command: Command,
    /// Local day number on which this alarm last resolved, fired or missed.
    /// Keeps a daily alarm from firing twice in one day.
    #[serde(default)]
    pub last_day: Option<i64>,
    #[serde(default)]
    pub last_outcome: Option<Outcome>,
    /// When it last actually fired, for the UI. Wall-clock ms.
    #[serde(default)]
    pub last_fired_ms: Option<u64>,
}

impl Alarm {
    pub fn new(id: u64, hour: u8, minute: u8, command: Command) -> Self {
        Alarm {
            id,
            hour,
            minute,
            enabled: true,
            command,
            last_day: None,
            last_outcome: None,
            last_fired_ms: None,
        }
    }

    /// Minutes past local midnight.
    pub fn target_minute(&self) -> u32 {
        self.hour as u32 * 60 + self.minute as u32
    }
}

/// Local wall-clock, reduced to the two numbers scheduling actually needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalNow {
    /// Days since the Unix epoch, in local time.
    pub day: i64,
    /// Minutes past local midnight, 0..1440.
    pub minute_of_day: u32,
}

/// Convert UTC milliseconds to local day + minute.
///
/// `tz_offset_minutes` is minutes to ADD to UTC to get local time, i.e. the
/// negation of JavaScript's `Date.getTimezoneOffset()`. Rust's standard library
/// has no timezone database, so the view reports the offset and the core does
/// all the scheduling - the offset is environment data, not policy.
pub fn local_now(utc_ms: u64, tz_offset_minutes: i32) -> LocalNow {
    let local = utc_ms as i64 + (tz_offset_minutes as i64) * 60_000;
    LocalNow {
        // Euclidean division so pre-epoch and negative offsets still floor.
        day: local.div_euclid(86_400_000),
        minute_of_day: (local.rem_euclid(86_400_000) / 60_000) as u32,
    }
}

pub fn valid_time(hour: u8, minute: u8) -> bool {
    hour < 24 && minute < 60
}

/// Which alarms resolve at this instant, and how.
///
/// Pure - it does not mutate the alarms. The caller applies the outcomes, which
/// is what makes every branch here testable.
pub fn due(alarms: &[Alarm], now: LocalNow, grace_minutes: u32) -> Vec<(u64, Due)> {
    let mut out = Vec::new();

    for a in alarms {
        if !a.enabled {
            continue;
        }
        // Already resolved today: a daily alarm fires at most once per day.
        if a.last_day == Some(now.day) {
            continue;
        }

        let target = a.target_minute();
        if now.minute_of_day < target {
            continue; // still ahead of us today
        }

        if now.minute_of_day <= target.saturating_add(grace_minutes) {
            out.push((a.id, Due::Fire));
        } else {
            // Its moment passed while we were not watching.
            out.push((a.id, Due::Skip));
        }
    }

    out
}

/// Minutes until this alarm is next due, for the UI. `None` when disabled.
pub fn minutes_until_next(alarm: &Alarm, now: LocalNow) -> Option<u32> {
    if !alarm.enabled {
        return None;
    }
    let target = alarm.target_minute();
    let resolved_today = alarm.last_day == Some(now.day);

    if !resolved_today && now.minute_of_day <= target {
        Some(target - now.minute_of_day)
    } else {
        // Tomorrow.
        Some(1440 - now.minute_of_day + target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u8, minute: u8) -> Alarm {
        Alarm::new(1, hour, minute, Command::Power)
    }

    fn now(day: i64, hour: u32, minute: u32) -> LocalNow {
        LocalNow {
            day,
            minute_of_day: hour * 60 + minute,
        }
    }

    // ------------------------------------------------------------ clock ----

    #[test]
    fn local_now_applies_a_positive_offset() {
        // 1970-01-02T00:00:00Z, +8h -> 08:00 local on day 1.
        let l = local_now(86_400_000, 8 * 60);
        assert_eq!(l.day, 1);
        assert_eq!(l.minute_of_day, 8 * 60);
    }

    #[test]
    fn local_now_applies_a_negative_offset_across_midnight() {
        // 1970-01-02T02:00:00Z, -5h -> 21:00 local on day 0, not day 1.
        let l = local_now(86_400_000 + 2 * 3_600_000, -5 * 60);
        assert_eq!(l.day, 0, "a negative offset must roll the day back");
        assert_eq!(l.minute_of_day, 21 * 60);
    }

    #[test]
    fn local_now_handles_midnight_exactly() {
        let l = local_now(86_400_000, 0);
        assert_eq!(l.day, 1);
        assert_eq!(l.minute_of_day, 0);
    }

    // ------------------------------------------------------- validation ----

    #[test]
    fn times_are_validated() {
        assert!(valid_time(0, 0));
        assert!(valid_time(23, 59));
        assert!(!valid_time(24, 0));
        assert!(!valid_time(12, 60));
    }

    // ------------------------------------------------------------- due ----

    #[test]
    fn does_not_fire_before_its_time() {
        let a = at(7, 0);
        assert!(due(&[a], now(10, 6, 59), GRACE_MINUTES).is_empty());
    }

    #[test]
    fn fires_at_its_time() {
        let a = at(7, 0);
        assert_eq!(
            due(&[a], now(10, 7, 0), GRACE_MINUTES),
            vec![(1, Due::Fire)]
        );
    }

    #[test]
    fn fires_within_the_grace_window() {
        let a = at(7, 0);
        assert_eq!(
            due(&[a.clone()], now(10, 7, 2), GRACE_MINUTES),
            vec![(1, Due::Fire)]
        );
    }

    #[test]
    fn is_missed_rather_than_fired_late() {
        // The app was closed at 07:00 and started at 10:00. A Power toggle
        // three hours late is worse than none.
        let a = at(7, 0);
        assert_eq!(
            due(&[a], now(10, 10, 0), GRACE_MINUTES),
            vec![(1, Due::Skip)]
        );
    }

    #[test]
    fn does_not_fire_twice_in_one_day() {
        let mut a = at(7, 0);
        a.last_day = Some(10);
        a.last_outcome = Some(Outcome::Fired);
        assert!(due(&[a], now(10, 7, 1), GRACE_MINUTES).is_empty());
    }

    #[test]
    fn fires_again_the_next_day() {
        let mut a = at(7, 0);
        a.last_day = Some(10);
        a.last_outcome = Some(Outcome::Fired);
        assert_eq!(
            due(&[a], now(11, 7, 0), GRACE_MINUTES),
            vec![(1, Due::Fire)]
        );
    }

    #[test]
    fn a_disabled_alarm_never_resolves() {
        let mut a = at(7, 0);
        a.enabled = false;
        assert!(due(&[a.clone()], now(10, 7, 0), GRACE_MINUTES).is_empty());
        assert!(due(&[a], now(10, 23, 0), GRACE_MINUTES).is_empty());
    }

    #[test]
    fn just_after_midnight_a_morning_alarm_is_not_missed() {
        // 00:01 with a 07:00 alarm: it is ahead of us, not skipped.
        let a = at(7, 0);
        assert!(due(&[a], now(11, 0, 1), GRACE_MINUTES).is_empty());
    }

    #[test]
    fn a_midnight_alarm_fires_at_midnight() {
        let a = at(0, 0);
        assert_eq!(
            due(&[a], now(11, 0, 0), GRACE_MINUTES),
            vec![(1, Due::Fire)]
        );
    }

    #[test]
    fn several_alarms_resolve_independently() {
        let on = Alarm::new(1, 7, 0, Command::Power);
        let off = Alarm::new(2, 9, 0, Command::Power);
        let quiet = Alarm::new(3, 22, 0, Command::Silent);

        let got = due(&[on, off, quiet], now(10, 9, 0), GRACE_MINUTES);
        assert_eq!(
            got,
            vec![(1, Due::Skip), (2, Due::Fire)],
            "the 07:00 was slept through, the 09:00 is on time, the 22:00 is ahead"
        );
    }

    // ------------------------------------------------- next occurrence ----

    #[test]
    fn counts_down_to_today() {
        let a = at(7, 0);
        assert_eq!(minutes_until_next(&a, now(10, 6, 30)), Some(30));
    }

    #[test]
    fn counts_down_to_tomorrow_once_resolved() {
        let mut a = at(7, 0);
        a.last_day = Some(10);
        // 08:00 today, already fired -> 23 hours to 07:00 tomorrow.
        assert_eq!(minutes_until_next(&a, now(10, 8, 0)), Some(23 * 60));
    }

    #[test]
    fn a_disabled_alarm_has_no_next_time() {
        let mut a = at(7, 0);
        a.enabled = false;
        assert_eq!(minutes_until_next(&a, now(10, 6, 0)), None);
    }

    #[test]
    fn every_alarm_command_is_still_non_idempotent() {
        // The whole honesty argument for "Send Power at 07:00" rests on this.
        let a = at(7, 0);
        assert!(
            !a.command.is_idempotent(),
            "if a command ever becomes idempotent, the alarm wording can change"
        );
    }
}
