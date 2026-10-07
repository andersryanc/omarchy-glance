// The host environment: everything the shared controls take from the host
// that places them (docs/desktop-client.md). Controls read only this and the
// size of the item they're given; they never create a window, read theme
// files or ask the compositor. Each host fills it in: the Omarchy panel
// plugin from the shell's live theme and bar state, the development host
// from fixed values it can change at runtime.
import QtQuick

QtObject {
  // Palette, in the roles of omarchy-shell's Color and Style.
  property color background: "#101315"
  property color foreground: "#cacccc"
  property color accent: "#cacccc"
  property color urgent: "#a55555"
  property color muted: "#707880"
  property color fill: Qt.alpha(foreground, 0.04) // a control's face
  property color hoverFill: Qt.alpha(foreground, 0.08)
  property color pressedFill: Qt.alpha(foreground, 0.22)
  property color border: Qt.alpha(foreground, 0.4)

  // Graph gradients from btop's current theme, as name_start/_mid/_end ->
  // colour (BtopTheme.qml reads it); empty: the built-in fallbacks.
  property var btop: ({})

  // Directory URL of the agent provider logos (<agent>.svg); empty: a glyph.
  property string agentIcons: ""

  // Type: the family and the base size in logical pixels.
  property string fontFamily: "JetBrainsMono Nerd Font"
  property real fontSize: 13

  // A multiplier for sizes inside the row (padding, gaps, minimum widths) on
  // top of the screen's own device pixel ratio, which Qt already applies.
  property real scale: 1

  // Draw no background of our own; the host's surface shows through.
  property bool transparent: false

  // Whether the host shows the row. A hidden row keeps its connection but
  // tells the backend, which then stops providers only it needs.
  property bool shown: true
}
