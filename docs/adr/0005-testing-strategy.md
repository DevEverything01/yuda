# ADR 0005: 分层测试体系与 mock 策略

- 状态：accepted
- 日期：2026-08-24
- 细化：第 5 条「真机行为不进 hosted PR 门」由 ADR 0006 进一步规定为「经 self-hosted Omarchy runner 纳入手动/夜间 CI」

## 背景

在本 ADR 提出时，现状只有模块内单元测试，覆盖纯逻辑（状态机、配置 round-trip、上屏序列的纯函数描述）；真实执行路径（`inject_text` 调用外部命令）无覆盖，无 `tests/` 目录，无 CI，真机验证只有 AGENTS.md 的口头纪律。

## 决策

1. **四层金字塔**：T0 静态检查（fmt/clippy）→ T1 模块内单元测试 → T2 `tests/` mock 集成测试 → T3 真机 Hyprland 冒烟脚本；打包验证（T4 `extra-x86_64-build`）发布前手动执行。
2. **外部命令 mock 走 PATH 注入**：`wl-copy`/`wl-paste`/`wtype`/`ydotool` 的测试替身是临时目录里的假可执行脚本，记录调用序列供断言。不为此引入 trait 抽象或 mock 框架。
3. **云端 ASR 协议测试随模块 PR 交付**：进程内 mock WebSocket server（127.0.0.1 随机端口），覆盖握手头、~100ms 分帧、final 解析、超时回退。
4. **测试依赖最小化**：dev-dependency 只新增 `tempfile`；不引入测试框架。
5. **真机行为不进 hosted PR 门**：热键/上屏/悬浮条焦点由脚本化手动冒烟覆盖；Omarchy self-hosted job 仅在手动/夜间执行，不阻塞 PR。

## 理由

- `Command::new` 天然经 PATH 解析，seam 已存在；PATH 注入零生产代码改动即可端到端断言调用序列。
- 分层与可运行环境对齐：默认 features 任意平台可跑（含 macOS 开发机），`linux-runtime` 只在 Linux，真机层只在 Hyprland 会话；避免为不可运行的层付出 CI 成本。
- sherpa-onnx static 编译重，`daemon` feature 组合在 CI 必须缓存 `target/`；分层让默认组合保持秒级反馈。

## 被否决方案

- **mockall / trait 注入命令执行器**：为一个已存在的 PATH seam 引入接口与 mock 框架，违背最小改动；且会把 `injection` 的私有实现细节逼成 `pub`。
- **容器内全桌面 e2e（Hyprland in Docker）**：Wayland 会话 + evdev + 剪贴板在容器内不可靠，维护成本高、收益低；真机脚本替代。
- **insta 快照 / proptest 属性测试**：当前没有可匹配的复杂解析器或大状态空间，YAGNI；云端协议 framing 落地时重新评估。
- **单元测试搬到 `tests/`**：集成测试只能访问 `pub` API，会倒逼不必要的可见性放宽；单测留在模块内。

## 影响

- `docs/testing.md` 为测试体系主文档；`tests/AGENTS.md` 成为该目录的强制规范。
- Makefile 已补 `test`/`check`/`hooks` target；`lint` 已覆盖 `--all-targets`。
- 云端 ASR 模块的 PR 必须附带 `tests/cloud_protocol.rs`，否则不满足 PROMPT 验收。
- dev-dependencies 已新增 `tempfile`；未实现的云端协议测试仍待对应业务模块落地。
