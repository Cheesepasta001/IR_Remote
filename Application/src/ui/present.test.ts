import { describe, expect, it } from 'vitest';

import type { Alarm, Snapshot } from '../types';
import {
  alarmLineFor,
  bannerFor,
  controlStatusFor,
  deviceLineFor,
  formatAge,
  formatClock,
  formatCountdown,
  parseClock,
  reage,
} from './present';

const BASE: Snapshot = {
  state: {
    connection: { kind: 'unknown' },
    command: { kind: 'idle' },
    staleAfterMs: 5000,
  },
  nowMs: 100_000,
  ageMs: null,
  isStale: true,
  baseUrl: 'http://127.0.0.1:8080',
  baseUrlSource: 'dotEnv',
  pollIntervalMs: 2000,
  commandTimeoutMs: 3000,
  probeTimeoutMs: 1500,
  staleAfterMs: 5000,
  username: '',
  hasCredential: false,
  canAbort: true,
  alarms: [],
  tzOffsetMinutes: 480,
};

function snap(patch: Partial<Snapshot>, statePatch: Partial<Snapshot['state']> = {}): Snapshot {
  return { ...BASE, ...patch, state: { ...BASE.state, ...statePatch } };
}

function alarm(patch: Partial<Alarm> = {}): Alarm {
  return {
    id: 1,
    hour: 7,
    minute: 0,
    enabled: true,
    command: 'Power',
    lastDay: null,
    lastOutcome: null,
    lastFiredMs: null,
    minutesUntilNext: 30,
    ...patch,
  };
}

describe('formatAge', () => {
  it('shows sub-second ages with a decimal, as rule 2 asks', () => {
    expect(formatAge(400)).toBe('0.4 s ago');
  });

  it('scales to seconds, minutes and hours', () => {
    expect(formatAge(12_000)).toBe('12 s ago');
    expect(formatAge(180_000)).toBe('3 m ago');
    expect(formatAge(7_200_000)).toBe('2 h ago');
  });

  it('returns null when nothing has been observed', () => {
    expect(formatAge(null)).toBeNull();
  });
});

describe('reage (rule 2: ages keep counting between snapshots)', () => {
  it('ages a sighting forward against the current clock', () => {
    const s = snap({}, { connection: { kind: 'online', lastSeenMs: 100_000 } });
    expect(reage(s, 100_400).ageMs).toBe(400);
    expect(reage(s, 103_000).ageMs).toBe(3000);
  });

  it('goes stale on its own once the threshold passes, with no new snapshot', () => {
    const s = snap({}, { connection: { kind: 'online', lastSeenMs: 100_000 } });
    expect(reage(s, 104_999).isStale).toBe(false);
    expect(reage(s, 105_001).isStale).toBe(true);
  });

  it('reports no age at all when the device was never seen', () => {
    const s = snap({}, { connection: { kind: 'unknown' } });
    expect(reage(s, 999_999).ageMs).toBeNull();
    expect(reage(s, 999_999).isStale).toBe(true);
  });

  it('never produces a negative age if the clock steps back', () => {
    const s = snap({}, { connection: { kind: 'online', lastSeenMs: 100_000 } });
    expect(reage(s, 99_000).ageMs).toBe(0);
  });
});

describe('connection banner: connected or not, and nothing else', () => {
  it('shows connecting before anything is observed', () => {
    expect(bannerFor(BASE).label).toBe('Connecting');
  });

  it('shows connected when a recent sighting exists', () => {
    const b = bannerFor(
      snap({ ageMs: 400, isStale: false }, { connection: { kind: 'online', lastSeenMs: 99_600 } }),
    );
    expect(b.label).toBe('Connected');
    expect(b.tone).toBe('ok');
  });

  it('shows disconnected when offline', () => {
    const b = bannerFor(
      snap(
        { ageMs: 30_000, isStale: true },
        { connection: { kind: 'offline', sinceMs: 80_000, reason: 'refused', lastSeenMs: 70_000 } },
      ),
    );
    expect(b.label).toBe('Disconnected');
    expect(b.tone).toBe('bad');
  });

  it('reads a stale sighting as disconnected, never as connected', () => {
    // Claiming "Connected" from a sighting that has aged out would assert
    // something the app no longer knows.
    const b = bannerFor(
      snap({ ageMs: 9000, isStale: true }, { connection: { kind: 'online', lastSeenMs: 91_000 } }),
    );
    expect(b.label).toBe('Disconnected');
  });

  it('carries no detail text at all', () => {
    // The banner is deliberately just the state now.
    expect(Object.keys(bannerFor(BASE)).sort()).toEqual(['label', 'tone']);
  });
});

