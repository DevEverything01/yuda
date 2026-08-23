# 语打 Yuda

**Omarchy（Arch Linux + Hyprland）上的中文-first 语音输入法：按住说话，松手上屏。**

对标豆包输入法的语音体验——豆包同源的云端语音大模型 + 端侧离线兜底 + AI 保守纠错，填补 Linux 桌面中文语音输入的空白。

> 当前状态：**UI 产品化预览已可运行**。Rust 状态机、配置、上屏序列与浏览器交互流程已建立；真实 Hyprland、evdev、音频设备与系统级上屏仍待 Linux 目标环境验证。

## 为什么是语打

- Linux 上现有的语音听写工具（hyprwhspr、linuxwhisper 等）全部是 Whisper 系，英文强、中文够用但不出色：标点、中英混输、口语纠错都不是为中国用户设计。
- 豆包输入法证明了现代语音输入的体验上限，但它没有、也不见得有 Linux 版。
- 语打把这套体验带给中文 Arch/Omarchy 用户：**云端豆包大模型流式识别为主，SenseVoice-Small 端侧模型离线兜底**，断网也能用。

## 计划特性

- 🎙 **按住说**：可配置全局热键（默认右 Ctrl），按住录音、松手上屏
- ☁️🔌 **双引擎**：火山引擎豆包语音识别大模型（流式，中英混输、自动标点）+ sherpa-onnx SenseVoice-Small int8 离线模型（断网自动切换）
  - 本地模型默认使用 `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17`（SenseVoice-Small int8）；首次使用时放在 `~/.local/share/yuda/models`，不把模型文件提交到仓库。
    包内包含 `model.int8.onnx` 与 `tokens.txt`；官方模型发布页：<https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models>。
  模型运行时在 Linux 目标环境通过 `--features sensevoice` 启用；该特性使用官方 `sherpa-onnx` Rust API 加载 `model.int8.onnx` 与 `tokens.txt`。当前 macOS 验证环境不会自动下载原生 sherpa-onnx 库。
- 🪟 **现代悬浮条**：屏幕底部胶囊浮窗，实时 RMS 波形 + 流式文字预览，毛玻璃模糊，不抢焦点
- 🤖 **保守 AI 纠错**：只修明显识别错误（同音字、配森→Python 这类术语误转），绝不改写你的话
- 📋 **可靠上屏**：剪贴板快照 → 写入 → 模拟 Ctrl+V → 恢复原剪贴板
- 🧩 **Waybar / 托盘集成**：状态一目了然，GTK4 设置界面管理引擎、热键与密钥

## 本地预览

无需音频设备即可检查 UI 层交互：

```bash
python3 ui/serve.py
```

然后打开 <http://127.0.0.1:4173>。

可操作流程：

- **键盘输入**：点击编辑区直接输入或修改文字，点击右下角箭头提交；界面显示「正在上屏」，随后回到空闲态。
- **按住说话**：按住「按住说话」按钮，或在页面中按住右 Ctrl；观察录音计时、波形与「正在聆听」状态，松手后进入整理态，再显示可编辑识别结果。
- **防误触**：短于 300 ms 的按住会显示明确错误提示，约 1.5 秒后回到可操作的空闲态。
- **错误与设置**：空内容提交会提示原因；设置面板可调整识别模式、语言、热键与保守优化开关，测试按钮会报告模拟端到端延迟。

预览只在当前页面内模拟识别和上屏，不读取麦克风、不上传文本、不保存密钥；它不能替代真实 Linux/Hyprland 验收。

## SenseVoice-Small 本地运行时

默认配置只保存模型目录和运行参数，不把模型文件放进仓库。Linux 目标机上：

```bash
mkdir -p ~/.local/share/yuda/models
cd ~/.local/share/yuda/models
curl -LO https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2
tar -xjf sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2
rm sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2
```

启用官方 Rust 运行时：

```bash
cargo build --release --features sensevoice
```

`sherpa-onnx` 构建脚本会按目标平台准备原生库；若构建环境不能访问 GitHub，可预先设置 `SHERPA_ONNX_LIB_DIR` 或 `SHERPA_ONNX_ARCHIVE_DIR`。当前 macOS 验证环境未下载原生库，因此只验证了默认特性和模型路径/参数逻辑，未声称完成真实音频推理。

## Rust 验证

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build
```

`cargo run -- --simulate` 会在终端演示空闲、录音、整理、优化与准备上屏状态转换。`cargo run -- --json` 输出 Waybar 风格状态 JSON。

真实目标环境安装后：

```bash
systemctl --user enable --now yuda
```

真实 Hyprland/evdev/麦克风验证不在当前 macOS UI 预览范围内。
## 文档

| 文档 | 内容 |
|---|---|
| [PROMPT.md](PROMPT.md) | 实现提示词——产品规格的权威来源 |
| [docs/research.md](docs/research.md) | 调研报告：竞品、ASR 选型、上屏技术 |
| [docs/architecture.md](docs/architecture.md) | 架构总览与模块划分 |
| [docs/roadmap.md](docs/roadmap.md) | 里程碑 M0–M3 |
| [docs/adr/](docs/adr/) | 架构决策记录 |
| [AGENTS.md](AGENTS.md) | 仓库协作标准 |

## 许可

[MIT](LICENSE)
