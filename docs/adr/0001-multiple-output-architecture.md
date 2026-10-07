# ADR 0001: Backend service with Touch Bar and desktop clients

- Status: Accepted; rename (R01) done, backend split pending
- Date: 2026-10-06 (revised the same day after review)

## Context

The Rust application currently combines provider polling, configuration,
widget state, layer selection, input handling, actions, Cairo drawing, and
t1bridge transport in `src/renderer.rs`. The Python renderer is a fallback.
The hardware output has a usable width of 2060 pixels and hardware-specific
touch and key handling. `--preview` produces a static image without hardware.

We want the same widgets on machines without a physical Touch Bar: a live
desktop row below the Omarchy top bar that reserves screen space for normal
windows. The desktop row is for widgets that should stay visible all the time.
It is not a replacement keyboard row, so keyboard-like Touch Bar features (Esc,
F-keys, the Fn layer) do not carry over. Later, we may also explore a true
second row inside the Omarchy bar.

Two facts about the current launch path constrain the design:

- t1bridge's `t1-touchbar` user service starts `~/.config/t1bridge/renderer`
  once, with no arguments. If it exits, the service runs the built-in renderer
  until it is restarted; it never retries the selected renderer
  (`crates/t1-touchbar/src/renderer_launcher.rs`).
- The renderer exits on any hardware-socket error. So anything that lives in
  that process, including a desktop backend, dies with the hardware connection
  and does not come back by itself.

Desktop machines have no t1bridge at all, so the backend cannot depend on it.

Proposed features in `TODO.md` (sliders, gestures, media seeking, workspaces,
notifications, and so on) remain proposals; this decision provides extension
boundaries rather than approving them.

## Decision

Rename the project to **omarchy-glance**, since it is no longer Touch
Bar-specific, and split it into a **backend service** and **output clients**.
The bare name `glance` is taken on the AUR (a self-hosted dashboard that
installs `/usr/bin/glance`), so the package, binary, config directory, and
runtime paths all use `omarchy-glance`.

- The backend is its own systemd user service. It owns configuration,
  providers, widget state, and action execution, and serves clients on a
  user-private Unix socket. It runs with or without t1bridge.
- The **Touch Bar client** is what `~/.config/t1bridge/renderer` launches. It
  owns the t1bridge connection, Cairo drawing, touch handling, the Fn layer,
  and hardware key taps. It is the first client of the backend protocol.
- The **desktop client** comes later: QML controls shared by a layer-shell host
  (standalone Quickshell or an Omarchy panel plugin, chosen by a spike) and,
  in future, an embedded Omarchy row.

Order of work: build the backend and the Touch Bar client first and prove the
Touch Bar matches today's behavior and appearance. Then spike the desktop hosts.
Then build the desktop renderer.

Retire the Python renderer once the Touch Bar client is proven; it will not
implement the backend protocol.

```text
                    +-------------------- backend (user service) --------------------+
                    |  config -> providers -> widget state        actions -> shell   |
                    +-----------------------------^------------------------^-----------+
                                       snapshots  |  semantic input/actions |
                         +------------------------+------------+-----------+
                         |                                     |
           Touch Bar client (t1bridge renderer)       Desktop client (QML)
           Cairo, touch, Fn layer, key taps,          layer-shell panel host,
           t1bridge buffers                           later an Omarchy row host
```

## Boundaries

| Part | Owns | Does not own |
| --- | --- | --- |
| Providers (backend) | Samples, usage records, media state, microphone levels, command output | Geometry, drawing, outputs |
| Core (backend) | Config reload, stable widget identity, visibility, action availability, timers, per-client sessions and demand | Cairo/QML objects, pixel geometry |
| Actions (backend) | Shell execution, press/release and repeat scheduling, cancellation, results | Drawing, hardware keys |
| Touch Bar client | Layout within 2060 px, Cairo drawing, press feedback, touch contacts, Fn layer, idle dimming, `TapKeys` (later, if approved, display and keyboard-backlight IPC) | Providers, shell commands, config parsing beyond its own section |
| Desktop client | QML layout and controls, mouse input, theme/host environment, window placement | Providers, shell commands |

Widget state is structured data: graph samples and ranges, media title and
playback capabilities, agent limits, mute state, command output, and action
availability. It also carries each widget's presentation options from config
(width, icon, font size, and so on) so clients don't parse config themselves.
State never contains drawing surfaces or pixel coordinates.

Clients send semantic input: press and release a widget, activate it, or (later)
set a value. The backend runs the configured action. Clients never send shell
commands. Hold-to-repeat lives in the backend so that a client that disconnects
mid-press cannot leave an action repeating.

