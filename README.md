# IR Remote

An ESP32 that pretends to be an air-conditioner remote control, driven from the
Blynk app on your phone.

Press a button in Blynk → the ESP32 fires the matching infrared code at the air
conditioner. Because it goes through Blynk's cloud rather than your LAN, it
works from anywhere with an internet connection.

```
Blynk app  ──►  Blynk cloud  ──►  ESP32  ──►  IR LED  ──►  air conditioner
 (phone)          (sgp1)                       (GPIO 14)
```

---

## What you need

| | |
|---|---|
| Board | ESP32 dev board (built with `esp32:esp32:esp32`, core 3.3.11) |
| Emitter | IR LED on **GPIO 14**, with a current-limiting resistor |
| Account | A free [Blynk](https://blynk.cloud) account |
| IDE | Arduino IDE, or `arduino-cli` |

**Libraries** — install both from the Arduino Library Manager:

| Library | Version built against |
|---|---|
| `Blynk` by Blynk | 1.3.5 |
| `IRremoteESP8266` | 2.9.0 |

An IR LED driven straight off a GPIO pin is dim and gives you maybe a metre of
range. A transistor driver and a lower-value resistor will get you across a
room.

---

## Setup

### 1. Blynk console

Create a **template**, then add four **datastreams** — one per button. Each is a
Virtual Pin, Integer, range 0–1:

| Virtual pin | Datastream name | What the ESP32 does |
|---|---|---|
| **V0** | Power | toggles the unit on/off |
| **V1** | Silent | silent / quiet mode |
| **V2** | Temp + | temperature **up** |
| **V3** | Temp − | temperature **down** |

> **Watch V2 and V3.** V2 is the temperature *increase* and V3 the *decrease*.
> They are easy to wire up the wrong way round in the console, and nothing will
> complain — the buttons will just quietly do each other's job.

Then add four **Button** widgets on the web dashboard or mobile app, one bound
to each datastream. Note the widget mode — see the trap below.

Copy the **Template ID**, **Template Name** and **AuthToken** from the device's
*Device Info* tab.

### 2. `Main/secret.h`

This file is gitignored and is **not** in the repository — you have to create
it. It holds both your WiFi password and your Blynk token, which is why.

```cpp
#ifndef SECRET_H
#define SECRET_H

// --- WiFi ---
#define NetworkName       "your-ssid"
#define NetworkPassword   "your-wifi-password"

// --- Blynk, from Device Info in the console ---
#define BLYNK_ID          "xxxxxxxxxxx"
#define BLYNK_NAME        "IR Remote"
#define BLYNK_TOKEN       "your-32-character-auth-token"

// --- IR codes for YOUR air conditioner (see below) ---
#define MY_BIT            32          // NEC is normally 32 bits
#define POWER_CODE        0x00000000
#define SILENT_CODE       0x00000000
#define HIGH_TEMP_CODE    0x00000000
#define LOW_TEMP_CODE     0x00000000

#endif
```

### 3. Capturing your IR codes

The codes above are specific to one air conditioner. To find yours, flash the
`IRrecvDumpV2` example that ships with **IRremoteESP8266**, wire up an IR
receiver (TSOP38238 or similar), point your existing remote at it and press each
button. The sketch prints the protocol, the code and the bit length.

This project assumes **NEC** — `irsend.sendNEC(CODE, MY_BIT)`. If the dump says
something else (Samsung, Coolix, Daikin…), change the send call to the matching
`irsend.sendXxx()`. Many air conditioners send their entire state in one long
frame rather than one code per button, in which case you want
`IRac` / `IRsend.sendDaikin`-style state objects instead of discrete codes.

### 4. Flash it

Arduino IDE: open `Main/Main.ino`, select your ESP32 board, upload.

Or:

```bash
arduino-cli compile --fqbn esp32:esp32:esp32 Main
arduino-cli upload  --fqbn esp32:esp32:esp32 -p COM3 Main
```

Open the serial monitor at **115200** to watch it connect and to see a line each
time it fires a code.

---

## How it works

`Main.ino` is short enough to read in one sitting.

```cpp
BLYNK_WRITE(V0){ powerButton(); }     // called when V0 is written
```

`Blynk.begin()` connects out to the cloud and `Blynk.run()` keeps that
connection alive. When you press a button, Blynk delivers the write to the
matching `BLYNK_WRITE(Vn)` handler, which calls `irsend.sendNEC()`.

There is no web server and no open port — the ESP32 dials out, so it works
behind NAT with nothing to forward.

---

## Traps worth knowing

### A push-button widget fires the code twice

The handlers ignore the value that was written:

```cpp
BLYNK_WRITE(V0){ powerButton(); }     // fires on ANY write, 1 or 0
```

A Blynk button in **push / momentary** mode writes `1` on press and `0` on
release. Both are writes, so both fire the IR code — and since Power is a
toggle, the unit turns on and straight back off.

Two ways out. Either set the widgets to **Switch** mode, or guard on the value:

```cpp
BLYNK_WRITE(V0){
  if (param.asInt() == 1) powerButton();
}
```

The guard is the sturdier fix, because it survives someone changing the widget
later.

### Power is a toggle, and the ESP32 has no idea what state the unit is in

`POWER_CODE` is the single code your remote sends for the power button. It
means "flip", not "on". So:

- if the air conditioner is already running, a Power press turns it **off**
- nothing in this project can tell you which way it went

The ESP32 only transmits — it has no IR receiver wired up and reads nothing
back. Blynk will happily show a button that looks like a switch, but there is no
state behind it.

This matters most for **automations**. A Blynk schedule that sends Power at
07:00 will turn the unit on only if it happened to be off. If you want reliable
scheduling, capture the discrete **on** and **off** codes if your remote has
them — many do, even when the physical button is a single toggle — and give them
their own virtual pins.

### A "Connected" device in Blynk is not a working IR link

Blynk tells you whether the **ESP32** is online, which is genuinely useful. It
says nothing about whether the IR LED is aimed at the air conditioner, in range,
or actually wired up. If Blynk says connected and nothing happens, that is
almost always the IR side.

---

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| Device shows offline in Blynk | Wrong SSID/password, or 5 GHz-only WiFi — ESP32 needs 2.4 GHz |
| "Invalid token" | Token mistyped, or the device belongs to a different Blynk region |
| Connects, but nothing happens | IR LED backwards, no resistor, out of range, or wrong IR protocol |
| One press turns it on then off | Push-button widget firing twice — see the trap above |
| Temp + cools, Temp − heats | V2 and V3 swapped in the console |
| Works up close, not across the room | Bare GPIO drive; add a transistor |

Serial monitor at 115200 is the first place to look — it prints a line for every
code it sends, so you can tell a Blynk problem (no line) from an IR problem
(line appears, unit ignores it).

---

## Repository layout

```
Main/
  Main.ino      the whole firmware
  secret.h      WiFi + Blynk + IR codes — gitignored, create it yourself
LICENSE         MIT
```

### Housekeeping

Two leftovers from an earlier version of this project, both safe to delete:

- `package-lock.json` at the repo root — from a desktop/Android app that used to
  drive the ESP32 over HTTP on the LAN. It was replaced by Blynk in `22daacc`.
- `LOCALIP`, `GATEWAY`, `SUBNET` and `pDNS` in `secret.h` — these configured a
  static IP back when the ESP32 ran its own web server. Nothing reads them now.

---

## License

MIT — see [LICENSE](LICENSE).
