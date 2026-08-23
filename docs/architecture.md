# 语打 Yuda · 架构总览

> 权威实现规格见 `PROMPT.md`；本文档描述模块划分与数据流。架构变化必须同 PR 更新本文件。

## 总体数据流

```
Hyprland 会话（Wayland）
│
├─ evdev 全局热键（只读，默认右 Ctrl，按住说 push-to-talk）
│      ▼ key down / key up
├─ 音频采集 cpal ── rubato 重采样 16kHz mono ──┬─► RMS 电平表（驱动波形 UI）
│      ▼                                        │
├─ ASR 路由（cloud / offline / auto）◄──────────┘
│   ├─ 云端：火山豆包大模型流式 WebSocket（ITN + 标点，部分结果实时渲染）
│   └─ 离线：sherpa-onnx + SenseVoice int8 + silero-vad（2s 超时自动切换，UI 显示「离线」徽标）
│      ▼ 最终文本（key up 后定稿）
├─ LLM 保守纠错（可选，OpenAI 兼容端点，5s 超时 → 放行原文）
│      ▼
├─ 上屏：剪贴板快照 → wl-copy → 模拟 Ctrl+V（wtype；兜底 ydotool）→ 120ms → 恢复剪贴板
│
├─ UI：gtk4-layer-shell 胶囊悬浮条（overlay 层，keyboard-mode=none，namespace=yuda）
└─ 状态外露：StatusNotifierItem 托盘（ksni）+ $XDG_RUNTIME_DIR/yuda.sock（Waybar JSON 行）
```

## 模块划分（src/ 建议结构）

| 模块 | 职责 | 关键依赖 |
|---|---|---|
| `config` | TOML 配置加载/保存，0600 权限，模型名与 endpoint 的唯一默认值来源 | serde, toml, directories |
| `hotkey` | evdev 监听、设备热插拔、alone-press 判定、300ms 防抖 | evdev, udev, inotify |
| `audio` | cpal 采集、rubato 重采样、RMS 电平广播 | cpal, rubato |
| `asr` | ASR trait + 云端 WS 客户端 + 离线 sherpa 实现 + auto 路由 | tokio, tokio-tungstenite, sherpa-rs |
| `refine` | LLM 保守纠错（OpenAI 兼容 chat/completions，5s 超时） | reqwest |
| `inject` | 剪贴板快照/恢复 + Ctrl+V 合成（wtype → ydotool 兜底链） | wl-clipboard, wtype（外部命令） |
| `ui` | 胶囊悬浮条（波形/状态机/动画）+ 设置窗口 | gtk4, gtk4-layer-shell |
| `tray` | 托盘菜单 + Waybar socket 状态发布 | ksni, tokio unix socket |
| `daemon` | 组装以上模块；systemd user unit 托管 | tokio |

## 状态机（悬浮条）

```
idle ──key down──► recording ──key up──► transcribing ──► refining ──► injecting ──► idle
                     │                        │              │
                     └──<300ms──► idle        └──超时/失败──► error（红，1.5s 自动消失）──► idle
```

## 关键设计约束

1. **永不抢焦点**：悬浮条 layer-shell keyboard-mode=none；托盘/设置窗口除外（用户主动打开）。
2. **上屏幂等**：一次会话只上屏一次；任何阶段失败走 error 态，不得半截文本留在剪贴板。
3. **模型配置唯一权威**：`config.toml` + `src/config.rs` 默认值常量；业务代码零硬编码（同扶摇 `backend/config.py` 纪律）。
4. **优雅降级**：云端失败 → 离线；LLM 超时 → 原文；wtype 缺失 → ydotool → 仅复制到剪贴板并提示。
