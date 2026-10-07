# Multiple-output implementation tasks

Architecture: [ADR 0001](adr/0001-multiple-output-architecture.md).
Unchecked items are planned work, not implemented behavior. There is no
backward compatibility: when a task renames or moves something, it also updates
the user's files and removes the old name. Every stage ends
with a working Touch Bar. Proposed features in `TODO.md` are separate scope
unless included below.

## Stage 0: Rename to omarchy-glance

### R01 — Rename the project

- [x] Rename the GitHub repo to `omarchy-glance` and update the local remote;
  move the checkout to `~/Work/omarchy-glance`.
- [x] Rename the crate and binary to `omarchy-glance`, and fold
  `touchbar-custom` into its subcommands (`on`, `off`, `restart`, `status`,
  `log`, `config [edit]`); the t1bridge renderer link points at the binary
  until T04 introduces the wrapper.
- [x] Read the user config from `~/.config/omarchy-glance/touchbar.json` only
  (no fallback to the old path); move the existing
  `~/.config/touchbar/config.json` there by hand; rename the built-in default
  to `touchbar.default.json`.
- [x] Switch widget IDs to the `glance.` prefix in code, the default config,
  docs, and the moved user config; drop `touchbar.*` IDs entirely.
- [x] Update README, TODO, ADR references, and the Python renderer only as far
  as needed to keep it working until T05 removes it.

Acceptance: a fresh build from the renamed checkout drives the Touch Bar
exactly as before with the moved and updated config; no `touchbar.*` widget IDs
or old paths remain in code or docs. Dependencies: none. Do this before T00 so
golden images and new names (units, socket, protocol) start out under the new
name.

## Stage 1: Backend service with the Touch Bar as first client

### T00 — Regression safety net

- [x] Add a deterministic preview: fixed clock and fixed provider data (graph
  history, agent usage, media, mic state, command output).
- [x] Commit golden PNGs for the default and Fn layers of `touchbar.default.json`
  and of a config that uses every widget kind.
- [x] Record current performance (RSS, CPU, full-frame time) with the same
  method as the baseline in the Rust-port notes. That method wasn't recorded,
  so [performance.md](performance.md) defines a repeatable one and holds the
  results.

Acceptance: a test compares preview output to the golden images and fails on
any pixel difference. Dependencies: none.

### T01 — Inventory widgets and label output support

- [x] Go through every widget kind, option, and behavior in `renderer.rs` and
  record which provider it needs, which actions it runs, and what is
  hardware-specific.
- [x] Confirm the draft Touch Bar / desktop labels in the ADR and add them to
  the README widget table.
- [x] List state that each widget needs to draw it, and its presentation options.

Result: [widget-inventory.md](widget-inventory.md), with open questions for
review at the end.

Acceptance: every widget kind and option is labelled; open questions are listed
for review. Dependencies: none.

### T02 — Specify the backend protocol

- [x] Document framing, handshake (version, output type, capabilities),
  snapshot and revisioned updates, widget/session IDs, press/release/activate,
  acknowledgements and errors, demand, disconnect cleanup, and resync.
- [x] Define per-client queue limits and coalescing.
- [x] Define socket location, permissions, and socket activation.

Result: [backend-protocol.md](backend-protocol.md). It resolves the T01 open
questions on shared providers, history length, repeat timings and the battery
icon, and uses newline-delimited rather than length-prefixed JSON for the QML
client; the user approved these decisions on 2026-10-06.

Acceptance: worked examples cover connect, update, press/hold/release,
disconnect mid-press, backend restart, and an unsupported widget. Clients
cannot request commands that are not configured. Dependencies: T01.

### T03 — Extract the backend service

- [x] Move config loading and reload, providers (graphs, agent usage, commands,
  media, mic), visibility rules, and action execution out of `renderer.rs` into
  backend modules with no Cairo dependency.
- [x] Implement the socket server inside the existing poll loop, with demand
  tracking so providers only run for visible, connected clients.
- [x] Move hold-to-repeat into the backend; cancel on release, widget removal,
  config reload, and disconnect.
- [x] Add the `backend` mode and systemd user units
  (`omarchy-glance.service` and `omarchy-glance.socket`).

Done in `src/backend/` (core, providers, server, tests) and `systemd/`.

Acceptance: a test client receives a snapshot and updates; the last client
disconnecting stops capture and polling; protocol tests cover malformed
requests, slow clients, and reconnection. Dependencies: T02.

### T04 — Turn the renderer into the Touch Bar client

- [x] Draw from backend snapshots instead of in-process state; keep layout,
  Cairo drawing, press feedback, Fn layer and contact latch, idle dimming, and
  `TapKeys` in the client.
