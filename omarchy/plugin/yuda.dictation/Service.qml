import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import "DictationModel.js" as Model

// The single connection to the yuda daemon (docs/ipc.md). The shell loads one
// instance while the plugin is enabled; the capsule below and every bar widget
// (one per monitor) read their state from it.
//
// Reconnecting is event driven. While the socket file is missing, nothing
// runs: an inotify watch on $XDG_RUNTIME_DIR and $XDG_RUNTIME_DIR/yuda wakes
// the service when the daemon creates its socket. A backoff timer runs only
// while the socket file exists but does not accept (daemon restarting, or a
// stale file left by a crash).
Item {
  id: root

  // Injected by omarchy-shell.
  property var shell: null
  property var manifest: null
  property string omarchyPath: ""

  readonly property string socketDir: Quickshell.env("XDG_RUNTIME_DIR") + "/yuda"
  readonly property string socketPath: socketDir + "/yuda.sock"
  readonly property int minRetryMs: 1000
  readonly property int maxRetryMs: 30000

  // Live state. `online` turns true with the first snapshot after connecting,
  // so a socket that accepts but never answers still reads as not running.
  property bool online: false
  property string phase: "idle"
  property real level: 0
  property string text: ""
  property string engine: ""
  property bool fallback: false
  property string language: ""
  property string message: ""
  property int elapsedMs: 0

  property int retryMs: minRetryMs
  property bool versionWarned: false
  property bool waitingLogged: false

  function applySnapshot(snapshot) {
    phase = snapshot.state
    level = snapshot.level
    text = snapshot.text
    engine = snapshot.engine
    fallback = snapshot.fallback
    language = snapshot.language
    message = snapshot.message
    elapsedMs = snapshot.elapsedMs
  }

  function handleLine(line) {
    var snapshot = Model.parseSnapshot(line)
    if (!snapshot) {
      console.warn("yuda.dictation: ignored a line that is not a snapshot")
      return
    }
    if (snapshot.v !== Model.PROTOCOL_VERSION && !versionWarned) {
      versionWarned = true
      console.warn("yuda.dictation: daemon speaks protocol v" + snapshot.v
        + ", plugin expects v" + Model.PROTOCOL_VERSION)
    }
    applySnapshot(snapshot)
    if (!online) {
      online = true
      waitingLogged = false
      // Reset the backoff only once the daemon really answers, so a socket
      // that accepts and drops at once cannot pin the retry at 1 s.
      retryMs = minRetryMs
      console.info("yuda.dictation: subscribed to " + socketPath)
    }
  }

  function goOffline() {
    online = false
    applySnapshot(Model.idleSnapshot())
  }

  // Look for the socket file without connecting: an async stat on a worker
  // thread. A missing file is not worth a connect attempt (each failed
  // connect is logged by Quickshell).
  function probe() {
    if (socket.connected) return
    socketFile.reload()
  }

  function socketMissing() {
    retryTimer.stop()
    retryMs = minRetryMs
    if (!waitingLogged) {
      waitingLogged = true
      console.info("yuda.dictation: waiting for the daemon to create " + socketPath)
    }
  }

  function scheduleRetry() {
    if (socket.connected || retryTimer.running) return
    retryTimer.interval = retryMs
    retryTimer.start()
    console.info("yuda.dictation: no connection to the daemon, checking again in " + (retryMs / 1000) + " s")
    retryMs = Math.min(retryMs * 2, maxRetryMs)
  }

  // Check again now and restart the backoff from 1 s. Bound to the IPC target
  // below and to clicks on the bar widget.
  function reconnect() {
    retryTimer.stop()
    retryMs = minRetryMs
    probe()
  }

  function statusJson() {
    return JSON.stringify({
      online: online,
      socketConnected: socket.connected,
      state: phase,
      level: level,
      text: text,
      engine: engine,
      fallback: fallback,
      language: language,
      message: message,
      elapsed_ms: elapsedMs,
      retryScheduled: retryTimer.running,
      nextRetryMs: retryMs
    })
  }

  Socket {
    id: socket
    path: root.socketPath

    parser: SplitParser {
      onRead: function(line) { root.handleLine(line) }
    }

    onConnectedChanged: {
      if (connected) {
        retryTimer.stop()
        write("subscribe\n")
        flush()
      } else {
        console.info("yuda.dictation: disconnected from daemon")
        root.goOffline()
        root.scheduleRetry()
      }
    }

    // A failed connect only reports an error; `connected` never changes.
    onError: function(error) {
      if (!connected) root.scheduleRetry()
    }
  }

  // Existence probe for the socket. A socket is not a regular file, so a
  // successful stat reports NotAFile; a missing one reports FileNotFound.
  FileView {
    id: socketFile
    path: root.socketPath
    printErrors: false
    onLoadFailed: function(error) {
      if (error === FileViewError.FileNotFound) root.socketMissing()
      else if (!socket.connected) socket.connected = true
    }
    onLoaded: if (!socket.connected) socket.connected = true
  }

  // Wakes the service when the daemon creates its directory or socket. A
  // FileView watch also covers the parent directory, so pointing it at
  // $XDG_RUNTIME_DIR/yuda sees both the directory and the socket appear.
  // Off while connected: the open socket reports the daemon going away.
  FileView {
    id: runtimeWatch
    path: root.socketDir
    preload: false
    printErrors: false
    watchChanges: !root.online
    onFileChanged: if (!root.online) probeDebounce.restart()
  }

  // Coalesces bursts of directory events into one probe.
  Timer {
    id: probeDebounce
    interval: 100
    onTriggered: root.probe()
  }

  // Runs only while the socket file exists but does not accept:
  // 1 s, 2 s, 4 s … up to 30 s.
  Timer {
    id: retryTimer
    repeat: false
    onTriggered: root.probe()
  }

  Capsule {
    service: root
  }

  ShellIpc {
    target: "yuda.dictation"

    function reconnect(): string {
      root.reconnect()
      return "ok"
    }

    function status(): string {
      return root.statusJson()
    }
  }
}
