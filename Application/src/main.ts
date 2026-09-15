/**
 * Frontend entry. This is a view (Instruction.md section 5).
 *
 * It never constructs a URL, never holds a credential, and never decides retry
 * or scheduling policy. It dispatches intents over Tauri IPC and renders
 * whatever the core emits. The password field is write-only: it goes into the
 * core, and the core puts it in the OS keychain. Nothing reads one back.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import { COMMANDS, type Command, type Snapshot } from './types';
import {
  ALARM_CAVEAT,
  NO_STATE_CAVEAT,
  alarmLineFor,
  bannerFor,
  controlStatusFor,
  deviceLineFor,
  exactAlarmWarningFor,
  parseClock,
  reage,
} from './ui/present';

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
};

let snapshot: Snapshot | null = null;
/** Settings inputs are not overwritten while the user is editing them. */
let settingsDirty = false;
/** The address field is not overwritten while the user is typing in it. */
let deviceDirty = false;
/** Last offset sent to the core, so we only re-send when it actually changes. */
let sentTzOffset: number | null = null;

// ------------------------------------------------------------- render ----

function renderBanner(s: Snapshot) {
  const b = bannerFor(s);
  $('banner').className = `banner tone-${b.tone}`;
  $('banner-label').textContent = b.label;
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
      b.addEventListener('click', () => dispatch(c.id));
      host.appendChild(b);
    }
  }

  for (const el of Array.from(host.children) as HTMLButtonElement[]) {
    const id = el.dataset.command as Command;
    const def = COMMANDS.find((c) => c.id === id)!;
    const status = controlStatusFor(s, id);

    el.classList.toggle('pending', status === 'pending');
    // Only an in-flight command gates the others. Being disconnected does not:
    // the user may still try, and the attempt is recorded either way.
    el.disabled = status === 'blocked';
    el.innerHTML =
      status === 'pending'
        ? `${def.label}<span class="sub">sending…</span>`
        : `${def.label}<span class="sub">${def.path}</span>`;
  }
}

function renderAlarms(s: Snapshot) {
  const host = $('alarms');
  host.replaceChildren();

  if (s.alarms.length === 0) {
    const p = document.createElement('p');
    p.className = 'caveat';
    p.textContent = 'No alarms set.';
    host.appendChild(p);
  }

  for (const a of s.alarms) {
    const line = alarmLineFor(a);

    const row = document.createElement('div');
    row.className = `alarm-row${a.enabled ? '' : ' off'}`;

    const time = document.createElement('span');
    time.className = 'alarm-time';
    time.textContent = line.time;

    const body = document.createElement('div');
    body.className = 'alarm-body';
    const action = document.createElement('span');
    action.className = 'alarm-action';
    action.textContent = line.action;
    const status = document.createElement('span');
    status.className = `alarm-status tone-${line.tone}`;
    status.textContent = `${line.next} · ${line.status}`;
    body.append(action, status);

    const toggle = document.createElement('button');
    toggle.className = 'alarm-toggle';
    toggle.textContent = a.enabled ? 'On' : 'Off';
    toggle.setAttribute('aria-pressed', String(a.enabled));
    toggle.addEventListener('click', () => setAlarmEnabled(a.id, !a.enabled));

    const remove = document.createElement('button');
    remove.className = 'alarm-remove';
    remove.textContent = '✕';
    remove.title = 'Delete alarm';
    remove.addEventListener('click', () => removeAlarm(a.id));

    row.append(time, body, toggle, remove);
    host.appendChild(row);
  }

  const warning = exactAlarmWarningFor(s);
  $('exact-warning').hidden = warning.text === '';
  $('exact-warning-text').textContent = warning.text;
  $<HTMLButtonElement>('exact-grant').hidden = !warning.canRequest;

  $('alarm-caveat').textContent = ALARM_CAVEAT;
}

const BASE_URL_NOTE: Record<Snapshot['baseUrlSource'], string> = {
  stored: 'Saved in the app. Change it above — it takes effect on the next poll.',
  environment: 'Read from the IR_REMOTE_BASE_URL environment variable, which overrides everything else.',
  dotEnv: 'Read from the .env file at startup. Edit .env and restart to point at a different device.',
  compiled: 'Compiled in when this build was made. Override it above if the device moved.',
  default: 'No IR_REMOTE_BASE_URL configured — this is the built-in mock address.',
};

