# 语打 Yuda · 架构总览

> 产品规格见 `PROMPT.md`；Omarchy 集成方式见 `docs/adr/0007-omarchy-native-integration.md`；进程间协议见 `docs/ipc.md`。架构变化必须同 PR 更新本文件。

## 组成

```
Omarchy（Hyprland + Quickshell）                     语打
──────────────────────────────────                  ─────────────────────────────────────────
按住 Right Alt / F9、Super+Ctrl+X
  └─ omarchy dictation start|stop|toggle
       └─ omarchy-dictation-yuda ───── start/stop ──► yuda daemon（systemd 用户服务）
按住 Shift+F9、Super+Ctrl+Shift+X（~/.config/yuda/shortcuts.lua，
  Omarchy 在默认后端为 yuda 时加载）──── start-edit / toggle-edit ──►│
                                                     │  $XDG_RUNTIME_DIR/yuda/yuda.sock
omarchy-shell 插件 yuda.dictation ◄── subscribe ─────┤  （一行一条 JSON 快照，~30 Hz）
  ├─ 录音胶囊（波形 + 实时文本 + 离线徽标）           │
  └─ 顶栏麦克风组件（点击 = omarchy dictation toggle）│
                                                     │
Omarchy 菜单 Setup › Yuda 语打 ── yuda menu … ──────►│  reload
Setup › Defaults › Dictation ── omarchy-install-dictation-yuda（模型、服务、插件、菜单、选为默认）
```

## daemon 数据流

```
start ──► audio::Capture（cpal → PipeWire，单声道 16 kHz，30 Hz 电平）
              │ 样本                    │ 电平
              ▼                         ▼
          asr 工作线程               daemon 主循环 ──► 订阅者（shell 插件、yuda watch）
          ├─ 云端：cloud_asr（火山引擎豆包大模型，WebSocket 二进制协议，100 ms 分包）
          └─ 离线：offline（中英 zipformer / 粤日韩 SenseVoice + Silero VAD 分段，尾段重解码出预览；
                           句段补边距，VAD 漏判的有声音频不丢）
              │ Partial / Final
              ▼
stop  ──► text::process（去语气词、时间数字规整、缩写合并、技术名词写法、中英间距、用户词库、标点整理）
              ▼
          听写：[refine：OpenAI 兼容端点保守纠错，5 s 超时放行原文，偏离过大拒收]
          修改：edit::parse → 替换 / 删除 / 撤回 / 追加；自由指令交给 refine::rewrite
              ▼
          injection（等修饰键松开 → 剪贴板快照 → wl-copy --sensitive → Hyprland send_key_state 粘贴
                     → 等待 → 恢复；修改时先确认焦点窗口未变、按字符数退格）
              ▼
          history（本机 jsonl，0600）
```

`engine = auto`：云端已配置且在 `connect_timeout_ms` 内连上 → 云端；否则离线并标记 `fallback`。云端中途出错 → 用缓存的整段音频离线重识别。

## 模块

| 模块 | 职责 | feature |
|---|---|---|
| `config` | TOML 配置（0600）、枚举化字段、`set_value` 保留注释就地修改；模型名与 endpoint 默认值的唯一来源 | 默认 |
| `ipc` | 命令、回应、状态快照；带超时的客户端 | 默认 |
| `session` | 纯状态机：命令 → 动作，最短按住时长，会话号让迟到结果失效 | 默认 |
| `text` | 语气词过滤、时间日期数字规整、缩写合并、技术名词写法、中英间距、词库替换、分段拼接、标点整理 | 默认 |
| `edit` | 语音修改指令解析（替换 / 删除 / 撤回 / 追加 / 自由指令）与应用，汉字数字与阿拉伯数字互配 | 默认 |
| `injection` | 剪贴板快照/恢复与粘贴后端链（hyprctl → wtype → ydotool → 仅剪贴板） | 默认 |
| `history` | 识别历史 jsonl，定长压缩 | 默认 |
| `omarchy` | 菜单扩展的标记块合并/移除与 JSONC 校验 | 默认 |
| `audio` | cpal 采集、下混、线性重采样、电平 | `audio` |
| `offline` | 按目录识别模型族（SenseVoice / transducer）、按语言选模型、VAD 模拟流式（句段补边距、漏判音频并入相邻句段）、静音能量门 | `sensevoice` |
| `cloud_asr` | 火山引擎大模型流式识别客户端 | `cloud` |
| `refine` | LLM 保守纠错与偏离检查 | `llm` |
| `asr` | 识别工作线程：常驻离线模型、按会话路由引擎、降级 | `audio` + `sensevoice` |
| `daemon` | 主循环：socket、状态机、录音、识别事件、纠错、上屏、通知 | `audio` + `sensevoice` |
| `hotkey` | 可选的 evdev 监听（`trigger.source = "evdev"`），只读、不 GRAB | `evdev` |

`daemon` feature = 全部。默认 feature 只含纯逻辑，任何平台（含 macOS）可编译和测试。

## 线程

- **主循环**持有全部可变状态，只通过 channel 收消息：IPC 请求、订阅、电平、识别事件、纠错结果、上屏结果；用 `recv_timeout` 驱动错误展示（1.5 s）与录音上限两个定时器。
- **IPC 线程**逐个处理连接；控制命令转给主循环，200 ms 内回应。`subscribe` 连接交给主循环持有，非阻塞写，写不进就断开。
- **录音线程**每次会话新建，持有 cpal Stream；停止时 join，保证样本全部送达识别线程后才发 `End`。
- **识别线程**常驻，持有当前语言的离线模型（约 2–3 s 加载）；会话期间每轮先取完积压音频再解码预览（间隔不小于上次解码耗时的两倍），结束后回到待命。
- **纠错 / 上屏线程**每次会话短暂存在，结果发回主循环。

## 状态机

```
idle ──start──► recording ──stop──► transcribing ──► [refining] ──► injecting ──► idle
                  │   │                  │                │              │
                  │   └─cancel / 按住 <300ms──► idle      └──失败───────► error ──1.5s──► idle
                  └─麦克风/引擎失败──► error
```

一次会话最多上屏一次：取消或新会话会推进会话号，旧会话的识别、纠错、上屏结果被丢弃。

修改会话（`mode = edit`）复用同一状态机：识别结果作为指令，`injecting` 阶段先删除上一段再粘贴新版本。daemon 记住上一段上屏的文字、时间与聚焦窗口地址，只修改 10 分钟内、焦点未变的文字；`yuda edit <指令>` 跳过录音直接进入 `transcribing`。

## 关键约束

1. **永不抢焦点**：浮层是 layer-shell overlay，不接收键盘；粘贴发往当前聚焦窗口。
2. **结果不丢**：粘贴后端全部失败时结果留在剪贴板并通知；只有确认粘贴发出后才恢复原剪贴板。
3. **模型配置唯一权威**：`config.toml` + `src/config.rs` 常量；业务代码不硬编码模型名与 endpoint。
4. **优雅降级**：云端 → 离线；LLM 超时/偏离 → 原文；VAD 缺失 → 整段解码；Hyprland 不可用 → wtype → ydotool → 剪贴板。
5. **只扩展 Omarchy**：不改 Omarchy 自带文件；菜单、插件、默认后端选择均可由 `omarchy-remove-dictation-yuda` 撤销。
