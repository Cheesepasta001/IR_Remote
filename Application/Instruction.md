# Instruction.md

Build a **desktop application** that controls an ESP32 over its existing HTTP REST API on the local network.

**The ESP32 firmware is finished and is not in scope.** Do not propose firmware changes. If the API is missing something the app needs, say so and stop — do not work around it silently.

Read this file completely before writing code. Where it conflicts with a convention you would otherwise apply, **this file wins**.

---

## 1. The target API — FILL THIS IN FIRST

⚠️ **Do not write app code until this is filled in.** The app is a client of a real device; guessing its contract produces something that compiles and cannot talk to anything.

|                  |                  |
| ---------------- | ---------------- |
| **Base URL**     | 192.168.0.102    |
| **Auth**         | basic            |
| **Content type** | application/json |

### Endpoints

`<!-- One row per endpoint. Method, path, request body, response body, status codes. -->`

| Method | Path       | Request     | Response            | Codes |
| ------ | ---------- | ----------- | ------------------- | ----- |
| GET    | /Power     | "Power"     | "Success" or "Fail" |       |
| GET    | /Silent    | "Silent"    | "Success" or "Fail" |       |
| GET    | /Low_Temp  | "Low_Temp"  | "Success" or "Fail" |       |
| GET    | /High_Temp | "High_Temp" | "Success" or "Fail" |       |

### Error shape

```jsonc
// <!-- paste a real error response from the device -->
```

**If this section is blank when you are asked to build:** stop and ask. If asked to discover it instead, probe with `curl`, write what you find into this table, confirm it with the user, and only then start. **Do not infer endpoints from convention** — this device already exists and has whatever shape it has.

---

## 2. Stack

**Tauri.** Rust core, web frontend, one installable binary.

Chosen over Electron for two reasons that matter here:

- **The API token never reaches the frontend.** It lives in the Rust core and in the OS keychain. In Electron the equivalent needs `contextIsolation`, a preload bridge and main-process HTTP — achievable, but more places to get it wrong.
- **No CORS, ever.** All HTTP is issued from Rust, not a browser context. The device does not need to send CORS headers and probably doesn't.

Frontend framework is your choice — plain TypeScript is fine and preferred for something this small. Do not add React unless the UI grows to justify it.

*If the user later prefers Electron:* the architecture below is unchanged — "Rust core" becomes "main process", `#[tauri::command]` becomes IPC over a preload bridge, and `reqwest` becomes `undici`. The rules in §4 and §5 apply identically.

---

## 3. What you can and cannot verify

⚠️ **You cannot reach the user's ESP32.** It is on their LAN; you are not.

| You can verify | You cannot verify |
|---|---|
| `cargo build` / `npm run tauri build` succeed | That the app talks to the real device |
| Rust unit tests, frontend tests | Real latency, real error responses |
| Behaviour against the **mock server** (§7) | That the device accepts your payload shape |
| UI renders and state transitions are correct | Anything about the hardware's actual response |

**Therefore:**

- **Build + tests against the mock are your gates.** A change is not done until they pass.
- When work needs the real device, **say so and list what to check.** Never write "tested and working" for anything that requires the ESP32.
- **Build the mock server first** (§7). It is what makes the rest of this testable.

---

## 4. Hard rules

These are what separate a hardware-control app from a CRUD app. Breaking them produces a UI that lies about a physical object.

1. **Never show optimistic state.** When the user clicks a control, the UI shows *pending* — not the new value. It shows the new value only when the device confirms it. **A control panel that says ON while the device is OFF is the worst failure this app can have**, and optimistic updates are how you get one.

2. **Every displayed value carries its age.** A reading is "as of 0.4 s ago", not a bare number. If the last successful poll is older than a threshold, the display goes visibly stale — greyed, struck through, whatever — rather than continuing to show a number that looks live.

3. **Disconnected is a first-class state, not an error toast.** The device will go unreachable: Wi-Fi drops, it reboots, the user closes the laptop. The UI must have a defined disconnected appearance and must not present stale values as current while in it.

4. **Every HTTP request has an explicit timeout.** No unbounded waits. 2–3 s for a command, shorter for a status poll. A request without a timeout will eventually hang the app.

5. **Retries only on idempotent requests**, with backoff, and bounded. Never silently retry something that might act twice unless the API documents it as idempotent. If in doubt, surface the failure to the user rather than retrying.

