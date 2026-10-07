// The glance row: lays out the widgets of a GlanceClient's snapshot in left,
// center and right sections inside whatever item the host gives it. It never
// creates a window; the host sizes it and passes a GlanceHost and the
// client, which rows on several screens share.
//
// Layout: left packs from the left edge, right from the right edge, center
// sits in the middle (pushed aside rather than overlapping the others). When
// the widgets don't fit, text that can give way does first (media titles and
// commands without a `width` elide down to a minimum); then whole widgets
// are left out, center from its end, then left from its end, then right
// from its start, so the outermost widgets stay.
pragma ComponentBehavior: Bound
import QtQuick

Item {
  id: row

  required property GlanceHost host
  required property GlanceClient client

  readonly property real gap: 4 * host.scale
  readonly property real sectionGap: 16 * host.scale
  readonly property string offlineText: row.client.fatalError !== ""
    ? "omarchy-glance: " + row.client.fatalError
    : "omarchy-glance: backend unavailable, reconnecting…" // the Touch Bar's wording

  // Unsupported widgets take no space.
  readonly property var widgets: row.client.widgets.filter(w => w.layer === row.client.layer && w.supported)
  property int dropped: 0 // widgets left out for lack of space, for hosts and checks
  property bool showErrorDetail: false // expand the config error marker without hovering (checks)
  readonly property bool configError: row.client.status === "ready" && row.client.configError !== ""

  Rectangle {
    anchors.fill: parent
    visible: !row.host.transparent
    color: row.host.background
  }

  function relayout() { layoutTimer.restart() }
  Timer { id: layoutTimer; interval: 0; onTriggered: row.layout() }
  onWidthChanged: relayout()
  onWidgetsChanged: relayout()

  onConfigErrorChanged: relayout()
  function layout() {
    const avail = area.width
    const items = []
    for (let i = 0; i < repeater.count; i++) {
      const it = repeater.itemAt(i)
      if (it && it.shown) items.push(it)
    }
    const kept = { left: [], center: [], right: [] }
    for (const it of items) kept[it.widget.section].push(it)
    const sum = (list, f) => list.reduce((a, it) => a + f(it), 0) + Math.max(0, list.length - 1) * row.gap
    const need = f => {
      const parts = ["left", "center", "right"].filter(s => kept[s].length > 0)
      return parts.reduce((a, s) => a + sum(kept[s], f), 0) + Math.max(0, parts.length - 1) * row.sectionGap
    }
    // Leave widgets out until the minimum widths fit.
    const order = [["center", -1], ["left", -1], ["right", 0]]
    while (need(it => it.minWidth) > avail) {
      const next = order.find(([s]) => kept[s].length > 0)
      if (!next) break
      if (next[1] < 0) kept[next[0]].pop(); else kept[next[0]].shift()
    }
    // Then shrink what can give way, evenly, down to its minimum.
    const natural = need(it => it.implicitWidth)
    const slack = natural - need(it => it.minWidth)
    const k = natural > avail && slack > 0 ? Math.min(1, (natural - avail) / slack) : 0
    const keptSet = new Set([...kept.left, ...kept.center, ...kept.right])
    for (let i = 0; i < repeater.count; i++) {
      const it = repeater.itemAt(i)
      if (!it) continue
      it.fits = !it.shown || keptSet.has(it) // hidden agents come back by themselves
      if (keptSet.has(it)) it.width = it.implicitWidth - (it.implicitWidth - it.minWidth) * k
    }
    row.dropped = items.length - keptSet.size
    // Place: left from the left edge, right against the right edge, center
    // in the middle but clear of both.
    let x = 0
    for (const it of kept.left) { it.x = x; x += it.width + row.gap }
    const leftEnd = kept.left.length > 0 ? x - row.gap + row.sectionGap : 0
    const rightW = sum(kept.right, it => it.width)
    x = avail - rightW
    const rightStart = kept.right.length > 0 ? x - row.sectionGap : avail
    for (const it of kept.right) { it.x = x; x += it.width + row.gap }
    const centerW = sum(kept.center, it => it.width)
    x = Math.max(leftEnd, Math.min(rightStart - centerW, (avail - centerW) / 2))
    for (const it of kept.center) { it.x = x; x += it.width + row.gap }
  }

  Item {
    id: area
    anchors.fill: parent
    anchors.margins: row.gap
    anchors.leftMargin: row.gap + (row.configError ? errorMarker.collapsedWidth + row.gap : 0)
    visible: row.client.status === "ready"

    Repeater {
      id: repeater
      model: row.widgets
      WidgetView {
        required property var modelData
        widget: modelData
        client: row.client
        host: row.host
        height: area.height
        onImplicitWidthChanged: row.relayout()
        onMinWidthChanged: row.relayout()
        onShownChanged: row.relayout()
      }
      onItemAdded: row.relayout()
    }
  }

  // While the config file doesn't parse, the backend keeps the previous
  // config; a red marker says so, and hovering it shows why, over the row.
  Rectangle {
    id: errorMarker
    readonly property real pad: 8 * row.host.scale
    readonly property real collapsedWidth: shortText.implicitWidth + 2 * pad
    readonly property bool expanded: errorArea.containsMouse || row.showErrorDetail
    visible: row.configError
    z: 1
    x: row.gap
    y: row.gap
    height: row.height - 2 * row.gap
    width: expanded ? Math.min(row.width - 2 * row.gap, longText.implicitWidth + 2 * pad) : collapsedWidth
    radius: 6 * row.host.scale
    color: row.host.urgent
    Text {
      id: shortText
      visible: !errorMarker.expanded
      anchors.centerIn: parent
      text: "\u{F0026} config error"
      color: row.host.background
      font.family: row.host.fontFamily
      font.pixelSize: row.host.fontSize
    }
    Text {
      id: longText
      visible: errorMarker.expanded
      anchors { left: parent.left; right: parent.right; leftMargin: errorMarker.pad; rightMargin: errorMarker.pad }
      anchors.verticalCenter: parent.verticalCenter
      elide: Text.ElideRight
      text: "\u{F0026} " + row.client.output + ".json: " + row.client.configError + " (showing the last good config)"
      color: row.host.background
      font.family: row.host.fontFamily
      font.pixelSize: row.host.fontSize
    }
    MouseArea { id: errorArea; anchors.fill: parent; hoverEnabled: true }
  }

  Text {
    anchors.centerIn: parent
    visible: row.client.status === "offline"
    text: row.offlineText
    color: row.host.muted
    font.family: row.host.fontFamily
    font.pixelSize: row.host.fontSize
  }
}
