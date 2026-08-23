# ADR 0001: Rust 单二进制

- 状态：accepted
- 日期：2026-08-23

## 背景

语打需要常驻后台、监听 evdev 全局热键、做实时音频采集与 WebSocket 流式通信，并通过 AUR 向 Arch/Omarchy 用户分发。

## 决策

使用 Rust stable 构建单二进制 daemon。

## 理由

- **分发**：`cargo build --release` 产出单二进制，`cargo install` 与 PKGBUILD 都极简；用户侧零运行时依赖（Python 方案需要 venv/解释器版本拉扯）。
- **先例**：hyprwhspr-rs 已验证该形态在 Omarchy 社区的接受度。
- **能力**：evdev/cpal/tokio/gtk4-rs 生态完整覆盖全部需求；音频与 WS 流式性能余量大。

## 被否决方案

- **Python**（hyprwhspr 原版路线）：开发与调试快，但打包分发脆弱（解释器版本、依赖冲突、启动速度），且全局热键/音频的延迟控制更差。
- **Go**：GUI 层（GTK4 绑定）生态不成熟。

## 影响

- GTK4 通过 gtk4-rs 绑定使用；若某 UI 需求在 Rust 绑定中缺失，允许局部用 C 库兜底，但须注释论证。
- 编译时间成本由 CI 与增量构建吸收。
