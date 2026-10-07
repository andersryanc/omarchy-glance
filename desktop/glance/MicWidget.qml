// glance.mic: muted (crossed out, muted colour) or live; while another app
// records, the icon turns the active colour and a live waveform grows out
// from the middle beside it. Clicking toggles mute.
import QtQuick

Face {
  id: root
  required property var view
  readonly property var o: view.options
  readonly property var st: view.current
  readonly property bool live: st.muted === false
  readonly property bool recording: live && !!st.inUse && (o.waveform === undefined || !!o.waveform)
  readonly property color active: o.activeColor ?? host.urgent
  readonly property real pad: view.px(12)
  readonly property real sp: Math.max(2, view.px(view.opt("dotSpacing", 4)))
  readonly property int columns: Math.max(1, Math.floor(view.opt("waveformWidth", 80) / view.opt("dotSpacing", 4)))

  host: view.host
  pressed: view.pressedZone !== ""
  hovered: view.hovered && view.pressable
  implicitWidth: recording ? pad + icon.implicitWidth + view.px(10) + columns * sp + pad
                           : (o.width !== undefined ? view.px(o.width) : Math.max(height, icon.implicitWidth + 2 * pad))

  Text {
    id: icon
    x: root.recording ? root.pad : (parent.width - implicitWidth) / 2
    anchors.verticalCenter: parent.verticalCenter
    text: root.live ? "\u{F036C}" : "\u{F036D}"
    color: !root.live ? (root.o.mutedColor ?? root.host.muted) : root.st.inUse ? root.active : root.host.foreground
    font.family: root.host.fontFamily
    font.pixelSize: root.view.px(root.view.opt("iconSize", root.host.fontSize * 1.6 / root.host.scale))
  }

  Repeater {
    model: root.recording ? [1, -1] : []
    Dots {
      required property int modelData
      x: icon.x + icon.implicitWidth + root.view.px(10)
      height: parent.height - 2 * root.view.px(6)
      anchors.verticalCenter: parent.verticalCenter
      columns: root.columns
      spacing: root.sp
      radius: root.view.px(root.view.opt("dotSize", root.view.opt("dotSpacing", 4) * 0.32))
      grid: false
      stops: [root.active]
      half: modelData
      values: root.st.levels ?? []
    }
  }
}
