import QtQuick
import qs.Ui
import qs.Commons
import qs.Commons as Commons
import "DictationModel.js" as Model

// Microphone glyph for the bar. It reads the plugin service's shared daemon
// connection; it opens no socket of its own, so one per monitor costs nothing.
BarWidget {
  id: root
  moduleName: "yuda.dictation"

  // Null under a replacement bar (its widgets get a service-less facade) and
  // until the service has loaded; both read as "not running".
  readonly property var service: bar && bar.shell ? bar.shell.serviceFor("yuda.dictation") : null
  readonly property bool online: !!service && service.online
  readonly property string phase: online ? service.phase : "idle"
  readonly property bool busy: online && Model.isBusy(phase)
  readonly property bool failed: online && phase === "error"

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    // nf-md-microphone, written as a code point so editors cannot strip it.
    text: String.fromCodePoint(0xF036C)
    active: root.busy || root.failed
    activeColor: root.failed ? Commons.Color.urgent : Commons.Color.accent
    dimmed: !root.online
    tooltipText: Model.tooltip(root.online, root.phase,
      root.service ? root.service.engine : "",
      root.service ? root.service.language : "",
      root.service ? root.service.fallback : false,
      root.service ? root.service.message : "")

    onPressed: function(mouseButton) {
      if (mouseButton !== Qt.LeftButton || !root.bar) return
      root.bar.run("omarchy-dictation toggle")
      // The toggle may be what starts the daemon; retry soon, not after the
      // current backoff step.
      if (root.service && !root.service.online) root.service.reconnect()
    }
  }
}