## Output support

Each widget kind and feature is labelled by the outputs that support it. The
labels go in the README widget table and in the backend's capability data, so a
client can reject or hide a widget it does not support, with a log message,
rather than silently drawing nothing. Labels confirmed by the inventory in
[`../widget-inventory.md`](../widget-inventory.md) (task T01):

| Widget / feature | Touch Bar | Desktop | Notes |
| --- | --- | --- | --- |
| `glance.esc` | yes | no | Keyboard key |
| `button` with `key` (Esc, F1–F12) | yes | no | Uses t1bridge `TapKeys` |
| `button` with `exec` | yes | yes | Hold-to-repeat on both |
| `command` | yes | yes | |
| `glance.agents` | yes | yes | Usage-based hiding on both |
| Graphs (`cpu`, `memory`, `gpu`, `network`, `disk`, `battery`, `fan`) | yes | yes | |
| `glance.mic` | yes | yes | One shared capture |
| `glance.media` | yes | yes | |
| `glance.spacer` | yes | yes | Sizes are per-output |
| Fn layer | yes | no | Driven by the physical Fn key |
| Idle dimming | yes | no | |
| `debug.*` (background, border, test pattern) | yes | no | |
| `repeatDelay`, `repeatInterval` | yes | yes | Applied by the backend, from the pressing output's file |
| `colors`, `font` | yes | yes | Desktop defaults to the host theme |
| Display brightness, keyboard backlight via t1bridge IPC | proposed | no | Not used today; see `TODO.md` |

New widgets and proposals in `TODO.md` must state their label.

## Backend protocol

The backend listens at `$XDG_RUNTIME_DIR/omarchy-glance/backend.sock`, mode 0600, and
is socket-activated so that whichever client connects first starts it.

The protocol provides:

- A version and capability handshake in which the client declares its output
  type (`touchbar` or `desktop`), followed by a complete snapshot and then
  revisioned updates.
- Stable widget and session IDs, and press/release/activate requests with
  acknowledgements or errors.
- Demand: the backend only runs providers that a connected, visible client
  needs. The microphone keeps today's conditions: another application is
  recording, input is live, and some visible widget needs levels. Clients share
  one capture.
- Cleanup on disconnect (cancel presses and repeats, release demand) and
  resynchronization by snapshot after reconnecting or missing updates.
- Bounded per-client queues with coalescing, so a slow client cannot stall the
  backend or other clients.

Start with simple length-prefixed JSON messages and measure graph and waveform
traffic (the waveform runs at 20 levels per second) before considering anything
cheaper. Document framing, ordering, errors, and version compatibility in
`docs/` before the desktop client relies on it.

## Process lifecycle

- One binary, `omarchy-glance`, with explicit modes: `touchbar` runs the Touch
  Bar client, `backend` runs the service, and `preview` runs both in one
  process to draw a PNG. t1bridge starts its renderer with no arguments, so
  `~/.config/t1bridge/renderer` is a small installed wrapper that runs
  `omarchy-glance touchbar`. Control commands (on/off/restart/status/log/config)
  are subcommands of the same binary (since R01).
- The Touch Bar client must not exit on recoverable errors. Exiting hands the
  bar to t1bridge's built-in renderer for the rest of the session. It reconnects
  to the backend and to the t1bridge socket instead, and shows a clear
  disconnected state on the bar while the backend is unavailable.
- A desktop client failing or disconnecting never affects the Touch Bar, and
  the backend keeps running for desktop clients when t1bridge is absent.
- Only one backend runs per user; the socket and service unit guarantee it.

## Configuration

Each output has its own file in `~/.config/omarchy-glance/`:

- `touchbar.json`: the existing Touch Bar format, unchanged (`version`,
  `layers.default` and `layers.fn`, `colors`, `font`, `idleDimSeconds`,
  repeat timings, `debug`).
- `desktop.json`: the same `version` and section shape, but a single widget
  list (left/center/right) plus desktop settings such as monitor and height.
  Colors and font come from the host theme by default.

Each user file replaces its own built-in default entirely, as today; there is
no merging between files or with defaults. Separate files keep that rule
practical: changing the desktop row never requires copying the Touch Bar
config, and the two share almost no settings. They also fail independently: an
invalid `desktop.json` falls back to the desktop default and leaves the Touch
Bar config alone. A widget wanted on both outputs is written in both files;
shared widget definitions can be added later if that becomes tedious.

