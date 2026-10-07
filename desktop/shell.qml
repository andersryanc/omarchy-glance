// Development host: the shared glance row in a plain floating window, with
// controls for the host environment, so controls can be worked on without
// Omarchy's shell. Run: qs -p desktop
// GLANCE_OUTPUT picks the output (default touchbar until desktop.json exists,
// T08); GLANCE_SOCKET overrides the backend socket.
import Quickshell
import QtQuick
import "glance"

ShellRoot {
  id: root

  readonly property var themes: [
    { name: "dark", background: "#101315", foreground: "#cacccc", accent: "#7aa2f7", urgent: "#a55555", muted: "#707880" },
    { name: "light", background: "#e1e2e7", foreground: "#3760bf", accent: "#2e7de9", urgent: "#f52a65", muted: "#8990b3" }
  ]
  readonly property var scales: [1, 1.25, 1.5, 2]
  property int theme: 0
  property int scaleIndex: 0

  GlanceHost {
    id: env
    background: root.themes[root.theme].background
    foreground: root.themes[root.theme].foreground
    accent: root.themes[root.theme].accent
    urgent: root.themes[root.theme].urgent
    muted: root.themes[root.theme].muted
    fontSize: 13 * scale
    scale: root.scales[root.scaleIndex]
  }

  FloatingWindow {
    id: window
    title: "omarchy-glance development host"
    implicitWidth: 1400
    implicitHeight: 120
    color: "#404040" // shows through a transparent row

    GlanceRow {
      id: glance
      host: env
      output: Quickshell.env("GLANCE_OUTPUT") || "touchbar"
      path: Quickshell.env("GLANCE_SOCKET") || glance.client.defaultPath
      anchors { left: parent.left; right: parent.right; top: parent.top }
      height: 30 * env.scale
      visible: env.shown
    }

    Column {
      anchors { left: parent.left; right: parent.right; bottom: parent.bottom; margins: 6 }
      spacing: 4

      Row {
        spacing: 6
        component Toggle: Rectangle {
          id: toggle
          property string text
          signal clicked
          width: label.implicitWidth + 16
          height: 24
          radius: 4
          color: area.pressed ? "#808080" : "#303030"
          Text { id: label; anchors.centerIn: parent; text: toggle.text; color: "white"; font.family: "monospace" }
          MouseArea { id: area; anchors.fill: parent; onClicked: toggle.clicked() }
        }
        Toggle { text: "theme: " + root.themes[root.theme].name; onClicked: root.theme = (root.theme + 1) % root.themes.length }
        Toggle { text: "scale: " + env.scale; onClicked: root.scaleIndex = (root.scaleIndex + 1) % root.scales.length }
        Toggle { text: "transparent: " + env.transparent; onClicked: env.transparent = !env.transparent }
        Toggle { text: "shown: " + env.shown; onClicked: env.shown = !env.shown }
      }
      Text {
        color: "white"
        font.family: "monospace"
        text: glance.output + " · " + glance.client.status + " · session " + glance.client.session + " · generation "
              + glance.client.generation + " · " + glance.client.widgets.length + " widgets"
              + (glance.client.configError ? " · config error: " + glance.client.configError : "")
      }
    }
  }
}
