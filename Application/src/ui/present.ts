/**
 * Pure presentation logic: snapshot in, display model out. No DOM, no IPC.
 * This is the part the frontend tests exercise.
 *
 * The rules from Instruction.md section 4 that this file is responsible for:
 *  - rule 1: a pending command NEVER renders as its new value
 *  - rule 2: every displayed value carries its age, and goes visibly stale
 *  - rule 3: disconnected is a defined appearance, not an error toast
 */

import type { Alarm, Command, CommandState, Snapshot } from '../types';

export type Tone = 'ok' | 'pending' | 'bad' | 'stale' | 'idle';

/**
 * Re-derive `nowMs`, `ageMs` and `isStale` against the current clock.
 *
 * Rule 2 requires the age to keep counting between snapshots: if the device
 * stops answering, the core emits nothing, and an age frozen at the last
 * snapshot would keep looking fresh forever. The UI ticks this every 200 ms so
 * a value visibly ages and then goes stale on its own.
 */
export function reage(s: Snapshot, nowMs: number): Snapshot {
  const c = s.state.connection;
  const lastSeen =
    c.kind === 'online' ? c.lastSeenMs : c.kind === 'offline' ? c.lastSeenMs : null;

  const ageMs = lastSeen === null ? null : Math.max(0, nowMs - lastSeen);
  return {
    ...s,
    nowMs,
    ageMs,
    isStale: ageMs === null ? true : ageMs > s.staleAfterMs,
  };
}

export interface Banner {
  label: string;
  tone: Tone;
}

export interface DeviceLine {
  label: string;
  tone: Tone;
  age: string | null;
  stale: boolean;
}

