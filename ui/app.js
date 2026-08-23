const recordButton = document.querySelector('#recordButton');
const recordLabel = document.querySelector('#recordLabel');
const inputCard = document.querySelector('#inputCard');
const waveform = document.querySelector('#waveform');
const textarea = document.querySelector('#transcript');
const placeholder = document.querySelector('#placeholder');
const confidence = document.querySelector('#confidence');
const stateLabel = document.querySelector('#stateLabel');
const timerLabel = document.querySelector('#timerLabel');
const flowHint = document.querySelector('#flowHint');
const clearButton = document.querySelector('#clearButton');
const submitButton = document.querySelector('#submitButton');
const settingsButton = document.querySelector('#settingsButton');
const settingsDialog = document.querySelector('#settingsDialog');

let recording = false;
let timer;
let elapsed = 0;
let pressedAt = 0;
let isPointerHold = false;

function setText(value) {
  textarea.value = value;
  inputCard.classList.toggle('has-text', Boolean(value.trim()));
  placeholder.hidden = Boolean(value.trim());
  confidence.hidden = !value.trim();
}

function updateTimer() {
  elapsed += 100;
  timerLabel.textContent = `00:${String(Math.floor(elapsed / 1000)).padStart(2, '0')}`;
}

function startRecording() {
  if (recording) return;
  recording = true;
  pressedAt = performance.now();
  elapsed = 0;
  timerLabel.textContent = '00:00';
  inputCard.classList.add('recording');
  recordButton.classList.add('is-recording');
  recordLabel.textContent = '松开完成';
  stateLabel.textContent = 'LISTENING NOW';
  flowHint.textContent = '正在实时识别 · 松开右 Ctrl 完成';
  textarea.placeholder = '正在听……';
  timer = window.setInterval(updateTimer, 100);
  animateWaveform();
}

function finishRecording() {
  if (!recording) return;
  const duration = performance.now() - pressedAt;
  recording = false;
  window.clearInterval(timer);
  inputCard.classList.remove('recording');
  recordButton.classList.remove('is-recording');
  recordLabel.textContent = '按住说话';
  textarea.placeholder = '你也可以直接键盘输入……';
  if (duration < 300) {
    stateLabel.textContent = 'TOO SHORT';
    flowHint.textContent = '按住 300ms 以上，避免误触';
    window.setTimeout(() => {
      stateLabel.textContent = 'READY TO LISTEN';
      flowHint.textContent = '键盘可直接编辑 · 松手自动整理 · 支持中英混输';
    }, 1200);
    return;
  }
  stateLabel.textContent = 'TRANSCRIBED';
  flowHint.textContent = '识别完成 · 文字仍可直接编辑';
  setText(textarea.value.trim() || '帮我把这段会议记录整理成三个重点');
}

function animateWaveform() {
  if (!recording) return;
  waveform.querySelectorAll('span').forEach((bar, index) => {
    const level = 20 + Math.round(Math.random() * 68 * (1 - Math.abs(index - 4.5) / 8));
    bar.style.setProperty('--h', `${level}%`);
  });
  window.requestAnimationFrame(() => window.setTimeout(animateWaveform, 110));
}

recordButton.addEventListener('pointerdown', (event) => {
  event.preventDefault();
  isPointerHold = true;
  recordButton.setPointerCapture?.(event.pointerId);
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

window.addEventListener('keydown', (event) => {
  if (event.code !== 'ControlRight' || event.repeat || event.target === textarea) return;
  event.preventDefault();
  startRecording();
});
window.addEventListener('keyup', (event) => {
  if (event.code !== 'ControlRight' || event.target === textarea) return;
  event.preventDefault();
  finishRecording();
});

textarea.addEventListener('input', () => {
  inputCard.classList.toggle('has-text', Boolean(textarea.value.trim()));
  placeholder.hidden = Boolean(textarea.value.trim());
  confidence.hidden = !textarea.value.trim();
  stateLabel.textContent = textarea.value.trim() ? 'EDITING' : 'READY TO LISTEN';
});
clearButton.addEventListener('click', () => {
  setText('');
  textarea.focus();
  stateLabel.textContent = 'READY TO LISTEN';
  flowHint.textContent = '键盘可直接编辑 · 松手自动整理 · 支持中英混输';
});
submitButton.addEventListener('click', () => {
  if (!textarea.value.trim()) {
    textarea.focus();
    flowHint.textContent = '先输入文字，或按住按钮开始说话';
    return;
  }
  stateLabel.textContent = 'COPIED TO CLIPBOARD';
  flowHint.textContent = '已准备粘贴到当前窗口';
  submitButton.textContent = '✓';
  window.setTimeout(() => { submitButton.textContent = '↗'; }, 1400);
});
settingsButton.addEventListener('click', () => settingsDialog.showModal());

setText('');
