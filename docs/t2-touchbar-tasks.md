# T2 Touch Bar adapter tasks

Goal: run the omarchy-glance Touch Bar client on T2 MacBook Pros (target:
16-inch 2019, A2141) as well as on the T1 machine it was built on. Unchecked
items are planned work, not implemented behavior. **On hold until Omarchy is
installed on the T2 machine**: development and testing happen on that machine,
because nearly every question below needs real hardware to answer.

Planned 2026-10-09. Task IDs use an `H` prefix (hardware) so they don't clash
with the `R`/`T` IDs in [multi-output-tasks.md](multi-output-tasks.md).

## Background

On the T1 machine, t1bridge owns the hardware. Its root service holds DRM
master and the touch device, and the renderer only speaks the Touch Bar
hardware IPC v1 ([t1bridge-interfaces.md](t1bridge-interfaces.md)): `Hello`,
memfd `RegisterBuffer`, `SubmitFrame` with damage, `FrameReleased`,
`InputFrame` (contacts plus Fn state) and `TapKeys`. Those calls are made
directly from `src/renderer.rs` through `src/proto.rs`.

A T2 Mac has no t1bridge. The Touch Bar uses standard kernel interfaces
instead (as of 2026; verify on the machine):

| Need | T1 (t1bridge) | T2 (expected) |
| --- | --- | --- |
| Display | `SubmitFrame` over the IPC socket | `appletbdrm` DRM device: dumb buffer, `DIRTYFB` for damage |
| Touch | `InputFrame` contacts | evdev multitouch device (`hid-multitouch`) |
| Fn state | in `InputFrame` | Fn key on the internal keyboard's evdev device |
| Key taps (Esc, F-keys) | `TapKeys` | uinput virtual keyboard |
| Bar brightness | `SetDisplayBrightness` (unused today; renderer dims pixels) | `appletb_backlight` backlight device |
| Default bar when we're not running | t1bridge built-in renderer | `hid-appletb-kbd` (kernel F-key/media mode) or tiny-dfr |
| Who launches us | t1bridge launcher runs `~/.config/t1bridge/renderer` | our own systemd unit |