- [x] Translate touches into press/release requests for `exec` actions; Esc and
  F-key buttons tap keys locally.
- [x] Reconnect to the backend and to the t1bridge socket instead of exiting;
  show a clear disconnected state while the backend is unavailable.
- [x] Add the `touchbar` mode and the installed t1bridge renderer wrapper that
  runs it; `preview` runs the backend and client in one process.

Done in `src/renderer.rs`: it draws from snapshots and updates (graph labels,
widths and gradients still come from a spec-only `Source`; the battery glyph
from the `battery` state), sends `press`/`release` with the contact id as the
pointer and `view` when the shown layer changes, and repeats held F-keys
itself, so `settings` gained `repeatDelay` and `repeatInterval`. Contacts that
are down when the config reloads or the backend reconnects are ignored until
they lift instead of pressing again. The golden tests now run the protocol
against an in-process backend that serves the fixture data
(`Backend::inject`); all four images match the T00 ones unchanged.
`omarchy-glance on` installs `~/.config/t1bridge/renderer` as a script running
`omarchy-glance touchbar`, and `--preview` became `preview`. On 2026-10-07 the
bar ran as a client and recovered from a backend restart and from the backend
being stopped for 3 s without exiting; the hands-on check of every widget,
touch, hold-to-repeat, Fn, reload and invalid config is the user's.

Acceptance: golden previews match exactly; on hardware, every widget, touch,
hold-to-repeat, Fn, config reload, and invalid-config fallback behaves as
before; restarting the backend while the bar runs recovers without falling
back to the t1bridge built-in renderer. Dependencies: T03.

### T05 — Cut over, measure, and retire Python

- [x] Update the control subcommands and the README for the backend service;
  remove the hardcoded checkout path where practical.
- [x] Compare performance against T00 and record the result.
- [x] Remove `python/` and the `on python` option, and drop Python
  compatibility notes from docs.

Done 2026-10-07. `on` now installs the backend's units (compiled into the
binary, with `ExecStart` pointing at it) and the renderer wrapper, enables the
socket and restarts both services, so setup is `cargo build --release` and
`omarchy-glance on`, with no `systemctl link` against the checkout and no
compiled-in checkout path. `restart`, `status` and `log` cover the backend
too, and `tools/perf.sh` measures both processes. Results are in
[performance.md](performance.md): same frame times, +5.7 MB for the backend,
CPU within 0.1 percentage point. `python/` and `on python` are gone (recover
them from the history before this commit if needed); the only remaining
mentions of Python are historical, in the ADR and this file.

Acceptance: a fresh setup following the README gives a working Touch Bar;
performance is recorded; no Python references remain.
Dependencies: T04, and a final go-ahead from the user to delete `python/`.

## Stage 2: Desktop host spike

### T06 — Compare desktop hosts

- [x] Build a throwaway standalone Quickshell `PanelWindow` with hardcoded
  content, and the same as an Omarchy third-party `panel` plugin using
  `PluginBarStateApi`.
- [ ] For each, check: stacking below the Omarchy bar, including after an
  `omarchy-shell` restart; clicks that keep focus on the active window;
  fractional scaling; fullscreen windows; the bar moved to another edge or
  hidden; monitor hotplug; space released on exit.
- [x] Check what theme and transparency data each host can follow.

Done 2026-10-07 except clicks and focus, fractional scaling and monitor
hotplug, which need someone at the machine; the comparison and the
recommendation (the panel plugin as the host, standalone kept for
development; approved by the user 2026-10-07) are in [desktop-hosts.md](desktop-hosts.md), the spikes in
`tools/spikes/desktop-host/`.

Acceptance: a short written comparison with a recommended primary host and
whether the other is kept as an option. Dependencies: none (can overlap
Stage 1, but desktop work waits for T05).

## Stage 3: Desktop renderer

### T07 — QML client and host contract

- [x] Implement connection, snapshots and updates, action results, and
  reconnection in QML.
- [x] Define the host environment (palette, font, scale, size, transparency,
  visibility); shared controls never create their own window.
- [x] Add a small development host for working on controls without Omarchy.

Done 2026-10-07 in `desktop/` ([desktop-client.md](desktop-client.md)), with
placeholder widget controls until T09. The backend doesn't serve the `desktop`
output yet (T08), so the development host shows the Touch Bar config.
`tools/desktop-check.sh` checks the client headlessly against a private
backend, including a backend restart.

Acceptance: controls mount in a plain container; a backend restart restores
state and shows a coherent disconnected state. Dependencies: T05, T06.

