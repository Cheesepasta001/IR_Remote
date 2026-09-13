/**
 * Frontend entry. This is a view (Instruction.md section 5).
 *
 * It never constructs a URL, never holds a credential, and never decides retry
 * policy. It dispatches intents over Tauri IPC and renders whatever the core
 * emits. The password field below is write-only: it goes into the core, and the
 * core puts it in the OS keychain. Nothing reads one back.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import { COMMANDS, type Command, type LogEntry, type Snapshot } from './types';
import {
  NO_STATE_CAVEAT,
  bannerFor,
  cancelEnabled,
  controlStatusFor,
  deviceLineFor,
  logLine,
  reage,
  toneForLog,
} from './ui/present';

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
};

let snapshot: Snapshot | null = null;
let entries: LogEntry[] = [];
/** Settings inputs are not overwritten while the user is editing them. */
let settingsDirty = false;

// ------------------------------------------------------------- render ----

function renderBanner(s: Snapshot) {
  const b = bannerFor(s);
  const el = $('banner');
  el.className = `banner tone-${b.tone}`;
  $('banner-label').textContent = b.label;
  $('banner-detail').textContent = b.detail;
}

function renderDevice(s: Snapshot) {
  const d = deviceLineFor(s);
  const label = $('device-label');
  label.textContent = d.label;
  label.className = `device-label tone-${d.tone}${d.stale ? ' stale' : ''}`;

  const age = $('device-age');
  age.textContent = d.age ?? '';
  age.className = `age${d.stale ? ' stale' : ''}`;

  $('device-caveat').textContent = NO_STATE_CAVEAT;
}

function renderControls(s: Snapshot) {
  const host = $('controls');

  if (host.childElementCount === 0) {
    for (const c of COMMANDS) {
      const b = document.createElement('button');
      b.dataset.command = c.id;
      b.innerHTML = `${c.label}<span class="sub">${c.path}</span>`;
      b.addEventListener('click', () => dispatch(c.id));
      host.appendChild(b);
    }
  }

  for (const el of Array.from(host.children) as HTMLButtonElement[]) {
    const id = el.dataset.command as Command;
    const status = controlStatusFor(s, id);
    const def = COMMANDS.find((c) => c.id === id)!;

    el.classList.toggle('pending', status === 'pending');
    // Only an in-flight command gates the others. Being disconnected does not:
    // the user may still try, and the attempt is logged either way.
    el.disabled = status === 'blocked';
    el.innerHTML =
      status === 'pending'
        ? `${def.label}<span class="sub">sending…</span>`
        : `${def.label}<span class="sub">${def.path}</span>`;
  }
}

function renderCancel(s: Snapshot) {
  // Rule 6: never disabled, in any state.
  const btn = $<HTMLButtonElement>('cancel');
  btn.disabled = !cancelEnabled(s);

  $('cancel-note').textContent =
    'Cancels this app’s in-flight request. It does NOT switch the air conditioner off — the API exposes no safe off, and /Power is a toggle.';
}

function renderLog() {
  const host = $('log');
  host.replaceChildren();

  if (entries.length === 0) {
    const p = document.createElement('div');
    p.className = 'empty';
    p.textContent = 'no requests yet';
    host.appendChild(p);
  } else {
    for (const e of entries) {
      const row = document.createElement('div');
      row.className = `tone-${toneForLog(e.kind)}`;
      row.textContent = logLine(e);
      host.appendChild(row);
    }
  }

  $('log-count').textContent = `${entries.length}/100`;
  host.scrollTop = host.scrollHeight;
}

function renderSettings(s: Snapshot) {
  if (settingsDirty) return;
  $<HTMLInputElement>('base-url').value = s.baseUrl;
  $<HTMLInputElement>('poll-interval').value = String(s.pollIntervalMs);
  $<HTMLInputElement>('command-timeout').value = String(s.commandTimeoutMs);
  $<HTMLInputElement>('probe-timeout').value = String(s.probeTimeoutMs);
  $<HTMLInputElement>('stale-after').value = String(s.staleAfterMs);
  $<HTMLInputElement>('cred-user').value = s.username;

  $('cred-note').textContent = s.hasCredential
    ? `A credential for ${s.username} is stored in the OS keychain. It is attached in the Rust core and never sent back here.`
    : 'No credential stored. The current firmware performs no authentication, so this can stay empty.';
}

