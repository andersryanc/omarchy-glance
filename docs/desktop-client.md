# Desktop client

Task T07 of [multi-output-tasks.md](multi-output-tasks.md). The desktop row is
QML in `desktop/`: shared controls in `desktop/glance/` and the hosts that
place them. The host is the Omarchy panel plugin, with a standalone
development host ([desktop-hosts.md](desktop-hosts.md); chosen 2026-10-07).
The plugin itself arrives with T10; T09 replaces the placeholder widgets.

```text
desktop/
  shell.qml            development host: qs -p desktop
  check.qml            headless check, run by tools/desktop-check.sh
  glance/
    GlanceRow.qml      the row: one connection, left/center/right sections
    GlanceClient.qml   protocol v1 client (docs/backend-protocol.md)
    GlanceHost.qml     the host environment
    WidgetView.qml     placeholder control per widget (until T09)
```

The plugin will live in `desktop/` too (its `manifest.json` and panel entry
point next to `shell.qml`), because Quickshell only resolves imports inside
the config folder, and the plugin directory is what Omarchy installs.

## Host contract

A host creates a `GlanceHost`, gives a `GlanceRow` that environment and an
item to fill, and owns everything about the window. The shared controls never
create a window, read theme or shell files, or talk to the compositor.

```qml
GlanceHost { id: env; background: Color.background; foreground: Color.foreground /* … */ }
PanelWindow {
  implicitHeight: 30
  GlanceRow { anchors.fill: parent; host: env }
}
```

| `GlanceHost` property | Meaning | Panel plugin (T10) |
| --- | --- | --- |
| `background`, `foreground`, `accent`, `urgent`, `muted` | Palette | `Color.*` |
| `fill`, `pressedFill`, `border` | A control's face, pressed face and border | `Style.normalFill`, `Style.pressedFill`, `Style.normalBorderColor` |
| `fontFamily`, `fontSize` | Family and base size in logical pixels | `shell.bar.fontFamily`, the `Style` type scale |
| `scale` | Multiplier for sizes inside the row (padding, gaps, widths), on top of the screen's device pixel ratio, which Qt applies already | 1, or from `Style` |
| `transparent` | Draw no row background; the host's surface shows through | `bar.transparent` from `shell.json` |
| `shown` | Whether the row is shown. A hidden row keeps its connection and sends `view` with `shown: false`, so the backend stops providers only it needs | The host's own visibility (e.g. fullscreen) |

Size isn't a property: controls lay out within the `GlanceRow` item's width
and height, which the host sets. Unless a host overrides them, colours and
font come from the host theme; `desktop.json` overrides arrive with T08.

`GlanceRow` also takes `output` (`"desktop"`) and `path` (the backend socket,
by default `$XDG_RUNTIME_DIR/omarchy-glance/backend.sock`), and exposes its
`client`. Each row is one session; a host with a row on each of two monitors
has two sessions, which share providers in the backend.

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
  placeholder flashes an urgent border) and forgets its pointer. Pointers
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
  `unsupported_output` while the backend has no `desktop.json` support). A
  quick restart therefore doesn't flash it. The next snapshot brings back a
  new session with full state; graph history restarts empty, as on the Touch
  Bar.
- **Config errors:** `configError` holds the backend's parse error while it
  keeps serving the previous config.

Quickshell logs a warning for each failed connection attempt (at most every
2 s while the backend can't be started); there's no way to silence it from
QML.

## Development host

```sh
qs -p desktop                          # the user's backend, touchbar output
GLANCE_SOCKET=/path/to/backend.sock qs -p desktop
```

A plain floating window with the row at the top and buttons that change the
environment at runtime: theme (dark or light), scale (1, 1.25, 1.5, 2),
transparency and shown. A status line shows the output, connection status,
session, config generation, widget count and any config error. It needs
Quickshell, not Omarchy's shell. Until T08 serves the `desktop` output it
shows the Touch Bar config (`GLANCE_OUTPUT` picks the output); presses run
those actions for real.

## Check

`tools/desktop-check.sh` (after `cargo build --release`) runs `check.qml`
offscreen against a private backend with a small config, and checks: the
first snapshot and live command updates; a forced `rev` gap resyncs; a press
runs a button's `exec`; a press on an unknown widget is refused; stopping the
backend shows the disconnected state; restarting it brings back a new
session. It prints the `check:` log and where it saved a screenshot of each
status.
