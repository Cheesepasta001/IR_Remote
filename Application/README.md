# IR Remote

Desktop control panel for the ESP32 IR blaster, built to `Instruction.md`.

Tauri (Rust core + plain-TypeScript webview). All HTTP is issued from Rust, so
there is no CORS surface and no credential in the frontend.

---

## ⚠️ Read this first: the firmware does not implement the documented API

`Instruction.md` section 1 describes four endpoints returning `"Success"` or
`"Fail"` over Basic auth. The firmware in `../Main/Main.ino` does not do that.

| `Instruction.md` section 1 | `Main.ino` as it stands |
|---|---|
| `/Power`, `/Silent`, `/Low_Temp`, `/High_Temp` | only `GET /Power` is matched |
| responds `"Success"` / `"Fail"` | **writes no HTTP response at all** |
| `Content-Type: application/json` | nothing is ever sent to the client |
| Basic auth | no authentication check anywhere |
| status codes | none — there is no response |

The handler fires the IR code and then sits in `while (client.connected())`
without ever writing. A client gets zero bytes and hangs until its own timeout.

**Consequence:** with the current firmware, every command times out from the
app's side *even when the IR code was sent successfully*. Rule 1 forbids showing
a value the device has not confirmed, and this device confirms nothing.

This is why `mock/server.js` exists and why it is the contract of record. The
agreed shape (see "Contract" below) is what the app is built against; the
firmware is expected to grow into it. **No firmware change is included here —
that is out of scope per `Instruction.md` section 5.**

Mock mode `silent` reproduces the current firmware exactly, so you can see what
the app does against the real device today without leaving your desk.

---

## Contract

Agreed with the owner, implemented by `mock/server.js`:

```
GET /Power      200  {"result":"Success","command":"Power"}
GET /Silent     200  {"result":"Success","command":"Silent"}
GET /Low_Temp   200  {"result":"Success","command":"Low_Temp"}
GET /High_Temp  200  {"result":"Success","command":"High_Temp"}

unknown path    404  {"result":"Fail","error":"unknown command"}
bad credential  401  {"result":"Fail","error":"unauthorized"}
IR send failed  500  {"result":"Fail","error":"ir send failed"}
busy            503  {"result":"Fail","error":"busy"}
```

There is **no status endpoint**. The device stores no state (`enableIR` in the
firmware is assigned and never read), so nothing can be polled for the air
conditioner's actual condition.

---

## Build, run, test

```bash
npm install
npm run mock          # the mock device on :8080, control on :8081
npm run tauri dev     # the app, pointed at the mock by default
npm run tauri build   # release binary + MSI installer
npm test              # frontend tests           (27)
npm run check:secrets # rule 7 check on the built bundle and the IPC surface
npm run verify        # build + frontend tests + secrets check
cd src-tauri && cargo test   # Rust unit + integration tests   (47)
```

**Installer targets: MSI only.** NSIS is deliberately disabled. Its toolchain is
cached under `%LOCALAPPDATA%` on `C:`, and the bundler finishes by renaming the
result onto the project drive — which fails with `os error 17` (cannot move a
file to a different disk) whenever the project lives elsewhere, as this one does
on `B:`. Re-enable `"nsis"` in `src-tauri/tauri.conf.json` if it ever moves to `C:`.

Rust must be on PATH. If `cargo` is not found, add `%USERPROFILE%\.cargo\bin`.

### Switching between the mock and a real device — one step

Set `IR_REMOTE_BASE_URL` before launching:

```bash
# the mock (this is the default when the variable is unset)
npm run tauri dev

# the real ESP32
IR_REMOTE_BASE_URL=http://192.168.0.102 npm run tauri dev
```

On PowerShell:

```powershell
$env:IR_REMOTE_BASE_URL = "http://192.168.0.102"; npm run tauri dev
```

The same field is editable at runtime in the app's Settings pane, which takes
effect on the next poll without a restart.

---

## How the rules in section 4 are met

| Rule | Where | How to see it |
|---|---|---|
| 1. Never optimistic | `state.rs` `reduce`, `present.ts` `deviceLineFor` | Mock mode `slow`; the control reads "sending…" and never flips to a value |
| 2. Values carry their age | `AppState::age_ms`, `present.ts` `reage` | The age ticks every 200 ms; past `staleAfterMs` it greys and strikes through |
| 3. Disconnected is first-class | `Connection::Offline` | Mock mode `refuse`; the banner becomes a defined Disconnected state, not a toast |
| 4. Explicit timeouts | `api_client.rs` `Timeouts` | 3 s command, 1.5 s probe, both per-request; no unbounded wait exists |
| 5. Retries only when idempotent | `Command::is_idempotent` (always false) | No command is ever retried; only the TCP probe retries, 3 attempts with backoff |
| 6. Cancel always reachable | `AppState::can_abort`, published on the snapshot | Enabled while disconnected, while pending, and at rest — the core decides it, the view only renders it |
| 7. Token never in the frontend | `auth.rs`, `Snapshot.has_credential` | The snapshot carries a boolean; `Credentials`' `Debug` prints `<redacted>` |

