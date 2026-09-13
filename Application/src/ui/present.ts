/**
 * Pure presentation logic: snapshot in, display model out. No DOM, no IPC.
 * This is the part the frontend tests exercise.
 *
 * The rules from Instruction.md section 4 that this file is responsible for:
 *  - rule 1: a pending command NEVER renders as its new value
 *  - rule 2: every displayed value carries its age, and goes visibly stale
 *  - rule 3: disconnected is a defined appearance, not an error toast
 *  - rule 6: the cancel control is never reported as disabled
 */

import type { Command, CommandState, LogEntry, Snapshot } from '../types';

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
  detail: string;
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

export function formatLatency(ms: number | null): string {
  if (ms === null || ms === undefined) return '';
  return `${ms} ms`;
}

/** Rule 3: connection is a first-class, always-rendered state. */
export function bannerFor(s: Snapshot): Banner {
  const c = s.state.connection;

  if (c.kind === 'unknown') {
    return {
      label: 'Connecting',
      tone: 'idle',
      detail: `no reply yet from ${s.baseUrl}`,
    };
  }

  if (c.kind === 'offline') {
    const seen = formatAge(s.ageMs);
    return {
      label: 'Disconnected',
      tone: 'bad',
      detail: seen
        ? `${c.reason} · last seen ${seen}`
        : `${c.reason} · never reached`,
    };
  }

  // Online, but the last sighting has aged out: do not present it as live.
  if (s.isStale) {
    return {
      label: 'Stale',
      tone: 'stale',
      detail: `no confirmation for ${formatAge(s.ageMs)} · ${s.baseUrl}`,
    };
  }

  return {
    label: 'Connected',
    tone: 'ok',
    detail: `reachable ${formatAge(s.ageMs)} · ${s.baseUrl}`,
  };
}

function labelOf(command: Command): string {
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

/**
 * Rule 6: the cancel control is never disabled - not while a request is in
 * flight, not while disconnected, not while another control is pending.
 *
 * The guarantee comes from the core (`AppState::can_abort`), which is the only
 * place that decides policy. The view renders the answer; it does not invent it.
 */
export function cancelEnabled(s: Snapshot): boolean {
  return s.canAbort;
}

export function logLine(e: LogEntry): string {
  const t = new Date(e.atMs).toLocaleTimeString();
  const latency = e.latencyMs !== null ? ` (${formatLatency(e.latencyMs)})` : '';
  return `${t}  ${e.message}${latency}`;
}

export function toneForLog(kind: LogEntry['kind']): Tone {
  switch (kind) {
    case 'commandOk':
    case 'probeOk':
      return 'ok';
    case 'commandFail':
    case 'probeFail':
      return 'bad';
    case 'commandSent':
      return 'pending';
    default:
      return 'idle';
  }
}
