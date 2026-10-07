// One widget as a plain labelled control: enough to see its live state and
// press it. T09 replaces this with the real desktop widgets.
import QtQuick

Rectangle {
  id: view

  required property var widget // the snapshot entry: key, id, kind, options, action
  required property GlanceClient client
  required property GlanceHost host
  property var current: client.stateOf(widget.key)
  property bool failed: false

  readonly property var options: widget.options ?? {}
  readonly property var action: widget.action
  readonly property bool pressable: !!action && (action.press === true || !!action.zones)
  readonly property real pad: 8 * host.scale

  implicitWidth: widget.kind === "spacer" ? (options.size ?? 10) * host.scale : label.implicitWidth + 2 * pad
  radius: 4 * host.scale
  color: widget.kind === "spacer" ? "transparent" : area.pressed && pressable ? host.pressedFill : host.fill
  border.width: failed ? 1 : 0
  border.color: host.urgent

  function summary() {
    const s = current
    switch (widget.kind) {
    case "button": return options.label ?? options.icon ?? widget.id
    case "command": return s.text ?? "…"
    case "graph": return (options.label ? options.label + " " : "") + (s.lines ?? []).map(l => l[0]).join(" ")
    case "agents": return (s.limits ?? []).map(l => l.label + " " + (l.fraction === null ? "–" : Math.round(l.fraction * 100) + "%")).join("  ")
    case "mic": return s.muted ? "mic muted" : s.inUse ? "mic live" : "mic"
    case "media": return s.hasMedia ? (s.playing ? "▶ " : "⏸ ") + s.title : "no media"
    default: return ""
    }
  }

  Text {
    id: label
    anchors.centerIn: parent
    text: view.summary()
    color: view.current.urgent ? view.host.urgent : view.host.foreground
    font.family: view.host.fontFamily
    font.pixelSize: view.host.fontSize
  }

  Connections {
    target: view.client
    function onWidgetChanged(key, state) { if (key === view.widget.key) view.current = state }
    function onRequestFailed(key) { if (key === view.widget.key) { view.failed = true; failedTimer.restart() } }
  }
  Timer { id: failedTimer; interval: 600; onTriggered: view.failed = false }

  MouseArea {
    id: area
    anchors.fill: parent
    enabled: view.pressable
    acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
    // Media splits into its zones; the real control (T09) draws them.
    function zone(x) {
      const zones = view.action.zones
      return zones ? zones[Math.min(zones.length - 1, Math.floor(x / width * zones.length))] : undefined
    }
    onPressed: mouse => view.client.press(view.widget.key, mouse.button, zone(mouse.x))
    onReleased: mouse => view.client.release(mouse.button)
    onCanceled: { for (const b of [Qt.LeftButton, Qt.MiddleButton, Qt.RightButton]) view.client.release(b) }
  }
}
