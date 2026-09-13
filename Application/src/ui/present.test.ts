import { describe, expect, it } from 'vitest';

import type { Snapshot } from '../types';
import {
  bannerFor,
  cancelEnabled,
  controlStatusFor,
  deviceLineFor,
  formatAge,
  logLine,
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
  pollIntervalMs: 2000,
  commandTimeoutMs: 3000,
  probeTimeoutMs: 1500,
  staleAfterMs: 5000,
  username: '',
  hasCredential: false,
  canAbort: true,
};

function snap(patch: Partial<Snapshot>, statePatch: Partial<Snapshot['state']> = {}): Snapshot {
  return { ...BASE, ...patch, state: { ...BASE.state, ...statePatch } };
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
    // The core has emitted nothing since; the UI must still notice.
    expect(reage(s, 104_999).isStale).toBe(false);
    expect(reage(s, 105_001).isStale).toBe(true);
  });

  it('keeps ageing from the last sighting while offline', () => {
    const s = snap(
      {},
      {
        connection: { kind: 'offline', sinceMs: 101_000, reason: 'refused', lastSeenMs: 100_000 },
      },
    );
    expect(reage(s, 130_000).ageMs).toBe(30_000);
    expect(reage(s, 130_000).isStale).toBe(true);
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

describe('connection banner (rule 3)', () => {
  it('shows connecting before anything is observed', () => {
    const b = bannerFor(BASE);
    expect(b.label).toBe('Connecting');
    expect(b.detail).toContain('http://127.0.0.1:8080');
  });

  it('shows connected with the age and base URL', () => {
    const b = bannerFor(
      snap({ ageMs: 400, isStale: false }, { connection: { kind: 'online', lastSeenMs: 99_600 } }),
    );
    expect(b.label).toBe('Connected');
    expect(b.tone).toBe('ok');
    expect(b.detail).toContain('0.4 s ago');
    expect(b.detail).toContain('http://127.0.0.1:8080');
  });

  it('goes stale rather than continuing to look live', () => {
    const b = bannerFor(
      snap({ ageMs: 9000, isStale: true }, { connection: { kind: 'online', lastSeenMs: 91_000 } }),
    );
    expect(b.label).toBe('Stale');
    expect(b.tone).toBe('stale');
  });

  it('has a defined disconnected appearance carrying the last sighting', () => {
    const b = bannerFor(
      snap(
        { ageMs: 30_000, isStale: true },
        {
          connection: {
            kind: 'offline',
            sinceMs: 80_000,
            reason: 'connection refused',
            lastSeenMs: 70_000,
          },
        },
      ),
    );
    expect(b.label).toBe('Disconnected');
    expect(b.tone).toBe('bad');
    expect(b.detail).toContain('connection refused');
    expect(b.detail).toContain('30 s ago');
  });

  it('says never reached when the device was never seen', () => {
    const b = bannerFor(
      snap(
        { ageMs: null },
        { connection: { kind: 'offline', sinceMs: 80_000, reason: 'refused', lastSeenMs: null } },
      ),
    );
    expect(b.detail).toContain('never reached');
  });
});

describe('device line (rule 1: never optimistic)', () => {
  it('renders a pending command as pending, not as its new value', () => {
    const d = deviceLineFor(
      snap({}, { command: { kind: 'pending', command: 'Power', startedMs: 99_500 } }),
    );
    expect(d.tone).toBe('pending');
    expect(d.label).toContain('Sending');
    // The critical assertion: nothing claims the command took effect.
    expect(d.label).not.toMatch(/confirmed|\bON\b|\bOFF\b/i);
  });

  it('only claims success once the device confirmed it', () => {
    const d = deviceLineFor(
      snap({}, { command: { kind: 'succeeded', command: 'Power', atMs: 99_800, latencyMs: 42 } }),
    );
    expect(d.label).toContain('confirmed by device');
    expect(d.tone).toBe('ok');
    expect(d.age).toBe('0.2 s ago');
    expect(d.stale).toBe(false);
  });

  it('marks an old confirmation stale', () => {
    const d = deviceLineFor(
      snap({}, { command: { kind: 'succeeded', command: 'Power', atMs: 90_000, latencyMs: 42 } }),
    );
    expect(d.stale).toBe(true);
  });

  it('reports an unreachable failure without implying anything happened', () => {
    const d = deviceLineFor(
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
    expect(d.tone).toBe('bad');
    expect(d.label).toContain('did not reach the device');
    expect(d.label).toContain('timed out');
  });

  it('distinguishes a device refusal from unreachability', () => {
    const d = deviceLineFor(
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
    expect(d.label).toContain('refused');
    expect(d.label).toContain('500');
  });

  it('names a cancelled command as cancelled', () => {
    const d = deviceLineFor(
      snap(
        {},
        {
          command: {
            kind: 'failed',
            command: 'Power',
            atMs: 99_000,
            error: { kind: 'aborted' },
          },
        },
      ),
    );
    expect(d.label).toContain('cancelled');
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
    const s = snap({}, { command: { kind: 'succeeded', command: 'Power', atMs: 99_000, latencyMs: 5 } });
    expect(controlStatusFor(s, 'Power')).toBe('ready');
    expect(controlStatusFor(s, 'Silent')).toBe('ready');
  });
});

describe('cancel control (rule 6)', () => {
  it('is enabled while disconnected', () => {
    const s = snap(
      {},
      { connection: { kind: 'offline', sinceMs: 1, reason: 'refused', lastSeenMs: null } },
    );
    expect(cancelEnabled(s)).toBe(true);
  });

  it('is enabled while a request is in flight', () => {
    const s = snap({}, { command: { kind: 'pending', command: 'Power', startedMs: 1 } });
    expect(cancelEnabled(s)).toBe(true);
  });

  it('is enabled at rest', () => {
    expect(cancelEnabled(BASE)).toBe(true);
  });
});

describe('log pane (section 6.5)', () => {
  it('shows the time, the message and the latency', () => {
    const line = logLine({
      seq: 1,
      atMs: Date.now(),
      kind: 'commandOk',
      message: '/Power confirmed',
      latencyMs: 42,
    });
    expect(line).toContain('/Power confirmed');
    expect(line).toContain('42 ms');
  });

  it('omits latency when there is none', () => {
    const line = logLine({
      seq: 2,
      atMs: Date.now(),
      kind: 'commandSent',
      message: 'GET /Power',
      latencyMs: null,
    });
    expect(line).not.toContain('ms)');
  });
});
