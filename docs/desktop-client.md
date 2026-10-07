# Desktop client

Task T07 of [multi-output-tasks.md](multi-output-tasks.md). The desktop row is
QML in `desktop/`: shared controls in `desktop/glance/` and the hosts that
place them. The host is the Omarchy panel plugin, with a standalone
development host ([desktop-hosts.md](desktop-hosts.md); chosen 2026-10-07).
The widgets are T09's; T10 finishes the plugin (a first version is in, see
[Panel plugin](#panel-plugin)).

```text
desktop/
  manifest.json        the Omarchy panel plugin glance.row
  Panel.qml            its entry point: one row under the bar per screen
  shell.qml            development host: qs -p desktop
  check.qml            headless check, run by tools/desktop-check.sh
  glance/
    GlanceRow.qml      the row: left/center/right sections of a client's widgets
    GlanceClient.qml   protocol v1 client (docs/backend-protocol.md)
    GlanceHost.qml     the host environment
    WidgetView.qml     one widget: picks its control, maps mouse to press/release
    Face.qml           a key face (fill, hover, pressed)
    ButtonWidget.qml, CommandWidget.qml, GraphWidget.qml, AgentsWidget.qml,
    MicWidget.qml, MediaWidget.qml, Meter.qml, Dots.qml (btop-style dots)
    BtopTheme.qml      for hosts: btop's current theme, for graph gradients
    util.js            graph presentation per source, colours, reset times
```

The plugin lives in `desktop/` too, because Quickshell only resolves imports
inside the config folder, and the plugin directory is what Omarchy installs.

## Host contract

A host creates one `GlanceClient` and a `GlanceHost`, gives each `GlanceRow`
both and an item to fill, and owns everything about the window. The shared controls never
create a window, read theme or shell files, or talk to the compositor.

```qml
GlanceClient { id: glanceClient }   // output "desktop"
GlanceHost { id: env; background: glanceClient.color("background", Color.bar.background) /* … */ }
PanelWindow {
  implicitHeight: glanceClient.settings.height || Math.round(1.5 * Style.bar.sizeHorizontal)
  GlanceRow { anchors.fill: parent; host: env; client: glanceClient }
}
```

| `GlanceHost` property | Meaning | Panel plugin (T10) |
| --- | --- | --- |
| `background`, `foreground`, `accent`, `urgent`, `muted` | Palette | `Color.*` |
| `fill`, `hoverFill`, `pressedFill` | A control's face (no border), hovered and pressed | `Style.normalFill`, `Style.hoverFill`, `Style.pressedFill` |
| `fontFamily`, `fontSize` | Family and base size in logical pixels | `shell.bar.fontFamily`, `Style.font.body` (12) |
| `scale` | Multiplier for sizes inside the row (padding, gaps, widths), on top of the screen's device pixel ratio, which Qt applies already | 1, or from `Style` |
| `transparent` | Draw no row background; the host's surface shows through | `bar.transparent` from `shell.json` |
| `shown` | Whether the row is shown. A hidden row keeps its connection and sends `view` with `shown: false`, so the backend stops providers only it needs | The host's own visibility (e.g. fullscreen) |

Size isn't a property: controls lay out within the `GlanceRow` item's width
and height, which the host sets. Colours and font come from the host theme
unless `desktop.json` overrides them: hosts fill `GlanceHost` through
`client.color(name, themeValue)` and `client.font(themeValue)`. The client's
`settings.monitors` and `settings.height` say where the host puts rows and
how tall.

`GlanceClient` takes `output` (`"desktop"`) and `path` (the backend socket,
by default `$XDG_RUNTIME_DIR/omarchy-glance/backend.sock`). One client is one
session, so a host with a row on each of two monitors shares one client
between them; the host sets its `shown`.

## Widgets

Each widget kind has its own control, drawn the way the Touch Bar draws it
(`src/renderer.rs`) with the host's palette and font: the same behaviour, not
the same pixels. Graphs use the same dot rows, mirrored halves (network,
disk), per-core meters and battery level meter, with btop's current theme
for gradients (`GlanceHost.btop`, which hosts fill from `BtopTheme`) or a
widget's `gradient`; agents show Omarchy's provider logos
(`GlanceHost.agentIcons`; `<agent>-light.svg` first on a light background, as
the agents panel does) and format reset times from the local clock each
minute; the mic shows its waveform while another app records; media splits
into previous, play/pause, next and title zones. Option sizes are desktop
pixels, multiplied by `GlanceHost.scale`; the defaults are in the README's
[Desktop row](../README.md#desktop-row).

`WidgetView` maps mouse buttons to semantic input: press on mouse down
(`client.press(key, button, zone)`, one pointer per button), release on
mouse up or when the press is cancelled. A second button while one is held is
ignored. The backend repeats held `repeat` buttons. Hover lightens pressable
faces and shows a pointing-hand cursor.

`GlanceRow` lays the widgets out (left from the left edge, right against the
right edge, center in the middle but clear of both) and handles overflow:
first media titles and commands without a `width` give way down to a
minimum (eliding), then whole widgets are left out, center from its end, then
left from its end, then right from its start. `dropped` counts the widgets
left out. Hidden agents widgets take no space.

`tools/desktop-shot.sh desktop.json out.png [width] [height] [scale]` renders
the row headlessly against a private backend, to see a config or a narrow
width without touching the desktop.

## Connection

`GlanceClient` implements the client side of
[backend-protocol.md](backend-protocol.md):

- **Handshake:** `hello` with the output and `omarchy-glance-desktop 0.1.0`,
  then `ack`, `welcome`, `snapshot`. After the first snapshot it sends `view`
  (layer `default`, `shown` from the host), and again whenever `shown`
  changes.
- **State:** `widgets` is replaced by each snapshot. Updates change single
  widget states and emit `widgetChanged(key, state)`, so only the controls
  whose widget changed re-evaluate. An update whose `rev` isn't the previous
  plus one sends `resync`, and updates are ignored until the snapshot comes.
- **Input:** `press(key, pointer, zone)` and `release(pointer)` with one
  pointer per mouse button (`Qt.LeftButton`, …), and `activate(key, zone)`.
  Press feedback is the control's own and doesn't wait for the `ack`. A
  refused press or activation emits `requestFailed(key, code, message)` (the
  widget flashes an urgent border) and forgets its pointer. Pointers
  held across a config reload are dropped, since the backend ends those
  presses; a resync keeps them.
- **Reconnection:** on EOF, a refused connection or a fatal error, it retries
  after 0.1 s, doubling to at most 2 s, like the Touch Bar client. Each
  attempt uses a new Quickshell `Socket`, because one that failed to connect
  never tries again. Socket activation starts the backend on the next
  attempt if it isn't running.
- **Disconnected state:** if no snapshot arrives within 1 s of losing the
  backend (or of starting), `status` becomes `offline`: widgets are cleared
  and the row shows "omarchy-glance: backend unavailable, reconnecting…",
  the Touch Bar's wording, or the fatal error (for example
  `unsupported_output` from a backend too old for the desktop output). A
  quick restart therefore doesn't flash it. The next snapshot brings back a
  new session with full state; graph history restarts empty, as on the Touch
  Bar.
- **Config errors:** `configError` holds the backend's parse error while it
  keeps serving the previous config.

Quickshell logs a warning for each failed connection attempt (at most every
2 s while the backend can't be started); there's no way to silence it from
QML.

## Panel plugin

`glance.row` is a `panel` plugin with `keepLoaded: true`, so omarchy-shell
mounts it at startup once enabled. `Panel.qml` puts a `PanelWindow`
(namespace `omarchy-glance`, top layer, no keyboard focus, its own exclusive
zone) at the top of each screen, `Style.bar.sizeHorizontal` (26 px) high, and
keeps itself mapped after the bar (Hyprland stacks exclusive zones in map
order, and a scale or monitor change recreates the bar's windows): it hides
on `closelayer>>omarchy-bar`, and once `openlayer>>omarchy-bar` arrives it
maps again after 250 ms without layer or monitor events (or after 2 s if
the bar doesn't come back), fading its content in over 150 ms. Polling
`hyprctl layers` during a scale change shows the row at the top for one or
two polls (about 20–40 ms) before it hides, which can't be avoided because
nothing announces the bar's close in advance. It fills its `GlanceHost` from `Color.bar.*`, `Color.*`, `Style` fills and the
bar's font, under `desktop.json`'s overrides. It shows a row on the
monitors `desktop.json` names (every monitor by default), as tall as its
`height` or 1.5 times the bar (39 px).

`omarchy-glance desktop on` installs it: the plugin files are compiled into
the binary (`PLUGIN_FILES` in `src/cli.rs`, checked against `desktop/` by a
test) and written to `~/.config/omarchy/plugins/glance.row/`. For
development, link the checkout instead:

```sh
ln -sfn ~/Work/omarchy-glance/desktop ~/.config/omarchy/plugins/glance.row
omarchy-shell shell rescanPlugins
omarchy-shell shell setPluginEnabled glance.row true   # "unknown" until the rescan finishes
```

The shell notices changed files and reloads its plugins, but a running
shell keeps the plugin's first compiled version, so edits only take effect
after `omarchy-restart-shell`.

### Behaviour with the bar

Checked 2026-10-07 on one monitor (`hyprctl layers` and `hyprctl monitors`):

| Bar | Row | Reserved at the top |
| --- | --- | --- |
| Top (default) | Under the bar | bar + row (26 + 39 px); tiled and maximised windows start below both |
| Hidden (`omarchy-toggle-bar`) | At the top of the screen | row only (39 px); back under the bar when it's shown |
| At the bottom (`omarchy bar position bottom`) | Stays at the top: the first release is a top row only | row at the top, bar at the bottom |
| Transparent (`bar.transparent` in `~/.config/omarchy/shell.json`) | No row background, faces keep their fill; text in the colour `omarchy-bar-text-color` picks for the wallpaper behind both rows, as the bar does | unchanged |
| Absent (another bar plugin, or none) | Not tried; expected at the top of the screen, as when hidden | row only |
| Scale change, shell restart | Hides while the bar is recreated, then maps under it | unchanged |
| Plugin disabled | Gone | bar only (26 px) |

Transparency isn't in the plugin facade, so the plugin reads `shell.json`
itself and re-picks the text colour when transparency, the theme's bar text
colour or the row height changes (not when only the wallpaper changes, like
the bar). Muted text (reset times, "No media") can be faint on a busy
wallpaper.

Remaining gaps: monitor hotplug and a second monitor are untested (one
monitor here; each screen gets its own window through `Variants`, and the
row on a monitor not named in `desktop.json` isn't created); the row can't
follow the bar to the bottom or the sides; a vertical bar isn't handled.

## Development host

```sh
qs -p desktop                          # the user's backend, desktop output
GLANCE_SOCKET=/path/to/backend.sock qs -p desktop
```

A plain floating window with the row at the top and buttons that change the
environment at runtime: theme (dark or light), scale (1, 1.25, 1.5, 2),
transparency and shown. A status line shows the output, connection status,
session, config generation, widget count and any config error. It needs
Quickshell, not Omarchy's shell. `GLANCE_OUTPUT=touchbar` shows the Touch
Bar config instead; presses run the configured actions for real.

## Check

`tools/desktop-check.sh` (after `cargo build --release`) runs `check.qml`
offscreen against a private backend with a small `desktop.json`, and
checks: Esc is unsupported on the desktop; the first snapshot and live command updates; a forced `rev` gap resyncs; a press
runs a button's `exec`; a press on an unknown widget is refused; stopping the
backend shows the disconnected state; restarting it brings back a new
session. It prints the `check:` log and where it saved a screenshot of each
status.
