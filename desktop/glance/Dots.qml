// btop-style dot columns, one per value (0-1), newest on the right, as the
// Touch Bar draws them. `half` 0 fills the height from the bottom; 1 and -1
// use the top half growing up from the middle, or the bottom half growing
// down (network and disk, the mic waveform).
import QtQuick
import "util.js" as Util

Canvas {
  id: dots

  property var values: []
  property int columns: 25
  property var stops: ["#ffffff"]
  property int half: 0
  property real spacing: 4
  property real radius: spacing * 0.32
  property bool bars: false
  property bool grid: true
  property color gridColor: "#1fffffff"

  implicitWidth: columns * spacing
  onValuesChanged: requestPaint()
  onStopsChanged: requestPaint()
  onGridColorChanged: requestPaint()
  onHeightChanged: requestPaint()
  onWidthChanged: requestPaint()

  onPaint: {
    const ctx = getContext("2d")
    ctx.reset()
    const sp = spacing
    let rows = Math.max(2, Math.floor((height - 2 * radius) / sp) + 1)
    const mid = height / 2
    let base, step
    if (half !== 0) {
      rows = Math.max(1, Math.floor(rows / 2))
      base = mid - half * sp / 2
      step = -half * sp
    } else {
      base = mid + (rows - 1) * sp / 2
      step = -sp
    }
    const vals = values.slice(Math.max(0, values.length - columns))
    const pad = columns - vals.length
    const rowColors = []
    for (let r = 0; r < rows; r++) rowColors.push(Util.lerp(stops, rows > 1 ? r / (rows - 1) : 0))
    const lit = [] // per row: x centres
    for (let r = 0; r < rows; r++) lit.push([])
    ctx.fillStyle = gridColor
    ctx.beginPath()
    for (let col = 0; col < columns; col++) {
      const v = col < pad ? null : vals[col - pad]
      const cx = col * sp + sp / 2
      let n = 0
      if (v !== null && v !== undefined) {
        n = Util.roundEven(v * rows)
        if (n === 0 && v > 0.01) n = 1
        n = Math.max(0, Math.min(rows, n))
      }
      if (bars) {
        if (n > 0) {
          const y0 = base - step / 2, y1 = base + (n - 0.5) * step
          const g = ctx.createLinearGradient(0, y0, 0, base + (rows - 0.5) * step)
          stops.forEach((c, i) => g.addColorStop(i / Math.max(1, stops.length - 1), c))
          ctx.save()
          ctx.fillStyle = g
          ctx.fillRect(cx - sp / 2 + 0.5, Math.min(y0, y1), sp - 1, Math.abs(y1 - y0))
          ctx.restore()
        }
        continue
      }
      for (let r = 0; r < rows; r++) {
        if (r < n) {
          lit[r].push(cx)
        } else if (grid) {
          const cy = base + r * step
          ctx.moveTo(cx + radius, cy)
          ctx.arc(cx, cy, radius, 0, 2 * Math.PI)
        }
      }
    }
    if (!bars) ctx.fill()
    for (let r = 0; r < rows; r++) {
      if (lit[r].length === 0) continue
      ctx.fillStyle = rowColors[r]
      ctx.beginPath()
      const cy = base + r * step
      for (const cx of lit[r]) {
        ctx.moveTo(cx + radius, cy)
        ctx.arc(cx, cy, radius, 0, 2 * Math.PI)
      }
      ctx.fill()
    }
  }
}