Widget IDs use a `glance.` prefix (`glance.cpu`, `glance.agents`). The project
has a single user, so there is no backward compatibility: old paths, file
names, and `touchbar.*` IDs are not read, and existing files are moved and
updated as part of each change.

The backend validates each file against output labels at load time and logs
unsupported widgets, for example `glance.esc` in `desktop.json`.

## Desktop host

The desktop controls take a host environment (palette, font, scale, available
size, transparency, visibility) and make no assumption about their window.

Two hosts are candidates, to be compared by a spike before the desktop
renderer is built:

- **Standalone Quickshell `PanelWindow`** (layer-shell). It works on any
  layer-shell compositor and is the portable baseline for machines that are
  not running Omarchy.
- **Omarchy third-party `panel` plugin.** It runs inside `omarchy-shell` and
  receives `PluginBarStateApi` (`barHidden`, `barSize`, `fontFamily`,
  `position`), which the shell provides "for plugins that position independent
  windows". That is a supported interface for following the bar without
  forking or replacing it. It has no transparency field, and plugins run
  unsandboxed in the shell.

The spike must check: exclusive-zone stacking against the Omarchy bar,
including after `omarchy-shell` restarts (Hyprland may order zones by map
order); clicks that do not take focus from the active window; fractional
scaling; fullscreen behavior; and what happens when the bar is moved to another
edge or hidden. The first release supports a top-anchored horizontal row only.
When the bar is on another edge or hidden, the row stays at the top.

An embedded row inside the Omarchy bar needs a supported upstream extension
API for mounting, sizing, combined reservation, theme, transparency, input
boundaries, and popups. The installed bar has no second-row option. This
remains future work and is not a prerequisite.

## Alternatives

| Alternative | Reason not selected |
| --- | --- |
| Backend inside the t1bridge-launched process | Dies with the hardware connection and cannot come back (the launcher never retries); machines without t1bridge would need a second way to launch it |
| Mirror Cairo frames into a desktop window | Keeps hardware sizing and styling; no native desktop controls |
| Implement each output independently | Duplicates providers, actions, and resource management |
| Replacement Omarchy bar plugin | Copies bar maintenance; not needed for an extra row |
| Fork the Omarchy package | Large maintenance scope; does not follow package updates |
| Ordinary floating window | Does not reserve space |

## Consequences

- Two presentations, Cairo and QML. They need the same behavior, not the same
  pixels.
- The Touch Bar gains an IPC hop. Press feedback stays local to the client so
  touch response does not depend on the backend; only the action goes over the
  socket.
- The protocol is exercised by the Touch Bar before any desktop code exists,
  which tests its completeness early.
- Installation must work without t1bridge and without a checkout at a fixed
  path. Today only `omarchy-glance on python` depends on the checkout (the
  path is compiled in), and that goes with Python in T05.
- Python is removed, so `touchbar.default.json` and docs no longer need to stay
  compatible with it.
- Media seeking needs position/duration support beyond `omarchy-shell media
  status`. Notifications need integration with the existing service; the
  backend must not start a competing daemon. Non-Hyprland compositors need
  capability checks.

## Validation

- Before the split, add a deterministic preview (fixed clock, fixed provider
  data) and golden PNGs for the default and Fn layers. The Touch Bar client
  must reproduce them pixel for pixel.
- Focused tests for visibility, session demand, press/repeat cancellation,
  config reload, and protocol reconnection.
- Performance: in Touch Bar-only use, backend plus client together should stay
  close to the current baseline (14.5 MB RSS, about 0.8% CPU, 2.35 ms full
  frame). Record the new numbers.
- Real hardware checks of every widget, touch, repeat, Fn, and reload, plus
  restarting the backend while the bar is running.

Tasks and acceptance criteria are in
[`../multi-output-tasks.md`](../multi-output-tasks.md).

## References

- [Current features and configuration](../../README.md)
- [Feature proposals](../../TODO.md)
- [Touch Bar hardware IPC](../t1bridge-interfaces.md)
- [Quickshell PanelWindow](https://quickshell.org/docs/types/Quickshell/PanelWindow)
- [Layer-shell protocol](https://github.com/swaywm/wlr-protocols/blob/master/unstable/wlr-layer-shell-unstable-v1.xml)
- t1bridge launcher: `crates/t1-touchbar/src/renderer_launcher.rs` in the
  t1bridge source.
- Installed Omarchy sources: `/usr/share/omarchy/shell/README.md` (plugin
  kinds), `services/PluginBarStateApi.qml`, `plugins/bar/Bar.qml`,
  `Ui/PluginBarApi.qml`.
