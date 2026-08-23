const recordButton = document.querySelector('#recordButton');
const recordLabel = document.querySelector('#recordLabel');
const inputCard = document.querySelector('#inputCard');
const waveform = document.querySelector('#waveform');
const textarea = document.querySelector('#transcript');
const placeholder = document.querySelector('#placeholder');
const confidence = document.querySelector('#confidence');
const confidenceValue = document.querySelector('#confidenceValue');
const stateLabel = document.querySelector('#stateLabel');
const timerLabel = document.querySelector('#timerLabel');
const flowHint = document.querySelector('#flowHint');
const clearButton = document.querySelector('#clearButton');
const submitButton = document.querySelector('#submitButton');
const settingsButton = document.querySelector('#settingsButton');
const settingsDialog = document.querySelector('#settingsDialog');
const errorBanner = document.querySelector('#errorBanner');
const errorMessage = document.querySelector('#errorMessage');
const dismissError = document.querySelector('#dismissError');
const settingsForm = document.querySelector('#settingsForm');
const settingsFeedback = document.querySelector('#settingsFeedback');
const testButton = document.querySelector('#testButton');
const engineMode = document.querySelector('#engineMode');
const language = document.querySelector('#language');
const hotkey = document.querySelector('#hotkey');
const refineEnabled = document.querySelector('#refineEnabled');

const STATES = Object.freeze({
  IDLE: 'idle',
  EDITING: 'editing',
  RECORDING: 'recording',
  TRANSCRIBING: 'transcribing',
  REFINING: 'refining',
  READY: 'ready',
  INJECTING: 'injecting',
  ERROR: 'error',
});

const COPY = Object.freeze({
  idle: { label: 'READY TO LISTEN', hint: '键盘可直接编辑 · 松手自动整理 · 支持中英混输' },
  editing: { label: 'EDITING', hint: '文字可继续修改 · 准备好后提交' },
  recording: { label: 'LISTENING NOW', hint: '实时识别中 · 松开右 Ctrl 完成' },
  transcribing: { label: 'TRANSCRIBING', hint: '正在生成可编辑文本 · 请稍候' },
  refining: { label: 'REFINING', hint: '只修正明显的识别错误 · 不改写原意' },
  ready: { label: 'READY TO SEND', hint: '识别完成 · 文字仍可直接编辑' },
  injecting: { label: 'SENDING', hint: '正在复制并粘贴到当前窗口' },
  error: { label: 'ACTION NEEDED', hint: '可以修改内容后重试，或打开设置检查引擎' },
});

let state = STATES.IDLE;
let timer = 0;
let elapsed = 0;
let pressedAt = 0;
let isPointerHold = false;
let isHotkeyDown = false;
let finishTimeout;
let errorTimeout;
let waveformFrame;
let submitTimeout;
let savedSettings = {
  engine: engineMode.value,
  language: language.value,
  hotkey: hotkey.value,
  refine: refineEnabled.checked,
};

function hasText() {
  return Boolean(textarea.value.trim());
}

function setText(value) {
  textarea.value = value;
  const visible = Boolean(value.trim());
  inputCard.classList.toggle('has-text', visible);
  document.querySelector('.transcript-wrap').classList.toggle('has-text', visible);
  placeholder.hidden = visible;
  confidence.hidden = !visible;
}

function setState(nextState, { hint, label } = {}) {
  state = nextState;
  inputCard.dataset.state = nextState;
  inputCard.classList.toggle('recording', nextState === STATES.RECORDING);
  inputCard.classList.toggle('has-error', nextState === STATES.ERROR);
  const copy = COPY[nextState] || COPY.idle;
  stateLabel.textContent = label || copy.label;
  flowHint.textContent = hint || copy.hint;
  recordButton.disabled = [STATES.TRANSCRIBING, STATES.REFINING, STATES.INJECTING].includes(nextState);
  submitButton.disabled = [STATES.RECORDING, STATES.TRANSCRIBING, STATES.REFINING, STATES.INJECTING].includes(nextState);
  recordButton.classList.toggle('is-recording', nextState === STATES.RECORDING);
  recordLabel.textContent = nextState === STATES.RECORDING ? '松开完成' : '按住说话';
  textarea.placeholder = nextState === STATES.RECORDING ? '正在听……' : '你也可以直接键盘输入……';
}

function updateTimer() {
  elapsed += 100;
  timerLabel.textContent = `00:${String(Math.floor(elapsed / 1000)).padStart(2, '0')}`;
}

function startRecording() {
  if (state === STATES.RECORDING || recordButton.disabled) return;
  clearError();
  clearTimeout(finishTimeout);
  setText('');
  pressedAt = performance.now();
  elapsed = 0;
  timerLabel.textContent = '00:00';
  setState(STATES.RECORDING);
  timer = window.setInterval(updateTimer, 100);
  animateWaveform();
}

function finishRecording() {
  if (state !== STATES.RECORDING) return;
  const duration = performance.now() - pressedAt;
  window.clearInterval(timer);
  timer = undefined;
  cancelAnimationFrame(waveformFrame);
  inputCard.classList.remove('recording');
  if (duration < 300) {
    setState(STATES.ERROR, { label: 'TOO SHORT', hint: '按住 300ms 以上，避免误触；这次没有生成文字' });
    showError('录音太短了。按住右 Ctrl 或按钮至少 0.3 秒再试。', 1500);
    finishTimeout = window.setTimeout(() => {
      if (state === STATES.ERROR) {
        clearError();
        setState(hasText() ? STATES.EDITING : STATES.IDLE);
      }
    }, 1500);
    return;
  }
  setState(STATES.TRANSCRIBING);
  flowHint.textContent = '松手完成 · 正在整理最后一段语音';
  finishTimeout = window.setTimeout(() => {
    if (state !== STATES.TRANSCRIBING) return;
    setText(hasText() ? textarea.value.trim() : '帮我把这段会议记录整理成三个重点');
    confidenceValue.textContent = '94%';
    setState(STATES.READY, { hint: '识别完成 · 文字仍可直接编辑 · 点击箭头提交' });
  }, 620);
}

