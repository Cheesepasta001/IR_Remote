# IR Remote — Android

The same app, from the same source tree. The Rust core (`state.rs`, `alarm.rs`,
`api_client.rs`, `core.rs`, `poller.rs`) and the whole frontend are shared
unchanged. This file covers only what differs.

---

## Prerequisites

| Need | Why |
|---|---|
| JDK 17+ | Gradle |
| Android SDK, platform 34+, build-tools | the APK |
| NDK (27.x tested) | compiling Rust for Android |
| Rust targets | `aarch64-linux-android`, `armv7-linux-androideabi`, `i686-linux-android`, `x86_64-linux-android` |

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi \
                  i686-linux-android x86_64-linux-android
```

Set these before building (adjust the paths):

```bash
export JAVA_HOME="$LOCALAPPDATA/Programs/jdk/jdk-17.0.20.1+1"
export ANDROID_HOME="$LOCALAPPDATA/Android/Sdk"
export NDK_HOME="$ANDROID_HOME/ndk/27.0.12077973"
```

## Build

```bash
# Debug APK for a modern phone
IR_REMOTE_BASE_URL=http://192.168.0.102 npx tauri android build --debug --target aarch64

# Release, all ABIs
IR_REMOTE_BASE_URL=http://192.168.0.102 npx tauri android build

# Run on a connected device or emulator
npx tauri android dev
```

APKs land in `src-tauri/gen/android/app/build/outputs/apk/`.

### ⚠️ On this machine, `tauri android build` cannot finish

Tauri **symlinks** the compiled `.so` into `app/src/main/jniLibs/`, and Windows
refuses to create symlinks without Developer Mode or an elevated shell. This
machine has `AllowDevelopmentWithoutDevLicense = 0`, so the build stops with:

```
Creation symbolic link is not allowed for this system.
```

Everything before that point succeeds — the Rust compiles for
`aarch64-linux-android` correctly. Two ways past it:

**A. Enable Developer Mode** (one-time, needs admin). Settings → System → For
developers → Developer Mode. Then `tauri android build` works normally. This is
a machine-wide setting, so it is your call rather than something to enable
casually.

**B. Copy the library and drive Gradle yourself** (no admin). This is the path
used to produce the current APK:

```bash
# 1. Let the Tauri CLI compile Rust with the right NDK toolchain.
#    It will fail at the symlink step - that is expected.
IR_REMOTE_BASE_URL=http://192.168.0.102 npx tauri android build --debug --target aarch64

# 2. Copy the freshly built library in place of the symlink.
cp -f src-tauri/target/aarch64-linux-android/debug/libir_remote_lib.so \
      src-tauri/gen/android/app/src/main/jniLibs/arm64-v8a/libir_remote_lib.so

