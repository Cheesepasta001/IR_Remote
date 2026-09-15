# IR Remote

Desktop control panel for the ESP32 IR blaster, built to `Instruction.md`.

Tauri (Rust core + plain-TypeScript webview). All HTTP is issued from Rust, so
there is no CORS surface and no credential in the frontend.

**There is an Android build too — see [ANDROID.md](ANDROID.md).** It shares the
entire Rust core and frontend; what differs is where the device address comes
from, that cleartext HTTP must be explicitly permitted, and that alarms are
handled by Android's AlarmManager rather than the tokio scheduler, because
Android kills background timers.

---

## ⚠️ Read this first: one firmware change is still needed

`Main.ino` now serves all four commands (`/Power`, `/Silent`, `/Low_Temp`,
`/High_Temp`), each firing its own IR code and writing a reply. That is most of
the way there. One thing still blocks the app.

**The reply has no HTTP framing.** `client.println("success")` puts exactly this
on the wire:

```
success\r\n
```

No status line, no headers, no blank line. That is the body-only HTTP/0.9 shape,
and no HTTP client will parse it:

- `curl` → `Received HTTP/0.9 when not allowed`
- this app → `Unreachable("error sending request for url …")`

So the app still shows **"did not reach the device"** while the IR code fired and
the device replied `success`. Rule 1 forbids showing a value the device has not
confirmed, and an unparseable reply is not a confirmation.

### The fix

Firmware is out of scope for this repo per `Instruction.md` section 5, so this is
written down rather than applied. A `socketHandler()` is being added in
`Main.ino`; this is the shape the app expects.

```cpp
void socketHandler(int instance){
  client.println("HTTP/1.1 200 OK");
  client.println("Content-Type: application/json");
  client.println("Connection: close");
  client.println();                    // the blank line is required
  switch (instance){
    case 1:  client.println("{\"result\":\"Success\",\"command\":\"Power\"}");     break;
    case 2:  client.println("{\"result\":\"Success\",\"command\":\"Silent\"}");    break;
    case 3:  client.println("{\"result\":\"Success\",\"command\":\"High_Temp\"}"); break;
    case 4:  client.println("{\"result\":\"Success\",\"command\":\"Low_Temp\"}");  break;
    default: client.println("{\"result\":\"Fail\",\"error\":\"ir send failed\"}"); break;
  }
  client.stop();                       // otherwise the socket is never closed
}
```

**Every `case` needs its `break`.** Without them C++ falls through and one
command writes all five bodies in a row, which is not valid JSON. The app
rejects that as `BadResponse` rather than reading the first "Success" —
see `a_switch_fallthrough_body_is_rejected`.

Then call it from `loop()` and delete the `client.println("success")` /
`client.println("fail")` lines inside the button functions, so each request
produces exactly one reply:

```cpp
if (currentLine.endsWith("GET /Power")) {
  socketHandler(powerButton() == 0 ? 1 : 0);
}
```

Dropping the `while (trial < 3 && …)` wrapper at the same time removes the
re-transmit described below.

**The app already accepts a bare `success` / `fail` body too**, so if you would
rather keep `client.println("success")` as the body, only the header lines and
`client.stop()` are actually required. Matching is case-insensitive and ignores
the trailing CRLF that `println` appends.