function animateWaveform() {
  if (state !== STATES.RECORDING) return;
  waveform.querySelectorAll('span').forEach((bar, index) => {
    const centerWeight = 1 - Math.abs(index - 4.5) / 8;
    const level = 15 + Math.round(Math.random() * 72 * centerWeight);
    bar.style.setProperty('--h', `${level}%`);
  });
  waveformFrame = window.requestAnimationFrame(() => window.setTimeout(animateWaveform, 110));
}

function showError(message, dismissAfter = 0) {
  errorMessage.textContent = message;
  errorBanner.hidden = false;
  clearTimeout(errorTimeout);
  if (dismissAfter > 0) errorTimeout = window.setTimeout(clearError, dismissAfter);
}

function clearError() {
  clearTimeout(errorTimeout);
  errorBanner.hidden = true;
}

function submitText() {
  if (!hasText()) {
    textarea.focus();
    setState(STATES.ERROR, { label: 'NOTHING TO SEND', hint: '先输入文字，或按住按钮开始说话' });
    showError('还没有可提交的文字。', 1800);
    window.setTimeout(() => {
      if (state === STATES.ERROR) setState(STATES.IDLE);
    }, 1800);
    return;
  }
  clearError();
  setState(STATES.INJECTING);
  submitButton.textContent = '…';
  window.setTimeout(() => {
    setState(STATES.IDLE, { hint: '已提交到当前窗口 · 随时可以开始下一句' });
    submitButton.textContent = '✓';
    submitTimeout = window.setTimeout(() => { submitButton.textContent = '↗'; }, 1200);
  }, 420);
}

function saveSettings() {
  savedSettings = {
    engine: engineMode.value,
    language: language.value,
    hotkey: hotkey.value.trim() || 'KEY_RIGHTCTRL',
    refine: refineEnabled.checked,
  };
  const labels = { auto: '云端优先 · 自动兜底', cloud: '仅云端', offline: '仅离线' };
  document.querySelector('#engineLabel').textContent = labels[savedSettings.engine];
  recordButton.querySelector('small').textContent = savedSettings.hotkey === 'KEY_RIGHTCTRL' ? '右 Ctrl' : '已配置';
  settingsFeedback.hidden = false;
  settingsFeedback.textContent = '设置已保存（预览仅保存在当前页面）';
  window.setTimeout(() => settingsDialog.close(), 500);
}

function testPipeline() {
  settingsFeedback.hidden = false;
  settingsFeedback.textContent = '测试中 · 模拟采集、识别与上屏流程……';
  testButton.disabled = true;
  window.setTimeout(() => {
    settingsFeedback.textContent = '测试完成 · 模拟端到端延迟 420 ms';
    testButton.disabled = false;
  }, 620);
}

recordButton.addEventListener('pointerdown', (event) => {
  event.preventDefault();
  isPointerHold = true;
  try {
    recordButton.setPointerCapture?.(event.pointerId);
  } catch (_error) {
    // Synthetic preview events do not own an active pointer; recording still starts.
  }
  startRecording();
});
recordButton.addEventListener('pointerup', () => {
  if (!isPointerHold) return;
  isPointerHold = false;
  finishRecording();
});
recordButton.addEventListener('pointercancel', () => {
  isPointerHold = false;
  finishRecording();
});
recordButton.addEventListener('lostpointercapture', () => {
  if (isPointerHold) {
    isPointerHold = false;
    finishRecording();
  }
});

window.addEventListener('keydown', (event) => {
  if (event.code !== 'ControlRight' || event.repeat || event.target === textarea || isHotkeyDown) return;
  event.preventDefault();
  isHotkeyDown = true;
  startRecording();
});
window.addEventListener('keyup', (event) => {
  if (event.code !== 'ControlRight') return;
  event.preventDefault();
  isHotkeyDown = false;
  finishRecording();
});
window.addEventListener('blur', () => {
  if (isHotkeyDown) {
    isHotkeyDown = false;
    finishRecording();
  }
});
textarea.addEventListener('input', () => {
  clearError();
  const visible = hasText();
  inputCard.classList.toggle('has-text', visible);
  document.querySelector('.transcript-wrap').classList.toggle('has-text', visible);
  placeholder.hidden = visible;
  confidence.hidden = !visible;
  if (state === STATES.RECORDING || state === STATES.TRANSCRIBING || state === STATES.REFINING) return;
  setState(visible ? STATES.EDITING : STATES.IDLE);
});
clearButton.addEventListener('click', () => {
  clearTimeout(finishTimeout);
  clearError();
  setText('');
  setState(STATES.IDLE);
  textarea.focus();
});
submitButton.addEventListener('click', submitText);
dismissError.addEventListener('click', () => {
  clearError();
  setState(hasText() ? STATES.EDITING : STATES.IDLE);
});
settingsButton.addEventListener('click', () => {
  engineMode.value = savedSettings.engine;
  language.value = savedSettings.language;
  hotkey.value = savedSettings.hotkey;
  refineEnabled.checked = savedSettings.refine;
  settingsFeedback.hidden = true;
  settingsDialog.showModal();
});
settingsForm.addEventListener('submit', (event) => {
  if (event.submitter?.id !== 'testButton') saveSettings();
});
testButton.addEventListener('click', testPipeline);

setText('');
setState(STATES.IDLE);
