# 语打 Yuda · 路线图

## M0 — 技术验证

- [x] sherpa-onnx + SenseVoice-Small int8 在 Omarchy 上跑通（中/英/粤测试语音；5.6 秒中文约 0.36 秒出结果）
- [x] 火山引擎豆包大模型流式协议：按官方文档与官方 demo 实现，mock 服务器覆盖全部帧类型；TLS 握手与 401 映射用占位密钥实测
- [ ] 火山引擎真实密钥联调（流式结果、热词、末包）
- [x] 上屏可行性：Hyprland `send_key_state` / wtype / wl-clipboard

## M1 — Omarchy 原生骨架

- [x] Omarchy 听写后端：`omarchy-dictation-yuda`、安装与卸载脚本、`omarchy-dictation-use`
- [x] IPC 驱动的 daemon（systemd 用户服务，`graphical-session.target`）
- [x] cpal 采集（优先 16 kHz，线性重采样兜底）、电平
- [x] 剪贴板快照 / 注入 / 恢复，终端自动 Ctrl+Shift+V，不进剪贴板历史
- [x] config.toml（0600，保留注释的就地修改）

## M2 — 对标豆包

- [x] 边说边出字：离线 VAD 分段模拟流式；云端流式
- [x] 自动 / 云端 / 离线路由，云端失败用缓存音频离线重识别
- [x] 去语气词、个人词库、云端热词、AI 保守纠错（超时与偏离保护）
- [x] 识别历史与重新上屏
- [x] Omarchy shell 插件：录音胶囊（波形、实时文本、离线徽标）+ 顶栏组件，跟随主题
- [x] Omarchy 菜单：Setup › Defaults › Dictation 与 Setup › Yuda 语打
- [x] Omarchy VM 全自动端到端验收（`scripts/omarchy-e2e`）
- [ ] `packaging/PKGBUILD` 净 chroot 构建验证，发布 AUR `yuda-git`
- [ ] 演示 GIF
- [ ] 语音改写指令（「把 A 改成 B」「删掉上一句」）
- [ ] 真人语料准确性评估（coding / chat / email）

## M3 — 键盘一体化（预研）

- [ ] fcitx5 module + 自研候选窗（Omarchy 已预装 fcitx5）
- [ ] Rime 内核 + 雾凇词库 / 万象语法模型
- [ ] LLM 智能层：整句纠错 / 联想（句末触发，非逐键）
- [ ] 语音改走 IME commit 通道（光标处上屏、可撤销）

## 非目标

- X11；GNOME/KDE 专属适配
- 自研拼音引擎（Rime 就是内核）
- 修改 Omarchy 自带文件
