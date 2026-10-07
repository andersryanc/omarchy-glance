// `type: command`: its script's output on a key face, red while urgent. It
// sizes to the text unless `width` is set, and gives way (eliding) when the
// row is short of space.
import QtQuick

Face {
  id: root
  required property var view
  readonly property var o: view.options
  readonly property real pad: view.px(12)
  readonly property bool fixed: o.width !== undefined

  host: view.host
  pressed: view.pressedZone !== ""
  hovered: view.hovered && view.pressable
  implicitWidth: fixed ? view.px(o.width) : Math.max(height, metrics.advanceWidth + 2 * pad)
  readonly property real minWidth: fixed ? implicitWidth : Math.min(implicitWidth, Math.max(height, view.px(60)))

  TextMetrics { id: metrics; font: label.font; text: label.text }
  Text {
    id: label
    anchors { fill: parent; leftMargin: root.pad; rightMargin: root.pad }
    verticalAlignment: Text.AlignVCenter
    horizontalAlignment: Text.AlignHCenter
    elide: Text.ElideRight
    text: root.view.current.text ?? ""
    color: root.view.current.urgent ? root.host.urgent : root.host.foreground
    font.family: root.host.fontFamily
    font.pixelSize: root.view.px(root.view.opt("fontSize", root.host.fontSize / root.host.scale))
  }
}
