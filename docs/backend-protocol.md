# Backend protocol v1

Task T02 of [multi-output-tasks.md](multi-output-tasks.md). The backend service
and its output clients (the Touch Bar client first, a desktop client later)
talk over this protocol; see [ADR 0001](adr/0001-multiple-output-architecture.md)
for the architecture and [widget-inventory.md](widget-inventory.md) for the
widgets it carries. The backend side is implemented in `src/backend/` (T03);
the Touch Bar client is `src/renderer.rs` (T04). Decisions marked **(approved)** settle open questions from
the inventory; the user approved them on 2026-10-06.

## Principles

- The backend owns config, providers, widget state and actions. Clients own
  layout, drawing, press feedback and their hardware or host.
- Clients send semantic input (press, release, activate a widget). They
  never send shell commands, and the backend never sends them commands to run:
  a client can only trigger the action configured for a widget in its own
  output's config file.
- State is data, never drawing: no pixels, surfaces or coordinates.
- A client that disconnects, stalls or misbehaves cannot affect other clients
  or leave an action repeating.

## Transport

### Socket

- Path: `$XDG_RUNTIME_DIR/omarchy-glance/backend.sock` (normally
  `/run/user/<uid>/omarchy-glance/backend.sock`). Directory mode 0700, socket
  mode 0600.
- Unix stream socket (`SOCK_STREAM`). The backend also checks `SO_PEERCRED`
  and closes connections from other users.
- Socket activation: systemd user units `omarchy-glance.socket`
  (`ListenStream=%t/omarchy-glance/backend.sock`, `SocketMode=0600`,
  `DirectoryMode=0700`) and `omarchy-glance.service`
  (`ExecStart=omarchy-glance backend`). The first client to connect starts
  the backend. The backend takes the listening socket from `LISTEN_FDS`; when
  run by hand without it, it binds the path itself (and refuses to start if
  another backend answers there). Only one backend runs per user.