# 3. Assemble, skipping the task that re-invokes the Tauri CLI.
cd src-tauri/gen/android
./gradlew assembleArm64Debug -x rustBuildArm64Debug
```

**Do not run `cargo build --target aarch64-linux-android` directly** — without
the environment the Tauri CLI sets up, cargo picks the wrong `cc` and the link
fails. Always let the CLI do step 1.

**Check the timestamp after step 2.** If step 1 failed before recompiling, you
will copy a stale `.so` and ship old code in a build that otherwise looks fine.

`src-tauri/gen/android` is **tracked, not generated-and-forgotten** — the
manifest, the alarm receiver and the release cleartext setting all live there.
Only its build output is gitignored. Re-running `tauri android init` will
overwrite those customisations.

---

## Three things that are not the same as desktop

### 1. The device address

An installed APK has no `.env` beside it and no shell to export a variable in,
so the address resolves differently:

1. **saved in the app** — the editable field in Settings, Android only
2. the process environment (not normally reachable on a phone)
3. a `.env` file (desktop)
4. **compiled in** from `IR_REMOTE_BASE_URL` at build time
5. the built-in mock default

So `IR_REMOTE_BASE_URL` at build time gives the APK a sensible default, and the
Settings field lets you re-point it without rebuilding. Desktop is unchanged:
the field is hidden there and `.env` remains the single source of truth.
`build.rs` declares `rerun-if-env-changed`, so changing the variable actually
triggers a rebuild rather than reusing a cached one.

### 2. Cleartext HTTP is explicitly enabled

The ESP32 serves plain HTTP with no TLS, and Android 9+ blocks cleartext by
default. Tauri only enables it for debug builds, so **without the override in
`app/build.gradle.kts` a release APK would reach nothing while the debug build
worked** — a trap worth knowing about. It is enabled app-wide because the target
address is set at runtime, and Android's network security config keys on literal
hostnames; it cannot express "any private LAN address".

### 3. Alarms are handled by Android, not by the Rust scheduler

This is the significant one.

`scheduler.rs` runs a tokio timer, which is fine on desktop where the process
lives as long as the window is open. **Android suspends and kills backgrounded
processes, and Doze defers work**, so that timer stops firing the moment the app
leaves the screen. A naive port would leave every overnight alarm marked
"missed".

So on Android the Rust scheduler stands down entirely
(`android_alarm::native_scheduler()`), and the schedule is owned by:

```
Rust core            writes alarms.json, then calls the plugin
  └─ AlarmPlugin.kt      saves the path + address, arms AlarmManager
      └─ AlarmScheduler.kt   setExactAndAllowWhileIdle, one shot each
          └─ AlarmReceiver.kt    fires with the app DEAD: sends the command,
                                  records the outcome into alarms.json,
                                  re-arms tomorrow
BootReceiver.kt      re-arms everything after a reboot
```

Rust passes the `alarms.json` path to Kotlin rather than letting Kotlin guess
it, because `app_config_dir()` resolves through Tauri's own path plugin and is
not something the Kotlin side can recompute.

**The cost, stated plainly:** the request rules now exist in two languages.
`AlarmReceiver.sendOnce` is a second implementation of `api_client.rs` and obeys
the same rules — exactly one attempt and never retried (rule 5), an explicit
timeout (rule 4), and only a positive confirmation counts as success (rule 1);
a timeout or an unreadable body is recorded as `failed`, never as `fired`.
**If the response contract changes, both must change.**

#### Permissions

- `INTERNET` — the requests.
- `SCHEDULE_EXACT_ALARM` / `USE_EXACT_ALARM` — firing on the minute while the
  device dozes. On Android 12+ the exact permission can be withheld by the user;
  `AlarmScheduler.canScheduleExact()` checks it and falls back to an inexact
  alarm, which the OS may batch by minutes, rather than failing silently.
- `RECEIVE_BOOT_COMPLETED` — AlarmManager forgets everything across a reboot.

The receivers are **not exported**: nothing outside the app can trigger an IR
transmission.

#### What an alarm still cannot promise

Unchanged from desktop, and worth repeating: `/Power` is a toggle and the device
reports no state, so an alarm means "send this command at this time", never
"turn on". If the air conditioner is already running when a Power alarm fires,
it switches off. Discrete `/On` and `/Off` codes in the firmware are what would
make alarms idempotent.

---

## Unverified — needs a real device

Everything below compiles and installs, but none of it has been exercised on
hardware from here.

- [ ] Does the app reach `192.168.0.102` from the phone's Wi-Fi? Both must be on
      the same LAN, and phone Wi-Fi often differs from the desktop's network.
- [ ] Does cleartext actually work in a **release** APK, not just debug?
- [ ] Does an alarm fire with the app swiped away and the screen off? This is the
      whole point of the AlarmManager work and the one thing that cannot be
      checked without waiting for a real wall-clock time on a real device.
- [ ] Does it survive Doze? Leave the phone idle for an hour before the alarm.
- [ ] Does it survive a reboot (`BootReceiver`)?
- [ ] On Samsung/Xiaomi/Oppo, is the app exempted from aggressive battery
      optimisation? These OEMs kill alarms that stock Android honours.
- [ ] Is `SCHEDULE_EXACT_ALARM` granted on Android 13+? If not, alarms drift.
- [ ] Does the saved address survive an app restart (`device.txt`)?
- [ ] Layout on a real phone — the CSS is responsive but has only been reasoned
      about, not seen at 375 px on a device.
