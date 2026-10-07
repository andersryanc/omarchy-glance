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

  // One session for every screen's row.
  GlanceClient { id: glanceClient }
  BtopTheme { id: btop }

  // The live theme, with desktop.json's overrides on top.
  GlanceHost {
    id: env
    background: glanceClient.color("background", Color.bar.background)
    foreground: glanceClient.color("foreground", Color.bar.text)
    accent: glanceClient.color("accent", Color.accent)
    urgent: glanceClient.color("urgent", Color.urgent)
    muted: glanceClient.color("muted", Color.muted)
    fill: Style.normalFill
    hoverFill: Style.hoverFill
    pressedFill: Style.pressedFill
    border: Style.normalBorderColor
    btop: btop.colors
    agentIcons: "file:///usr/share/omarchy/shell/plugins/agents/assets/"
    fontFamily: glanceClient.font(root.bar && root.bar.fontFamily ? root.bar.fontFamily : Style.font.family)
    fontSize: Style.font.subtitle
  }

  // desktop.json's "monitor": the named screens, or every screen.
  readonly property var screens: {
    const names = glanceClient.settings.monitors ?? []
    return names.length === 0 ? Quickshell.screens : Quickshell.screens.filter(s => names.includes(s.name))
  }

  // Hyprland stacks exclusive zones in map order, so the row must map after
  // the bar. A scale or monitor change recreates the bar's windows: hide the
  // row as soon as the bar closes, and once the bar is back, map the row
  // again when layers and monitors have been quiet for a moment, fading its
  // content in, so the change settles before the row returns.
  property bool mapped: true
  property bool barOpen: true
  readonly property var settleEvents: ["openlayer", "closelayer", "configreloaded", "monitoradded", "monitorremoved", "monitoraddedv2", "monitorremovedv2"]
  Connections {
    target: Hyprland
    function onRawEvent(event) {
      if (event.data === "omarchy-bar" && event.name === "closelayer") {
        root.barOpen = false
        root.mapped = false
        fallback.restart()
      } else if (event.data === "omarchy-bar" && event.name === "openlayer") {
        root.barOpen = true
        root.mapped = false // a no-op unless the bar opened while the row was up
        settle.restart()
      } else if (!root.mapped && root.barOpen && root.settleEvents.includes(event.name)) {
        settle.restart()
      }
    }
  }
  Timer { id: settle; interval: 250; onTriggered: { fallback.stop(); root.mapped = true } }
  Timer { id: fallback; interval: 2000; onTriggered: root.mapped = true } // the bar didn't come back

  Variants {
    model: root.screens

    PanelWindow {
      required property var modelData
      screen: modelData
      visible: root.mapped
      // The first release is a top row only; it stays at the top when the
      // bar moves to another edge or hides.
      anchors { top: true; left: true; right: true }
      implicitHeight: glanceClient.settings.height || 2 * Style.bar.sizeHorizontal // twice the bar, unless desktop.json says
      exclusionMode: ExclusionMode.Auto
      WlrLayershell.namespace: "omarchy-glance"
      WlrLayershell.layer: WlrLayer.Top
      WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
      color: "transparent"

      onVisibleChanged: if (visible) fadeIn.restart()
      NumberAnimation { id: fadeIn; target: row; property: "opacity"; from: 0; to: 1; duration: 150 }

      GlanceRow {
        id: row
        anchors.fill: parent
        host: env
        client: glanceClient
      }
    }
  }
}
