// For hosts: btop's current theme as name -> colour, for GlanceHost.btop.
// Omarchy points ~/.config/btop/themes/current.theme at the theme's file, so
// it's re-read every few seconds rather than watched (as the Touch Bar does).
import QtQuick
import Quickshell
import Quickshell.Io

Scope {
  id: theme
  property var colors: ({})

  FileView {
    id: file
    path: Quickshell.env("HOME") + "/.config/btop/themes/current.theme"
    printErrors: false
    onLoaded: {
      const out = {}
      for (const line of text().split("\n")) {
        const m = line.match(/^theme\[([^\]]+)\]\s*=\s*"?([^"]*)"?/)
        if (m) out[m[1]] = m[2].trim()
      }
      if (JSON.stringify(out) !== JSON.stringify(theme.colors)) theme.colors = out
    }
    onLoadFailed: if (Object.keys(theme.colors).length > 0) theme.colors = ({})
  }
  Timer { interval: 3000; running: true; repeat: true; onTriggered: file.reload() }
}
