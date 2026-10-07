// The glance row: connects to the backend and lays out the output's widgets
// in left, center and right sections inside whatever item the host gives it.
// It never creates a window; the host sizes it and passes a GlanceHost.
pragma ComponentBehavior: Bound
import QtQuick

Item {
  id: row

  required property GlanceHost host
  property alias output: client.output // "desktop"; the development host can pick "touchbar" to show that config
  property alias path: client.path // the backend socket; the default is the user service's
  readonly property alias client: client

  readonly property real gap: 4 * host.scale
  readonly property string offlineText: client.fatalError !== ""
    ? "omarchy-glance: " + client.fatalError
    : "omarchy-glance: backend unavailable, reconnecting…" // the Touch Bar's wording

  GlanceClient {
    id: client
    shown: row.host.shown
  }

  // Unsupported widgets and hidden agents take no space.
  function section(name) {
    return client.widgets.filter(w => w.layer === client.layer && w.section === name && w.supported)
  }

  Rectangle {
    anchors.fill: parent
    visible: !row.host.transparent
    color: row.host.background
  }

  component Section: Row {
    id: sec
    required property string name
    anchors.top: parent.top
    anchors.bottom: parent.bottom
    spacing: row.gap
    Repeater {
      model: row.section(sec.name)
      WidgetView {
        required property var modelData
        widget: modelData
        client: row.client
        host: row.host
        height: sec.height
        visible: widget.kind !== "agents" || current.visible !== false
      }
    }
  }

  Item {
    anchors.fill: parent
    anchors.margins: row.gap
    visible: client.status === "ready"
    Section { name: "left"; anchors.left: parent.left }
    Section { name: "center"; anchors.horizontalCenter: parent.horizontalCenter }
    Section { name: "right"; anchors.right: parent.right }
  }

  Text {
    anchors.centerIn: parent
    visible: client.status === "offline"
    text: row.offlineText
    color: row.host.muted
    font.family: row.host.fontFamily
    font.pixelSize: row.host.fontSize
  }
}
