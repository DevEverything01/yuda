# 语打 Yuda · 调研报告

> 调研时间：2026-08。结论已固化为 `PROMPT.md` 中的实现规格；本文档记录依据与来源。

## 1. 对标产品拆解：豆包输入法

字节跳动（北京春田知韵科技）2025 年 11 月上线，Android/iOS/macOS，**无 Linux 版**。核心体验：

- 豆包同款语音大模型：支持 15 种方言（方言识别准确率 98.2%）、英语及中英混输
- 轻声/快语速/嘈杂环境鲁棒；**离线模式**（端侧模型，无网可用）
- 语义级纠错：「一次调整，长期精准」的个性化学习；同音词快捷修正（他/她/它）
- 自动标点、上下文智能联想；按住空格说话（push-to-talk）+ 点击输入

来源：科技日报报道、百度百科词条、应用宝官网页。

## 2. Linux/Omarchy 现有方案（竞品与参照）

| 项目 | 形态 | 引擎 | 中文短板 |
|---|---|---|---|
| hyprwhspr (goodroot) | Python，Omarchy 原生安装器 | Whisper / Parakeet | 英文-first；标点、混输、口语纠错非为中国用户设计 |
| hyprwhspr-rs (better-slop) | Rust 单二进制，"Wispr Flow alternative" | whisper.cpp + 可选 Groq/Gemini | 同上 |
| omarchy-cmd-dictate (AUR) | 极简脚本 | whisper.cpp + wtype | 非流式：按一下录、再按一下转 |
| linuxwhisper (AUR) | GTK4 GUI | faster-whisper | 通用 Linux，非 Omarchy 定制 |
| 闪电说（shandianshuo.cn） | **闭源商业产品**，macOS/Windows | 本地模型 + AI 纠错 | 无 Linux 版；UX 是绝佳参照（按住 Fn 说、<200ms、过滤语气词） |

**结论：现有开源方案全部是 Whisper 系，没有一个做中文-first 的方言/混输/语义纠错/离线——这就是真实缺口。** 闪电说证明了中文付费市场存在，但它闭源且不做 Linux。

## 3. ASR 引擎选型

| 方案 | 类型 | 中文能力 | 成本 | 结论 |
|---|---|---|---|---|
| 火山引擎·豆包语音识别大模型（流式 WebSocket） | 云端 | 最强：中英混输、ITN、标点、方言 | ≈ ¥2.2/小时 | **主力引擎**；参考客户端：PyPI `doubao-speech` |
| sherpa-onnx + SenseVoice int8 | 端侧 | 强：中/英/日/韩/粤，CPU 可实时（RK3588 级） | 免费 | **离线兜底**；模型 `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09` |
| FunASR Paraformer（流式中文） | 端侧/自部署 | 强 | 免费 | 备选；生态整合不如 sherpa-onnx 直接 |
| 阿里 Fun-ASR-Realtime / Qwen-Audio-ASR | 云端 | 强 | 按量 | 备选云端供应商（配置层预留） |
| Groq Whisper / SiliconFlow SenseVoiceSmall | 云端 | 中/强 | 极低/有免费额度 | 备选；Whisper 系**不采用**为主引擎 |

**决策（ADR 0002）**：混合架构——云端豆包流式为主（准确率上限），SenseVoice 离线兜底（断网/隐私下限），`auto` 模式 2 秒超时自动切换。

## 4. Wayland/Hyprland 文字上屏三条路

1. `wtype`/`ydotool` 虚拟键盘注入：简单，但 **CJK 字符经 keysym 映射不可靠**，且无候选/无撤销
2. 剪贴板 + 模拟 Ctrl+V：兼容性最好，需快照/恢复原剪贴板 → **主路径（ADR 0003）**
3. input-method-v2 协议（真 IME 通道）：体验天花板（光标处上屏、可撤销），开发量大 → Phase 2 再议

已知限制：fcitx5 在 Hyprland 的候选框/全屏遮挡问题（fcitx5#821、Hyprland#6179）是 fcitx5 前端问题，与本项目无涉；XWayland 边缘应用可能收不到粘贴。

## 5. 键盘层的决策（Phase 2）

- 三大平台输入法架构完全不同（TSF / InputMethodKit / fcitx5+IM 协议），闭源产品无从移植。
- **Rime 就是跨平台内核**：鼠须管(macOS)/小狼毫(Windows)/同文(Android) 都是它的壳；Linux 即 `fcitx5-rime` + 雾凇拼音词库 + 万象语法模型。
- 因此键盘 Phase 2 = fcitx5 框架 + Rime 内核 + **自研现代 UI + LLM 智能层**，工作量砸在壳和智能上（ADR 0004）。

## 6. 机会缺口总结

中文-first × Omarchy 原生 × 开源 ×（云端大模型 + 离线兜底）——四者交集当前为空。语打的生态位。