### T08 — Desktop configuration

- [x] Add `~/.config/omarchy-glance/desktop.json` and its built-in default:
  one widget list (left/center/right), presentation settings, monitor and
  height; colors and font from the host theme unless overridden.
- [x] Validate against output labels; log unsupported widgets.
- [x] Document it with examples, including a desktop-only config for machines
  without a Touch Bar.

Done 2026-10-07: `desktop.default.json`, `Config::parse_desktop`, the
`desktop` output in the backend, `omarchy-glance config desktop`, and the
README's [Desktop row](../README.md#desktop-row) section. The default row is
1.5 times the bar's height (the user's call; first twice, then reduced). The plugin already applies
`monitor` and `height`. Not run on a machine without t1bridge; the backend
has no t1bridge code, and `tools/desktop-check.sh` runs a backend with only
a desktop session.

Acceptance: an invalid `desktop.json` never affects the Touch Bar; a desktop-only config
works with no t1bridge installed. Dependencies: T01, T07.

### T09 — Desktop widgets

- [x] Implement every widget labelled for desktop: `exec` buttons (with
  hold-to-repeat), commands, graphs, agents, media, mic, spacer.
- [x] Responsive left/center/right layout, text truncation, and defined
  overflow behavior at narrow widths and large scales.
- [x] Map mouse press/release/hold to semantic input.

Done 2026-10-07 ([desktop-client.md](desktop-client.md#widgets)). Checked
headlessly with `tools/desktop-shot.sh` (every desktop widget, a narrow row)
and against the live backend; clicking through the plugin, hold-to-repeat
with a mouse, and hover are for the user to try.

Acceptance: desktop widgets share providers with the Touch Bar when both run;
hidden agent widgets give up their space; clicks keep focus on the active
window. Dependencies: T08.

### T10 — Placement and theme

- [ ] Implement the host chosen in T06 with monitor selection and height.
- [ ] Follow the live theme palette and font; follow supported bar state
  (hidden, size, position) where the host provides it.
- [ ] Document behavior when the bar moves, hides, or is absent, and remaining
  gaps.

Started early on 2026-10-07 (the user asked to see the row in place):
`desktop/manifest.json` and `Panel.qml` put the row under the bar on every
screen with the live theme ([desktop-client.md](desktop-client.md#panel-plugin)).

Acceptance: normal tiled and maximized windows sit below both rows on the
selected monitor; other monitors are unaffected; theme changes apply without a
backend restart; no reservation remains after exit. Dependencies: T09.

### T11 — Package for machines without a Touch Bar

- [ ] Installation that does not require t1bridge or a checkout at a fixed
  path; autostart of backend and desktop host.
- [ ] Record runtime dependencies (Omarchy, which brings Hyprland, Quickshell
  and the Nerd Font).
- [ ] Release smoke check: widgets, reload, reconnection, reservation, focus,
  theme, scaling, monitor changes, and running alongside the Touch Bar.

Acceptance: documented steps produce a working desktop row on a machine with no
Touch Bar, and the Touch Bar workflow still works on this one.
Dependencies: T10.

## Stage 4: Native Omarchy row (proposed)

### T12 — Design a supported upstream extension contract

- [ ] Draft a proposal for mounting an extra row while preserving the existing
  row's contents, gestures, and popups: sizing, combined reservation, host
  environment, visibility, input boundaries, popup anchoring, lifecycle.
- [ ] Compare with what the T06 panel plugin already provides.

Acceptance: a reviewable proposal and local prototype; submitting it upstream
is a separate, explicitly authorized step. No packaged Omarchy files are
modified. Dependencies: T06.

### T13 — Embedded host when the contract exists

- [ ] Mount the shared QML controls through the accepted API; let Omarchy own
  background, transparency, hiding, theme, and reservation.

Acceptance: the existing top row is unchanged; transparency and theme changes
apply to both rows; popups anchor correctly. Dependencies: T09 and an accepted
T12 contract.

## Follow-on feature work

After Stage 3, refine `TODO.md` proposals into tasks. Each must state its
output labels and which clients create provider demand.

| Feature family | Main work |
| --- | --- |
| Volume/brightness sliders and media seeking | Provider capabilities, value actions, Cairo/QML controls |
| Workspaces, active window, app-aware layers | Hyprland provider, actions, core rules |
| Notifications | Integration with the existing notification service |
| Claude sessions, Git/CI, timers | Providers and core state, then both presentations |
| Gestures and additional layers | Semantic input; mostly Touch Bar |
| Animations and theme polish | Presentation and host environment |
| Touch Bar brightness / ambient light | Touch Bar client only |