- The backend keeps running when its last client disconnects; with no
  clients, all providers are stopped (see [Demand](#demand)). Exiting when
  idle is left for later.

### Framing

Each message is one JSON object encoded as UTF-8 on a single line, ended by
`\n` (newline-delimited JSON). JSON encoders escape newlines inside strings,
so a raw newline only ever ends a message.

The ADR asked for length-prefixed JSON. Newline-delimited is chosen instead
**(approved)** because the desktop client is QML: Quickshell's `Socket` with a
`SplitParser` (split marker `"\n"` by default) delivers one message per line
with no byte handling in JavaScript. On the Rust side it costs the same.

- Maximum message size: 1 MiB including the newline. A longer line is a fatal
  `too_large` error.
- Every message has a string `"type"`. Unknown fields are ignored by both
  sides. A client ignores unknown message types from the backend; the
  backend answers unknown types from a client with a `bad_message` error.
- Numbers are JSON numbers; times are RFC 3339 strings with an offset.

## Messages

### Requests and replies

Client messages that expect a reply carry an `"id"`: an integer the client
chooses, unique among its outstanding requests. The backend answers each with
exactly one `ack` or `error` carrying that id:

```json
{"type":"ack","id":7}
{"type":"error","id":7,"code":"unknown_widget","message":"no widget default.right.9"}
```

`ack` means the request was accepted (an action was started, demand
recorded), not that an action finished. Errors without an `id` concern the
connection as a whole.

| Code | Fatal | Meaning |
| --- | --- | --- |
| `bad_message` | no | Not valid JSON, not an object, missing or wrong-typed fields, or unknown type |
| `too_large` | yes | Line over 1 MiB |
| `unsupported_protocol` | yes | The client's protocol major isn't 1 |
| `unsupported_output` | yes | The `output` isn't `touchbar` or `desktop` |
| `not_ready` | no | A request before `hello` |
| `unknown_widget` | no | No such widget key in the client's current config generation |
| `stale_config` | no | The request names an older config generation |
| `not_pressable` | no | The widget has no backend action (spacer, Esc, `key` buttons, or `onTap: ""`) |
| `hidden` | no | The widget is hidden (agents without data) |
| `unknown_pointer` | no | `release` for a pointer that isn't pressed |
| `queue_overflow` | yes | The client left too many replies unread (see [Queues](#queues-and-coalescing)) |

After a fatal error the backend closes the connection.

### Handshake

The client speaks first:

```json
{"type":"hello","id":1,"protocol":1,"output":"touchbar","client":"omarchy-glance 0.1.0"}
```

- `protocol`: the major version. The backend accepts 1. Compatible
  additions (new fields, message types, widget kinds, error codes) don't change
  it.
- `output`: `"touchbar"` or `"desktop"`. It picks the config file
  (`~/.config/omarchy-glance/touchbar.json` or `desktop.json`) and which widgets
  are supported. Several clients may share an output type (e.g. two desktop
  rows); each gets its own session.

The backend answers with `ack`, then `welcome`, then a `snapshot`:

```json
{"type":"welcome","protocol":1,"session":"4f1c","backend":"omarchy-glance 0.1.0",
 "features":["repeat","demand"]}
```

- `session`: an opaque id for logs. Sessions don't survive a disconnect;
  reconnecting starts a new one.
- `features`: optional behaviours this backend has, so later additions can be
  detected without a protocol bump.

### Snapshot

A snapshot is the complete state for the client's output. It's sent after
`hello`, after every config reload, after a `resync` request, and when the
backend replaces an overflowing update queue.

```json
{"type":"snapshot","rev":1,
 "config":{"generation":3,"path":"~/.config/omarchy-glance/touchbar.json","error":null},
 "settings":{"colors":{"background":"#000000","key":"#303030","keyPressed":"#808080",
             "text":"#ffffff","urgent":"#e05a5a","debugBackground":"#106090","debugBackgroundFn":"#602090"},
             "font":"JetBrainsMono Nerd Font","idleDimSeconds":0,"repeatDelay":0.4,"repeatInterval":0.12,
             "debug":{"background":false,"border":false,"testPattern":false},"hasFn":true},
 "widgets":[
   {"key":"default.left.0","id":"glance.esc","kind":"esc","layer":"default","section":"left",
    "supported":true,"options":{},"action":{"key":"esc"},"state":{}},
   {"key":"default.right.2","id":"glance.agents","kind":"agents","layer":"default","section":"right",
    "supported":true,"options":{"agent":"claude","layout":"stacked"},"action":{"press":true,"repeat":false},
    "state":{"visible":true,"limits":[{"label":"Session","fraction":0.12,"resetsAt":"2026-10-06T14:00:00-07:00"}]}}
 ]}
```

- `rev`: the session's revision counter. A snapshot sets it; each `update`
  increments it by exactly 1.
- `config.generation`: increases on every successful reload. `path` is the
  file in use, or `"<built-in touchbar.default.json>"`. `error` is the parse
  error when the user file is invalid; the backend then keeps serving the
  previous config (or the built-in default at startup), as today.
- `settings`: output-level presentation settings from the config, so clients
  never parse config files. The backend applies `repeatDelay` and
  `repeatInterval` to backend actions; they're in `settings` too for the keys
  a Touch Bar client repeats itself. The desktop output's settings are
  `{"colors": {...}, "font", "monitors": [...], "height", "repeatDelay",
  "repeatInterval", "hasFn": false}`, where `colors` holds only the
  `desktop.json` overrides (any of `background`, `foreground`, `accent`,
  `urgent`, `muted`), `font` and `height` are `null` unless set, and
  `monitors` is empty for every monitor: the host theme fills in the rest.
- `widgets`: in config order: layer, then section, then position.

Widget fields:

| Field | |
| --- | --- |
| `key` | `<layer>.<section>.<index>`; stable for a config generation. Identity is the key, not `id`, because ids repeat. |
| `id`, `kind` | The configured id; kind is `esc`, `button`, `command`, `agents`, `graph`, `mic`, `media` or `spacer`. A graph's id names its source (`glance.cpu`, …). |
| `layer`, `section` | `default` or `fn`; `left`, `center` or `right`. |
| `supported` | `false` for a widget this output can't show, with `"reason"` (see the example below). The client gives it no space. |
| `options` | The widget's config object without `exec` and `onTap`: presentation options, plus provider options the client can ignore. |
| `action` | What a press does: `{"press": true, "repeat": bool}` for a backend action, `{"key": "f1", "repeat": bool}` for a key the client taps (and repeats) itself (Touch Bar only; Esc is `{"key": "esc", "repeat": false}`), `{"zones": [...]}` for media (`previous`, `playPause`, `next`, and `title` only when the widget has an `onTap`), or `null`. |
| `state` | The widget's data (below). |

### Widget state

Each `state` object is replaced whole by updates.

| Kind | State |
| --- | --- |
| `esc`, `button`, `spacer` | `{}` |
| `command` | `{"text": "up 3d 4h", "urgent": false}` |
| `agents` | `{"visible": bool, "limits": [{"label", "fraction" (0–1 or null), "resetsAt" (or null)}]}`; with no limits the backend sends the "Session"/"Weekly" placeholders with null fractions. Reset text is formatted by the client from its own clock (it redraws each minute). |
| `graph` | `{"series": [[raw values, oldest first]], "scale": [one per series], "lines": [["23%", false]], "cores": [fractions], "battery": {"charge": 64, "status": "Discharging"} or null, "error": bool}`. Clients divide by `scale` for the 0–1 height. History is long enough for the longest graph sharing this provider; clients draw the newest samples that fit **(approved)**. The client picks the battery glyph from `battery` **(approved)**. |
| `mic` | `{"muted": bool or null, "inUse": bool, "levels": [0–1, newest last], "fps": 20}`; `levels` is empty unless the capture is running. It holds the newest 64 levels **(approved)**, enough for any waveform width the client draws. |
| `media` | `{"hasMedia", "playing", "title", "artist", "identity", "canGoPrevious", "canTogglePlaying", "canGoNext"}` as reported by `omarchy-shell media status`; `{}` before the first report. |

### Updates

```json
{"type":"update","rev":2,"widgets":{"default.right.0":{"series":[[0.31,0.29]],"scale":[1],"lines":[["29%",false]],"cores":[],"battery":null,"error":false}}}
```

`widgets` maps keys to their new `state`. If an update's `rev` isn't the
previous `rev + 1`, the client has missed something: it sends `resync` and
ignores updates until the snapshot arrives.

```json
{"type":"resync","id":12}
```

### Input

```json
{"type":"press","id":20,"generation":3,"widget":"fn.left.1","pointer":4}
{"type":"press","id":21,"generation":3,"widget":"default.left.1","pointer":5,"zone":"next"}
{"type":"release","id":22,"pointer":4}
{"type":"activate","id":23,"generation":3,"widget":"default.right.2"}
```

- `press` runs the widget's action at once and, if `action.repeat`, repeats
  it after `repeatDelay` and every `repeatInterval` until the pointer is
  released. `pointer` is a client-chosen integer (the Touch Bar uses the
  contact id; a desktop client uses one per mouse button), so several fingers
  can hold several buttons.
- `release` ends a pointer's press and cancels its repeat. A release for a
  widget without repeat is still sent, so both sides agree on which pointers
  are down.
- `activate` runs the action once, with no pointer and no repeat (keyboard
  activation, scripted use).
- `zone` selects a media part; it's required for media and ignored otherwise.
- The backend cancels repeats itself when the pointer is released, the
  widget becomes hidden, the config reloads, or the client disconnects.
- Press feedback is the client's: it draws the pressed face on touch-down
  without waiting for the `ack`.
- `key` actions (Esc, F-keys) never reach the backend; the Touch Bar client
  taps them through t1bridge.

### Demand

```json
{"type":"view","id":30,"layer":"fn","shown":true}
```

A client says which layer it shows and whether it's shown at all (a desktop
host may hide its row). The backend computes each session's visible widgets
and runs providers by today's rules:

| Provider | Runs while |
| --- | --- |
| Graphs, commands, agent records and refresh jobs | any session's config has the widget (including hidden layers, so history stays continuous) |
| Media status polling | a media widget is on a shown layer of some session (`dbus-monitor` while any session has one) |
| Mic level capture | a mic widget with `waveform` is on a shown layer of some session, another app is recording, and the mic is live (the renderer captured even with `waveform: false`; the backend doesn't) |
| Mic mute/recording state | any session's config has a mic widget |

Sessions share providers when the widget id and its provider options match
(see the inventory's open question 1), so a Touch Bar and a desktop row
showing CPU sample `/proc/stat` once **(approved)**. With no sessions, every
provider stops and the mic capture closes. Until a client's first `view`, the
backend assumes `{"layer":"default","shown":true}`.

### Queues and coalescing

- The backend keeps, per session, the latest state of each changed widget
  rather than a queue of updates. When the socket is writable it sends one
  `update` with every changed widget, at most every 16 ms. A slow client
  therefore sees fewer, larger updates and never stale intermediate ones.
- Replies (`ack`, `error`) and snapshots are queued in order. If more than
  256 replies wait unsent, the client isn't reading: `queue_overflow`, and
  the connection closes.
- The server hands a client new lines only once everything before them has
  been written to its socket, so at most one batch (queued replies, one
  snapshot, one update) waits per client. Snapshots coalesce too: a resync or
  reload while one is queued doesn't queue another.
- All backend sockets are non-blocking. A client that hasn't read anything
  for 10 s while data is waiting is disconnected. Nothing a client does blocks
  the backend's loop or other sessions.

### Ordering

Within a connection, messages arrive in the order sent. A reply comes after
every update the backend sent before it read the request. A snapshot replaces
everything before it. There's no ordering between connections.

## Lifecycle

- **Disconnect:** the backend releases every pointer the session held
  (cancelling repeats), drops its demand, and forgets the session.
- **Reconnect:** a new `hello`, a new session and a full snapshot. Nothing
  resumes.
- **Touch Bar client:** it must not exit on backend errors (that would hand
  the bar to t1bridge's built-in renderer for the rest of the session). On
  EOF or a fatal error it shows a disconnected state on the bar and
  reconnects after 0.1 s, doubling to at most 2 s. Socket activation restarts
  the backend if needed.
- **Config reload:** a new generation and a snapshot to each session of that
  output. Presses against the old generation get `stale_config`.

## Worked examples

`→` is client to backend, `←` backend to client. Fields not relevant to the
example are left out of snapshots.

### Connect

```text
→ {"type":"hello","id":1,"protocol":1,"output":"touchbar","client":"omarchy-glance 0.1.0"}
← {"type":"ack","id":1}
← {"type":"welcome","protocol":1,"session":"4f1c","backend":"omarchy-glance 0.1.0","features":["repeat","demand"]}
← {"type":"snapshot","rev":1,"config":{"generation":1,"path":"~/.config/omarchy-glance/touchbar.json","error":null},"settings":{…},"widgets":[…]}
→ {"type":"view","id":2,"layer":"default","shown":true}
← {"type":"ack","id":2}
```

### Update

A CPU sample and a command result arrive in the same 16 ms; they go out as
one update.

```text
← {"type":"update","rev":2,"widgets":{
     "default.right.0":{"series":[[0.30,0.29,0.31]],"scale":[1],"lines":[["31%",false]],"cores":[],"battery":null,"error":false},
     "fn.left.4":{"text":"up 3d 4h","urgent":false}}}
```

### Press, hold, release

Volume-up has `"repeat": true`; `repeatDelay` 0.4 s, `repeatInterval` 0.12 s.

```text
t=0.00  → {"type":"press","id":10,"generation":1,"widget":"fn.left.6","pointer":2}
t=0.00  ← {"type":"ack","id":10}            backend runs omarchy-audio-output-volume raise
t=0.40                                      backend repeats it
t=0.52                                      … and again every 0.12 s
t=0.70  → {"type":"release","id":11,"pointer":2}
t=0.70  ← {"type":"ack","id":11}            repeat cancelled
```

### Disconnect mid-press

```text
t=0.00  → {"type":"press","id":10,"generation":1,"widget":"fn.left.6","pointer":2}
t=0.00  ← {"type":"ack","id":10}
t=0.40                                      repeat
t=0.45  (client crashes; the backend reads EOF)
                                            backend releases pointer 2, cancels the repeat, drops demand
```

No further repeats run. If the client was the only one showing a mic widget,
the mic capture stops too.

### Backend restart

```text
(backend exits; the Touch Bar client reads EOF)
client: draws the disconnected state, keeps t1bridge connected, retries after 0.1 s
→ connect fails (ECONNREFUSED) unless socket activation already restarted the backend
client: retries after 0.2 s, 0.4 s, … (at most 2 s)
→ {"type":"hello","id":1,"protocol":1,"output":"touchbar","client":"omarchy-glance 0.1.0"}
← {"type":"ack","id":1}
← {"type":"welcome","protocol":1,"session":"9a20",…}
← {"type":"snapshot","rev":1,…}
→ {"type":"view","id":2,"layer":"default","shown":true}
```

Graph histories restart empty after a backend restart. The client went through
the whole sequence without exiting, so t1bridge keeps using it.

### Unsupported widget

`desktop.json` lists `glance.esc`. The backend logs once at load,
`desktop.json: left[0]: glance.esc is not supported on desktop (Touch Bar only)`,
and the snapshot carries it so positions and keys stay consistent:

```text
← {"type":"snapshot","rev":1,"widgets":[
     {"key":"default.left.0","id":"glance.esc","kind":"esc","layer":"default","section":"left",
      "supported":false,"reason":"Touch Bar only","options":{},"action":null,"state":{}},
     …]}
→ {"type":"press","id":5,"generation":1,"widget":"default.left.0","pointer":1}
← {"type":"error","id":5,"code":"not_pressable","message":"default.left.0 (glance.esc) has no action on desktop"}
```

The desktop client gives it no space and doesn't draw it.

### A client asking for something it isn't configured for

There's no message that carries a command, so the closest a client can get is
pressing a key that doesn't exist:

```text
→ {"type":"press","id":9,"generation":1,"widget":"default.left.99","pointer":1}
← {"type":"error","id":9,"code":"unknown_widget","message":"no widget default.left.99"}
```

## In-process preview

`omarchy-glance preview` runs the backend and the Touch Bar client in one
process. They exchange the same messages over an in-memory channel instead of
the socket, so the preview exercises the protocol code. The golden tests
inject fixed provider data on the backend side.

## Traffic

Measured 2026-10-07 with a test client on the built-in default config: each
graph sends one update a second, growing to about 500 bytes once its history
is full (values are sent at full precision); the first seconds measured 0.6 to
1.9 KB/s including agents and media. A waveform update while recording is
estimated at about 600 bytes 20 times a second. The peak stays well under
15 KB/s, so cheaper encodings aren't needed. Rounding sample values would
roughly halve graph traffic if it ever matters.
