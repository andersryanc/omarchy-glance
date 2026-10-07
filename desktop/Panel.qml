// The Omarchy panel plugin host: one glance row under the bar on each
// screen. keepLoaded mounts it at shell startup once enabled. The theme comes
// from the shell's Color and Style singletons, bar state from the
// PluginBarStateApi facade (docs/desktop-client.md).
import QtQuick
import Quickshell
import Quickshell.Hyprland
import Quickshell.Wayland
import qs.Commons
import "glance"

Item {
  id: root

  property var shell: null // injected: the third-party facade (shell.bar = PluginBarStateApi)
  property var manifest: null
  property string omarchyPath: ""
  readonly property var bar: shell ? shell.bar : null

  function open(payloadJson) {}
  function close() {}

  GlanceHost {
    id: env
    background: Color.bar.background
    foreground: Color.bar.text
    accent: Color.accent
    urgent: Color.urgent
    muted: Color.muted
    fill: Style.normalFill
    pressedFill: Style.pressedFill
    border: Style.normalBorderColor
    fontFamily: root.bar && root.bar.fontFamily ? root.bar.fontFamily : Style.font.family
    fontSize: Style.font.body
  }

  // Hyprland stacks exclusive zones in map order, so the row must map after
  // the bar. A scale or monitor change recreates the bar's windows: hide the
  // row as soon as the bar closes and map it again right after the bar
  // reopens, so the wrong order is never on screen.
  property bool mapped: true
  Connections {
    target: Hyprland
    function onRawEvent(event) {
      if (event.data !== "omarchy-bar") return
      if (event.name === "closelayer") {
        root.mapped = false
        fallback.restart()
      } else if (event.name === "openlayer") {
        root.mapped = false // a no-op unless the bar opened while the row was up
        show.restart()
      }
    }
  }
  Timer { id: show; interval: 16; onTriggered: { fallback.stop(); root.mapped = true } }
  Timer { id: fallback; interval: 2000; onTriggered: root.mapped = true } // the bar didn't come back

  Variants {
    model: Quickshell.screens

    PanelWindow {
      required property var modelData
      screen: modelData
      visible: root.mapped
      // The first release is a top row only; it stays at the top when the
      // bar moves to another edge or hides.
      anchors { top: true; left: true; right: true }
      implicitHeight: Style.bar.sizeHorizontal
      exclusionMode: ExclusionMode.Auto
      WlrLayershell.namespace: "omarchy-glance"
      WlrLayershell.layer: WlrLayer.Top
      WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
      color: "transparent"

      GlanceRow {
        anchors.fill: parent
        host: env
        output: "touchbar" // until the backend serves desktop.json (T08)
      }
    }
  }
}
