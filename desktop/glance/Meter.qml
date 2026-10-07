// A usage meter: a rounded track, filled to the fraction, urgent at 90%.
import QtQuick

Item {
  id: meter
  required property GlanceHost host
  property var fraction: null
  property bool urgent: false
  height: 6 * host.scale
  Rectangle { anchors.fill: parent; radius: height / 2; color: Qt.alpha(meter.host.foreground, 0.2) }
  Rectangle {
    visible: meter.fraction !== null && meter.fraction !== undefined && meter.fraction > 0
    width: Math.max(meter.height, meter.width * Math.min(1, meter.fraction ?? 0))
    height: meter.height
    radius: height / 2
    color: meter.urgent ? meter.host.urgent : meter.host.foreground
  }
}