Omarchy's T2 support (`install/hardware/apple/fix-t2.sh` in the Omarchy repo)
installs `linux-t2`, `apple-t2-audio-config`, `apple-bcm-firmware-fetcher` and
`t2fanrd`, and its manual says the Touch Bar runs on the kernel's built-in Boot
Camp-style support. It doesn't install tiny-dfr. Prior art for a DRM + evdev
Touch Bar daemon: [tiny-dfr](https://github.com/AsahiLinux/tiny-dfr).

## Stage 0: Before installing Omarchy on the T2 machine

### H00 — Prepare the T2 machine

- [ ] Confirm the model (About This Mac shows "MacBook Pro (16-inch, 2019)").
  The A2141 isn't in Omarchy's manual list of T2 models (checked 2026-10-09),
  but the installer detects T2 by PCI ID (`106b:1801`/`1802`), so it should
  still take the T2 path. Check the manual again and `#omarchy-on-other` on
  Discord for reports on this model.
- [ ] **Save the Wi-Fi/Bluetooth firmware before wiping macOS.**
  `apple-bcm-firmware-fetcher` reads it from an on-disk macOS volume. A
  single-boot install that erases macOS finds nothing, and the T2 then has no
  Wi-Fi. In macOS, download the t2linux script
  (<https://wiki.t2linux.org/tools/firmware.sh>), run `bash firmware.sh` and
  choose method 2, which writes `~/Downloads/firmware.tar`. Not method 1: it
  stores the firmware on the EFI partition, which the install recreates. Copy
  the tarball off the machine, next to the T1 machine's backups. Install it
  on Linux as the "On Linux" section of
  <https://wiki.t2linux.org/guides/wifi-bluetooth/> describes. Keep a wired
  Ethernet or USB tethering option ready anyway.
- [ ] In macOS Recovery (Cmd-R), open Startup Security Utility and set Secure
  Boot to "No Security" and "Allow booting from external media".
- [ ] Optionally do a full macOS backup or a macOS install on an external drive
  first, as was done for the T1 machine.

Acceptance: firmware saved off the machine; the USB installer boots.
Dependencies: none.

## Stage 1: Explore the hardware (on the T2 machine)

### H01 — Survey the Touch Bar devices

- [ ] Record the kernel and driver versions (`uname -r`, `lsmod | grep -E
  'appletb|t2bce|hid_multitouch'`).
- [ ] DRM: which `cardN` is `appletbdrm`; connector name, mode, reported
  width × height (landscape or portrait), pixel format, whether `DIRTYFB` is
  supported, and whether Hyprland lists it as a monitor (`hyprctl monitors
  all`).
- [ ] Measure the visible area: adapt `tools/cutoff_test.py`
  (draw markers, read off the last visible column) to find the real usable
  width, the equivalent of the T1's `VISIBLE_WIDTH = 2060` in `renderer.rs`.
  Check for an offset at either end too.
- [ ] Touch: which `/dev/input/event*` is the Touch Bar digitizer; axis ranges,
  slot count, and how its coordinates map to display pixels (scale, flips,
  rotation).
- [ ] Fn: which device reports the Fn key, and whether `hid-appletb-kbd`
  swallows it or changes bar mode on it (`/sys/bus/hid/drivers/hid-appletb-kbd/*/mode`).
- [ ] Backlight: `/sys/class/backlight/appletb_backlight` range and whether it
  is writable by a user.
- [ ] Find out what happens to the kernel's built-in bar when another process
  takes DRM master on the card, and when it lets go.

Result goes in a new `docs/t2-hardware.md`, like
[t1bridge-interfaces.md](t1bridge-interfaces.md) for the T1.

Acceptance: every row of the Background table is confirmed or corrected, with
the device paths and numbers. Dependencies: H00, Omarchy installed.

### H02 — Decide ownership and permissions (ADR 0002)

- [ ] Keep Hyprland off the Touch Bar card. On the T1 machine t1bridge puts it
  on `seat-touchbar`; try a udev rule that does the same on the T2 (and check
  Aquamarine's `AQ_DRM_DEVICES` as a fallback).
- [ ] Choose how an unprivileged client gets DRM master plus the touch and
  uinput devices. Options:
  1. udev rules: a group with access to the card, the touch device and
     `/dev/uinput`; the client opens them directly.
  2. A small root helper (or the backend service as a system unit) that owns
     the devices and hands the client fds, in effect a minimal t1bridge.
  3. Use the T1 protocol unchanged: write a T2 "hardware service" that speaks
     Touch Bar hardware IPC v1 on the T2. Then the client needs no backend
     abstraction at all.
- [ ] Decide what runs when omarchy-glance is off or crashed: the kernel's
  built-in F-key/media mode must come back on its own.

Write the result up as `docs/adr/0002-t2-touch-bar.md`. Option 3 is worth a
serious look: it keeps one client code path, and the protocol already covers
everything the client uses.

Acceptance: ADR approved by the user. Dependencies: H01.

## Stage 2: Build the adapter

Tasks below assume options 1 or 2 (a backend interface in the client). If the
ADR picks option 3, H03 is dropped and H04–H07 become parts of the T2 hardware
service instead.

### H03 — Hardware backend interface in the client

- [ ] Put an interface between `renderer.rs` and the hardware, covering:
  geometry and visible width, frame buffers (Cairo surfaces), submit with
  damage, buffer release, input contacts with Fn state, key taps, display
  brightness, and an fd for the poll loop.
- [ ] Move the current `proto.rs` use behind it as the T1 backend, with no
  behavior change.
- [ ] Make `VISIBLE_WIDTH` per-backend instead of a constant.

This refactor needs no T2 hardware and could be done earlier on the T1
machine, guarded by the golden tests and a hands-on check of the bar.

Acceptance: golden previews match exactly; on the T1 machine every widget,
touch, hold-to-repeat, Fn, reconnect and backend restart behaves as before.
Dependencies: H02.

### H04 — T2 display output

- [ ] Open the `appletbdrm` card, take DRM master, create two dumb buffers,
  set the mode, and flush damage with `DIRTYFB` (the `drm` crate).
- [ ] Wrap the mapped buffers as Cairo surfaces; handle rotation if the panel
  reports portrait (the renderer's `portrait` path already exists).
- [ ] Recover from losing the card (driver reload, suspend/resume).

Acceptance: the default layout draws on the T2 bar with nothing cut off, and
partial redraws work. Dependencies: H03, H01.

### H05 — T2 touch input

- [ ] Read the digitizer through evdev (multitouch protocol B slots) and
  convert to the client's contacts `{id, x, y, tip}` in display pixels.
- [ ] Grab the device exclusively while running, if the kernel driver
  otherwise acts on touches too.

Acceptance: taps, holds (hold-to-repeat), multi-finger touches and the contact
latch behave as on the T1. Dependencies: H03, H01.

### H06 — T2 keys and Fn

- [ ] Track Fn from the keyboard's evdev device (read only, no grab) to switch
  to the Fn layer.
- [ ] Send Esc and F-key taps through a uinput virtual keyboard, replacing
  `TapKeys`.

Acceptance: Esc, F1–F12 and the Fn layer work in Hyprland and in a terminal.
Dependencies: H03, H01.

### H07 — T2 brightness and idle dimming

- [ ] Keep pixel dimming (`DIM`) as is, or switch to `appletb_backlight` on
  the T2 if it looks better; the proposed Touch Bar brightness control in
  `TODO.md` would use the same path.
- [ ] Turn the backlight off when the lid is closed or the screen locks, if
  the kernel doesn't already.

Acceptance: the bar dims when idle and wakes on touch. Dependencies: H04.

### H08 — Install, launch and fall back

- [ ] `omarchy-glance on`/`off`/`status` detect the T2 (no t1bridge socket and
  an `appletbdrm` card) and install or remove a systemd user unit for the
  client instead of `~/.config/t1bridge/renderer`.
- [ ] Install the udev rules or helper chosen in H02.
- [ ] On exit or crash, hand the bar back to the kernel's built-in mode.
- [ ] Update the README: T2 support, requirements, limitations.

Acceptance: on a fresh Omarchy T2 install, `omarchy-glance on` gives the
custom bar after a reboot, and `off` (or killing the client) brings back the
default F-key/media bar. Dependencies: H04–H07.

## Stage 3: Verify

### H09 — Hardware check and measurements

- [ ] Hands-on check on the T2 of every widget, touch, hold-to-repeat, Fn,
  config reload, invalid config, backend restart, suspend/resume and
  lock/unlock.
- [ ] Measure RSS, CPU and full-frame time with the method in
  [performance.md](performance.md) and record the T2 numbers.
- [ ] Re-run the T1 hands-on check to confirm nothing regressed there.

Acceptance: both machines pass; numbers recorded. Dependencies: H08.
