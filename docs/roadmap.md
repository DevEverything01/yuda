# 语打 Yuda · 路线图

## M0 — 技术验证 spike（1–2 天）

- [ ] sherpa-onnx + SenseVoice-Small int8 在目标 Omarchy 机器上跑通中文识别（实测准确率/延迟/CPU 占用）
- [ ] 火山引擎豆包流式 ASR WebSocket 连通性（申请 appid/token，跑通双向流式 + 部分结果回调）
- [ ] evdev 热键监听 + wtype/wl-clipboard 上屏的最小可行性验证

## M1 — 自用骨架（第 1–2 周）

按住说 → 录音 → 云端 ASR → 上屏全链路，无悬浮条 UI（notify-send 显示状态）。

- [ ] evdev 全局热键（alone-press 判定、300ms 防抖、input 组检查）
- [ ] cpal + rubato 音频管线
- [ ] 云端 ASR 客户端（流式，部分结果暂存）
- [ ] 剪贴板快照/注入/恢复
- [ ] config.toml 加载（0600）

## M2 — 产品化（第 3–4 周）

- [ ] 胶囊悬浮条：RMS 波形（5 条、权重 [0.5,0.8,1.0,0.75,0.55]、attack 40%/release 15%、±4% 抖动）、弹性宽度、进出动画
- [ ] 离线 SenseVoice-Small 兜底 + auto 路由 + 「离线」徽标
- [ ] LLM 保守纠错（可开关，5s 超时放行原文）
- [ ] GTK4 设置窗口 + 托盘 + Waybar socket
- [ ] `packaging/PKGBUILD`，净 chroot 构建验证，AUR 发布 `yuda-git`
- [ ] 中文 README 完整化（Omarchy 设置走查、演示 GIF）

## M3 — 键盘一体化（第 2 个月起，可与 M2 并行预研）

- [ ] spike（3 天）：fcitx5 module + 自研候选窗 UI 的可行性验证（classicui 替换 vs input-method-v2 直连）
- [ ] fcitx5 + Rime 内核 + 雾凇词库/万象语法模型
- [ ] 自研现代候选窗 UI（毛玻璃/圆角/动画）
- [ ] LLM 智能层：整句纠错/联想（句末触发，非逐键）
- [ ] 语音改走 IME commit 通道（光标处上屏、可撤销）

## 非目标（明确不做）

- X11 支持；GNOME/KDE 专属适配
- 自研拼音引擎（Rime 就是内核）
- 方言全集（MVP 只保普通话 + 中英混输；方言依赖云端大模型能力，后续按用户反馈开）