describe('device line (rule 1: never optimistic)', () => {
  it('renders a pending command as pending, not as its new value', () => {
    const d = deviceLineFor(
      snap({}, { command: { kind: 'pending', command: 'Power', startedMs: 99_500 } }),
    );
    expect(d.tone).toBe('pending');
    expect(d.label).toContain('Sending');
    expect(d.label).not.toMatch(/confirmed|\bON\b|\bOFF\b/i);
  });

  it('only claims success once the device confirmed it', () => {
    const d = deviceLineFor(
      snap({}, { command: { kind: 'succeeded', command: 'Power', atMs: 99_800, latencyMs: 42 } }),
    );
    expect(d.label).toContain('confirmed by device');
    expect(d.age).toBe('0.2 s ago');
    expect(d.stale).toBe(false);
  });

  it('marks an old confirmation stale', () => {
    const d = deviceLineFor(
      snap({}, { command: { kind: 'succeeded', command: 'Power', atMs: 90_000, latencyMs: 42 } }),
    );
    expect(d.stale).toBe(true);
  });

  it('distinguishes a device refusal from unreachability', () => {
    const unreachable = deviceLineFor(
      snap(
        {},
        {
          command: {
            kind: 'failed',
            command: 'Power',
            atMs: 99_000,
            error: { kind: 'unreachable', detail: 'timed out' },
          },
        },
      ),
    );
    expect(unreachable.label).toContain('did not reach the device');

    const refused = deviceLineFor(
      snap(
        {},
        {
          command: {
            kind: 'failed',
            command: 'Silent',
            atMs: 99_000,
            error: { kind: 'deviceError', detail: 'HTTP 500 - ir send failed' },
          },
        },
      ),
    );
    expect(refused.label).toContain('refused');
  });
});

describe('controls', () => {
  it('is ready when nothing is in flight', () => {
    expect(controlStatusFor(BASE, 'Power')).toBe('ready');
  });

  it('marks the in-flight control pending and the rest blocked', () => {
    const s = snap({}, { command: { kind: 'pending', command: 'Power', startedMs: 99_000 } });
    expect(controlStatusFor(s, 'Power')).toBe('pending');
    expect(controlStatusFor(s, 'Silent')).toBe('blocked');
  });

  it('re-enables every control once the command settles', () => {
    const s = snap(
      {},
      { command: { kind: 'succeeded', command: 'Power', atMs: 99_000, latencyMs: 5 } },
    );
    expect(controlStatusFor(s, 'Power')).toBe('ready');
    expect(controlStatusFor(s, 'Silent')).toBe('ready');
  });
});

describe('clock formatting', () => {
  it('zero-pads for display and for <input type="time">', () => {
    expect(formatClock(7, 0)).toBe('07:00');
    expect(formatClock(23, 59)).toBe('23:59');
    expect(formatClock(0, 5)).toBe('00:05');
  });

  it('parses a valid time', () => {
    expect(parseClock('07:00')).toEqual({ hour: 7, minute: 0 });
    expect(parseClock(' 23:59 ')).toEqual({ hour: 23, minute: 59 });
  });

  it('rejects anything that is not a real time', () => {
    expect(parseClock('24:00')).toBeNull();
    expect(parseClock('12:60')).toBeNull();
    expect(parseClock('')).toBeNull();
    expect(parseClock('7')).toBeNull();
    expect(parseClock('abc')).toBeNull();
  });

  it('formats a countdown that reads as a countdown', () => {
    expect(formatCountdown(0)).toBe('due now');
    expect(formatCountdown(4)).toBe('in 4 m');
    expect(formatCountdown(60)).toBe('in 1 h');
    expect(formatCountdown(135)).toBe('in 2 h 15 m');
    expect(formatCountdown(null)).toBe('disabled');
  });
});

describe('alarm line', () => {
  it('says "Send Power", never "Turn on"', () => {
    // The device has only a toggle and reports no state, so the UI must not
    // promise an outcome it cannot deliver.
    const line = alarmLineFor(alarm({ command: 'Power' }));
    expect(line.action).toBe('Send Power');
    expect(line.action).not.toMatch(/turn on|turn off|switch on/i);
  });

  it('shows the time and the countdown', () => {
    const line = alarmLineFor(alarm({ hour: 7, minute: 5, minutesUntilNext: 135 }));
    expect(line.time).toBe('07:05');
    expect(line.next).toBe('in 2 h 15 m');
  });

  it('says a missed alarm was missed, and why', () => {
    const line = alarmLineFor(alarm({ lastOutcome: 'missed', lastDay: 10 }));
    expect(line.status).toContain('missed');
    expect(line.status).toContain('not running');
    expect(line.tone).toBe('bad');
  });

  it('reports a fired alarm as sent', () => {
    const line = alarmLineFor(
      alarm({ lastOutcome: 'fired', lastDay: 10, lastFiredMs: 1_700_000_000_000 }),
    );
    expect(line.status).toContain('last sent');
    expect(line.tone).toBe('ok');
  });

  it('shows a disabled alarm as disabled rather than counting down', () => {
    const line = alarmLineFor(alarm({ enabled: false, minutesUntilNext: null }));
    expect(line.next).toBe('disabled');
    expect(line.tone).toBe('idle');
  });

  it('names the command for non-Power alarms too', () => {
    expect(alarmLineFor(alarm({ command: 'LowTemp' })).action).toBe('Send Temp −');
    expect(alarmLineFor(alarm({ command: 'Silent' })).action).toBe('Send Silent');
  });
});

describe('a failed alarm is never reported as sent', () => {
  it('says the device did not confirm it', () => {
    // Claiming "last sent" for a command the device never acknowledged is the
    // same optimistic lie rule 1 forbids, just on a timer.
    const line = alarmLineFor(alarm({ lastOutcome: 'failed', lastDay: 10 }));
    expect(line.status).not.toMatch(/last sent/);
    expect(line.status).toContain('did not confirm');
    expect(line.tone).toBe('bad');
  });

  it('never shows a last-sent time even if one is somehow present', () => {
    const line = alarmLineFor(
      alarm({ lastOutcome: 'failed', lastDay: 10, lastFiredMs: 1_700_000_000_000 }),
    );
    expect(line.status).not.toMatch(/last sent/);
  });
});
