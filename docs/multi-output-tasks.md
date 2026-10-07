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

- [ ] Add a deterministic preview: fixed clock and fixed provider data (graph
  history, agent usage, media, mic state, command output).
- [ ] Commit golden PNGs for the default and Fn layers of `touchbar.default.json`
  and of a config that uses every widget kind.
- [ ] Record current performance (RSS, CPU, full-frame time) with the same
  method as the baseline in the Rust-port notes.

Acceptance: a test compares preview output to the golden images and fails on
any pixel difference. Dependencies: none.

### T01 — Inventory widgets and label output support

- [ ] Go through every widget kind, option, and behavior in `renderer.rs` and
  record which provider it needs, which actions it runs, and what is
  hardware-specific.
- [ ] Confirm the draft Touch Bar / desktop labels in the ADR and add them to
  the README widget table.
- [ ] List state that each widget needs to draw it, and its presentation options.

Acceptance: every widget kind and option is labelled; open questions are listed
for review. Dependencies: none.

### T02 — Specify the backend protocol

- [ ] Document framing, handshake (version, output type, capabilities),
  snapshot and revisioned updates, widget/session IDs, press/release/activate,
  acknowledgements and errors, demand, disconnect cleanup, and resync.
- [ ] Define per-client queue limits and coalescing.
- [ ] Define socket location, permissions, and socket activation.

Acceptance: worked examples cover connect, update, press/hold/release,
disconnect mid-press, backend restart, and an unsupported widget. Clients
cannot request commands that are not configured. Dependencies: T01.

### T03 — Extract the backend service

- [ ] Move config loading and reload, providers (graphs, agent usage, commands,
  media, mic), visibility rules, and action execution out of `renderer.rs` into
  backend modules with no Cairo dependency.
- [ ] Implement the socket server inside the existing poll loop, with demand
  tracking so providers only run for visible, connected clients.
- [ ] Move hold-to-repeat into the backend; cancel on release, widget removal,
  config reload, and disconnect.
- [ ] Add the `backend` mode and systemd user units
  (`omarchy-glance.service` and `omarchy-glance.socket`).

Acceptance: a test client receives a snapshot and updates; the last client
disconnecting stops capture and polling; protocol tests cover malformed
requests, slow clients, and reconnection. Dependencies: T02.

### T04 — Turn the renderer into the Touch Bar client

- [ ] Draw from backend snapshots instead of in-process state; keep layout,
  Cairo drawing, press feedback, Fn layer and contact latch, idle dimming, and
  `TapKeys` in the client.
- [ ] Translate touches into press/release requests for `exec` actions; Esc and
  F-key buttons tap keys locally.
- [ ] Reconnect to the backend and to the t1bridge socket instead of exiting;
  show a clear disconnected state while the backend is unavailable.
- [ ] Add the `touchbar` mode and the installed t1bridge renderer wrapper that
  runs it; `preview` runs the backend and client in one process.

Acceptance: golden previews match exactly; on hardware, every widget, touch,
hold-to-repeat, Fn, config reload, and invalid-config fallback behaves as
before; restarting the backend while the bar runs recovers without falling
back to the t1bridge built-in renderer. Dependencies: T03.

### T05 — Cut over, measure, and retire Python

- [ ] Update the control subcommands and the README for the backend service;
  remove the hardcoded checkout path where practical.
- [ ] Compare performance against T00 and record the result.
- [ ] Remove `python/` and the `on python` option, and drop Python
  compatibility notes from docs.

Acceptance: a fresh setup following the README gives a working Touch Bar;
performance is recorded; no Python references remain.
Dependencies: T04, and a final go-ahead from the user to delete `python/`.

## Stage 2: Desktop host spike

### T06 — Compare desktop hosts

- [ ] Build a throwaway standalone Quickshell `PanelWindow` with hardcoded
  content, and the same as an Omarchy third-party `panel` plugin using
  `PluginBarStateApi`.
- [ ] For each, check: stacking below the Omarchy bar, including after an
  `omarchy-shell` restart; clicks that keep focus on the active window;
  fractional scaling; fullscreen windows; the bar moved to another edge or
  hidden; monitor hotplug; space released on exit.
- [ ] Check what theme and transparency data each host can follow.

Acceptance: a short written comparison with a recommended primary host and
whether the other is kept as an option. Dependencies: none (can overlap
Stage 1, but desktop work waits for T05).

## Stage 3: Desktop renderer

### T07 — QML client and host contract

- [ ] Implement connection, snapshots and updates, action results, and
  reconnection in QML.
- [ ] Define the host environment (palette, font, scale, size, transparency,
  visibility); shared controls never create their own window.
- [ ] Add a small development host for working on controls without Omarchy.

Acceptance: controls mount in a plain container; a backend restart restores
state and shows a coherent disconnected state. Dependencies: T05, T06.

### T08 — Desktop configuration

- [ ] Add `~/.config/omarchy-glance/desktop.json` and its built-in default:
  one widget list (left/center/right), presentation settings, monitor and
  height; colors and font from the host theme unless overridden.
- [ ] Validate against output labels; log unsupported widgets.
- [ ] Document it with examples, including a desktop-only config for machines
  without a Touch Bar.

Acceptance: an invalid `desktop.json` never affects the Touch Bar; a desktop-only config
works with no t1bridge installed. Dependencies: T01, T07.

### T09 — Desktop widgets

- [ ] Implement every widget labelled for desktop: `exec` buttons (with
  hold-to-repeat), commands, graphs, agents, media, mic, spacer.
- [ ] Responsive left/center/right layout, text truncation, and defined
  overflow behavior at narrow widths and large scales.
- [ ] Map mouse press/release/hold to semantic input.

Acceptance: desktop widgets share providers with the Touch Bar when both run;
hidden agent widgets give up their space; clicks keep focus on the active
window. Dependencies: T08.

### T10 — Placement and theme

- [ ] Implement the host chosen in T06 with monitor selection and height.
- [ ] Follow the live theme palette and font; follow supported bar state
  (hidden, size, position) where the host provides it.
- [ ] Document behavior when the bar moves, hides, or is absent, and remaining
  gaps.

Acceptance: normal tiled and maximized windows sit below both rows on the
selected monitor; other monitors are unaffected; theme changes apply without a
backend restart; no reservation remains after exit. Dependencies: T09.

### T11 — Package for machines without a Touch Bar

- [ ] Installation that does not require t1bridge or a checkout at a fixed
  path; autostart of backend and desktop host.
- [ ] Record runtime dependencies (Quickshell, Hyprland or other compositor
  requirements, Nerd Font).
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
