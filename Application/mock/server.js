#!/usr/bin/env node
'use strict';
/*
 * Mock ESP32 IR blaster - see Instruction.md section 7.
 *
 * This file is the executable contract. The real firmware (Main/Main.ino) does
 * not yet match it: as of this writing it handles only GET /Power and writes no
 * HTTP response at all. Mode "silent" below reproduces that exact behaviour so
 * the app can be tested against what the device really does today.
 *
 * No dependencies. Node >= 18.
 */

const http = require('http');

// ---------------------------------------------------------------- config ----

function arg(name, fallback) {
  const hit = process.argv.find((a) => a.startsWith('--' + name + '='));
  return hit ? hit.slice(name.length + 3) : fallback;
}

const PORT = Number(arg('port', 8080));
const CONTROL_PORT = Number(arg('control-port', 8081));

// The four commands from Instruction.md section 1.
const COMMANDS = ['Power', 'Silent', 'Low_Temp', 'High_Temp'];

// Basic auth credentials the mock accepts when auth is switched on.
const AUTH_USER = arg('user', 'esp32');
const AUTH_PASS = arg('pass', 'secret');

// ----------------------------------------------------------------- state ----

const MODES = ['normal', 'slow', 'refuse', 'malformed', 'silent', '401', '500', '503'];

const state = {
  mode: arg('mode', 'normal'),
  slowMs: Number(arg('slow-ms', 10000)),
  authRequired: arg('auth', 'off') === 'on',
  requests: 0,
  // Section 7 asks for "a value that changes on its own, so staleness handling
  // is visible". The agreed contract exposes no readable device value, so this
  // counter has no endpoint to surface it on. It is kept and reported on the
  // control endpoint only; staleness in the app is driven by the reachability
  // probe going dark in "refuse" mode instead. See README.
  drift: 0,
};

setInterval(() => {
  state.drift = (state.drift + 1) % 1000;
}, 1000).unref();

// ------------------------------------------------------------------ util ----

function ts() {
  return new Date().toISOString();
}

function log(...parts) {
  console.log('[' + ts() + '] ' + parts.join(' '));
}

function sendJson(res, code, body) {
  const payload = JSON.stringify(body);
  res.writeHead(code, {
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(payload),
    Connection: 'close',
  });
  res.end(payload);
}

function checkAuth(req) {
  if (!state.authRequired) return true;
  const header = req.headers.authorization || '';
  if (!header.startsWith('Basic ')) return false;
  const decoded = Buffer.from(header.slice(6), 'base64').toString('utf8');
  const idx = decoded.indexOf(':');
  if (idx < 0) return false;
  return decoded.slice(0, idx) === AUTH_USER && decoded.slice(idx + 1) === AUTH_PASS;
}

// ------------------------------------------------------------ device api ----

function handleDevice(req, res) {
  state.requests += 1;
  const path = req.url.split('?')[0];
  const id = state.requests;
  log('--> #' + id + ' ' + req.method + ' ' + path + ' (mode=' + state.mode + ')');

  // "silent" reproduces the current firmware: fire and never reply. The socket
  // is held open exactly as Main.ino holds it, so the client must time out.
  if (state.mode === 'silent') {
    log('    #' + id + ' holding socket open, sending no response (firmware behaviour)');
    return; // deliberately no res.end()
  }

  if (state.mode === 'slow') {
    log('    #' + id + ' delaying ' + state.slowMs + 'ms');
    setTimeout(() => {
      if (!res.writableEnded) respond(req, res, path, id);
    }, state.slowMs).unref();
    return;
  }

  respond(req, res, path, id);
}

function respond(req, res, path, id) {
  if (state.mode === 'malformed') {
    res.writeHead(200, { 'Content-Type': 'application/json', Connection: 'close' });
    log('    #' + id + ' <-- 200 malformed JSON');
    return res.end('{"result":"Succ');
  }

  if (state.mode === '401' || !checkAuth(req)) {
    log('    #' + id + ' <-- 401');
    res.setHeader('WWW-Authenticate', 'Basic realm="esp32"');
    return sendJson(res, 401, { result: 'Fail', error: 'unauthorized' });
  }

  if (state.mode === '500') {
    log('    #' + id + ' <-- 500');
    return sendJson(res, 500, { result: 'Fail', error: 'ir send failed' });
  }

  if (state.mode === '503') {
    log('    #' + id + ' <-- 503');
    return sendJson(res, 503, { result: 'Fail', error: 'busy' });
  }

  const command = COMMANDS.find((c) => path === '/' + c);
  if (!command) {
    log('    #' + id + ' <-- 404 unknown command');
    return sendJson(res, 404, { result: 'Fail', error: 'unknown command' });
  }

  if (req.method !== 'GET') {
    log('    #' + id + ' <-- 405');
    return sendJson(res, 405, { result: 'Fail', error: 'method not allowed' });
  }

  log('    #' + id + ' <-- 200 ' + command + ' Success');
  sendJson(res, 200, { result: 'Success', command });
}

// --------------------------------------------------------------- control ----

let deviceServer = null;
let listening = false;

function startDevice() {
  return new Promise((resolve) => {
    if (listening) return resolve();
    deviceServer = http.createServer(handleDevice);
    deviceServer.on('clientError', (err, socket) => socket.destroy());
    deviceServer.listen(PORT, '127.0.0.1', () => {
      listening = true;
      log('device listening on http://127.0.0.1:' + PORT);
      resolve();
    });
  });
}

function stopDevice() {
  return new Promise((resolve) => {
    if (!listening || !deviceServer) return resolve();
    if (deviceServer.closeAllConnections) deviceServer.closeAllConnections();
    deviceServer.close(() => {
      listening = false;
      log('device stopped listening - connections will be REFUSED');
      resolve();
    });
  });
}

async function applyMode(next) {
  state.mode = next;
  if (next === 'refuse') await stopDevice();
  else await startDevice();
  log('mode = ' + state.mode);
}

const controlServer = http.createServer(async (req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1:' + CONTROL_PORT);

  if (url.pathname === '/__control/state') {
    return sendJson(res, 200, {
      mode: state.mode,
      modes: MODES,
      slowMs: state.slowMs,
      authRequired: state.authRequired,
      listening,
      requests: state.requests,
      drift: state.drift,
    });
  }

  if (url.pathname === '/__control/mode') {
    const next = url.searchParams.get('set');
    if (!MODES.includes(next)) {
      return sendJson(res, 400, { error: 'mode must be one of ' + MODES.join(', ') });
    }
    const ms = url.searchParams.get('ms');
    if (ms) state.slowMs = Number(ms);
    await applyMode(next);
    return sendJson(res, 200, { mode: state.mode, slowMs: state.slowMs, listening });
  }

  if (url.pathname === '/__control/auth') {
    const set = url.searchParams.get('set');
    if (set !== 'on' && set !== 'off') {
      return sendJson(res, 400, { error: 'set must be on or off' });
    }
    state.authRequired = set === 'on';
    log('auth required = ' + state.authRequired);
    return sendJson(res, 200, { authRequired: state.authRequired });
  }

  sendJson(res, 404, { error: 'unknown control path' });
});

// ------------------------------------------------------------------ boot ----

async function main() {
  await applyMode(state.mode);
  controlServer.listen(CONTROL_PORT, '127.0.0.1', () => {
    log('control listening on http://127.0.0.1:' + CONTROL_PORT);
    log('modes: ' + MODES.join(' | '));
    log('switch with: curl "http://127.0.0.1:' + CONTROL_PORT + '/__control/mode?set=slow"');
  });
}

main();
