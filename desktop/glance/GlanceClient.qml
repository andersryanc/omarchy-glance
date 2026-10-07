// The desktop client's connection to the backend: protocol v1 over
// $XDG_RUNTIME_DIR/omarchy-glance/backend.sock (docs/backend-protocol.md).
// It keeps the latest snapshot and widget states, sends input and demand,
// and reconnects with the same backoff as the Touch Bar client. It draws
// nothing; GlanceRow.qml shows its state.
pragma ComponentBehavior: Bound
import QtQuick
import Quickshell
import Quickshell.Io

Scope {
  id: client

  // Set by the host.
  property string output: "desktop"
  readonly property string defaultPath: Quickshell.env("XDG_RUNTIME_DIR") + "/omarchy-glance/backend.sock"
  property string path: defaultPath
  property string layer: "default"
  property bool shown: true // false while the host hides its row: the backend stops providers only it needs

  // Read by controls. `widgets` is replaced whole by each snapshot; states
  // change through stateChanged and stateOf.
  property string status: "connecting" // "connecting", "ready" or "offline"
  property string session: ""
  property int generation: 0
  property string configPath: ""
  property string configError: "" // the backend keeps the previous config while the user file is invalid
  property string fatalError: "" // the last error that closed the connection, e.g. unsupported_output
  property var settings: ({})
  property var widgets: []

  signal widgetChanged(string key, var state)
  signal requestFailed(string key, string code, string message) // a press or activate the backend refused

  readonly property string clientName: "omarchy-glance-desktop 0.1.0"
  readonly property int retryMin: 100 // ms, doubling up to retryMax, as on the Touch Bar
  readonly property int retryMax: 2000
  readonly property int offlineAfter: 1000 // keep the last state this long before showing the disconnected state

  property var states: ({}) // key -> latest state; not bound to directly, use stateOf
  property int rev: 0
  property bool synced: false
  property bool resyncing: false
  property int nextId: 1
  property var pending: ({}) // request id -> {key, pointer}, for presses and activations
  property var down: ({}) // pointer -> widget key
  property int retryDelay: retryMin

  function stateOf(key) { return states[key] ?? {} }
  function isDown(pointer) { return down[pointer] !== undefined }

  function send(msg) {
    if (!socket || !socket.connected) return 0
    msg.id = nextId++
    socket.write(JSON.stringify(msg) + "\n")
    socket.flush()
    return msg.id
  }

  // A press runs the widget's action now and, for repeating actions, until
  // release(pointer). Desktop pointers are one per mouse button.
  function press(key, pointer, zone) {
    if (status !== "ready" || isDown(pointer)) return false
    const msg = { type: "press", generation: generation, widget: key, pointer: pointer }
    if (zone) msg.zone = zone
    const id = send(msg)
    if (!id) return false
    pending[id] = { key: key, pointer: pointer }
    down[pointer] = key
    return true
  }

  function release(pointer) {
    if (!isDown(pointer)) return
    delete down[pointer]
    send({ type: "release", pointer: pointer })
  }

  function activate(key, zone) {
    if (status !== "ready") return
    const msg = { type: "activate", generation: generation, widget: key }
    if (zone) msg.zone = zone
    const id = send(msg)
    if (id) pending[id] = { key: key }
  }

  function sendView() {
    if (synced) send({ type: "view", layer: layer, shown: shown })
  }
  onLayerChanged: sendView()
  onShownChanged: sendView()

  function receive(line) {
    let msg
    try { msg = JSON.parse(line) } catch (e) { console.warn("glance: bad line from backend:", e); return }
    switch (msg.type) {
    case "snapshot": applySnapshot(msg); break
    case "update": applyUpdate(msg); break
    case "welcome": session = msg.session; break
    case "ack": delete pending[msg.id]; break
    case "error": applyError(msg); break
    }
  }

  function applySnapshot(msg) {
    const s = {}
    for (const w of msg.widgets) s[w.key] = w.state
    states = s
    rev = msg.rev
    if (msg.config.generation !== generation) down = ({}) // the backend ended presses against the old config
    generation = msg.config.generation
    configPath = msg.config.path
    configError = msg.config.error ?? ""
    settings = msg.settings
    widgets = msg.widgets
    const first = !synced
    synced = true
    resyncing = false
    fatalError = ""
    retryDelay = retryMin
    offline.stop()
    status = "ready"
    if (first) sendView()
  }

  function applyUpdate(msg) {
    if (!synced || resyncing) return
    if (msg.rev !== rev + 1) { // missed something: ignore updates until the new snapshot
      resyncing = true
      send({ type: "resync" })
      return
    }
    rev = msg.rev
    for (const key in msg.widgets) {
      states[key] = msg.widgets[key]
      widgetChanged(key, msg.widgets[key])
    }
  }

  function applyError(msg) {
    const req = msg.id !== undefined ? pending[msg.id] : undefined
    if (msg.id !== undefined) delete pending[msg.id]
    if (req) {
      if (req.pointer !== undefined && down[req.pointer] === req.key) delete down[req.pointer] // never held
      requestFailed(req.key, msg.code, msg.message)
    } else if (msg.code === "unknown_pointer") {
      return // a release that crossed its refused press
    } else if (["too_large", "unsupported_protocol", "unsupported_output", "queue_overflow"].includes(msg.code)) {
      fatalError = msg.code + ": " + msg.message // the backend closes the connection next
    }
    console.warn("glance: backend error", msg.code, msg.message)
  }

  function lost(sock) {
    if (sock !== socket) return // a replaced socket's late signals
    socket = null
    sock.destroy()
    synced = false
    resyncing = false
    pending = ({})
    down = ({}) // the backend releases every pointer of a closed session
    if (status === "ready") offline.restart()
    retry.interval = retryDelay
    retryDelay = Math.min(retryDelay * 2, retryMax)
    retry.restart()
  }

  // A fresh Socket per attempt: one that failed to connect never retries.
  property var socket: null
  function connect() {
    socket = socketComponent.createObject(client, { path: path })
    socket.connected = true // only now, so its signals find it in `socket`
  }
  Component.onCompleted: connect()

  Component {
    id: socketComponent
    Socket {
      id: sock
      parser: SplitParser { onRead: line => { if (sock === client.socket) client.receive(line) } }
      onConnectedChanged: {
        if (connected) {
          client.nextId = 1
          client.send({ type: "hello", protocol: 1, output: client.output, client: client.clientName })
        } else {
          client.lost(sock)
        }
      }
      onError: if (!connected) client.lost(sock) // a failed connect doesn't change `connected`
    }
  }

  Timer {
    id: retry
    onTriggered: client.connect()
  }

  // The disconnected state appears only if no snapshot came back in time,
  // so a quick backend restart doesn't flash it.
  Timer {
    id: offline
    interval: client.offlineAfter
    onTriggered: {
      client.widgets = []
      client.states = ({})
      client.session = ""
      client.status = "offline"
    }
  }
  Timer {
    // the first connection: offline if the backend can't be reached at all
    running: true
    interval: client.offlineAfter
    onTriggered: if (client.status === "connecting") client.status = "offline"
  }
}
