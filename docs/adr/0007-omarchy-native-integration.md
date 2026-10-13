# 0007 · Omarchy 原生集成：听写后端 + shell 插件

- 状态：accepted
- 日期：2026-10-14
- 取代：PROMPT.md §1（evdev 默认热键）、§3（GTK4 悬浮条）、§6（ksni 托盘、Waybar 模块、GTK 设置窗）的实现方式

## 背景

PROMPT.md 按「通用 Hyprland」写成：语打自己读 evdev 监听右 Ctrl，自己画 GTK4 layer-shell 悬浮条，提供 ksni 托盘、Waybar 模块和 GTK 设置窗口。

Omarchy 4（quattro 分支，2026-10 的 edge 构建）改变了这些前提：

- **听写是一等公民。** Omarchy 用 `omarchy dictation start|stop|toggle` 统一管理听写快捷键（按住 Right Alt 或 F9，Super+Ctrl+X 切换），并开放了后端接口：提供 `omarchy-dictation-<backend>`、`omarchy-install-dictation-<backend>`，安装后调用 `omarchy-dictation-use <backend>` 即可。快捷键只在名为 `<backend>` 的命令存在时才绑定。
- **顶栏不再是 Waybar。** 桌面外壳是 Quickshell（`omarchy-shell`），支持 `~/.config/omarchy/plugins/<id>/` 下的第三方 QML 插件（顶栏组件、面板、服务），主题色通过 `qs.Commons` 的 token 提供。
- **菜单可扩展。** `~/.config/omarchy/extensions/omarchy-menu.jsonc` 按键覆盖/追加菜单项，`omarchy-menu-select` 提供系统级选择器。
- **Hyprland 配置是 Lua。** 通用粘贴（Super+V）用 `hl.dsp.send_key_state` 按显式修饰键发送，并按窗口的 `terminal` 标签选 Ctrl+V 或 Ctrl+Shift+V。

稳定版 4.0.4 还没有后端接口（只有 Voxtype 专用实现）；edge 已有，接口文件与 quattro 分支逐字节一致。

## 决策

1. **语打是 Omarchy 听写后端 `yuda`。** 交付 `omarchy-dictation-yuda`（只向 daemon 发命令，立即返回；`stop` 在空闲或不可用时成功）、`omarchy-install-dictation-yuda`、`omarchy-remove-dictation-yuda`。快捷键归 Omarchy 管。evdev 监听降为可选的 `trigger.source = "evdev"`，供非 Omarchy 的 Hyprland 使用。
2. **daemon 由 IPC 驱动。** `$XDG_RUNTIME_DIR/yuda/yuda.sock` 上的一行一条 JSON 协议（`docs/ipc.md`）同时服务于适配器、CLI 和 UI 插件。daemon 由 systemd 用户服务托管，绑定 `graphical-session.target`。
3. **UI 是 Omarchy shell 插件 `yuda.dictation`。** QML 实现录音胶囊与顶栏组件，订阅 daemon 状态流，颜色取自 Omarchy 主题 token，随主题切换。不再实现 GTK4 悬浮条、ksni 托盘和 Waybar 模块。
4. **设置走 Omarchy 菜单。** 安装器把条目合并进用户的菜单扩展文件（标记块包裹，写入前按 Omarchy 的 JSONC 规则校验，失败不写）：Setup › Defaults › Dictation 里出现「Yuda 语打」，Setup › Yuda 语打 提供引擎、语言、去语气词、AI 纠错、历史重新上屏、编辑配置、重启。选择器用 `omarchy-menu-select`，不做 GTK 设置窗。
5. **上屏复用 Omarchy 的通用粘贴路径。** `hyprctl eval` 执行与 `clipboard.lua` 相同的 `send_key_state` 逻辑；wtype、ydotool 依次兜底；都失败时结果留在剪贴板并通知，不恢复原剪贴板（否则结果丢失）。临时写入带 `wl-copy --sensitive`。
6. **离线识别模拟流式。** SenseVoice-Small 是非流式模型；用 Silero VAD 切句，已结束的句段解码一次后固定，尾段每 400 ms 重解码作为实时预览。松手后只需解码最后一段。整段兜底解码前加能量门（峰值 < -45 dBFS 视为静音），避免静音幻听。

## 理由

- 快捷键与 Superwhisper、Voxtype 一致，用户在 Setup › Defaults › Dictation 切换后端即可，不需要 `input` 组，也没有 evdev 与 Omarchy 绑定同时触发的双录音问题。
- UI 与 Omarchy 其他界面同源、同主题，不引入 GTK4/gtk4-layer-shell 依赖，也不常驻第二套图形栈。
- 粘贴与 Super+V 同一实现：终端自动用 Ctrl+Shift+V；显式修饰键不受用户仍按着的 Super 干扰（切换模式下 Super+Ctrl+X 可能还没松开）。
- 只扩展、不修改 Omarchy 自带文件，Omarchy 更新不会覆盖语打的集成。

## 被否决的方案

- **GTK4 layer-shell 悬浮条**：与 Omarchy shell 重复，主题需另行同步，增加依赖。
- **默认 evdev 热键**：需要 `input` 组，且与 Omarchy 自己的 F9/Right Alt 绑定冲突。
- **wtype 逐字直打**：中日韩字符在部分应用不可靠（ADR 0003）。
- **修改 Omarchy 的 Lua 绑定或 shell 源码**：`omarchy-update` 会覆盖。

## 影响

- 需要带 `omarchy-dictation-use` 的 Omarchy（当前为 edge）。安装器在旧版本上明确提示「先更新 Omarchy」并退出，不做第二套集成。
- 非 Omarchy 的 Hyprland 仍可用：`trigger.source = "evdev"` + `yuda watch`/通知；没有浮层。
- 端到端验收改为在 Omarchy 官方 VM 镜像里进行（`scripts/omarchy-vm` + `scripts/omarchy-e2e`），用 QMP 虚拟键盘触发 Omarchy 快捷键，PipeWire 虚拟麦克风播放测试语音。