function renderSettings(s: Snapshot) {
  $('base-url-value').textContent = s.baseUrl;
  $('base-url-note').textContent = BASE_URL_NOTE[s.baseUrlSource];

  // Editable only where there is no .env to edit and no shell to set a
  // variable in — Android. Desktop keeps one source of truth.
  const deviceForm = $('device-form');
  deviceForm.hidden = !s.baseUrlEditable;
  if (s.baseUrlEditable && !deviceDirty) {
    $<HTMLInputElement>('base-url-input').value = s.baseUrl;
  }

  if (settingsDirty) return;
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
  renderAlarms(s);
  renderSettings(s);
  syncTzOffset();
}

// ------------------------------------------------------------ intents ----

async function dispatch(command: Command) {
  try {
    await invoke('send_command', { command });
  } catch {
    // The core has already recorded the failure and moved the state machine.
    // Nothing to add here; never surface it as a value change.
  }
}

async function addAlarm(e: Event) {
  e.preventDefault();
  const error = $('alarm-error');
  error.textContent = '';

  const clock = parseClock($<HTMLInputElement>('alarm-time').value);
  if (!clock) {
    error.textContent = 'Enter a time as HH:MM.';
    return;
  }
  const command = $<HTMLSelectElement>('alarm-command').value as Command;

  try {
    await invoke('add_alarm', { hour: clock.hour, minute: clock.minute, command });
  } catch (err) {
    error.textContent = String(err);
  }
}

async function setAlarmEnabled(id: number, enabled: boolean) {
  try {
    await invoke('set_alarm_enabled', { id, enabled });
  } catch (err) {
    $('alarm-error').textContent = String(err);
  }
}

async function removeAlarm(id: number) {
  try {
    await invoke('remove_alarm', { id });
  } catch (err) {
    $('alarm-error').textContent = String(err);
  }
}

/**
 * Tell the core our UTC offset so it can schedule in local time. Rust's
 * standard library has no timezone database. This is environment data the view
 * happens to know — the scheduling itself stays in the core.
 */
function syncTzOffset() {
  const minutes = -new Date().getTimezoneOffset();
  if (minutes === sentTzOffset) return;
  sentTzOffset = minutes;
  void invoke('set_tz_offset', { minutes }).catch(() => {
    sentTzOffset = null; // let it retry on the next tick
  });
}

async function saveBaseUrl(e: Event) {
  e.preventDefault();
  const note = $('base-url-note');
  try {
    await invoke('set_base_url', {
      baseUrl: $<HTMLInputElement>('base-url-input').value.trim(),
    });
    deviceDirty = false;
  } catch (err) {
    note.textContent = String(err);
  }
}

async function applySettings(e: Event) {
  e.preventDefault();
  try {
    await invoke('update_settings', {
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

function toggleSettings() {
  const panel = $('settings-panel');
  const button = $('settings-toggle');
  const open = panel.hidden;
  panel.hidden = !open;
  button.setAttribute('aria-expanded', String(open));
  button.classList.toggle('active', open);
}

// --------------------------------------------------------------- boot ----

async function boot() {
  $('settings-toggle').addEventListener('click', toggleSettings);
  $('settings').addEventListener('submit', applySettings);
  $('creds').addEventListener('submit', saveCredentials);
  $('alarm-form').addEventListener('submit', addAlarm);
  $('device-form').addEventListener('submit', saveBaseUrl);
  $('exact-grant').addEventListener('click', () => {
    // Android only; a no-op elsewhere. The user grants it in system settings.
    void invoke('request_exact_alarm_permission').catch((err) => {
      $('alarm-error').textContent = String(err);
    });
  });
  $('base-url-input').addEventListener('input', () => {
    deviceDirty = true;
  });
  $('cred-clear').addEventListener('click', async () => {
    await invoke('clear_credentials', {
      username: $<HTMLInputElement>('cred-user').value.trim(),
    }).catch(() => undefined);
  });

  for (const id of ['poll-interval', 'command-timeout', 'probe-timeout', 'stale-after']) {
    $(id).addEventListener('input', () => {
      settingsDirty = true;
    });
  }

  // Esc closes the settings panel. No other shortcuts.
  window.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && !$('settings-panel').hidden) {
      ev.preventDefault();
      toggleSettings();
    }
  });

  await listen<Snapshot>('snapshot', (ev) => {
    snapshot = ev.payload;
    render();
  });

  syncTzOffset();
  snapshot = await invoke<Snapshot>('get_snapshot');
  render();

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
