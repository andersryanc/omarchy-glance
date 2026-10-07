// One widget: picks its control by kind, maps mouse buttons to press and
// release (one pointer per button; the backend repeats held `repeat`
// buttons), and reports the widths the row lays it out with.
import QtQuick

Item {
  id: widgetView

  required property var widget // the snapshot entry: key, id, kind, options, action
  required property GlanceClient client
  required property GlanceHost host
  property var current: client.stateOf(widget.key)

  readonly property var options: widget.options ?? {}
  readonly property var action: widget.action
  readonly property bool pressable: !!action && (action.press === true || !!action.zones)
  // Hidden agents give up their space; the row skips widgets that aren't shown.
  readonly property bool shown: widget.kind !== "agents" || current.visible !== false
  readonly property real minWidth: control.item && control.item.minWidth !== undefined ? control.item.minWidth : implicitWidth
  property bool fits: true // set by the row's layout
  property string pressedZone: "" // "" none, "all" the whole widget, else a media zone
  property bool failed: false

  implicitWidth: control.item ? control.item.implicitWidth : 0
  visible: fits && shown

  // Option sizes are desktop pixels, times the host's scale.
  function px(v) { return v * host.scale }
  function opt(name, fallback) { return options[name] !== undefined && options[name] !== null ? options[name] : fallback }

  Connections {
    target: widgetView.client
    function onWidgetChanged(key, state) { if (key === widgetView.widget.key) widgetView.current = state }
    function onRequestFailed(key) { if (key === widgetView.widget.key) { widgetView.failed = true; failedTimer.restart() } }
  }
  Timer { id: failedTimer; interval: 600; onTriggered: widgetView.failed = false }

  Component { id: button; ButtonWidget { view: widgetView } }
  Component { id: command; CommandWidget { view: widgetView } }
  Component { id: graph; GraphWidget { view: widgetView } }
  Component { id: agents; AgentsWidget { view: widgetView } }
  Component { id: mic; MicWidget { view: widgetView } }
  Component { id: media; MediaWidget { view: widgetView } }
  Component { id: spacer; Item { implicitWidth: widgetView.px(widgetView.opt("size", 40)) } }

  Loader {
    id: control
    anchors.fill: parent
    sourceComponent: ({ button: button, command: command, graph: graph, agents: agents, mic: mic,
                        media: media, spacer: spacer })[widgetView.widget.kind] ?? null
  }

  // A refused press flashes the border.
  Rectangle {
    anchors.fill: parent
    visible: widgetView.failed
    color: "transparent"
    radius: 6 * widgetView.host.scale
    border.width: 2
    border.color: widgetView.host.urgent
  }

  MouseArea {
    id: area
    anchors.fill: parent
    enabled: widgetView.pressable
    hoverEnabled: true
    cursorShape: widgetView.pressable ? Qt.PointingHandCursor : Qt.ArrowCursor
    acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
    property int held: 0 // the button that pressed; others are ignored until it's released

    function zone(x) {
      return widgetView.action && widgetView.action.zones && control.item && control.item.zoneAt ? control.item.zoneAt(x) : ""
    }
    onPressed: mouse => {
      if (held) return
      const z = zone(mouse.x)
      if (widgetView.action.zones && !widgetView.action.zones.includes(z)) return // e.g. the title without an onTap
      if (!widgetView.client.press(widgetView.widget.key, mouse.button, z || undefined)) return
      held = mouse.button
      widgetView.pressedZone = z || "all"
    }
    onReleased: mouse => {
      if (mouse.button !== held) return
      widgetView.client.release(held)
      held = 0
      widgetView.pressedZone = ""
    }
    onCanceled: {
      if (held) widgetView.client.release(held)
      held = 0
      widgetView.pressedZone = ""
    }
  }
  readonly property bool hovered: area.containsMouse
  readonly property real hoverX: area.mouseX
}
