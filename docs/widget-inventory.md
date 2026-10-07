# Widget inventory

Task T01 of [multi-output-tasks.md](multi-output-tasks.md): every widget kind,
option and behaviour in the current renderer (`src/renderer.rs`,
`src/sources.rs`, `src/mic.rs`, `src/config.rs`), what it needs from the system,
what it runs, and which outputs support it. This is the input for the backend
protocol (T02) and the split (T03, T04). Inventoried on 2026-10-06 at commit
`8ca2df8`.

Output labels: **TB** = Touch Bar, **D** = desktop row (planned). "Backend"
and "client" refer to the split in
[ADR 0001](adr/0001-multiple-output-architecture.md).

## Widget kinds

### Summary

| Widget | Outputs | Provider (backend) | Actions (backend) | Hardware-specific (TB client) |
| --- | --- | --- | --- | --- |
| `glance.esc` | TB | none | none | Esc through t1bridge `TapKeys` |
| `type: button` with `key` | TB | none | none | Esc/F1–F12 through `TapKeys`, repeats while held |
| `type: button` with `exec` | TB, D | none | `exec`, hold-to-repeat | none |
| `type: command` | TB, D | `exec` script output, every `interval` s | `onTap` | none |
| `glance.agents` | TB, D | Omarchy usage records, refresh job per provider | `onTap` (default: toggle the Omarchy agents panel) | none |
| `glance.cpu`, `.memory`, `.gpu`, `.network`, `.disk`, `.battery`, `.fan` | TB, D | `/proc` and `/sys` samples every `interval` s | `onTap` (default: open btop) | none |
| `glance.mic` | TB, D | `pactl` mute and recording state; `parec` levels | `onTap` (default: `omarchy-audio-input-mute`) | none |
| `glance.media` | TB, D | `omarchy-shell media status`; `dbus-monitor` for MPRIS changes | previous / play-pause / next; `onTap` on the title | none |
| `glance.spacer` | TB, D | none | none | none |

