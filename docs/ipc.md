# 语打 Yuda · IPC 协议

daemon（`yuda daemon`）、Omarchy 听写适配器（`omarchy-dictation-yuda`）与 Omarchy shell 插件（`yuda.dictation`）之间只通过一个 Unix socket 通信。本文件是三方的契约；改协议必须同步修改三方并更新本文件。

## Socket

- 路径：`$XDG_RUNTIME_DIR/yuda/yuda.sock`（目录权限 `0700`，socket 只对当前用户可用）。
- 编码：UTF-8，一行一条消息，以 `\n` 结尾。
- 每个连接先发送**一行命令**，daemon 按命令回应。

## 命令

| 命令 | 回应 | 说明 |
|---|---|---|
| `start` | 一行 `Reply` 后关闭 | 开始录音；已在录音时视为成功（幂等） |
| `stop` | 一行 `Reply` 后关闭 | 结束录音并识别、上屏；空闲时也返回成功 |
| `toggle` | 一行 `Reply` 后关闭 | 空闲→开始，录音中→结束 |
| `start-edit` / `toggle-edit` | 一行 `Reply` 后关闭 | 同 `start` / `toggle`，但这段语音是**修改指令**，作用于上一段上屏的文字（Shift+F9、Super+Ctrl+Shift+X） |
| `edit <指令>` | 一行 `Reply` 后关闭 | 不经录音，直接把文字指令作用于上一段，例如 `edit 把9点改成10点` |
| `cancel` | 一行 `Reply` 后关闭 | 丢弃当前录音，不上屏 |
| `status` | 一行 `Snapshot` 后关闭 | 读取当前状态 |
| `subscribe` | 先一行 `Snapshot`，之后状态每变化一次推送一行 `Snapshot`，直到客户端断开 | 供 UI 使用 |
| `reload` | 一行 `Reply` 后关闭 | 重新读取配置文件；录音中收到时在本次会话结束后生效 |

`Reply`：

```json
{"ok":true}
{"ok":false,"error":"麦克风不可用：……"}
```

命令必须在 200 ms 内回应（适配器在 Omarchy 的 `flock` 内运行，不得阻塞）。识别与上屏在 daemon 内异步进行，不在回应前等待。

## Snapshot

每行是完整状态快照，UI 直接用最新一行替换旧状态，不需要合并：

```json
{"v":1,"state":"recording","level":0.42,"text":"今天下午三点","engine":"offline","fallback":false,"language":"zh","message":"","elapsed_ms":1840,"mode":"dictate"}
```

| 字段 | 类型 | 含义 |
|---|---|---|
| `v` | 整数 | 协议版本，当前为 `1` |
| `state` | 字符串 | `idle` / `recording` / `transcribing` / `refining` / `injecting` / `error` |
| `level` | 0.0–1.0 | 麦克风电平（RMS 归一化，已做轻度平滑）；非录音状态为 `0` |
| `text` | 字符串 | 当前可见文本：录音中为实时部分结果，之后为最终结果；空闲时为空 |
| `engine` | 字符串 | 本次会话使用的引擎：`cloud` / `offline` |
| `fallback` | 布尔 | `true` 表示云端不可用、已自动切换离线（UI 显示「离线」徽标） |
| `language` | 字符串 | `zh` / `en` / `yue` / `ja` / `ko` |
| `message` | 字符串 | `error` 状态的错误说明；其他状态为空 |
| `elapsed_ms` | 整数 | 本次录音已持续的毫秒数；非录音状态保留最后值 |
| `mode` | 字符串 | `dictate`（听写）/ `edit`（修改上一段）；旧客户端可忽略 |

推送频率：录音中约 30 Hz（电平驱动波形）；其他状态只在变化时推送。`error` 状态保持 1.5 s 后 daemon 自动推送 `idle`。

## 状态机

```
idle ──start──► recording ──stop──► transcribing ──► [refining] ──► injecting ──► idle
                  │  │                    │               │              │
                  │  └─cancel / 按住 <300ms──► idle       └─失败──► error ──1.5s──► idle
                  └─麦克风/引擎失败──► error
```

- `refining` 只在启用 LLM 纠错时出现；LLM 超时（默认 5 s）直接用原文进入 `injecting`。
- 一次会话最多上屏一次。
- 修改会话（`mode = edit`）在 `injecting` 阶段先删除上一段（按字符数退格），再粘贴新版本；只作用于 10 分钟内由语打上屏、且聚焦窗口未改变的文字。
