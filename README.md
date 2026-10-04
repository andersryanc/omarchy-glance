# Touch Bar renderer

A custom renderer for the MacBook Pro T1 Touch Bar, driven through
[t1bridge](https://github.com/standardagents/t1bridge)'s Touch Bar hardware IPC
(`t1bridge-interfaces.md`). It draws an Esc key, a Claude usage widget, and
brightness/volume keys while Fn is held. Everything on the bar comes from a
JSON config.

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
| `touchbar.spacer` | `size` (40). Empty space. |
| `"type": "button"` | `icon` (Nerd Font glyph) or `label` (text), `iconSize` (30), `fontSize` (18), `width` (140); then either `exec` (shell command) or `key` (`"esc"`, `"f1"`…`"f12"`), and `repeat` (`true` repeats while held). |
| `"type": "command"` | `exec` (shell command whose output is shown), `interval` (seconds; omit to run once), `onTap` (shell command), `fontSize` (18), `width` (sized to the text when omitted). |

Commands run with `bash -c` as you, with Omarchy's commands on `PATH`, so
anything that works in a terminal or an Omarchy keybinding works here.

A command widget shows the first line of its script's output. Like Omarchy's
command modules, it also accepts Waybar-style JSON:
`{"text": "…", "class": "critical"}`. A class of `urgent` or `critical` turns
the text red.

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
