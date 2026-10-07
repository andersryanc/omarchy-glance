// glance.media: previous, play/pause and next keys, then the track title
// and artist, from Omarchy's media service. Keys the player can't use are
// greyed out; the title only presses with an `onTap`. The title gives way
// (eliding) when the row is short of space.
import QtQuick

Item {
  id: root
  required property var view
  readonly property var o: view.options
  readonly property var st: view.current
  readonly property var host: view.host
  readonly property real bw: view.px(view.opt("buttonWidth", 44))
  readonly property real titleW: view.px(view.opt("titleWidth", 260))
  readonly property real gap: view.px(3)
  readonly property bool titleTap: (view.action && view.action.zones || []).includes("title")
  readonly property string title: st.hasMedia ? (st.title || st.identity || "Unknown") : "No media"
  readonly property string artist: st.hasMedia ? (st.artist || st.identity || "") : ""

  implicitWidth: 3 * (bw + gap) + titleW
  readonly property real minWidth: 3 * (bw + gap) + Math.min(titleW, view.px(90))

  function zoneAt(x) {
    const i = Math.floor(x / (bw + gap))
    return ["previous", "playPause", "next"][i] ?? "title"
  }

  Repeater {
    model: [
      { zone: "previous", icon: "\u{F04AE}", enabled: !!root.st.canGoPrevious },
      { zone: "playPause", icon: root.st.playing ? "\u{F03E4}" : "\u{F040A}", enabled: !!root.st.canTogglePlaying },
      { zone: "next", icon: "\u{F04AD}", enabled: !!root.st.canGoNext }
    ]
    Face {
      required property var modelData
      required property int index
      host: root.host
      x: index * (root.bw + root.gap)
      width: root.bw
      height: parent.height
      pressed: root.view.pressedZone === modelData.zone
      hovered: root.view.hovered && root.zoneAt(root.view.hoverX) === modelData.zone
      Text {
        anchors.centerIn: parent
        text: parent.modelData.icon
        color: parent.modelData.enabled ? root.host.foreground : Qt.alpha(root.host.muted, 0.6)
        font.family: root.host.fontFamily
        font.pixelSize: root.view.px(root.view.opt("iconSize", root.host.fontSize * 1.6 / root.host.scale))
      }
    }
  }

  Face {
    id: titleFace
    host: root.host
    x: 3 * (root.bw + root.gap)
    width: parent.width - x
    height: parent.height
    pressed: root.view.pressedZone === "title"
    hovered: root.titleTap && root.view.hovered && root.zoneAt(root.view.hoverX) === "title"
    Column {
      anchors { left: parent.left; right: parent.right; leftMargin: root.view.px(12); rightMargin: root.view.px(12) }
      anchors.verticalCenter: parent.verticalCenter
      Text {
        width: parent.width
        elide: Text.ElideRight
        text: root.title
        color: root.st.hasMedia ? root.host.foreground : root.host.muted
        font.family: root.host.fontFamily
        font.pixelSize: root.host.fontSize
      }
      Text {
        width: parent.width
        visible: root.artist !== ""
        elide: Text.ElideRight
        text: root.artist
        color: root.host.muted
        font.family: root.host.fontFamily
        font.pixelSize: root.host.fontSize * 0.85
      }
    }
  }
}