Mock modes `raw` (today's firmware) and `plain` (once it sends headers) let you
see both behaviours without flashing anything.

### Two other things worth fixing while you are in there

1. **The socket is never closed after replying.** There is no `client.stop()` on
   the reply path, so `while (client.connected())` spins with no `delay()` until
   the client hangs up — burning CPU and holding the ESP32's only client slot.
2. **The retry loop re-fires the IR code.** `while (trial < 3 && powerButton())`
   calls `powerButton()` *as the loop condition*, so every iteration transmits.
   It is unreachable today — `sendNEC` does not throw and ESP32 Arduino builds
   normally have exceptions disabled, so it always returns `0` — but if it ever
   returned `1`, `/Power` would toggle the unit up to four times and write four
   reply lines. The app never retries a command for exactly this reason.

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

**Two body shapes are accepted**, because the firmware and `Instruction.md`
section 1 do not agree on one:

| Body | Read as |
|---|---|
| `{"result":"Success","command":"Power"}` | success, and the command name is cross-checked |
| `success` (any case, trailing CRLF ignored) | success |
| `{"result":"Fail","error":"…"}` | device error, with the reason |
| `fail` | device error |
| anything else | `BadResponse` — never optimistically a success |

**Known gap:** a bare `success` carries no command name, so the app cannot check
that the device confirmed the action it was actually asked for. It is only safe
because the app refuses to send a second command while one is in flight, so there
is never more than one outstanding request to confuse. If the firmware ever
replies out of order, or grows request queuing, switch to the JSON shape — the
cross-check and its test are already there.

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
npm test              # frontend tests           (34)
npm run check:secrets # rule 7 check on the built bundle and the IPC surface
npm run verify        # build + frontend tests + secrets check
cd src-tauri && cargo test   # Rust unit + integration tests   (82)
```

**Installer targets: MSI only.** NSIS is deliberately disabled. Its toolchain is
cached under `%LOCALAPPDATA%` on `C:`, and the bundler finishes by renaming the
result onto the project drive — which fails with `os error 17` (cannot move a
file to a different disk) whenever the project lives elsewhere, as this one does
on `B:`. Re-enable `"nsis"` in `src-tauri/tauri.conf.json` if it ever moves to `C:`.

Rust must be on PATH. If `cargo` is not found, add `%USERPROFILE%\.cargo\bin`.

### Switching between the mock and a real device — one step

The target is read once at startup, in this order:

1. an address **saved in the app** — Android only, see [ANDROID.md](ANDROID.md)
2. the `IR_REMOTE_BASE_URL` **environment variable**
3. `IR_REMOTE_BASE_URL` in a **`.env` file** — looked for beside the working
   directory, its parent, and the executable, so it is found both under
   `npm run tauri dev` (which runs from `src-tauri/`) and from an installed build
4. the value **compiled in** from `IR_REMOTE_BASE_URL` at build time — this is
   how an APK gets a default, since there is no `.env` inside an installed app
5. the built-in default, `http://127.0.0.1:8080` — the mock

So the normal way to point at your device is `Application/.env`:

```dotenv
IR_REMOTE_BASE_URL=http://192.168.0.102
```

`.env` is gitignored. Comments, `export ` prefixes and quoted values are handled.
A value that is blank or not a usable URL is reported on stderr and skipped
rather than silently becoming the target.

To override for one run without editing the file:

```bash
IR_REMOTE_BASE_URL=http://127.0.0.1:8080 npm run tauri dev
```

```powershell
$env:IR_REMOTE_BASE_URL = "http://127.0.0.1:8080"; npm run tauri dev
```

**The base URL is not editable at runtime.** Settings shows it read-only along
with where it came from, so there is one source of truth for which device this
is. Change `.env` and restart.

### Alarms

Daily, repeating, persisted to `alarms.json` in the OS app-config directory so
they survive a restart. Each alarm has a time and a command.

**An alarm cannot promise the unit ends up on or off.** `/Power` is a single
toggle code and the device reports no state, so the UI says *"Send Power at
07:00"*, never *"Turn on at 07:00"* — if the air conditioner is already running
when a Power alarm fires, it switches off, and nothing in the app can detect
that. Discrete `/On` and `/Off` IR codes in the firmware are what would make
alarms idempotent and let the wording change.

**A missed alarm is skipped, never fired late.** A desktop app cannot wake
itself, so if the app was closed through an alarm's time it resolves as *missed*
and is shown as such. Firing a Power toggle hours late is worse than not firing
it. An alarm fires at most once per day; re-enabling one clears that so it can
still run today.

All the scheduling decisions live in `alarm.rs` and are pure — the caller passes
the local time in, which is how every rule above is tested without waiting for a
clock. `scheduler.rs` only supplies the clock and performs the send.

Rust's standard library has no timezone database, so the **view reports its UTC
offset** (`-new Date().getTimezoneOffset()`) and the core does the scheduling.
That is environment data the view happens to know, not a policy decision it makes.

---

## How the rules in section 4 are met

| Rule | Where | How to see it |
|---|---|---|
| 1. Never optimistic | `state.rs` `reduce`, `present.ts` `deviceLineFor` | Mock mode `slow`; the control reads "sending…" and never flips to a value |
| 2. Values carry their age | `AppState::age_ms`, `present.ts` `reage` | The age ticks every 200 ms; past `staleAfterMs` it greys and strikes through |
| 3. Disconnected is first-class | `Connection::Offline` | Mock mode `refuse`; the banner becomes a defined Disconnected state, not a toast |
| 4. Explicit timeouts | `api_client.rs` `Timeouts` | 3 s command, 1.5 s probe, both per-request; no unbounded wait exists |
| 5. Retries only when idempotent | `Command::is_idempotent` (always false) | No command is ever retried; only the TCP probe retries, 3 attempts with backoff |
| 6. Cancel always reachable | `AppState::can_abort`, `abort` IPC command | **No longer surfaced as a button** — see the note below |
| 7. Token never in the frontend | `auth.rs`, `Snapshot.has_credential` | The snapshot carries a boolean; `Credentials`' `Debug` prints `<redacted>` |

### On rule 6, the cancel control

`Instruction.md` rule 6 asks for a stop control that is always reachable. This
app no longer shows one, by request. Two things make that defensible, and both
should be re-examined if either changes:

- **It never was a device stop.** The API exposes no safe "off", and `/Power` is
  a toggle, so a stop button could switch the unit **on**. What the button
  actually did was cancel the app's own in-flight HTTP request.
- **Commands self-clear.** Every request carries a 3 s timeout, so a pending
  command resolves on its own; there is nothing that can stick.

The `abort` IPC command and its state-machine tests are kept, so restoring the
button is a UI change only. If the firmware ever gains a real off endpoint, rule
6 applies again in full and a stop control should come back.

### What the UI no longer shows

- **The log pane.** The core still records the last 100 requests and `get_log`
  still returns them; nothing renders it. For a device you cannot see, this was
  the debugging surface — if you are diagnosing something, that command is where
  the history is.
- **The banner detail line.** The banner is now just Connected / Disconnected /
  Connecting. A **stale** reading reads as Disconnected rather than Connected,
  since a sighting that has aged past the threshold no longer supports the claim.

### Demonstrating them against the mock

```bash
CTRL=http://127.0.0.1:8081/__control

curl "$CTRL/mode?set=normal"      # the JSON contract; ages stay fresh
curl "$CTRL/mode?set=raw"         # Main.ino TODAY: no HTTP framing, app cannot read it
curl "$CTRL/mode?set=plain"       # Main.ino once it sends headers: bare 'success' body
curl "$CTRL/mode?set=plainfail"   # bare 'fail' body -> device error, still reachable
curl "$CTRL/mode?set=slow&ms=10000"  # rule 1 + rule 4: pending, then timeout
curl "$CTRL/mode?set=refuse"      # rule 3: connection refused, banner goes Disconnected
curl "$CTRL/mode?set=silent"      # the older firmware, which replied nothing at all
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
  alarm.rs      pure daily-alarm scheduling — no I/O, no clock reads
  poller.rs     background reachability probe
  scheduler.rs  background alarm task
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

1. **No command can be confirmed yet.** The firmware replies, but without HTTP
   framing, so the reply is unparseable and every command still reads as a
   failure. See "Read this first" for the fix. This is the one that decides
   whether the app is usable.
2. **No stop / safe control exists.** `Instruction.md` rule 6 requires one.
   `/Power` is a toggle, so wiring stop to it could switch the unit **on** —
   precisely the rule-1 failure the document is written to prevent. The app
   therefore ships a **Cancel** button that aborts the in-flight request and is
   honestly labelled as not commanding the device. A real stop needs an explicit
   off endpoint.
3. **The reply carries no command name**, so a bare `success` cannot be matched
   against what was asked. See "Known gap" under Contract.
4. **No status endpoint.** `enableIR` is still assigned and never read, so the
   device holds no state to poll. Section 6.2's "current device state" is
   therefore the last *confirmed command*, plus a permanent caveat in the UI
   saying the app does not know whether the air conditioner is on.
5. **No authentication.** The firmware checks none, so the keychain path is
   dormant: nothing is sent unless you store a credential, and the device would
   ignore it if you did. It is wired and tested so that adding Basic auth to the
   firmware needs no app change.
6. **Section 7's "value that changes on its own"** has no counterpart, because
   the contract exposes no readable value. Staleness is instead demonstrated by
   the reachability probe going dark in `refuse` mode.

### To check once the device is reachable

- [ ] `IR_REMOTE_BASE_URL=http://192.168.0.102` — does the banner reach Connected?
      (This only proves the TCP port is open, which is all it claims.)
- [ ] Does the ESP32's DHCP lease still put it at `192.168.0.102`?
- [ ] Press Power once **before** the firmware fix. Expect the unit to respond
      while the app reports a failure — that is the HTTP/0.9 problem, and seeing
      it confirms the diagnosis.
- [ ] Press Power once **after** the fix. Expect "Power confirmed by device"
      with a latency in the log pane.
- [ ] Does each of Silent, Temp − and Temp + move the right setting? The app
      trusts the firmware's path-to-IR-code mapping and cannot check it.
- [ ] With the app polling, does a command still get through? The firmware
      serves one client at a time; the poller skips while a command is in
      flight, but that ordering is untested against real hardware.
- [ ] Does the device ever actually send a `401`? If it gains Basic auth, check
      the username the firmware expects.
- [ ] Real latency: is the 3 s command timeout and 1.5 s probe timeout right for
      an ESP32 on your Wi-Fi?
