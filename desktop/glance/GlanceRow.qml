// The glance row: lays out the widgets of a GlanceClient's snapshot in left,
// center and right sections inside whatever item the host gives it. It never
// creates a window; the host sizes it and passes a GlanceHost and the
// client, which rows on several screens share.
pragma ComponentBehavior: Bound
import QtQuick

Item {
  id: row

  required property GlanceHost host
  required property GlanceClient client

  readonly property real gap: 4 * host.scale
  readonly property string offlineText: row.client.fatalError !== ""
    ? "omarchy-glance: " + row.client.fatalError
    : "omarchy-glance: backend unavailable, reconnecting…" // the Touch Bar's wording

  // Unsupported widgets and hidden agents take no space.
  function section(name) {
    return row.client.widgets.filter(w => w.layer === row.client.layer && w.section === name && w.supported)
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
    visible: row.client.status === "ready"
    Section { name: "left"; anchors.left: parent.left }
    Section { name: "center"; anchors.horizontalCenter: parent.horizontalCenter }
    Section { name: "right"; anchors.right: parent.right }
  }

  Text {
    anchors.centerIn: parent
    visible: row.client.status === "offline"
    text: row.offlineText
    color: row.host.muted
    font.family: row.host.fontFamily
    font.pixelSize: row.host.fontSize
  }
}
