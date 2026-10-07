# omarchy-glance to-do

Ideas for the renderer, from the 2026-10-04 brainstorm. **Proposed** items are
waiting for a yes or no.

## Done

- [x] Hide configured agent widgets without shared Omarchy usage data; show
      them again when data returns (2026-10-06).

- [x] Separate Claude and Codex usage widgets with the provider logos from the
      Omarchy agents panel; local testing layout replaces CPU/memory (2026-10-06).

- [x] Monitoring graphs, supported but not on the bar by default:
      `glance.gpu`, `.network`, `.disk`, `.battery`, `.fan` (2026-10-04)
- [x] Media controls on the Fn layer: `glance.media` (2026-10-04)
- [x] Mic mute toggle on the default layer with a live waveform while any app
      records: `glance.mic` (2026-10-04)
- [x] Keyboard backlight: button snippets in the README (2026-10-04)

## Approved

### Backend and desktop outputs ([ADR 0001](docs/adr/0001-multiple-output-architecture.md), [tasks](docs/multi-output-tasks.md))

- [x] Stage 0: rename the project to `omarchy-glance`, with the Touch Bar config
      at `~/.config/omarchy-glance/touchbar.json` (R01).
- [ ] Stage 1: split out a backend service; the Touch Bar renderer becomes its
      first client and must match current behavior and appearance (T00–T04).
- [ ] Retire the Python renderer once the Touch Bar client is proven (T05;
      confirm before deleting `python/`).
- [ ] Stage 2: spike standalone Quickshell vs Omarchy panel-plugin hosts (T06).
- [ ] Stage 3: desktop renderer for machines without a Touch Bar (T07–T11).

## Proposed

### Desktop outputs

- [ ] Explore a supported native Omarchy second-row extension (T12–T13);
      reuse the desktop controls rather than fork the system bar.

### Controls
- [ ] Brightness and volume **sliders**: drag along the bar to set the level
      directly instead of tapping −/+.
- [ ] Two-finger or long-press variants of controls.
- [ ] Touch Bar brightness control. The IPC has `SetDisplayBrightness` (and
      `SetKeyboardBacklight`) messages, which could also replace dimming.
- [ ] Media: a progress bar you can scrub (needs MPRIS position/length, which
      `omarchy-shell media status` doesn't report).

### Omarchy / Hyprland
- [ ] Workspace switcher: a key per workspace, active one highlighted, tap to
      switch (`hyprctl dispatch workspace N`), live from Hyprland's event socket.
- [ ] Active window: app icon and title, tap to close or float it.
- [ ] App-aware layers: swap the bar's contents for the focused app (browser
      tabs/nav, editor run/debug, Spotify playback), like macOS.
- [ ] Buttons for the Omarchy menu, theme switcher, screenshot and screen
      recording, lock and suspend.
- [ ] Notifications: show the latest one briefly, tap to dismiss or open.

### Claude / dev workflow
- [ ] Claude Code session status (idle, working, waiting for permission) via a
      hook that writes a state file; glow when Claude needs input.
- [ ] Git branch and dirty state of the focused terminal's directory.
- [ ] Build/CI status from `gh run list`, red on failure.
- [ ] Pomodoro timer or stopwatch with a draining progress bar.

### Bar behaviour and polish
- [ ] More layers: a third layer on long-press or swipe.
- [ ] Swipe gestures (e.g. switch workspaces, adjust volume on its widget).
- [ ] Animations: smooth meter transitions, a ripple on key press.
- [ ] Idle mode: a clock or screensaver visual instead of dimming.
- [ ] Auto-brightness for the bar itself, if the T1's ambient light sensor is
      ever exposed.
- [ ] Per-theme styling: take key and text colours from the Omarchy theme, as
      the graphs already do with btop's.
