// Headless screenshot of the row, for tools/desktop-shot.sh.
// Env: GLANCE_SOCKET, GLANCE_OUT (png), GLANCE_WIDTH, GLANCE_HEIGHT, GLANCE_SCALE.
import Quickshell
import QtQuick
import "glance"

ShellRoot {
  GlanceClient { id: glanceClient; path: Quickshell.env("GLANCE_SOCKET") }
  BtopTheme { id: btop }
  GlanceHost {
    id: env
    scale: Number(Quickshell.env("GLANCE_SCALE") || 1)
    background: glanceClient.color("background", "#101315")
    foreground: glanceClient.color("foreground", "#cacccc")
    accent: glanceClient.color("accent", "#cacccc")
    urgent: glanceClient.color("urgent", "#a55555")
    muted: glanceClient.color("muted", "#707880")
    fontSize: 13 * scale
    btop: btop.colors
    agentIcons: "file:///usr/share/omarchy/shell/plugins/agents/assets/"
  }
  FloatingWindow {
    implicitWidth: Number(Quickshell.env("GLANCE_WIDTH") || 1440)
    implicitHeight: Number(Quickshell.env("GLANCE_HEIGHT") || 39) * env.scale + 40
    color: "black"
    // Offscreen, the pointer sits near the window's corner; keep the row
    // clear of it so nothing shows as hovered.
    GlanceRow { id: glance; anchors { fill: parent; topMargin: 40 } host: env; client: glanceClient
                showErrorDetail: Quickshell.env("GLANCE_ERROR_DETAIL") === "1" }
  }
  Timer {
    interval: 3500
    running: true
    onTriggered: glance.grabToImage(r => {
      console.log("shot:", r.saveToFile(Quickshell.env("GLANCE_OUT")), "dropped", glance.dropped)
      Qt.quit()
    })
  }
}
