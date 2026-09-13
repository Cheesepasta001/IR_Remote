/**
 * Shared types mirroring the API and the Rust core's serialization.
 *
 * The frontend is a view (Instruction.md section 5): nothing here builds a URL,
 * holds a credential, or decides retry or scheduling policy. `Snapshot` is
 * everything it knows.
 */

/** The four commands from Instruction.md section 1. */
export type Command = 'Power' | 'Silent' | 'LowTemp' | 'HighTemp';

export const COMMANDS: { id: Command; label: string; path: string }[] = [
  { id: 'Power', label: 'Power', path: '/Power' },
  { id: 'Silent', label: 'Silent', path: '/Silent' },
  { id: 'LowTemp', label: 'Temp −', path: '/Low_Temp' },
  { id: 'HighTemp', label: 'Temp +', path: '/High_Temp' },
];

export type Failure =
  | { kind: 'unreachable'; detail: string }
  | { kind: 'deviceError'; detail: string }
  | { kind: 'badResponse'; detail: string }
  | { kind: 'aborted' };

export type Connection =
  | { kind: 'unknown' }
  | { kind: 'online'; lastSeenMs: number }
  | { kind: 'offline'; sinceMs: number; reason: string; lastSeenMs: number | null };

export type CommandState =
  | { kind: 'idle' }
  | { kind: 'pending'; command: Command; startedMs: number }
  | { kind: 'succeeded'; command: Command; atMs: number; latencyMs: number }
  | { kind: 'failed'; command: Command; atMs: number; error: Failure };

export interface AppState {
  connection: Connection;
  command: CommandState;
  staleAfterMs: number;
}

/** Where the base URL came from. It is read once at startup, not editable. */
export type BaseUrlSource = 'environment' | 'dotEnv' | 'default';

export type AlarmOutcome = 'fired' | 'failed' | 'missed';

/**
 * A daily alarm.
 *
 * It sends a command at a time — it cannot mean "turn on". `/Power` is a
 * toggle and the device reports no state, so the UI must say "Send Power",
 * never "Turn on". See `alarm.rs` for the full reasoning.
 */
export interface Alarm {
  id: number;
  hour: number;
  minute: number;
  enabled: boolean;
  command: Command;
  lastDay: number | null;
  lastOutcome: AlarmOutcome | null;
  lastFiredMs: number | null;
  /** Derived in the core so the view does no scheduling arithmetic. */
  minutesUntilNext: number | null;
}

export interface Snapshot {
  state: AppState;
  nowMs: number;
  ageMs: number | null;
  isStale: boolean;
  baseUrl: string;
  baseUrlSource: BaseUrlSource;
  pollIntervalMs: number;
  commandTimeoutMs: number;
  probeTimeoutMs: number;
  staleAfterMs: number;
  username: string;
  /** Whether a credential exists. Never the credential itself (rule 7). */
  hasCredential: boolean;
  /** Rule 6. The core decides this; the view only renders it. */
  canAbort: boolean;
  alarms: Alarm[];
  tzOffsetMinutes: number;
}

export type LogKind =
  | 'commandSent'
  | 'commandOk'
  | 'commandFail'
  | 'probeOk'
  | 'probeFail'
  | 'info';

/** Still recorded by the core, no longer rendered. */
export interface LogEntry {
  seq: number;
  atMs: number;
  kind: LogKind;
  message: string;
  latencyMs: number | null;
}
