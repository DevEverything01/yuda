# 语打 Yuda

**Omarchy（Arch Linux + Hyprland）上的中文-first 语音输入法：按住说话，松手上屏。**

对标豆包输入法的语音体验——豆包同源的云端语音大模型 + 端侧离线兜底 + AI 保守纠错，填补 Linux 桌面中文语音输入的空白。

> 当前状态：**UI 产品化原型已可运行**。Rust 状态机、配置与上屏序列已建立，并提供浏览器 UI 预览；真实 Hyprland、evdev、音频设备与系统级上屏仍待 Linux 目标环境验证。

## 为什么是语打

- Linux 上现有的语音听写工具（hyprwhspr、linuxwhisper 等）全部是 Whisper 系，英文强、中文够用但不出色：标点、中英混输、口语纠错都不是为中国用户设计。
- 豆包输入法证明了现代语音输入的体验上限，但它没有、也不见得有 Linux 版。
- 语打把这套体验带给中文 Arch/Omarchy 用户：**云端豆包大模型流式识别为主，SenseVoice 端侧模型离线兜底**，断网也能用。

## 计划特性

- 🎙 **按住说**：可配置全局热键（默认右 Ctrl），按住录音、松手上屏
- ☁️🔌 **双引擎**：火山引擎豆包语音识别大模型（流式，中英混输、自动标点）+ sherpa-onnx SenseVoice int8 离线模型（断网自动切换）
- 🪟 **现代悬浮条**：屏幕底部胶囊浮窗，实时 RMS 波形 + 流式文字预览，毛玻璃模糊，不抢焦点
- 🤖 **保守 AI 纠错**：只修明显识别错误（同音字、配森→Python 这类术语误转），绝不改写你的话
- 📋 **可靠上屏**：剪贴板快照 → 写入 → 模拟 Ctrl+V → 恢复原剪贴板
- 🧩 **Waybar / 托盘集成**：状态一目了然，GTK4 设置界面管理引擎、热键与密钥

## 本地预览

无需音频设备即可检查 UI 层交互：

```bash
python3 ui/serve.py
```

然后打开 <http://127.0.0.1:4173>。可以直接键盘编辑文本，也可以点击并按住「按住说话」按钮；在支持的浏览器中按住右 Ctrl 会进入录音态，松开后显示识别结果。设置按钮可检查引擎、语言与热键表单。

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