function render() {
  if (!snapshot) return;
  const s = reage(snapshot, Date.now());
  renderBanner(s);
  renderDevice(s);
  renderControls(s);
  renderCancel(s);
  renderSettings(s);
}

// ------------------------------------------------------------ intents ----

async function dispatch(command: Command) {
  try {
    await invoke('send_command', { command });
  } catch {
    // The core has already logged the failure and moved the state machine.
    // Nothing to add here; never surface it as a value change.
  }
}

async function cancel() {
  try {
    await invoke('abort');
  } catch {
    /* the cancel path must never throw at the user */
  }
}

async function applySettings(e: Event) {
  e.preventDefault();
  try {
    await invoke('update_settings', {
      baseUrl: $<HTMLInputElement>('base-url').value.trim(),
      pollIntervalMs: Number($<HTMLInputElement>('poll-interval').value),
      commandTimeoutMs: Number($<HTMLInputElement>('command-timeout').value),
      probeTimeoutMs: Number($<HTMLInputElement>('probe-timeout').value),
      staleAfterMs: Number($<HTMLInputElement>('stale-after').value),
    });
    settingsDirty = false;
  } catch (err) {
    $('cred-note').textContent = String(err);
  }
}

async function saveCredentials(e: Event) {
  e.preventDefault();
  const pass = $<HTMLInputElement>('cred-pass');
  try {
    await invoke('save_credentials', {
      username: $<HTMLInputElement>('cred-user').value.trim(),
      password: pass.value,
    });
  } catch (err) {
    $('cred-note').textContent = String(err);
  } finally {
    // Do not leave the secret sitting in the DOM.
    pass.value = '';
  }
}

// --------------------------------------------------------------- boot ----

async function boot() {
  $('cancel').addEventListener('click', cancel);
  $('settings').addEventListener('submit', applySettings);
  $('creds').addEventListener('submit', saveCredentials);
  $('cred-clear').addEventListener('click', async () => {
    await invoke('clear_credentials', {
      username: $<HTMLInputElement>('cred-user').value.trim(),
    }).catch(() => undefined);
  });

  for (const id of ['base-url', 'poll-interval', 'command-timeout', 'probe-timeout', 'stale-after']) {
    $(id).addEventListener('input', () => {
      settingsDirty = true;
    });
  }

  // Section 6: Esc triggers the cancel control. No other shortcuts.
  window.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      void cancel();
    }
  });

  await listen<Snapshot>('snapshot', (ev) => {
    snapshot = ev.payload;
    render();
  });

  await listen<LogEntry>('log', (ev) => {
    entries = [...entries, ev.payload].slice(-100);
    renderLog();
  });

  snapshot = await invoke<Snapshot>('get_snapshot');
  entries = await invoke<LogEntry[]>('get_log');
  render();
  renderLog();

  // Rule 2: keep ages counting between snapshots so a value visibly goes stale
  // instead of freezing at whatever it said when the device stopped answering.
  setInterval(render, 200);
}

boot().catch((err) => {
  // A blank window is the worst way for this to fail: it is indistinguishable
  // from a device that is merely quiet, and every control is missing because
  // the buttons are built in JS. Say plainly that the app itself did not start.
  //
  // This is not hypothetical - it is exactly what happened when the Tauri
  // capabilities file was missing and `listen()` was denied.
  const banner = $('banner');
  banner.className = 'banner tone-bad';
  $('banner-label').textContent = 'App failed to start';
  $('banner-detail').textContent =
    `${err} — the UI is not connected to the core, so nothing below is live.`;
  console.error('boot failed:', err);
});
