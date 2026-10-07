// The Omarchy panel plugin host: one glance row under the bar on each
// screen. keepLoaded mounts it at shell startup once enabled. The theme comes
// from the shell's Color and Style singletons, bar state from the
// PluginBarStateApi facade (docs/desktop-client.md).
import QtQuick
import Quickshell
import Quickshell.Hyprland
import Quickshell.Io
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

  readonly property int rowHeight: glanceClient.settings.height || Math.round(1.5 * Style.bar.sizeHorizontal) // 1.5 times the bar, unless desktop.json says

  // The bar's transparency isn't in the plugin facade: read it from
  // shell.json as the bar does, and pick legible text the way the bar does,
  // with omarchy-bar-text-color over the strip both rows cover.
  property bool transparent: false
  property string transparentText: ""
  FileView {
    path: Quickshell.env("HOME") + "/.config/omarchy/shell.json"
    watchChanges: true
    printErrors: false
    onFileChanged: reload()
    onLoaded: {
      try {
        const bar = JSON.parse(text()).bar
        root.transparent = !!bar && bar.transparent === true
      } catch (e) {
        // keep the last value while the file is mid-edit
      }
    }
    onLoadFailed: root.transparent = false
  }
  Process {
    id: textColor
    stdout: SplitParser {
      onRead: line => { if (/^#[0-9A-Fa-f]{6}$/.test(line.trim())) root.transparentText = line.trim() }
    }
  }
  Timer {
    id: textColorTimer
    interval: 120
    onTriggered: {
      if (!root.transparent) { root.transparentText = ""; return }
      const barSize = root.bar && root.bar.barSize > 0 ? root.bar.barSize : Style.bar.sizeHorizontal
      textColor.command = ["omarchy-bar-text-color", "top", String(barSize + root.rowHeight),
                           String(Color.bar.text), String(Color.background)]
      textColor.running = true
    }
  }
  onTransparentChanged: textColorTimer.restart()
  onRowHeightChanged: textColorTimer.restart()
  Connections {
    target: Color.bar
    function onTextChanged() { textColorTimer.restart() }
  }

  // The live theme, with desktop.json's overrides on top.
  GlanceHost {
    id: env
    transparent: root.transparent
    background: glanceClient.color("background", Color.bar.background)
    foreground: glanceClient.color("foreground", root.transparent && root.transparentText ? root.transparentText : Color.bar.text)
    accent: glanceClient.color("accent", Color.accent)
    urgent: glanceClient.color("urgent", Color.urgent)
    muted: glanceClient.color("muted", Color.muted)
    fill: Style.normalFill
    hoverFill: Style.hoverFill
    pressedFill: Style.pressedFill
    btop: btop.colors
    agentIcons: "file:///usr/share/omarchy/shell/plugins/agents/assets/"
    fontFamily: glanceClient.font(root.bar && root.bar.fontFamily ? root.bar.fontFamily : Style.font.family)
    fontSize: Style.font.body
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
      implicitHeight: root.rowHeight
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
