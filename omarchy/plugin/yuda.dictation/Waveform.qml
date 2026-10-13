import QtQuick
import qs.Commons

// Five-bar level meter. The daemon sends the microphone RMS at ~30 Hz; this
// smooths it per frame with a fast attack and a slow release, and adds a
// little per-bar jitter so the bars read as a live signal. It only animates
// while `running` is true (the capsule is on screen).
Item {
  id: wave

  property real level: 0
  property bool running: false
  property bool flat: false
  property color color: "white"

  readonly property var weights: [0.5, 0.8, 1.0, 0.75, 0.55]
  readonly property real barWidth: Style.space(4)
  readonly property real minBarHeight: Style.space(4)
  readonly property real jitter: 0.04
  property real envelope: 0

  implicitWidth: Style.space(44)
  implicitHeight: Style.space(32)

  function step(frameTime) {
    var target = wave.flat ? 0 : Math.max(0, Math.min(1, wave.level))
    // The attack/release factors are tuned per 60 fps frame; scale them by the
    // real frame time so high refresh rates do not speed the meter up.
    var frames = Math.max(0, Math.min(4, frameTime * 60))
    var k = target > wave.envelope ? 0.4 : 0.15
    wave.envelope += (target - wave.envelope) * (1 - Math.pow(1 - k, frames))

    for (var i = 0; i < bars.count; i++) {
      var bar = bars.itemAt(i)
      if (!bar) continue
      var noise = wave.flat ? 0 : (Math.random() * 2 - 1) * wave.jitter
      bar.amplitude = Math.max(0, Math.min(1, wave.envelope * wave.weights[i] + noise))
    }
  }

  function settle() {
    wave.envelope = 0
    for (var i = 0; i < bars.count; i++) {
      var bar = bars.itemAt(i)
      if (bar) bar.amplitude = 0
    }
  }

  onRunningChanged: if (!running) settle()

  Row {
    anchors.centerIn: parent
    height: wave.height
    spacing: (wave.width - wave.weights.length * wave.barWidth) / (wave.weights.length - 1)

    Repeater {
      id: bars
      model: wave.weights.length

      Rectangle {
        property real amplitude: 0
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        width: wave.barWidth
        height: wave.minBarHeight + (wave.height - wave.minBarHeight) * amplitude
        radius: width / 2
        color: wave.color
      }
    }
  }

  FrameAnimation {
    running: wave.running
    onTriggered: wave.step(frameTime)
  }
}
