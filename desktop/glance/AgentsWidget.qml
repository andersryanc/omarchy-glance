// glance.agents: the provider's logo, then its usage limits: side by side
// with the name and reset time above each meter ("row"), or one line per
// limit ("stacked"). Meters turn urgent at 90%. Reset times are formatted
// here from the local clock and redrawn each minute.
import QtQuick
import "util.js" as Util

Face {
  id: root
  required property var view
  readonly property var o: view.options
  readonly property var limits: Util.limits(view.current)
  readonly property bool stacked: o.layout === "stacked"
  readonly property real pad: view.px(12)
  readonly property real gap: view.px(10)
  readonly property real meterW: view.px(view.opt("meterWidth", stacked ? 90 : 170))
  readonly property real iconSize: Math.min(height - view.px(12), view.px(24))
  readonly property real small: host.fontSize * 0.9
  readonly property string agent: String(o.agent ?? "claude")
  // As Omarchy's agents panel picks them: <agent>-light.svg first on a light
  // surface (white marks have a dark twin), then <agent>.svg, then a glyph.
  readonly property var iconUrls: {
    if (!host.agentIcons) return []
    const c = host.background
    const lum = ch => (ch <= 0.03928 ? ch / 12.92 : Math.pow((ch + 0.055) / 1.055, 2.4))
    const light = 0.2126 * lum(c.r) + 0.7152 * lum(c.g) + 0.0722 * lum(c.b) >= 0.5
    return (light ? [host.agentIcons + agent + "-light.svg"] : []).concat([host.agentIcons + agent + ".svg"])
  }
  property int iconTry: 0
  onIconUrlsChanged: iconTry = 0
  property date now: new Date()

  host: view.host
  pressed: view.pressedZone !== ""
  hovered: view.hovered && view.pressable
  implicitWidth: pad + iconSize + gap + (stacked ? stackedBlock.implicitWidth : rowBlock.implicitWidth) + pad

  // Redraw reset times on the minute.
  Timer {
    running: true
    interval: (60 - new Date().getSeconds()) * 1000
    onTriggered: { root.now = new Date(); interval = 60000; restart() }
  }

  function alarm(l) { return l.fraction !== null && l.fraction !== undefined && l.fraction >= 0.9 }

  Image {
    id: logo
    x: root.pad
    anchors.verticalCenter: parent.verticalCenter
    width: root.iconSize; height: root.iconSize
    sourceSize: Qt.size(width * 2, height * 2)
    source: root.iconUrls[root.iconTry] ?? ""
    visible: status === Image.Ready
    onStatusChanged: if (status === Image.Error) root.iconTry++ // try the next candidate
  }
  Text {
    x: root.pad
    anchors.verticalCenter: parent.verticalCenter
    visible: !logo.visible
    text: "\u{F16A3}" // the Omarchy bar's agents glyph
    color: root.host.foreground
    font.family: root.host.fontFamily
    font.pixelSize: root.iconSize
  }

  // "row": per limit, its name and reset time over a meter, the percent at the right.
  Row {
    id: rowBlock
    visible: !root.stacked
    x: root.pad + root.iconSize + root.gap
    anchors.verticalCenter: parent.verticalCenter
    spacing: root.view.px(18)
    Repeater {
      model: root.stacked ? [] : root.limits
      Column {
        id: lim
        required property var modelData
        width: root.meterW
        spacing: root.view.px(5)
        Item {
          width: parent.width
          height: pct.implicitHeight
          Row {
            width: parent.width - pct.implicitWidth - root.view.px(6)
            clip: true
            spacing: root.view.px(6)
            Text {
              text: lim.modelData.label.split(" (")[0]
              color: root.host.foreground
              font.family: root.host.fontFamily
              font.pixelSize: root.small
            }
            Text {
              text: Util.resetText(root.o, lim.modelData.resetsAt, root.now)
              color: root.host.muted
              font.family: root.host.fontFamily
              font.pixelSize: root.small
            }
          }
          Text {
            id: pct
            anchors.right: parent.right
            text: Util.percent(lim.modelData.fraction)
            color: root.alarm(lim.modelData) ? root.host.urgent : root.host.foreground
            font.family: root.host.fontFamily
            font.pixelSize: root.small
          }
        }
        Meter { host: root.host; width: parent.width; fraction: lim.modelData.fraction; urgent: root.alarm(lim.modelData) }
      }
    }
  }

  // "stacked": a line per limit: short label, meter, percent, reset time.
  TextMetrics { id: pctW; font.family: root.host.fontFamily; font.pixelSize: root.small; text: "100%" }
  TextMetrics { id: resetW; font.family: root.host.fontFamily; font.pixelSize: root.small; text: Util.resetWidest(root.o) }
  TextMetrics {
    id: labelW
    font.family: root.host.fontFamily; font.pixelSize: root.small
    text: root.limits.map(l => Util.shortLabel(root.o, l.label)).reduce((a, b) => (a.length >= b.length ? a : b), "")
  }
  Column {
    id: stackedBlock
    visible: root.stacked
    x: root.pad + root.iconSize + root.gap
    anchors.verticalCenter: parent.verticalCenter
    spacing: 0
    Repeater {
      model: root.stacked ? root.limits : []
      Row {
        id: line
        required property var modelData
        spacing: root.view.px(8)
        height: Math.ceil(root.small * 1.15) // tighter than the font's line box, so the lines sit closer
        Text {
          width: labelW.advanceWidth
          text: Util.shortLabel(root.o, line.modelData.label)
          color: root.host.foreground
          font.family: root.host.fontFamily; font.pixelSize: root.small
          height: parent.height; verticalAlignment: Text.AlignVCenter
        }
        Meter {
          host: root.host
          width: root.meterW
          anchors.verticalCenter: parent.verticalCenter
          fraction: line.modelData.fraction
          urgent: root.alarm(line.modelData)
        }
        Text {
          width: pctW.advanceWidth
          horizontalAlignment: Text.AlignRight
          text: Util.percent(line.modelData.fraction)
          color: root.alarm(line.modelData) ? root.host.urgent : root.host.foreground
          font.family: root.host.fontFamily; font.pixelSize: root.small
          height: parent.height; verticalAlignment: Text.AlignVCenter
        }
        Text {
          visible: resetW.text !== ""
          width: resetW.advanceWidth
          text: Util.resetText(root.o, line.modelData.resetsAt, root.now)
          color: root.host.muted
          font.family: root.host.fontFamily; font.pixelSize: root.small
          height: parent.height; verticalAlignment: Text.AlignVCenter
        }
      }
    }
  }
}
