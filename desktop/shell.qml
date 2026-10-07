// Development host: the shared glance row in a plain floating window, with
// controls for the host environment, so controls can be worked on without
// Omarchy's shell. Run: qs -p desktop
// GLANCE_OUTPUT picks the output (default desktop; touchbar shows that
// config); GLANCE_SOCKET overrides the backend socket.
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

  GlanceClient {
    id: glanceClient
    output: Quickshell.env("GLANCE_OUTPUT") || "desktop"
    path: Quickshell.env("GLANCE_SOCKET") || defaultPath
    shown: env.shown
  }

  BtopTheme { id: btop }

  GlanceHost {
    id: env
    btop: btop.colors
    agentIcons: "file:///usr/share/omarchy/shell/plugins/agents/assets/"
    background: glanceClient.color("background", root.themes[root.theme].background)
    foreground: glanceClient.color("foreground", root.themes[root.theme].foreground)
    accent: glanceClient.color("accent", root.themes[root.theme].accent)
    urgent: glanceClient.color("urgent", root.themes[root.theme].urgent)
    muted: glanceClient.color("muted", root.themes[root.theme].muted)
    fontFamily: glanceClient.font("JetBrainsMono Nerd Font")
    fontSize: 13 * scale
    scale: root.scales[root.scaleIndex]
  }

  FloatingWindow {
    id: window
    title: "omarchy-glance development host"
    implicitWidth: 1400
    implicitHeight: 140
    color: "#404040" // shows through a transparent row

    GlanceRow {
      id: glance
      host: env
      client: glanceClient
      anchors { left: parent.left; right: parent.right; top: parent.top }
      height: (glanceClient.settings.height || 52) * env.scale
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
        text: glanceClient.output + " · " + glanceClient.status + " · session " + glanceClient.session + " · generation "
              + glanceClient.generation + " · " + glanceClient.widgets.length + " widgets"
              + (glanceClient.configError ? " · config error: " + glanceClient.configError : "")
      }
    }
  }
}
