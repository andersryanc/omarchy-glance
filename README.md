# omarchy-glance

A custom renderer for the MacBook Pro T1 Touch Bar, driven through
[t1bridge](https://github.com/standardagents/t1bridge)'s Touch Bar hardware IPC
(`docs/t1bridge-interfaces.md`). By default it draws an Esc key, media keys,
btop-style CPU and memory graphs and a Claude usage widget, and while Fn is
held, brightness and volume keys and a mic-mute key with a live waveform. Everything on the bar
comes from a JSON config.

It runs as two parts: a backend service (`omarchy-glance backend`) that reads
the config, samples the system and runs actions, and the Touch Bar client
(`omarchy-glance touchbar`) that draws the backend's widget state through
t1bridge and sends it touches. A desktop row for machines without a Touch Bar
is planned as a second client ([ADR 0001](docs/adr/0001-multiple-output-architecture.md)).

## Building

```
cargo build --release
```

Rust comes from mise (`mise use -g rust@stable`).

Then set it up once:

```
ln -s "$PWD/target/release/omarchy-glance" ~/.local/bin/   # optional: put it on PATH
target/release/omarchy-glance on
```

`on` installs the backend's systemd user units, enables its socket, and makes
t1bridge run the Touch Bar client (see [Switching renderers](#switching-renderers)).

To check a change without the hardware, draw one frame to a PNG (`fn` shows
the Fn layer). This runs the backend and the Touch Bar client in one process,
talking the backend protocol, with live data:

```
target/release/omarchy-glance preview /tmp/bar.png [fn]
```

`cargo test --release` also draws the default config and
`tests/golden/every-widget.json` (both layers) with fixed data
(`tests/golden/fixture.json`, served by an in-process backend) and a fixed clock, and fails if any pixel differs
from the images in `tests/golden/`. A failing test writes the actual frame to
`target/golden/`. After an intended visual change, look at the new frames and
then accept them with `UPDATE_GOLDEN=1 cargo test --release golden`. Text uses
the installed fonts, so the images only match where JetBrainsMono Nerd Font is
installed. Performance measurements and their method are in
[docs/performance.md](docs/performance.md).

## Switching renderers

```
omarchy-glance on            # install the backend and use this Touch Bar client
omarchy-glance off           # back to the t1bridge built-in bar
omarchy-glance restart       # restart the backend and the bar after rebuilding
omarchy-glance status        # which renderer is selected, and both services
omarchy-glance log           # follow the client's and the backend's logs
omarchy-glance config        # create ~/.config/omarchy-glance/touchbar.json if missing, print its path
omarchy-glance config edit   # ... and open it in $EDITOR
omarchy-glance config desktop [edit]   # the same for the desktop row's desktop.json
```

`on` installs `omarchy-glance.socket` and `omarchy-glance.service` into
`~/.config/systemd/user/` with this binary's path, enables the socket, and
installs `~/.config/t1bridge/renderer`, a small script that runs
`omarchy-glance touchbar`. It then restarts the backend (if running) and the
`t1-touchbar` user service, so run it again if the binary moves. `off` only
removes the renderer script; the backend stays installed but runs nothing
without clients. If the renderer exits,
t1bridge falls back to its built-in bar for the rest of the session, so the
client never exits on its own: when the backend or t1bridge goes away it shows
a disconnected state and reconnects.

## Backend service

`omarchy-glance backend` runs the backend service, which owns config,
providers and actions and serves output clients on
`$XDG_RUNTIME_DIR/omarchy-glance/backend.sock`
([protocol](docs/backend-protocol.md)). It's socket-activated by the systemd
user units in `systemd/` (which `on` installs): the Touch Bar client's first
connection starts it, and `journalctl --user -u omarchy-glance -f` follows its
log.

## Repository layout

A planned split into a backend service with Touch Bar and desktop clients (for
machines without a Touch Bar) is documented in
[ADR 0001](docs/adr/0001-multiple-output-architecture.md), with
[implementation tasks](docs/multi-output-tasks.md). The backend and the Touch
Bar client are in place; the desktop client's connection and host contract
are in `desktop/` ([desktop-client.md](docs/desktop-client.md)), its widgets
and the Omarchy panel plugin that hosts it are not yet implemented.

| Path | |
|---|---|
| `src/backend/` | Backend service: `mod.rs` (config, widgets, sessions, demand, actions), `providers.rs`, `server.rs` (socket), `tests.rs`. `src/protocol.rs` holds names shared with clients. |
| `systemd/` | User units for the backend, compiled into the binary for `on`. |
| `src/` | `main.rs` (entry and modes), `cli.rs` (control subcommands), `proto.rs` (t1bridge IPC, memfd buffers), `config.rs`, `renderer.rs` (the Touch Bar client: backend link, layout, drawing, input, event loop, `preview`) with `renderer/golden.rs` (golden-preview tests) and `renderer/tests.rs`, `sources.rs` (graph data), `mic.rs`, `proc.rs` (child processes). |
| `desktop/` | The desktop client (QML): shared controls in `glance/`, the development host `shell.qml` (`qs -p desktop`), and `check.qml` for `tools/desktop-check.sh`. |
| `touchbar.default.json`, `desktop.default.json` | The default Touch Bar and desktop row configs, compiled into the binary. |
| `docs/` | t1bridge's IPC spec and README, the nohzafk T1 notes, the T1's USB descriptors. |
| `tests/golden/` | Golden-preview images, the config that uses every widget kind, and the fixed data they're drawn with. |
| `tools/` | Hardware experiments, e.g. `cutoff_test.py` (the 2060 px limit), `perf.sh` (memory and CPU of the running bar), `desktop-check.sh` (headless check of the desktop client), and `spikes/desktop-host/` (the T06 desktop host spikes). |
| `TODO.md` | Backlog. |

## Configuration

The backend reads `~/.config/omarchy-glance/touchbar.json`, or `touchbar.default.json`
in this directory when that file doesn't exist. As with the Omarchy bar's
`shell.json`, your file replaces the default entirely (no merging); start from
a copy with `omarchy-glance config`.

The file is reloaded within a second of being saved. If it doesn't parse, the
renderer logs why (`omarchy-glance log`) and keeps the previous config.

```json
{
  "version": 1,
  "layers": {
    "default": {
      "left":   [{ "id": "glance.esc" }],
      "center": [],
      "right":  [{ "id": "glance.agents", "agent": "claude" }]
    },
    "fn": {
      "left": [
        { "id": "glance.esc" },
        { "id": "volume-up", "type": "button", "icon": "", "exec": "omarchy-audio-output-volume raise", "repeat": true }
      ]
    }
  }
}
```

### Layers and sections

- `layers.default` is the normal bar. `layers.fn` replaces it while Fn is held
  (omit it to make Fn do nothing). The Fn layer stays up while a finger is
  still on one of its widgets, so you can let go of Fn mid-press.
- Each layer has `left`, `center` and `right` lists. Widgets are placed in
  order from the left edge, centred as a group, or packed against the right
  edge of the visible area (2060 px; the panel's last 110 px don't light up).

### Widgets

Every widget has an `id`. Built-in widgets use `glance.*` ids; your own
widgets set a `type` and can use any id.

The Outputs column says where a widget works: on the Touch Bar (TB), and on
the planned desktop row for machines without one (D; see
[ADR 0001](docs/adr/0001-multiple-output-architecture.md)). The full
inventory is in [docs/widget-inventory.md](docs/widget-inventory.md).

| Widget | Outputs | Options |
|---|---|---|
| `glance.esc` | TB | `width` (140). Sends Esc. |
| `glance.agents` | TB, D | `agent` (`"claude"`; any record in `~/.local/state/omarchy/agents/usage/`), `layout` (`"row"`: meters side by side with their names above; `"stacked"`: meters on top of each other with short labels on the left), `meterWidth` (200, or 120 stacked), `shortLabels` (stacked labels, default `{"Session": "5h", "Weekly": "7d"}`), `resets` (when each limit resets: `"time"` shows the clock time, with the weekday when it's more than a day away; `"countdown"` shows time left, e.g. `4h 12m`; `"none"`), `timeFormat` / `dayTimeFormat` (strftime, `"%H:%M"` / `"%a %H:%M"`), `onTap` (toggles the Omarchy agents panel; `""` for nothing). |
| `glance.mic` | TB, D | Mic mute toggle with a live waveform while an app records. See [Microphone](#microphone). |
| `glance.media` | TB, D | Previous, play/pause and next keys and the current track. See [Media](#media). |
| `glance.cpu`, `.memory`, `.gpu`, `.network`, `.disk`, `.battery`, `.fan` | TB, D | A label, a btop-style dot graph of recent history, and the current value. See [Graphs](#graphs). |
| `glance.spacer` | TB, D | `size` (40). Empty space. |
| `"type": "button"` | TB, D (`key`: TB only) | `icon` (Nerd Font glyph) or `label` (text), `iconSize` (30), `fontSize` (18), `width` (140); then either `exec` (shell command) or `key` (`"esc"`, `"f1"`…`"f12"`), and `repeat` (`true` repeats while held). |
| `"type": "command"` | TB, D | `exec` (shell command whose output is shown), `interval` (seconds; omit to run once), `onTap` (shell command), `fontSize` (18), `width` (sized to the text when omitted). |

Commands run with `bash -c` as you, with Omarchy's commands on `PATH`, so
anything that works in a terminal or an Omarchy keybinding works here.

A command widget shows the first line of its script's output. Like Omarchy's
command modules, it also accepts Waybar-style JSON:
`{"text": "…", "class": "critical"}`. A class of `urgent` or `critical` turns
the text red.

Each provider can have its own `glance.agents` widget. Claude and Codex use
copies of the Omarchy agents panel logos; other providers use the agents glyph.
Configured provider widgets appear only when the shared Omarchy usage record
contains limits, a valid balance, or positive all-time/today prompt or session
counts or active days, matching the panel's local data check. Missing, invalid,
or empty records hide the widget and reclaim its space within a second.
`ready` alone does not make a widget visible. Records are read directly from
`${XDG_STATE_HOME:-~/.local/state}/omarchy/agents/usage/`; the Touch Bar starts Omarchy’s `omarchy-agent-usage-update` collector for each
configured provider at startup and every 60 seconds, without opening the panel.
Refreshes run independently in the background, including for hidden widgets,
and overlapping runs for the same provider are skipped. The collector writes
the shared records using the existing CLI credentials and its normal cache
policy. After renewing a CLI login, the next refresh picks up the credentials;
the Touch Bar cannot renew an expired login itself. Omarchy's provider settings and cross-device
history aggregation remain specific to the system panel.

For example, place these together in a layer's `right` list:

```json
{ "id": "glance.agents", "agent": "claude", "layout": "stacked" },
{ "id": "glance.agents", "agent": "codex", "layout": "stacked" }
```

### Graphs

The graph widgets look like btop's graphs: one dot column per sample, newest on
the right, coloured with btop's gradient. Tap one to open btop (as
Super+Ctrl+T does). Only CPU and memory are on the bar by default.

```json
{ "id": "glance.cpu", "cores": true, "temperature": true }
```

| Widget | Graph | Value | Own options |
|---|---|---|---|
| `glance.cpu` | usage | `%` | `cores` (`false`): a small meter per core after the graph. `temperature`, `sensor` (`"coretemp"`). |
| `glance.memory` | RAM in use (as btop and `free` count it) | `%` | |
| `glance.gpu` | GPU load (amdgpu `gpu_busy_percent`) | `%` | `card` (first GPU that reports load, e.g. `"card1"`). `temperature`, `sensor` (the GPU's own hwmon). |
| `glance.network` | download up, upload down | `↓1.2M` `↑40K` (bytes/s) | `interface` (the default route's). |
| `glance.disk` | reads up, writes down | `R 1.2M` `W 40K` (bytes/s) | `device` (first disk in `/sys/block`, e.g. `"nvme0n1"`), `show` (`"io"`, or `"usage"` for how full `mount` is), `mount` (`"/"`). |
| `glance.battery` | a level meter: filled left to right to the charge, in one colour from red (empty) to green (full) | `%` | Label is a battery icon for the level and charging state. `graph` (`"level"`; `"charge"` or `"power"` for a history of the charge or of power draw in watts). `gradient` sets the meter's colours, empty to full. `detail` (`"none"`; `"time"` adds time to empty or full under the %, `"power"` adds watts), `low` (`15`: % at which it turns red while discharging), `maxPower` (auto; the power graph's top), `battery` (`"BAT0"`). |
| `glance.fan` | speed as a fraction of the fan's max | rpm | `fan` (`1`). Read from hwmon (applesmc on this Mac). |

Network and disk graphs scale to the busiest recent sample, like btop, with a
floor of `minScale` bytes/s (`10240`).

Options for all graphs:

| Option | Default | |
|---|---|---|
| `label` | `"cpu"`, `"mem"`, … | Text before the graph (`""` for none). |
| `graphWidth` | `100` | Graph width in px. History is `graphWidth / dotSpacing` samples. |
| `interval` | `1` | Seconds between samples. |
| `style` | `"dots"` | `"dots"`, or `"bars"` for solid columns. |
| `dotSpacing`, `dotSize` | `4`, `1.28` | Dot pitch and radius in px. |
| `grid` | `true` | Faint dots where the graph is empty. |
| `gradient` | btop theme | Colours from low to high, e.g. `["#00ff00", "#ffff00", "#ff0000"]`; for network and disk, a list of two (`[[…down…], […up…]]`). By default it's the matching gradient of btop's current theme (`~/.config/btop/themes/current.theme`: `cpu_*`, `used_*`, `download_*`/`upload_*`, `temp_*`), so it follows the Omarchy theme. |
| `showValue` | `true` | The value text. |
| `alarm` | `0.9` | A `%` value turns `colors.urgent` at this fraction. |
| `temperature` | `false` | CPU and GPU: the temperature under the `%`. |
| `sensor`, `temperatureAlarm` | see above, `90` | hwmon device to read (its `temp1_input`; names are in `/sys/class/hwmon/*/name`), and the °C at which it turns red. |
| `fontSize` | `18` | Label and single-line value text. |
| `onTap` | open btop | Shell command (`""` for nothing). |

### Microphone

`glance.mic` shows whether the default input is muted (grey, crossed out) or
live (white), and tapping it toggles mute with Omarchy's
`omarchy-audio-input-mute`, which shows the OSD. While any app records from the
mic, the icon turns red and a live waveform appears next to it.

The waveform needs its own small capture of the mic. To avoid holding the mic
open for the Touch Bar alone, that capture runs only while another app is
recording, the mic is live and the key is on screen (e.g. while Fn is held,
when it's on the Fn layer), and stops within a fraction of a second after
any of those ends. It shows up in mixers as "Touch Bar level meter".

| Option | Default | |
|---|---|---|
| `waveform` | `true` | Show the waveform while recording. |
| `waveformWidth`, `fps` | `120`, `20` | Waveform width in px, and levels per second. |
| `activeColor`, `mutedColor` | `colors.urgent`, `#808080` | Icon and waveform while recording; icon while muted. |
| `iconSize`, `width` | `30`, `140` | |
| `onTap` | toggle mute | Shell command. |

### Media

`glance.media` is four keys: previous, play/pause and next, then the track
title and artist. It uses the Omarchy shell's media service, so it controls
the same player as the media keys and the bar, and greys out what the player
can't do. It updates as soon as a player reports a change over MPRIS (via
`dbus-monitor`), and asks for player state only while it's on screen.

| Option | Default | |
|---|---|---|
| `buttonWidth`, `titleWidth` | `100`, `360` | |
| `iconSize` | `28` | |
| `interval` | `30` | Seconds between player state checks when no change is reported. |
| `onTap` | none | Shell command for a tap on the title, e.g. `"omarchy-shell media sourceNext"` to switch player. |

### Keyboard backlight

There's no keyboard backlight widget; use buttons with Omarchy's command,
which shows the OSD:

```json
{ "id": "kbd-down", "type": "button", "icon": "󰌌", "iconSize": 20, "exec": "omarchy-brightness-keyboard down", "repeat": true },
{ "id": "kbd-up", "type": "button", "icon": "󰌌", "iconSize": 32, "exec": "omarchy-brightness-keyboard up", "repeat": true }
```

`omarchy-brightness-keyboard cycle` steps through levels and wraps to off, for
a single key.

### Appearance and behaviour

| Key | Default | |
|---|---|---|
| `colors.background` | `#000000` | Bar background. |
| `colors.key`, `colors.keyPressed` | `#303030`, `#808080` | Key faces. |
| `colors.text`, `colors.urgent` | `#ffffff`, `#e05a5a` | Text, and warnings (meters at 90%+). |
| `font` | `JetBrainsMono Nerd Font` | Labels and icons. |
| `repeatDelay`, `repeatInterval` | `0.4`, `0.12` | Seconds before a held button repeats, and between repeats. |
| `idleDimSeconds` | `0` | Dim to 25% after this long without a touch (`0` = never). |

## Desktop row

The desktop row is a row of widgets under the Omarchy bar, mainly for
machines without a Touch Bar ([desktop-client.md](docs/desktop-client.md)).
It has its own file, `~/.config/omarchy-glance/desktop.json`, or
`desktop.default.json` in this directory when that doesn't exist; create a
copy with `omarchy-glance config desktop`. Like `touchbar.json` it replaces
the default entirely and reloads within a second of being saved. An invalid
`desktop.json` only affects the row (the backend logs why and keeps its
previous config, or the default); the Touch Bar keeps running its own.

```json
{
  "version": 1,
  "left":   [{ "id": "glance.media" }],
  "center": [],
  "right":  [{ "id": "glance.cpu" }, { "id": "glance.memory" }, { "id": "glance.agents", "agent": "claude" }]
}
```

The widgets are the [Touch Bar's](#widgets) marked D, with the same options;
sizes such as a spacer's `size` are desktop pixels. There are no layers and no
Fn layer: `left`, `center` and `right` sit at the top level. `glance.esc` and
`key` buttons are Touch Bar only: the backend logs them and the row leaves
them out.

| Key | Default | |
|---|---|---|
| `monitor` | every monitor | A monitor name (`"eDP-1"`, as in `hyprctl monitors`) or a list of names. |
| `height` | 1.5 times the bar's height (39 px) | Row height in pixels (10 to 200). |
| `colors.background`, `foreground`, `accent`, `urgent`, `muted` | the Omarchy theme | `#rrggbb` overrides of the live theme. |
| `font` | the bar's font | Font family. |
| `repeatDelay`, `repeatInterval` | `0.4`, `0.12` | As for the Touch Bar. |

A desktop-only setup needs no t1bridge: the backend doesn't use it, and the
row starts the backend through its socket. On a machine without a Touch Bar,
a config such as this keeps everything on the row:

```json
{
  "version": 1,
  "left": [
    { "id": "glance.media" },
    { "id": "screenshot", "type": "button", "label": "shot", "exec": "omarchy-capture-screenshot" }
  ],
  "center": [{ "id": "uptime", "type": "command", "exec": "uptime -p", "interval": 60 }],
  "right": [
    { "id": "glance.cpu", "temperature": true },
    { "id": "glance.memory" },
    { "id": "glance.network" },
    { "id": "glance.mic" },
    { "id": "glance.agents", "agent": "claude" }
  ]
}
```

Installing on such a machine (without `omarchy-glance on`, which sets up the
Touch Bar) comes with T11.

The row draws each widget like the Touch Bar does (the same graphs, meters
and keys), in the theme's colours and the bar's font, so sizes differ: option
sizes are desktop pixels, and the defaults suit a row about 40 px high: text
at the theme's size, icons 1.6 times that, `meterWidth` 170 (90 stacked),
media `buttonWidth` 44 and `titleWidth` 260, mic `waveformWidth` 80. Keys and
commands size to their content unless they set `width`. Clicking presses on
mouse down and releases on mouse up, so a held `repeat` button repeats; any
mouse button works. Clicks don't take keyboard focus from your window.

When the widgets don't fit, media titles and commands without a `width`
elide first; then whole widgets are left out, center first (from its end),
then left (from its end), then right (from its start), so the widgets at the
outer edges stay.

### Debugging

| Key | |
|---|---|
| `debug.background` | Coloured background (`colors.debugBackground`, `colors.debugBackgroundFn` while Fn is held). |
| `debug.border` | Red outline around the visible area. |
| `debug.testPattern` | Ruler ticks every 100 px, a marker under each finger, touch coordinates in the log. |