/** Rule 2: ages read as ages, never as bare numbers. */
export function formatAge(ms: number | null): string | null {
  if (ms === null || ms === undefined) return null;
  if (ms < 0) return 'just now';
  if (ms < 1000) return `${(ms / 1000).toFixed(1)} s ago`;
  if (ms < 60_000) return `${Math.floor(ms / 1000)} s ago`;
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)} m ago`;
  return `${Math.floor(ms / 3_600_000)} h ago`;
}

/**
 * Rule 3: connection is a first-class, always-rendered state.
 *
 * Connected or not, and nothing else. A stale reading counts as NOT connected:
 * the last sighting has aged past the threshold, so claiming "Connected" would
 * be asserting something the app no longer knows.
 */
export function bannerFor(s: Snapshot): Banner {
  const c = s.state.connection;

  if (c.kind === 'unknown') return { label: 'Connecting', tone: 'idle' };
  if (c.kind === 'offline') return { label: 'Disconnected', tone: 'bad' };
  if (s.isStale) return { label: 'Disconnected', tone: 'bad' };
  return { label: 'Connected', tone: 'ok' };
}

export function labelOf(command: Command): string {
  switch (command) {
    case 'Power':
      return 'Power';
    case 'Silent':
      return 'Silent';
    case 'LowTemp':
      return 'Temp −';
    case 'HighTemp':
      return 'Temp +';
  }
}

function describeFailure(state: Extract<CommandState, { kind: 'failed' }>): string {
  const e = state.error;
  switch (e.kind) {
    case 'aborted':
      return `${labelOf(state.command)} cancelled`;
    case 'unreachable':
      return `${labelOf(state.command)} did not reach the device (${e.detail})`;
    case 'deviceError':
      return `${labelOf(state.command)} refused: ${e.detail}`;
    case 'badResponse':
      return `${labelOf(state.command)} got an unreadable reply: ${e.detail}`;
  }
}

/**
 * The "current device state" line (section 6.2).
 *
 * This device exposes NO readable state, so this reports the last command the
 * device actually confirmed - never an assumed switch position. A pending
 * command renders as pending (rule 1), never as its new value.
 */
export function deviceLineFor(s: Snapshot): DeviceLine {
  const c = s.state.command;

  if (c.kind === 'idle') {
    return { label: 'No command sent yet', tone: 'idle', age: null, stale: false };
  }

  if (c.kind === 'pending') {
    // Rule 1. Under no circumstance does this say the command took effect.
    return {
      label: `Sending ${labelOf(c.command)}…`,
      tone: 'pending',
      age: formatAge(Math.max(0, s.nowMs - c.startedMs)),
      stale: false,
    };
  }

  if (c.kind === 'succeeded') {
    const age = s.nowMs - c.atMs;
    return {
      label: `${labelOf(c.command)} confirmed by device`,
      tone: 'ok',
      age: formatAge(age),
      // Rule 2: an old confirmation must stop looking live.
      stale: age > s.staleAfterMs,
    };
  }

  const age = s.nowMs - c.atMs;
  return {
    label: describeFailure(c),
    tone: 'bad',
    age: formatAge(age),
    stale: age > s.staleAfterMs,
  };
}

/**
 * A permanent caveat under the device line. The API has no state endpoint, so
 * the app must never imply it knows whether the unit is on.
 */
export const NO_STATE_CAVEAT =
  'This device reports no state. The app shows only what it confirmed sending — not whether the air conditioner is on.';

export type ControlStatus = 'ready' | 'pending' | 'blocked';

/** Controls gate on a command already being in flight, nothing else. */
export function controlStatusFor(s: Snapshot, command: Command): ControlStatus {
  const c = s.state.command;
  if (c.kind !== 'pending') return 'ready';
  return c.command === command ? 'pending' : 'blocked';
}

// ------------------------------------------------------------- alarms ----

/** `07:05`, zero-padded, for both display and the <input type="time"> value. */
export function formatClock(hour: number, minute: number): string {
  return `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`;
}

/** Parse an <input type="time"> value. Returns null if it is not a valid time. */
export function parseClock(value: string): { hour: number; minute: number } | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(value.trim());
  if (!m) return null;
  const hour = Number(m[1]);
  const minute = Number(m[2]);
  if (!Number.isInteger(hour) || !Number.isInteger(minute)) return null;
  if (hour < 0 || hour > 23 || minute < 0 || minute > 59) return null;
  return { hour, minute };
}

/** "in 2 h 15 m", "in 4 m", "now". */
export function formatCountdown(minutes: number | null): string {
  if (minutes === null) return 'disabled';
  if (minutes <= 0) return 'due now';
  if (minutes < 60) return `in ${minutes} m`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m === 0 ? `in ${h} h` : `in ${h} h ${m} m`;
}

export interface AlarmLine {
  time: string;
  /** Deliberately "Send Power", never "Turn on" - see types.ts. */
  action: string;
  next: string;
  status: string;
  tone: Tone;
}

export function alarmLineFor(a: Alarm): AlarmLine {
  let status = 'never fired';
  let tone: Tone = 'idle';

  if (a.lastOutcome === 'fired') {
    status = a.lastFiredMs
      ? `last sent ${new Date(a.lastFiredMs).toLocaleString()}`
      : 'last sent today';
    tone = 'ok';
  } else if (a.lastOutcome === 'failed') {
    // Sent at the right time, but never confirmed. Saying "sent" here would
    // claim something the device never acknowledged.
    status = 'tried, but the device did not confirm it';
    tone = 'bad';
  } else if (a.lastOutcome === 'missed') {
    // Being explicit matters: the user must not assume it ran.
    status = 'missed — the app was not running at that time';
    tone = 'bad';
  }

  return {
    time: formatClock(a.hour, a.minute),
    action: `Send ${labelOf(a.command)}`,
    next: a.enabled ? formatCountdown(a.minutesUntilNext) : 'disabled',
    status,
    tone: a.enabled ? tone : 'idle',
  };
}

/**
 * The standing warning under the alarm list.
 *
 * Every command this device accepts actuates hardware, and `/Power` is a
 * toggle, so an alarm cannot promise the unit ends up on.
 */
export interface ExactAlarmWarning {
  /** Empty when there is nothing to say. */
  text: string;
  /** Whether to offer the button that opens Android's permission screen. */
  canRequest: boolean;
}

/**
 * Android can withhold exact-alarm permission, in which case the OS batches
 * alarms and they drift by minutes. Saying nothing would leave the UI promising
 * an "in 8 h 55 m" countdown it cannot actually keep.
 */
export function exactAlarmWarningFor(s: Snapshot): ExactAlarmWarning {
  if (s.exactAlarms === null || s.exactAlarms === true) {
    return { text: '', canRequest: false };
  }
  return {
    text:
      'Exact alarms are not permitted, so Android may fire these minutes late rather than on the minute.',
    canRequest: true,
  };
}

export const ALARM_CAVEAT =
  'Alarms send a command at a time — they cannot guarantee the unit ends up on or off. /Power is a toggle, so if the air conditioner is already running when a Power alarm fires, it switches off instead.';