6. **The stop / safe control is always reachable and never disabled.** Not while a request is in flight, not while disconnected, not while another control is pending. If the user wants the device to stop, the app tries — every time, immediately.

7. **The token never enters the frontend.** It is read from the OS keychain by the Rust core, attached to outgoing requests there, and never returned over IPC, never logged, never written to a config file in plaintext.

---

## 5. Architecture

```
┌── Frontend (webview) ──────────────────────┐
│  renders state · dispatches intents        │   knows no URLs, holds no token
└────────────┬───────────────────────────────┘
             │ Tauri IPC — typed commands + events
┌────────────▼───────────────────────────────┐
│  Rust core                                  │
│   api_client   reqwest, timeouts, retries   │
│   auth         OS keychain                  │
│   state        connection + device state    │  ← unit-testable, no I/O
│   poller       background status polling    │
└─────────────────────────────────────────────┘
```

**The frontend is a view.** It never constructs a URL, never holds a token, never decides retry policy. It dispatches an intent (`send_command`, `stop`) and renders whatever state the core emits.

**`state` must be pure** — no HTTP, no clock reads, no filesystem. It takes the current state plus an event plus a timestamp and returns the next state. That is what makes staleness, connection transitions and pending-command logic testable without a device.

**Polling** runs in the core and pushes updates to the frontend as Tauri events. The frontend does not poll.

---

## 6. UI requirements

Minimum viable window, in this order of priority:

1. **Connection banner** — connected / connecting / disconnected, with last-seen time and the base URL in use.
2. **Current device state**, with its age (rule 2).
3. **The control(s)** that send the signal, each with a clear pending state.
4. **Stop / safe button**, visually distinct, always enabled.
5. **A log pane** — timestamped, last ~100 entries, showing each request, its outcome and its latency. For a device you cannot see, this is the debugging surface; it is not optional.
6. **Settings** — base URL, token entry, poll interval, timeouts.

Keyboard: `Esc` triggers stop. Do not add other shortcuts unless asked.

---

## 7. Mock device server — build this first

A small local HTTP server that implements the §1 contract, in `mock/`.

It must be able to simulate, switchable at runtime:

- normal responses
- slow responses (past the app's timeout)
- connection refused
- malformed JSON
- each documented error code
- a value that changes on its own, so staleness handling is visible

Run it on a different port from anything else and point the app at it via config. **Every behaviour in §4 must be demonstrated against this mock before the real device is involved.** Node or Python, your choice; keep it to one file.

---

## 8. Repository layout

```
Instruction.md
src-tauri/
  src/
    main.rs           wiring only
    api_client.rs     reqwest, timeouts, retry policy
    auth.rs           keychain read/write
    state.rs          pure state machine — NO I/O
    poller.rs         background polling task
  tests/              Rust unit + integration tests
  tauri.conf.json
src/
  main.ts             frontend entry
  ui/                 components
  types.ts            shared types mirroring the API
mock/
  server.js           see §7
README.md             build, run, and how to point it at a device
```

---

## 9. Build, run, test

```bash
npm install
npm run tauri dev        # dev, against the mock
npm run tauri build      # release binary
cargo test               # Rust unit tests — must pass
npm test                 # frontend tests — must pass
node mock/server.js      # the mock device
```

`README.md` must document how to switch between the mock and a real device in one step.

---

## 10. Definition of done

- [ ] `cargo test` passes; `state.rs` has tests covering connect, disconnect, stale, pending and stop
- [ ] Frontend tests pass
- [ ] `npm run tauri build` succeeds
- [ ] Every §4 rule demonstrated against the mock, including timeout and connection-refused paths
- [ ] Token is not present anywhere in the frontend bundle, in logs, or in a plaintext file
- [ ] Stop works while disconnected, while a request is pending, and while another control is in flight
- [ ] `README.md` updated
- [ ] **Anything requiring the real ESP32 is listed as unverified**, with what to check

---

## 11. Working agreement

- **First milestone, and nothing beyond it until the user confirms against the real device:** mock server running, app connects, status displays with its age, stop button works, log pane shows requests. No extra controls, no polish.
- **Ask before adding a dependency.** `reqwest`, `serde`, `tokio` and a keychain crate are expected; anything else needs a reason.
- **If the API cannot do what a feature needs, say so and stop.** The firmware is out of scope — do not build a workaround that hides a missing endpoint.
- **Report what changed and what it affects**, not a step-by-step narration.
- **Never guess at the device's behaviour.** If §1 does not answer it, ask.
