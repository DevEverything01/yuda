# 语打 Omarchy 插件（yuda.dictation）

`yuda.dictation` 是 Omarchy 4 桌面外壳（omarchy-shell，基于 Quickshell）的第三方插件，为语打提供两样东西：

- **录音胶囊**：录音、识别、纠错、上屏期间显示在当前屏幕底部中央（距底边 120 px），左侧是实时音量波形，右侧是实时文字；云端不可用时显示「离线」徽标，出错时用警示色显示错误说明。胶囊不抢键盘焦点，不占屏幕空间，鼠标点击会穿透到下面的窗口。
- **顶栏图标**：麦克风图标。空闲时为前景色，录音和处理时为主题强调色，出错时为警示色，daemon 未运行时变暗；悬停显示状态、引擎和语言；左键单击执行 `omarchy-dictation toggle`。

插件只读语打 daemon 的状态，不录音，也不上屏。daemon 与插件的通信协议见 `docs/ipc.md`。

## 目录

```
omarchy/plugin/
├── yuda.dictation/          # 插件本体，安装到 ~/.config/omarchy/plugins/yuda.dictation
│   ├── manifest.json        # kinds: service + bar-widget
│   ├── Service.qml          # 唯一的 daemon 连接，持有胶囊窗口
│   ├── Capsule.qml          # 胶囊悬浮条（layer-shell overlay，namespace = yuda）
│   ├── Waveform.qml         # 五柱波形
│   ├── BarWidget.qml        # 顶栏图标
│   └── DictationModel.js    # 快照解析与文案
└── dev/fake-daemon.py       # 开发用假 daemon
```

## 安装与启用

需要 Omarchy 4（quattro）。插件是否启用由 `~/.config/omarchy/shell.json` 是否引用 `yuda.dictation` 决定。

```bash
mkdir -p ~/.config/omarchy/plugins
cp -r omarchy/plugin/yuda.dictation ~/.config/omarchy/plugins/
omarchy plugin validate ~/.config/omarchy/plugins/yuda.dictation
omarchy-shell shell rescanPlugins

# 重新扫描是异步的：等插件出现在列表里再启用，否则会报“plugin is not known”。
for _ in $(seq 50); do
  omarchy plugin list | grep -q '^yuda\.dictation ' && break
  sleep 0.1
done
omarchy plugin enable yuda.dictation
```

启用后图标默认放在顶栏右侧。可以用 `omarchy bar move yuda.dictation --section left` 等命令移动，或在 Setup › Plugins 里管理。

注意事项：

- **从顶栏移除图标会同时停用胶囊**，因为插件的启用状态就是 `shell.json` 里的这一条记录。只想要胶囊、不要图标时，把 `bar.layout` 里的 `{ "id": "yuda.dictation" }` 移到顶层的 `"plugins": [ … ]` 数组里，然后执行 `omarchy-restart-shell`。这时 `omarchy plugin list` 会显示 disabled（它只看图标在不在顶栏上），胶囊照常工作。
- 插件目录也可以是指向打包目录的符号链接，外壳能正常加载。但 `omarchy plugin validate` 不跟随路径起点的符号链接，会报 “symlinks are not allowed”；校验时在路径末尾加 `/`。
- 修改 QML 后执行 `omarchy-restart-shell`。实测 Quickshell 0.3.2 上，保存文件触发的热重载仍会使用缓存的旧代码。
- 换用第三方整条顶栏（`bar.id` 不是 `omarchy.bar`）时，第三方顶栏拿不到插件的服务对象，图标会一直显示“语打未运行”；胶囊不受影响。

## 毛玻璃效果（可选）

胶囊背景是半透明的主题背景色。要让它像 Omarchy 自己的面板一样模糊背后的内容，在 `~/.config/hypr/looknfeel.lua`（或 `~/.config/hypr/hyprland.lua` 末尾）加入下面的层规则。写法照抄 Omarchy 自带的 `default/hypr/toggles/window-glassy.lua` 与 `default/hypr/apps/omarchy-shell.lua`：

```lua
-- 语打胶囊：模糊背后的内容；透明区域（alpha < 0.3）不模糊。
hl.layer_rule({
  match = { namespace = "^yuda$" },
  blur = true,
  ignore_alpha = 0.3,
})

-- 胶囊自带进出场动画，关掉 Hyprland 自己的图层淡入淡出，避免叠加。
hl.layer_rule({ match = { namespace = "^yuda$" }, no_anim = true, animation = "none" })
```

Omarchy 默认关闭模糊（`decoration.blur.enabled = false`），只有开启玻璃效果（`omarchy hyprland window transparency glassy`）后第一条规则才生效。第二条规则在任何情况下都建议加上。

## 行为细节

- **连接**：连接 `$XDG_RUNTIME_DIR/yuda/yuda.sock`，发送 `subscribe`，每收到一行 JSON 快照就整体替换状态。一个外壳只有一条连接，所有显示器上的顶栏图标共用。
- **重连**：socket 文件不存在时不轮询，用 inotify 监视 `$XDG_RUNTIME_DIR` 和 `$XDG_RUNTIME_DIR/yuda`，daemon 一创建 socket 就连上。socket 文件存在却连不上（daemon 正在重启，或崩溃留下的旧文件）时，按 1 s、2 s、4 s … 30 s 退避重试。启动 daemon 的脚本可以执行 `omarchy-shell -q yuda.dictation reconnect` 立即重连，通常不需要。
- **减少动态效果**：Hyprland 动画关闭时（`omarchy toggle animations`，无 GPU 的机器默认如此），胶囊的进出场与宽度动画时长为 0，与 Omarchy 其他界面一致；波形仍随音量实时变化。
- **调试**：`omarchy-shell yuda.dictation status` 输出插件看到的当前状态；日志用 `journalctl --user -t omarchy-shell | grep yuda.dictation` 查看。

## 开发：假 daemon

`dev/fake-daemon.py` 实现同一协议，可以在没有麦克风和模型的环境里调胶囊。用法写在文件开头。

```bash
systemctl --user stop yuda.service          # 先停真 daemon：它会录音并把文字粘贴到焦点窗口
python3 omarchy/plugin/dev/fake-daemon.py demo                     # 循环演示各种状态
python3 omarchy/plugin/dev/fake-daemon.py hold recording --fallback  # 定格在某个状态，便于截图
systemctl --user start yuda.service         # 用完恢复
```

真 daemon 在监听时假 daemon 拒绝启动。此时不要再往 socket 发 `start`、`stop`、`toggle`，否则会发给真 daemon。
