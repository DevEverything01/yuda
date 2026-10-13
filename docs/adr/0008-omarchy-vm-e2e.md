# ADR 0008: 在 Omarchy 官方 VM 镜像里做全自动端到端验收

- 状态：accepted
- 日期：2026-10-14
- 细化：ADR 0006 第 1 条（T3 真机层的执行方式）

## 背景

ADR 0006 把 T3 放在已登录 Hyprland 会话的 self-hosted Omarchy runner 上，需要真人按键说话，或者配置 `YUDA_E2E_TRIGGER`；实际上 scheduled run 只能跳过。ADR 0007 之后，触发链路变成「Omarchy 快捷键 → `omarchy-dictation-yuda` → daemon」，T3 必须证明 Omarchy 的全局快捷键真的驱动了语打，而 VM 内的 wtype 做不到（Omarchy 验收指南明确指出这一点）。

Omarchy 每晚发布预装好的 qcow2 镜像（`nightly.omarchy.org/<channel>/latest.json`，用户与密码 `omarchy`，桌面自动登录），官方 CI 用 `virtio-vga` 软件渲染即可启动完整桌面。

## 决策

1. `scripts/omarchy-vm` 在任意带 `/dev/kvm` 的 Linux 宿主机上管理一台常驻 VM：下载并校验官方镜像，持久 overlay 与 SSH 密钥，QMP socket 提供虚拟键盘（`hold`、`tap`）与截图。
2. `scripts/omarchy-e2e` 同步源码 → VM 内构建与 `make install` → 无人值守运行 `omarchy-install-dictation-yuda` → 跑场景：
   - 宿主侧通过 QMP 按住 F9 / Right Alt、发送 Super+Ctrl+X，走 Omarchy 自己的 Hyprland 绑定；
   - VM 内 PipeWire null sink 的 monitor 作为默认输入，`pw-play` 播放 SenseVoice 自带的中/英/粤测试语音，经过与真实麦克风相同的 cpal → PipeWire 采集链路；
   - 断言文本进入聚焦的 foot 终端（Ctrl+Shift+V 路径）与 GTK4 探针窗口（Ctrl+V 路径），原剪贴板恢复，历史新增，误触与静音不上屏；
   - 截图与 daemon 日志写入 `target/e2e/`。
3. CI 的真机层改为 `[self-hosted, kvm]` runner 上运行 `scripts/omarchy-e2e`，仍只在手动或夜间触发，不阻塞 PR。

## 理由

- 全自动、可重复：每次从同一官方镜像出发，不依赖某台机器的桌面状态，也不需要真人说话。
- 覆盖真实集成点：Omarchy 快捷键、uwsm 会话环境、systemd 用户服务、Quickshell 插件、Hyprland Lua 粘贴、Omarchy 剪贴板历史。
- 宿主只需 KVM，不需要 GPU。

## 被否决方案

- **继续依赖真人 T3**：无法进入夜间 CI，回归发现太晚。
- **VM 内 wtype/ydotool 模拟快捷键**：绕过 Hyprland 的全局绑定，证明不了集成。
- **合成 TTS 语音**：Arch 没有可靠的中文 TTS；官方测试语音是真人录音，且识别结果已知。

## 影响

- 未覆盖：真实麦克风硬件、实体键盘的 Right Alt/AltGr 布局差异、GPU 渲染下的浮层动画性能。这些仍需在实体 Omarchy 机器上抽查。
- ASR 准确性评估（ADR 0006 第 3 条）不变，仍需真人语料。
- 第一次运行要下载约 4.5 GB 镜像；VM 默认 6 vCPU、7 GB 内存。
