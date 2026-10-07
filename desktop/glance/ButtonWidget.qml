// `type: button` with `exec`: an icon or label on a key face. Held with
// `repeat`, the backend repeats it until the mouse button is released.
import QtQuick

Face {
  id: root
  required property var view
  readonly property var o: view.options
  readonly property bool hasIcon: !!o.icon

  host: view.host
  pressed: view.pressedZone !== ""
  hovered: view.hovered
  implicitWidth: o.width !== undefined ? view.px(o.width) : Math.max(height, content.implicitWidth + 2 * view.px(12))

  Text {
    id: content
    anchors.centerIn: parent
    text: root.hasIcon ? String(root.o.icon) : String(root.o.label ?? root.view.widget.id)
    color: root.host.foreground
    font.family: root.host.fontFamily
    font.pixelSize: root.hasIcon ? root.view.px(root.view.opt("iconSize", root.host.fontSize * 1.6 / root.host.scale))
                                 : root.view.px(root.view.opt("fontSize", root.host.fontSize / root.host.scale))
  }
}