Nothing in a widget is tied to the Touch Bar except Esc and F-key taps. The
hardware-specific parts are bar-wide (see [Bar behaviour](#bar-behaviour)).

### `glance.esc`

- Draws a 5×7 pixel-font "esc" scaled to the bar height; `width` (140).
- Press: `TapKeys` Esc on touch-down. No repeat, no backend action.
- State: none. **TB only.**

### `type: button`

- Presentation: `icon` (glyph) or `label` (text), `iconSize` (30), `fontSize`
  (18), `width` (140).
- Action, one of:
  - `key`: `"esc"`, `"f1"`…`"f12"`, tapped through `TapKeys`. **TB only.**
    Anything else logs "unsupported key".
  - `exec`: shell command run without waiting. **TB, D.**
- `repeat: true`: press again after `repeatDelay`, then every
  `repeatInterval` while the finger stays down; works for `key` and `exec`.
- State: none.

### `type: command`

- Provider: runs `exec` with `bash -c`, reads up to 64 KiB of stdout, kills it
  after 10 s. Reruns every `interval` s (0 or omitted: once per config load).
  Output: the first line, or Waybar JSON `{"text", "class"}` where class
  `urgent`/`critical` (string or list) sets urgent.
- State: `text`, `urgent`.
- Presentation: `fontSize` (18), `width` (omitted: sized to the text, at least
  70, so the layout changes when the text does).
- Action: `onTap` (default none).

### `glance.agents`

- Provider: reads `${XDG_STATE_HOME:-~/.local/state}/omarchy/agents/usage/<agent>.json`
  each second (when its mtime changes) and starts
  `timeout 120s omarchy-agent-usage-update <agent>` at startup and every 60 s,
  one job per provider, in the background. Agent names are restricted to
  `[A-Za-z0-9_-]`, not starting with `-`.
- Visibility: shown only when the record has data, by the Omarchy panel's
  rule (limits, a valid balance, or positive prompt/session/day counts).
  Hidden widgets take no space, and contacts on them are dropped.
- State: has-data flag; limits `[{label, fraction, resetsAt}]` (label cut at
  `" ("`); with no limits, placeholders "Session" and "Weekly" showing "—".
- Presentation: `agent` (`"claude"`) also picks the icon (Claude and Codex
  logos built in, otherwise the agents glyph); `layout` (`"row"` or
  `"stacked"`); `meterWidth` (200 row, 120 stacked); `shortLabels`;
  `resets` (`"time"`, `"countdown"`, `"none"`); `timeFormat` (`%H:%M`),
  `dayTimeFormat` (`%a %H:%M`). Reset text depends on the wall clock, so the
  widget redraws every minute. Meters turn `colors.urgent` at 90%. The row
  layout's width depends on the number of limits.
- Action: `onTap` (default `omarchy-shell -q omarchy.agents toggle`).

### Graphs: `glance.cpu`, `.memory`, `.gpu`, `.network`, `.disk`, `.battery`, `.fan`

- Provider, sampled every `interval` s (1, at least 0.25):

  | Widget | Reads | Series | Value lines |
  | --- | --- | --- | --- |
  | cpu | `/proc/stat` (two readings for a rate); `temperature`: hwmon `sensor` (`coretemp`) | usage | `%`, optional `°` |
  | memory | `/proc/meminfo` | used fraction | `%` |
  | gpu | `card`'s `gpu_busy_percent` (first amdgpu); `temperature`: the GPU's hwmon or `sensor` | load | `%`, optional `°` |
  | network | default route (or `interface`) byte counters | down, up (bytes/s) | `↓…`, `↑…` |
  | disk | `/proc/diskstats` for `device` (first disk); `show: "usage"`: `statvfs(mount)` | reads, writes (bytes/s) | `R …`, `W …`, or `%` |
  | battery | `power_supply/<battery>` charge, status, current, voltage or power | level (fraction), or `graph: "charge"`/`"power"` history | `%`, optional `detail` (`"time"`, `"power"`) |
  | fan | hwmon `fan<fan>_input` | rpm | rpm |

  Provider options also set urgent flags: `alarm` (0.9) for `%`,
  `temperatureAlarm` (90 °C), battery `low` (15%, while discharging).
- Scale: network and disk scale to the recent peak (at least `minScale`,
  10240); battery power to `maxPower` or the peak (at least 10 W); fan to
  `fan<n>_max` from hwmon, else the peak (at least 1000). The fan maximum is
  read from sysfs, so the scale is provider data.
- State: history per series (raw values), value lines with urgent flags,
  per-core fractions (cpu `cores`), the battery icon (picked from charge and
  charging state), and an error flag (logged once; the graph keeps its old
  history).
- Presentation: `label`, `graphWidth` (100), `dotSpacing` (4), `dotSize`,
  `style` (`"dots"`/`"bars"`), `grid`, `gradient`, `showValue`, `fontSize`
  (18), `cores` (also a provider option), battery level meter. History length
  is `graphWidth / dotSpacing`. Without `gradient`, colours come from btop's
  current theme (`~/.config/btop/themes/current.theme`) or built-in fallbacks.
- Action: `onTap` (default `omarchy-launch-or-focus-tui btop`).

### `glance.mic`

- Provider: `pactl subscribe` for events, then `pactl` JSON for the default
  source's mute state and for other apps' recording streams (its own capture is
  excluded by PID). While another app records, the mic is live, `waveform` is
  on, and a mic widget is on the visible layer, it runs `parec` (8 kHz mono)
  and turns it into `fps` (20) levels a second; otherwise no capture runs.
  One `Mic` serves all mic widgets; `fps` comes from the first.
- State: muted (unknown until read), in use, recent levels.
- Presentation: `iconSize` (30), `width` (140, while not recording),
  `waveformWidth` (120), `activeColor`, `mutedColor`, `dotSpacing`,
  `dotSize`. The width grows while recording.
- Action: `onTap` (default `omarchy-audio-input-mute`, which shows the OSD).

### `glance.media`

- Provider: runs `omarchy-shell media status` (JSON) every `interval` s (30),
  only while the widget is on the visible layer, and 0.15 s after
  `dbus-monitor` reports an MPRIS `PropertiesChanged` or a player appearing or
  leaving. `dbus-monitor` runs while any media widget exists and restarts
  within 5 s if it dies.
- State: `hasMedia`, `playing`, `title`, `artist`, `identity`,
  `canGoPrevious`, `canTogglePlaying`, `canGoNext`.
- Presentation: `buttonWidth` (100), `titleWidth` (360), `iconSize` (28);
  press feedback per zone.
- Actions: zones previous / play-pause / next run
  `omarchy-shell -q media <previous|playPause|next>`, then refresh state
  0.3 s later; a title tap runs `onTap` (default none).

### `glance.spacer`

- `size` (40). Empty, takes no touches. On the desktop the size means desktop
  pixels, so each output's file sets its own.

## Bar-wide settings

| Setting | Default | Outputs | Belongs to |
| --- | --- | --- | --- |
| `version` | must be 1 | TB, D | both files |
| `layers.default` / `layers.fn` | `fn` optional | `default`: TB; `fn`: TB only | TB client |
| `left` / `center` / `right` | | TB, D | client layout |
| `colors.background`, `key`, `keyPressed`, `text`, `urgent` | `#000000`, `#303030`, `#808080`, `#ffffff`, `#e05a5a` | TB; D takes the host theme unless overridden | client |
| `font` | JetBrainsMono Nerd Font | TB; D from the host theme | client |
| `idleDimSeconds` | 0 (never) | TB | TB client |
| `repeatDelay`, `repeatInterval` | 0.4, 0.12 (at least 0.03) | TB, D | backend, per output's config |
| `debug.background`, `colors.debugBackground`, `colors.debugBackgroundFn` | off | TB | TB client |
| `debug.border`, `debug.testPattern` | off | TB | TB client |

## Bar behaviour

| Behaviour | Outputs | Where after the split |
| --- | --- | --- |
| Config read from `~/.config/omarchy-glance/touchbar.json`, polled each second; parse error keeps the previous config (or the built-in default at startup) | TB, D (own file) | backend |
| Widget identity is `<layer>.<section>.<index>`, not `id` (ids repeat, e.g. `glance.esc` on both layers, two `glance.agents`) | TB, D | backend |
| Layout: sections packed left, centred, right within 2060 px | TB | TB client |
| Fn layer while the hardware Fn flag is set, kept up while any contact is still on an Fn widget | TB | TB client |
| A contact belongs to the widget it first touched until it lifts; press on touch-down | TB | TB client (D: mouse press) |
| Press feedback (`keyPressed` face, per media zone) | TB, D | client |
| Hold-to-repeat, cancelled when the contact lifts, the widget hides, or the config reloads | TB, D | backend |
| Hidden agents widgets take no space and drop their contacts | TB, D | backend decides, clients lay out |
| Idle dimming to 25% after `idleDimSeconds` | TB | TB client |
| Damage tracking: only changed widgets redrawn, up to 64 rects per frame; full frames on layout changes | TB | TB client |
| Reset-time redraw each wall-clock minute | TB, D | client (backend sends timestamps) |
| Portrait buffers (logical x along the buffer's y) | TB | TB client |
| 2170 × 60 panel, 2060 px visible | TB | TB client |
| btop theme polled each second for graph gradients | TB, D | client (presentation default) |
| Fire-and-forget child processes reaped each loop | TB, D | backend |

## Hardware-specific (stays in the Touch Bar client)

- t1bridge IPC: Hello/HelloAck, up to three memfd buffers, SubmitFrame with
  damage rects, FrameReleased, InputFrame (contacts, Fn flag), TapKeys (Esc,
  F1–F12).
- The visible width (2060 px) and the 60 px height that metrics assume (the
  Esc glyph scale, row layouts at y = 25 and 33).
- Fn layer, contact ownership, idle dimming, debug background/border/test
  pattern, touch logging.
- Not used today: `SetDisplayBrightness` and `SetKeyboardBacklight` (only in
  TODO.md proposals).

## Corrections to the ADR's draft labels

- Confirmed: `glance.esc` and `key` buttons TB only; `exec` buttons, commands,
  agents, graphs, mic, media, spacer TB and D; Fn layer and idle dimming TB
  only.
- Changed: "Display brightness, keyboard backlight via t1bridge IPC" is not
  something the renderer does; it stays a TB-only proposal.
- Added: the `debug.*` settings are TB only; `repeatDelay`/`repeatInterval`
  apply on both outputs; colours and font default to the host theme on D.

## Open questions

Questions 1–4 are settled in [backend-protocol.md](backend-protocol.md) as
proposed (approved 2026-10-06). Question 5 is settled: the project targets
Omarchy only. Question 8 is noted in `TODO.md` under Bugs.

1. **Shared providers.** Two widgets with the same `id` currently get separate
   sources (two cpu graphs sample `/proc/stat` twice). Proposal: the backend
   shares a provider between widgets, and between outputs, when the id and the
   provider options (`interval`, `sensor`, `card`, `interface`, `device`,
   `show`, `mount`, `battery`, `fan`, `temperature`, `cores`, `graph`,
   `detail`, `low`, `alarm`, `temperatureAlarm`, `minScale`, `maxPower`, and
   for commands `exec`) match; otherwise it runs a separate one.
2. **History length.** History size comes from `graphWidth / dotSpacing`, a
   presentation option. Proposal: the backend keeps the longest history any
   connected widget sharing the provider needs; each client draws the newest
   samples that fit.
3. **Repeat timings.** With hold-to-repeat in the backend, `repeatDelay` and
   `repeatInterval` are read from the file of the output that pressed.
   Alternatively they move to a shared backend setting. Proposal: per output,
   as today.
4. **Battery icon.** Today it's a Nerd Font glyph picked from charge and
   status. Proposal: the backend sends charge and status, and each client
   picks its glyph.
5. **Omarchy defaults on other desktops.** The default `onTap` commands (btop
   via `omarchy-launch-or-focus-tui`, the agents panel, `omarchy-audio-input-mute`,
   `omarchy-shell media`) and the media provider assume Omarchy. Settled: the
   project targets Omarchy only, so these can be assumed everywhere.
6. **Mic demand on the desktop.** The capture runs only while a mic widget is
   visible. A desktop row is always visible unless its host hides it, so the
   desktop client must report hidden/visible for demand to be right.
7. **Graph colours on the desktop.** On the Touch Bar, graph gradients default
   to btop's current theme (`cpu_*`, `used_*`, `download_*`/`upload_*`,
   `temp_*` start/mid/end), while key faces and text use fixed colours from
   `touchbar.json`. Omarchy generates `btop.theme` and the shell palette
   (`shell.toml`) from the same theme `colors.toml`, so both follow theme
   changes. For the desktop row, either keep btop's three-stop gradients (the
   graphs look like btop and like the Touch Bar) or derive graph colours from
   the shell palette (they match the rest of the bar but lose the per-series
   gradients). Proposal: btop's gradients for graphs, the shell palette for
   everything else; `gradient` overrides either way. Decide by T09.
8. **Narrow `meterWidth` in the row agents layout** makes labels, reset times
   and percentages overlap (seen while building the golden config). It's a
   presentation bug, separate from the split.