### Demonstrating them against the mock

```bash
CTRL=http://127.0.0.1:8081/__control

curl "$CTRL/mode?set=normal"      # confirmed commands, ages stay fresh
curl "$CTRL/mode?set=slow&ms=10000"  # rule 1 + rule 4: pending, then timeout
curl "$CTRL/mode?set=refuse"      # rule 3: connection refused, banner goes Disconnected
curl "$CTRL/mode?set=silent"      # what the real firmware does today
curl "$CTRL/mode?set=malformed"   # unreadable reply is a failure, not a success
curl "$CTRL/mode?set=500"         # device error, but still demonstrably reachable
curl "$CTRL/auth?set=on"          # then save a credential in Settings (esp32 / secret)
curl "$CTRL/state"                # what mode am I in
```

---

## Architecture

```
src/            frontend — renders state, dispatches intents. No URLs, no token.
src-tauri/src/
  main.rs       wiring only
  lib.rs        module declarations + the Tauri builder
  api_client.rs reqwest, explicit timeouts, retry policy
  auth.rs       OS keychain (Windows Credential Manager)
  state.rs      pure state machine — no I/O, no clock reads
  poller.rs     background reachability probe
  core.rs       shared state + log ring buffer + snapshot emission
src-tauri/capabilities/default.json   webview permissions — required
mock/server.js  the contract, and every fault mode
```

**`capabilities/default.json` is not optional.** Tauri 2 denies the webview
`listen()` without it, which makes `boot()` reject at its first `await`. The
window then renders only its static HTML: the banner stuck on "Connecting", an
empty log, and no control buttons at all, since those are built in JS. If the
window ever comes up looking inert, check this file and the window `label` it
targets (`main`) first. A boot failure now writes itself into the banner rather
than leaving a blank window.

`core.rs` is a small deviation from the section 8 layout: `main.rs` is "wiring
only", so the shared state that both the IPC commands and the poller need has
its own module rather than living in `main.rs`.

### Why the poller opens a TCP socket instead of calling an endpoint

Every HTTP path this device serves fires an IR code. Polling `/Power` to check
liveness would toggle the air conditioner once per poll interval. The probe is a
bare TCP handshake to port 80: it proves the device is up and actuates nothing.

---

## Unverified / blocked

Per `Instruction.md` section 3, the ESP32 is on your LAN and unreachable from
here. Nothing below has been tested against real hardware.

### Blocked on the API — needs firmware work, out of scope here

1. **No command can be confirmed.** The firmware writes no HTTP response. Until
   it does, every command in this app will show as timed out even on success.
   This is the one that makes the app usable or not.
2. **No stop / safe control exists.** `Instruction.md` rule 6 requires one.
   `/Power` is a toggle, so wiring stop to it could switch the unit **on** —
   precisely the rule-1 failure the document is written to prevent. The app
   therefore ships a **Cancel** button that aborts the in-flight request and is
   honestly labelled as not commanding the device. A real stop needs an explicit
   off endpoint.
3. **`/Silent`, `/Low_Temp` and `/High_Temp` do not exist in the firmware.** The
   app sends them because the contract says they exist; against the current
   device they will hang like `/Power`. `lowerTempButton()` is currently an
   empty stub.
4. **No status endpoint.** Section 6.2's "current device state" is therefore the
   last *confirmed command*, plus a permanent caveat in the UI saying the app
   does not know whether the air conditioner is on. It never implies otherwise.
5. **Section 7's "value that changes on its own"** has no counterpart, because
   the contract exposes no readable value. Staleness is instead demonstrated by
   the reachability probe going dark in `refuse` mode.

### To check once the device is reachable

- [ ] `IR_REMOTE_BASE_URL=http://192.168.0.102` — does the banner reach Connected?
      (This only proves the TCP port is open, which is all it claims.)
- [ ] Does the ESP32's DHCP lease still put it at `192.168.0.102`?
- [ ] Press Power once. Does the air conditioner respond, and does the app show
      a confirmation or a timeout? A timeout with the unit responding confirms
      the missing-response problem above.
- [ ] With the app polling, does a command still get through? The firmware
      serves one client at a time; the poller skips while a command is in
      flight, but that ordering is untested against real hardware.
- [ ] Does the device ever actually send a `401`? If it gains Basic auth, check
      the username the firmware expects.
- [ ] Real latency: is the 3 s command timeout and 1.5 s probe timeout right for
      an ESP32 on your Wi-Fi?
