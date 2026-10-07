// T06 spike: a hardcoded glance row as an Omarchy third-party panel plugin.
// keepLoaded mounts it at shell startup once enabled. Theme comes from the
// shell's own Color singleton; bar state from the PluginBarStateApi facade.
import QtQuick
import Quickshell
import Quickshell.Wayland
import qs.Commons

Item {
  id: root

  property var shell: null // injected: third-party facade (shell.bar = PluginBarStateApi)
  property var manifest: null
  property string omarchyPath: ""
  property int clicks: 0
  readonly property var bar: shell ? shell.bar : null

  function open(payloadJson) {}
  function close() {}

  Variants {
    model: Quickshell.screens

    PanelWindow {
      required property var modelData
      screen: modelData
      anchors { top: true; left: true; right: true }
      implicitHeight: 30
      exclusionMode: ExclusionMode.Auto
      WlrLayershell.namespace: "omarchy-glance-spike-plugin"
      WlrLayershell.layer: WlrLayer.Top
      WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
      color: Color.background

      Text {
        anchors.centerIn: parent
        color: Color.foreground
        font.family: root.bar && root.bar.fontFamily ? root.bar.fontFamily : "monospace"
        font.pixelSize: 13
        text: "glance spike: plugin · clicks " + root.clicks
              + " · bar " + (root.bar ? root.bar.position + " " + root.bar.barSize + (root.bar.barHidden ? " hidden" : "") : "?")
              + " · scale " + modelData.devicePixelRatio
      }
      Rectangle {
        anchors { right: parent.right; top: parent.top; bottom: parent.bottom; margins: 4 }
        width: 90
        radius: 4
        color: area.pressed ? Color.accent : Qt.darker(Color.accent, 2.5)
        Text { anchors.centerIn: parent; text: "click me"; color: Color.foreground; font.family: "monospace" }
        MouseArea { id: area; anchors.fill: parent; onClicked: root.clicks++ }
      }
    }
  }
}
