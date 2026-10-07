// A key face: the control's fill with no border, lighter while hovered,
// the pressed fill while held. Press feedback is local and doesn't wait for the backend.
import QtQuick

Rectangle {
  required property GlanceHost host
  property bool pressed: false
  property bool hovered: false

  radius: 6 * host.scale
  color: pressed ? host.pressedFill : hovered ? host.hoverFill : host.fill
}
