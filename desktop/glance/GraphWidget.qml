// Graphs (glance.cpu, .memory, …): label, a btop-style dot graph of the
// history, per-core meters, and the current value, on a key face. A battery
// with `graph: "level"` shows a level meter instead of a history.
import QtQuick
import "util.js" as Util

Face {
  id: root
  required property var view
  readonly property var o: view.options
  readonly property var st: view.current
  readonly property string id_: view.widget.id
  readonly property real pad: view.px(10)
  readonly property real gap: view.px(8)
  readonly property real sp: Math.max(2, view.px(view.opt("dotSpacing", 4)))
  readonly property real dotRadius: view.px(view.opt("dotSize", view.opt("dotSpacing", 4) * 0.32))
  readonly property bool bars: o.style === "bars"
  readonly property bool grid: o.grid === undefined || !!o.grid
  readonly property int columns: Math.max(1, Math.floor(view.opt("graphWidth", 50) / view.opt("dotSpacing", 4)))
  readonly property var stops: Util.gradients(id_, o, host.btop)
  readonly property color gridColor: Qt.alpha(host.foreground, 0.12)
  readonly property real fontSize: view.px(view.opt("fontSize", host.fontSize / host.scale))
  readonly property var lines: (st.lines ?? []).length > 0 ? st.lines : [["—", false]]
  readonly property real valueSize: lines.length > 1 ? fontSize * 0.85 : fontSize
  readonly property bool showValue: o.showValue === undefined || !!o.showValue
  readonly property bool meter: Util.isMeter(id_, o)
  readonly property int cores: o.cores && Util.graphKind(id_) === "cpu" ? (st.cores ?? []).length : 0

  host: view.host
  pressed: view.pressedZone !== ""
  hovered: view.hovered && view.pressable
  implicitWidth: content.implicitWidth + 2 * pad

  TextMetrics { id: widest; font.family: root.host.fontFamily; font.pixelSize: root.valueSize
                text: Util.widestValues(root.id_, root.o).reduce((a, b) => (a.length >= b.length ? a : b)) }

  Row {
    id: content
    x: root.pad
    height: parent.height
    spacing: root.gap

    Text {
      text: Util.graphLabel(root.id_, root.o, root.st)
      visible: text !== ""
      anchors.verticalCenter: parent.verticalCenter
      color: root.host.foreground
      font.family: root.host.fontFamily
      font.pixelSize: root.fontSize
    }

    Item {
      width: root.columns * root.sp
      height: parent.height - 2 * root.view.px(6)
      anchors.verticalCenter: parent.verticalCenter

      // History: one Dots per series; network and disk mirror two halves.
      Repeater {
        model: root.meter ? 0 : (root.st.series ?? []).length
        Dots {
          required property int index
          anchors.fill: parent
          columns: root.columns
          spacing: root.sp
          radius: root.dotRadius
          bars: root.bars
          grid: root.grid && index === 0
          gridColor: root.gridColor
          stops: root.stops[Math.min(index, root.stops.length - 1)]
          half: Util.isMirrored(root.id_) ? (index === 0 ? 1 : -1) : 0
          values: {
            const scale = (root.st.scale ?? [])[index] || 1
            return root.st.series[index].map(v => v / scale)
          }
        }
      }
      // An empty grid until the first sample.
      Dots {
        anchors.fill: parent
        visible: !root.meter && (root.st.series ?? []).length === 0
        columns: root.columns; spacing: root.sp; radius: root.dotRadius; grid: root.grid; gridColor: root.gridColor
      }
      // Battery level: filled left to right in the gradient's colour for the level.
      Dots {
        anchors.fill: parent
        visible: root.meter
        readonly property var level: {
          const s = (root.st.series ?? [])[0]
          return s && s.length > 0 ? Math.min(1, Math.max(0, s[s.length - 1])) : null
        }
        columns: root.columns; spacing: root.sp; radius: root.dotRadius; grid: root.grid; gridColor: root.gridColor
        bars: root.bars
        stops: [level === null ? "#ffffff" : Util.lerp(root.stops[0], level)]
        values: {
          if (level === null) return []
          let lit = Util.roundEven(level * root.columns)
          if (lit === 0 && level > 0) lit = 1
          return Array.from({ length: root.columns }, (_, i) => (i < lit ? 1 : 0))
        }
      }
    }

    // Per-core meters: two dot columns each.
    Row {
      visible: root.cores > 0
      height: parent.height - 2 * root.view.px(6)
      anchors.verticalCenter: parent.verticalCenter
      spacing: root.sp
      Repeater {
        model: root.cores
        Dots {
          required property int index
          height: parent.height
          columns: 2; spacing: root.sp; radius: root.dotRadius; grid: root.grid; gridColor: root.gridColor
          stops: root.stops[0]
          values: { const f = root.st.cores[index]; return [f, f] }
        }
      }
    }

    // Values, right-aligned so the units stay put.
    Column {
      visible: root.showValue
      width: widest.advanceWidth
      anchors.verticalCenter: parent.verticalCenter
      Repeater {
        model: root.lines
        Text {
          required property var modelData
          width: parent.width
          horizontalAlignment: Text.AlignRight
          text: modelData[0]
          color: modelData[1] ? root.host.urgent : root.host.foreground
          font.family: root.host.fontFamily
          font.pixelSize: root.valueSize
        }
      }
    }
  }
}
