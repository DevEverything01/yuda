.pragma library

// Pure helpers for the yuda.dictation plugin. The wire format is defined in
// docs/ipc.md (protocol v1): one JSON snapshot per line, each one replacing
// the previous state completely.

var PROTOCOL_VERSION = 1

var STATES = ["idle", "recording", "transcribing", "refining", "injecting", "error"]
var BUSY_STATES = ["recording", "transcribing", "refining", "injecting"]

var STATE_LABELS = {
  idle: "空闲",
  recording: "录音中",
  transcribing: "识别中",
  refining: "优化中",
  injecting: "上屏中",
  error: "出错"
}

var PLACEHOLDERS = {
  recording: "正在聆听…",
  transcribing: "识别中…",
  refining: "优化中…",
  injecting: "上屏中…"
}

// 修改上一段（Shift+F9）时的提示语。
var EDIT_PLACEHOLDERS = {
  recording: "说出修改，例如「把九点改成十点」",
  transcribing: "理解修改…",
  refining: "AI 修改中…",
  injecting: "修改中…"
}

var ENGINE_LABELS = { cloud: "云端", offline: "离线" }
var LANGUAGE_LABELS = { zh: "中文", en: "英文", yue: "粤语", ja: "日语", ko: "韩语" }

function clampLevel(value) {
  var n = Number(value)
  if (!isFinite(n) || n <= 0) return 0
  return n >= 1 ? 1 : n
}

function idleSnapshot() {
  return {
    v: PROTOCOL_VERSION,
    state: "idle",
    level: 0,
    text: "",
    engine: "",
    fallback: false,
    language: "",
    message: "",
    elapsedMs: 0,
    mode: "dictate"
  }
}

// Returns a normalized snapshot, or null when the line is not a JSON object.
// Unknown states map to idle so a newer daemon never leaves the capsule stuck
// on screen.
function parseSnapshot(line) {
  var raw = String(line || "").trim()
  if (!raw) return null
  var data
  try {
    data = JSON.parse(raw)
  } catch (e) {
    return null
  }
  if (!data || typeof data !== "object" || Array.isArray(data)) return null

  var state = String(data.state || "idle")
  if (STATES.indexOf(state) === -1) state = "idle"
  var elapsed = Math.floor(Number(data.elapsed_ms))

  return {
    v: Number(data.v),
    state: state,
    level: clampLevel(data.level),
    text: typeof data.text === "string" ? data.text : "",
    engine: typeof data.engine === "string" ? data.engine : "",
    fallback: data.fallback === true,
    language: typeof data.language === "string" ? data.language : "",
    message: typeof data.message === "string" ? data.message : "",
    elapsedMs: isFinite(elapsed) && elapsed > 0 ? elapsed : 0,
    mode: data.mode === "edit" ? "edit" : "dictate"
  }
}

function isBusy(state) {
  return BUSY_STATES.indexOf(String(state)) !== -1
}

function singleLine(text) {
  return String(text || "").replace(/\s*[\r\n]+\s*/g, " ")
}

// The text the capsule shows, and whether it is a placeholder.
function capsuleText(state, text, message, mode) {
  if (state === "error") {
    var error = singleLine(message)
    return { text: error || "出错了", placeholder: false }
  }
  var live = singleLine(text)
  if (live) return { text: live, placeholder: false }
  var placeholders = mode === "edit" ? EDIT_PLACEHOLDERS : PLACEHOLDERS
  return { text: placeholders[state] || "", placeholder: true }
}

// 胶囊右侧徽标：修改模式与离线降级，可同时出现。
function badgeText(mode, fallback) {
  var parts = []
  if (mode === "edit") parts.push("修改")
  if (fallback) parts.push("离线")
  return parts.join(" · ")
}

function engineLabel(engine) {
  return ENGINE_LABELS[engine] || (engine ? String(engine) : "未知")
}

function languageLabel(language) {
  return LANGUAGE_LABELS[language] || (language ? String(language) : "未知")
}

function tooltip(online, state, engine, language, fallback, message) {
  if (!online) return "语打未运行"
  var lines = ["语打 · " + (STATE_LABELS[state] || state)]
  var engineText = engineLabel(engine) + (fallback ? "（云端不可用，已切换离线）" : "")
  lines.push("引擎：" + engineText + " · 语言：" + languageLabel(language))
  if (state === "error" && message) lines.push(singleLine(message))
  return lines.join("\n")
}
