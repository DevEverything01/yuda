import QtQuick
import Quickshell
import Quickshell.Hyprland
import Quickshell.Wayland
import qs.Commons
import qs.Commons as Commons
import "DictationModel.js" as Model

// The recording capsule: a passive layer-shell surface at the bottom of the
// focused screen. It never takes keyboard focus, never reserves space, and
// its input region is empty, so clicks pass through to the window below.
//
// The surface has a fixed size; only the capsule inside it animates. Resizing
// a layer surface while it is mapped makes the compositor stretch the old
// buffer for a frame (see the notification popups in Omarchy's shell).
PanelWindow {
  id: win

  required property var service

  // Geometry from the design spec, scaled with the theme's spacing.
  readonly property int capsuleHeight: Style.space(56)
  readonly property int bottomGap: Style.space(120)
  readonly property int padLeft: Style.space(18)
  readonly property int padRight: Style.space(22)
  readonly property int gap: Style.space(12)
  readonly property int minLabelWidth: Style.space(160)
  readonly property int maxLabelWidth: Style.space(560)
  // Room around the capsule for the spring overshoot on entry.
  readonly property int slack: Style.space(12)

  readonly property bool wanted: !!service && service.online && service.phase !== "idle"
  readonly property var live: ({
    phase: service ? service.phase : "idle",
    fallback: !!service && service.fallback,
    badge: Model.badgeText(service ? service.mode : "dictate", !!service && service.fallback),
    content: Model.capsuleText(service ? service.phase : "idle",
      service ? service.text : "", service ? service.message : "",
      service ? service.mode : "dictate")
  })

  // What the capsule draws. It follows `live` while the capsule is wanted and
  // freezes during the exit animation, so the text does not blank out and
  // the pill does not shrink as it fades (idle snapshots carry no text).
  property var view: live
  // Reads the new phase from `live` itself rather than `wanted`, whose
  // binding may not have caught up yet when this handler runs.
  onLiveChanged: if (live.phase !== "idle" && !!service && service.online) view = live
  readonly property bool failed: view.phase === "error"

  // True from the first entry frame until the exit animation has finished.
  property bool mapped: false
  // Set while waiting for the compositor to map the surface; the entry runs
  // from then, so its first frames are not spent on an unmapped window.
  property bool entryPending: false
  property var targetScreen: null

  function focusedScreen() {
    var monitor = Hyprland.focusedMonitor
    var name = monitor ? String(monitor.name || "") : ""
    for (var i = 0; i < Quickshell.screens.length; i++) {
      if (Quickshell.screens[i].name === name) return Quickshell.screens[i]
    }
    return Quickshell.screens.length > 0 ? Quickshell.screens[0] : null
  }

  function show() {
    view = live
    exitAnim.stop()
    if (mapped) {
      enterAnim.restart()
      return
    }
    targetScreen = focusedScreen()
    capsule.scale = 0.9
    capsule.opacity = 0
    entryPending = true
    mapped = true
    if (backingWindowVisible) startEntry()
  }

  function startEntry() {
    if (!entryPending) return
    entryPending = false
    enterAnim.restart()
  }

  function hide() {
    if (!mapped) return
    entryPending = false
    enterAnim.stop()
    exitAnim.restart()
  }

  onWantedChanged: wanted ? show() : hide()
  onBackingWindowVisibleChanged: if (backingWindowVisible) Qt.callLater(startEntry)
  Component.onCompleted: if (wanted) show()

  Connections {
    target: Quickshell
    function onScreensChanged() {
      if (win.mapped && Quickshell.screens.indexOf(win.targetScreen) < 0)
        win.targetScreen = win.focusedScreen()
    }
  }

  visible: mapped && targetScreen !== null
  screen: targetScreen

  anchors.bottom: true
  margins.bottom: Math.max(0, bottomGap - slack)
  exclusionMode: ExclusionMode.Ignore
  implicitWidth: padLeft + wave.implicitWidth + gap + maxLabelWidth + gap + Style.space(56) + padRight + slack * 2
  implicitHeight: capsuleHeight + slack * 2
  color: "transparent"
  mask: Region {}

  WlrLayershell.namespace: "yuda"
  WlrLayershell.layer: WlrLayer.Overlay
  WlrLayershell.keyboardFocus: WlrKeyboardFocus.None

  TextMetrics {
    id: labelMetrics
    font: label.font
    text: win.view.content.text
  }

  Rectangle {
    id: capsule

    // Animated separately so the label and the badge grow and shrink smoothly
    // instead of the pill snapping to a new width.
    property real labelWidth: Math.max(win.minLabelWidth,
      Math.min(win.maxLabelWidth, Math.ceil(labelMetrics.advanceWidth) + Style.space(2)))
    property real badgeSlot: win.view.badge !== "" ? badge.width + win.gap : 0

    Behavior on labelWidth {
      enabled: win.visible
      NumberAnimation { duration: Style.duration(250); easing.type: Easing.OutCubic }
    }
    Behavior on badgeSlot {
      enabled: win.visible
      NumberAnimation { duration: Style.duration(250); easing.type: Easing.OutCubic }
    }

    anchors.horizontalCenter: parent.horizontalCenter
    anchors.bottom: parent.bottom
    anchors.bottomMargin: win.slack
    width: win.padLeft + wave.width + win.gap + labelWidth + badgeSlot + win.padRight
    height: win.capsuleHeight
    radius: height / 2
    color: Util.alpha(Commons.Color.background, 0.85)
    border.width: 1
    border.color: win.failed
      ? Util.alpha(Commons.Color.urgent, 0.7)
      : Util.alpha(Commons.Color.foreground, 0.12)
    transformOrigin: Item.Center
    scale: 0.9
    opacity: 0

    Behavior on border.color {
      ColorAnimation { duration: Style.duration(160) }
    }

    Waveform {
      id: wave
      anchors.left: parent.left
      anchors.leftMargin: win.padLeft
      anchors.verticalCenter: parent.verticalCenter
      level: win.service ? win.service.level : 0
      running: win.visible
      flat: win.failed
      color: win.failed ? Commons.Color.urgent : Commons.Color.accent
    }

    Text {
      id: label
      anchors.left: wave.right
      anchors.leftMargin: win.gap
      anchors.verticalCenter: parent.verticalCenter
      width: capsule.labelWidth
      textFormat: Text.PlainText
      text: win.view.content.text
      wrapMode: Text.NoWrap
      maximumLineCount: 1
      // Live text grows at the end; keep the newest words visible.
      elide: Text.ElideLeft
      horizontalAlignment: Text.AlignLeft
      verticalAlignment: Text.AlignVCenter
      font.family: Style.font.family
      font.pixelSize: Style.font.heading
      color: win.failed
        ? Commons.Color.urgent
        : (win.view.content.placeholder ? Util.alpha(Commons.Color.foreground, 0.6) : Commons.Color.foreground)
    }

    Rectangle {
      id: badge
      anchors.right: parent.right
      anchors.rightMargin: win.padRight
      anchors.verticalCenter: parent.verticalCenter
      width: badgeLabel.implicitWidth + Style.space(14)
      height: Style.space(22)
      radius: height / 2
      color: Util.alpha(Commons.Color.accent, 0.16)
      border.width: 1
      border.color: Util.alpha(Commons.Color.accent, 0.55)
      opacity: win.view.badge !== "" ? 1 : 0
      visible: opacity > 0

      Behavior on opacity {
        NumberAnimation { duration: Style.duration(200); easing.type: Easing.OutCubic }
      }

      Text {
        id: badgeLabel
        anchors.centerIn: parent
        textFormat: Text.PlainText
        // view 在退出动画期间冻结，徽标文字不会先变空再淡出。
        text: win.view.badge
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
        color: Commons.Color.accent
      }
    }
  }

  // Entry: grow from 0.9 with a slight overshoot and fade in.
  ParallelAnimation {
    id: enterAnim
    NumberAnimation {
      target: capsule
      property: "scale"
      to: 1
      duration: Style.duration(350)
      easing.type: Easing.OutBack
      easing.overshoot: 1.3
    }
    NumberAnimation {
      target: capsule
      property: "opacity"
      to: 1
      duration: Style.duration(220)
      easing.type: Easing.OutCubic
    }
  }

  // Exit: shrink and fade, then unmap the surface.
  ParallelAnimation {
    id: exitAnim
    NumberAnimation {
      target: capsule
      property: "scale"
      to: 0.9
      duration: Style.duration(220)
      easing.type: Easing.InCubic
    }
    NumberAnimation {
      target: capsule
      property: "opacity"
      to: 0
      duration: Style.duration(220)
      easing.type: Easing.InCubic
    }
    onFinished: if (!win.wanted) win.mapped = false
  }
}
