// T06 spike: a hardcoded glance row as a standalone Quickshell layer-shell
// panel, beside omarchy-shell. Run: qs -p tools/spikes/desktop-host/standalone
import Quickshell
import Quickshell.Hyprland
import Quickshell.Io
import Quickshell.Wayland
import QtQuick

ShellRoot {
  id: root

  // The theme, read the way any outside program has to: omarchy-theme-set
  // replaces ~/.local/state/omarchy/current/theme/ and then rewrites
  // theme.name, so watch that file and re-read colors.toml.
  readonly property string themeDir: Quickshell.env("HOME") + "/.local/state/omarchy/current"
  property color foreground: "#cacccc"
  property color background: "#101315"
  property color accent: "#cacccc"
  property int themeReloads: 0
  property int clicks: 0
  property int remaps: 0
  property bool mapped: true

  // Hyprland stacks exclusive zones in map order, so when omarchy-shell
  // restarts its bar maps after us and lands below the row. Remap ourselves
  // after the bar appears to get back under it.
  Connections {
    target: Hyprland
    function onRawEvent(event) {
      if (event.name === "openlayer" && event.data === "omarchy-bar") remap.restart()
    }
  }
  Timer {
    id: remap
    interval: 50
    onTriggered: { root.mapped = false; root.remaps++; show.restart() }
  }
  Timer { id: show; interval: 50; onTriggered: root.mapped = true }

  function parseColors(text) {
    var out = {}
    for (const line of text.split("\n")) {
      const m = line.match(/^\s*([a-z_]+)\s*=\s*"(#[0-9a-fA-F]{6})"/)
      if (m) out[m[1]] = m[2]
    }
    if (out.foreground) root.foreground = out.foreground
    if (out.background) root.background = out.background
    if (out.accent) root.accent = out.accent
    root.themeReloads++
  }

  FileView {
    path: root.themeDir + "/theme.name"
    watchChanges: true
    onFileChanged: { reload(); colors.reload() }
  }
  FileView {
    id: colors
    path: root.themeDir + "/theme/colors.toml"
    onLoaded: root.parseColors(text())
  }

  Variants {
    model: Quickshell.screens

    PanelWindow {
      required property var modelData
      screen: modelData
      visible: root.mapped
      anchors { top: true; left: true; right: true }
      implicitHeight: 30
      exclusionMode: ExclusionMode.Auto
      WlrLayershell.namespace: "omarchy-glance-spike-standalone"
      WlrLayershell.layer: WlrLayer.Top
      WlrLayershell.keyboardFocus: WlrKeyboardFocus.None
      color: root.background

      Text {
        anchors.centerIn: parent
        color: root.foreground
        font.family: "monospace"
        font.pixelSize: 13
        text: "glance spike: standalone · clicks " + root.clicks + " · theme reloads " + root.themeReloads + " · remaps " + root.remaps
              + " · scale " + modelData.devicePixelRatio
      }
      Rectangle {
        anchors { right: parent.right; top: parent.top; bottom: parent.bottom; margins: 4 }
        width: 90
        radius: 4
        color: area.pressed ? root.accent : Qt.darker(root.accent, 2.5)
        Text { anchors.centerIn: parent; text: "click me"; color: root.foreground; font.family: "monospace" }
        MouseArea { id: area; anchors.fill: parent; onClicked: root.clicks++ }
      }
    }
  }
}
