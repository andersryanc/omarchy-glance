// Headless check of the shared row against a backend, driven by
// tools/desktop-check.sh (which starts, stops and restarts the backend).
// Logs "check: …" lines and saves a screenshot after every status change.
// Env: GLANCE_SOCKET, GLANCE_OUTPUT, GLANCE_SHOTS (a directory).
import Quickshell
import QtQuick
import "glance"

ShellRoot {
  id: root
  property int shots: 0
  property bool exercised: false
  readonly property var client: glance.client

  function log(...args) { console.log("check:", ...args) }

  GlanceHost { id: env }

  FloatingWindow {
    implicitWidth: 1200
    implicitHeight: 30
    color: "black"

    GlanceRow {
      id: glance
      anchors.fill: parent
      host: env
      output: Quickshell.env("GLANCE_OUTPUT") || "touchbar"
      path: Quickshell.env("GLANCE_SOCKET")
    }
  }

  Connections {
    target: root.client
    function onStatusChanged() {
      root.log("status", root.client.status, "session", root.client.session, "widgets", root.client.widgets.length)
      shot.restart()
      if (!root.exercised && root.client.status === "ready") { root.exercised = true; exercise.start() }
    }
    function onWidgetsChanged() {
      if (root.client.widgets.length === 0) return
      root.log("snapshot rev", root.client.rev, "generation", root.client.generation,
               root.client.widgets.map(w => w.key + "=" + w.kind).join(" "))
    }
    function onWidgetChanged(key, state) { if (state.text !== undefined) root.log("text", key, state.text) }
    function onRequestFailed(key, code) { root.log("failed", key, code) }
  }

  // Once per run: a missed update must resync, a press must run the
  // button's action and release cleanly, and hiding the row must be accepted.
  SequentialAnimation {
    id: exercise
    PauseAnimation { duration: 300 }
    ScriptAction { script: { root.log("forcing a gap"); root.client.rev -= 1 } }
    PauseAnimation { duration: 1500 }
    ScriptAction {
      script: {
        const button = root.client.widgets.find(w => w.kind === "button" && w.action && w.action.press)
        root.log("press", button.key, root.client.press(button.key, Qt.LeftButton))
        root.client.release(Qt.LeftButton)
        root.client.press("default.left.99", Qt.RightButton) // no such widget: the backend refuses it
        root.client.release(Qt.RightButton)
        env.shown = false // a hidden row tells the backend through view
      }
    }
    PauseAnimation { duration: 200 }
    ScriptAction { script: { env.shown = true; root.log("view toggled") } }
  }

  Timer {
    id: shot
    interval: 400
    onTriggered: {
      const name = Quickshell.env("GLANCE_SHOTS") + "/" + (++root.shots) + "-" + root.client.status + ".png"
      glance.grabToImage(r => root.log("shot", name, r.saveToFile(name)))
    }
  }
}
