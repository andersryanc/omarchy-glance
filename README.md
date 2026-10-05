# Touch Bar renderer

A custom renderer for the MacBook Pro T1 Touch Bar, driven through
[t1bridge](https://github.com/standardagents/t1bridge)'s Touch Bar hardware IPC
(`t1bridge-interfaces.md`). By default it draws an Esc key, a mic-mute key with
a live waveform, btop-style CPU and memory graphs and a Claude usage widget,
and while Fn is held, brightness, volume and media keys. Everything on the bar
comes from a JSON config.

## Switching renderers

```
touchbar-custom on            # use this renderer
touchbar-custom off           # back to the t1bridge built-in bar
touchbar-custom restart       # restart after editing renderer.py
touchbar-custom status        # which renderer is selected and running
touchbar-custom log           # follow the renderer's log
touchbar-custom config        # create ~/.config/touchbar/config.json if missing, print its path
touchbar-custom config edit   # ... and open it in $EDITOR
```

`on` points `~/.config/t1bridge/renderer` at `renderer.py` and restarts the
`t1-touchbar` user service. If the renderer exits, t1bridge falls back to its
built-in bar.

## Configuration

The renderer reads `~/.config/touchbar/config.json`, or `config.default.json`
in this directory when that file doesn't exist. As with the Omarchy bar's
`shell.json`, your file replaces the default entirely (no merging); start from
a copy with `touchbar-custom config`.

The file is reloaded within a second of being saved. If it doesn't parse, the
renderer logs why (`touchbar-custom log`) and keeps the previous config.

```json
{
  "version": 1,
  "layers": {
    "default": {
      "left":   [{ "id": "touchbar.esc" }],
      "center": [],
      "right":  [{ "id": "touchbar.agents", "agent": "claude" }]
    },
    "fn": {
      "left": [
        { "id": "touchbar.esc" },
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

Every widget has an `id`. Built-in widgets use `touchbar.*` ids; your own
widgets set a `type` and can use any id.

| Widget | Options |
|---|---|
| `touchbar.esc` | `width` (140). Sends Esc. |
| `touchbar.agents` | `agent` (`"claude"`; any record in `~/.local/state/omarchy/agents/usage/`), `meterWidth` (320), `onTap` (toggles the Omarchy agents panel; `""` for nothing). |
| `touchbar.mic` | Mic mute toggle with a live waveform while an app records. See [Microphone](#microphone). |
| `touchbar.media` | Previous, play/pause and next keys and the current track. See [Media](#media). |
| `touchbar.cpu`, `.memory`, `.gpu`, `.network`, `.disk`, `.battery`, `.fan` | A label, a btop-style dot graph of recent history, and the current value. See [Graphs](#graphs). |
| `touchbar.spacer` | `size` (40). Empty space. |
| `"type": "button"` | `icon` (Nerd Font glyph) or `label` (text), `iconSize` (30), `fontSize` (18), `width` (140); then either `exec` (shell command) or `key` (`"esc"`, `"f1"`…`"f12"`), and `repeat` (`true` repeats while held). |
| `"type": "command"` | `exec` (shell command whose output is shown), `interval` (seconds; omit to run once), `onTap` (shell command), `fontSize` (18), `width` (sized to the text when omitted). |

Commands run with `bash -c` as you, with Omarchy's commands on `PATH`, so
anything that works in a terminal or an Omarchy keybinding works here.

A command widget shows the first line of its script's output. Like Omarchy's
command modules, it also accepts Waybar-style JSON:
`{"text": "…", "class": "critical"}`. A class of `urgent` or `critical` turns
the text red.

### Graphs

The graph widgets look like btop's graphs: one dot column per sample, newest on
the right, coloured with btop's gradient. Tap one to open btop (as
Super+Ctrl+T does). Only CPU and memory are on the bar by default.

```json
{ "id": "touchbar.cpu", "cores": true, "temperature": true }
```

| Widget | Graph | Value | Own options |
|---|---|---|---|
| `touchbar.cpu` | usage | `%` | `cores` (`false`): a small meter per core after the graph. `temperature`, `sensor` (`"coretemp"`). |
| `touchbar.memory` | RAM in use (as btop and `free` count it) | `%` | |
| `touchbar.gpu` | GPU load (amdgpu `gpu_busy_percent`) | `%` | `card` (first GPU that reports load, e.g. `"card1"`). `temperature`, `sensor` (the GPU's own hwmon). |
| `touchbar.network` | download up, upload down | `↓1.2M` `↑40K` (bytes/s) | `interface` (the default route's). |
| `touchbar.disk` | reads up, writes down | `R 1.2M` `W 40K` (bytes/s) | `device` (first disk in `/sys/block`, e.g. `"nvme0n1"`), `show` (`"io"`, or `"usage"` for how full `mount` is), `mount` (`"/"`). |
| `touchbar.battery` | charge level | `%`, then `detail` | Label is a battery icon for the level and charging state. `graph` (`"charge"`, or `"power"` for power draw in watts). `detail` (`"time"`: time to empty or full; `"power"`: watts; `"none"`), `low` (`15`: % at which it turns red while discharging), `maxPower` (auto; the power graph's top), `battery` (`"BAT0"`). |
| `touchbar.fan` | speed as a fraction of the fan's max | rpm | `fan` (`1`). Read from hwmon (applesmc on this Mac). |

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

`touchbar.mic` shows whether the default input is muted (grey, crossed out) or
live (white), and tapping it toggles mute with Omarchy's
`omarchy-audio-input-mute`, which shows the OSD. While any app records from the
mic, the icon turns red and a live waveform appears next to it.

The waveform needs its own small capture of the mic. To avoid holding the mic
open for the Touch Bar alone, that capture runs only while another app is
recording and the mic is live, and stops within a fraction of a second after
it ends. It shows up in mixers as "Touch Bar level meter".

| Option | Default | |
|---|---|---|
| `waveform` | `true` | Show the waveform while recording. |
| `waveformWidth`, `fps` | `120`, `20` | Waveform width in px, and levels per second. |
| `activeColor`, `mutedColor` | `colors.urgent`, `#808080` | Icon and waveform while recording; icon while muted. |
| `iconSize`, `width` | `30`, `140` | |
| `onTap` | toggle mute | Shell command. |

### Media

`touchbar.media` is four keys: previous, play/pause and next, then the track
title and artist. It uses the Omarchy shell's media service, so it controls
the same player as the media keys and the bar, and greys out what the player
can't do. It asks for player state only while it's on screen.

| Option | Default | |
|---|---|---|
| `buttonWidth`, `titleWidth` | `100`, `360` | |
| `iconSize` | `28` | |
| `interval` | `1` | Seconds between player state checks. |
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

### Debugging

| Key | |
|---|---|
| `debug.background` | Coloured background (`colors.debugBackground`, `colors.debugBackgroundFn` while Fn is held). |
| `debug.border` | Red outline around the visible area. |
| `debug.testPattern` | Ruler ticks every 100 px, a marker under each finger, touch coordinates in the log. |
