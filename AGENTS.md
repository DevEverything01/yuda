# 语打 Yuda 协作指南

本文件是本仓库的唯一协作标准，所有人类贡献者与 AI Agent 都必须遵守。任何操作前重新执行 Git 检查，不要依赖记忆中的仓库状态。

## 项目定位

语打（Yuda）是 Omarchy（Arch Linux + Hyprland, Wayland）上的**中文-first 语音输入法**：按住说话，松手上屏，对标豆包输入法的语音体验。在 Omarchy 上它是一个**听写后端**（`omarchy-dictation-yuda`）加一个 **shell 插件**（`yuda.dictation`），见 ADR 0007。开源，目标 AUR 分发。

- 产品定义与实现规格：`PROMPT.md`（唯一权威实现提示词；Omarchy 偏离见 ADR 0007）
- 调研结论：`docs/research.md`
- 架构总览：`docs/architecture.md`
- 进程间协议：`docs/ipc.md`（daemon ↔ 适配器 ↔ shell 插件的契约）
- 测试体系：`docs/testing.md`（`tests/` 目录规范见 `tests/AGENTS.md`）
- 里程碑：`docs/roadmap.md`
- 重大决策记录：`docs/adr/`

## 目标平台与技术栈

- **唯一目标平台**：Arch Linux / Omarchy，Hyprland（Wayland）。不考虑 X11；不为 GNOME/KDE 做专门适配（能用是运气，不修专属 bug）。
- **Omarchy 版本**：需要带 `omarchy-dictation-use` 的 Omarchy（当前为 edge）；稳定版 4.0.4 没有听写后端接口。
- **语言**：Rust stable，单二进制 daemon（std 线程 + channel，无异步运行时）；UI 是 Omarchy shell 的 QML 插件。
- **关键依赖**：`cpal`（采集）、`sherpa-onnx`（SenseVoice + Silero VAD）、`tungstenite` + `rustls`（云端识别）、`ureq`（LLM 纠错）、`toml_edit`（保留注释改配置）、`evdev`（仅独立模式）；外部命令 `wl-clipboard`、`hyprctl`、`wtype`。
- 新增第三方 crate 前必须在 PR 描述里说明理由；优先成熟、维护活跃的 crate。

## 目录结构约定

```
yuda/
├── PROMPT.md            # 实现提示词（产品规格的权威来源）
├── README.md            # 面向用户的中文 README
├── AGENTS.md            # 本文件
├── docs/                # 调研、架构、路线图、ADR
│   └── adr/             # 架构决策记录，编号递增，不可复用
├── omarchy/             # Omarchy 集成：bin/（适配器、安装与卸载脚本）、plugin/（shell 插件 yuda.dictation）
├── packaging/           # PKGBUILD、systemd 单元（makepkg 的 src/ 会与 Rust src/ 冲突，严禁放仓库根目录构建）
├── scripts/             # pre-commit、冒烟、omarchy-vm（Omarchy VM 管理）、omarchy-e2e（端到端验收）
├── examples/            # config.example.toml 等示例（不含真实密钥）
├── tests/               # 可复用集成/契约测试（目录规范见 tests/AGENTS.md）
├── evals/               # 持续性质量评估与外部语料（目录规范见 evals/AGENTS.md）
└── src/                 # Rust 源码（模块表见 docs/architecture.md）
```

`target/`、`models/`、`.env`、日志、截图等生成物一律不提交。

## 代码规范

- `cargo fmt` 格式化；`cargo clippy -- -D warnings` 必须零警告通过。
- 错误处理：库代码用 `thiserror`，应用层用 `anyhow`；**生产路径禁止 `unwrap()`/`expect()`**（测试与 `main` 早期初始化除外）；禁止无注释论证的 `unsafe`。
- 日志统一用 `tracing`，禁止散落 `println!`/`eprintln!`。
- 平台行为红线：
  - 录音浮层**永远不抢焦点**（layer-shell overlay，不接收键盘）。
  - 上屏唯一主路径 = 剪贴板 + 模拟粘贴（剪贴板须先快照、用后恢复；临时内容 `--sensitive`）；粘贴失败时结果留在剪贴板，不得丢失。
  - 只扩展 Omarchy（后端脚本、插件、菜单扩展），**不修改 Omarchy 自带文件**。
  - 独立模式的 evdev 监听只读，**禁止 `EVIOCGRAB`**。
- 模型选择（ASR/LLM 模型名、endpoint、fallback 列表）只允许出现在配置文件与 `src/config.rs` 的默认值常量里，业务代码不得硬编码。

## 配置与密钥

- 用户配置唯一路径：`~/.config/yuda/config.toml`（TOML，权限 0600，安装/升级不得覆盖已有配置）。
- **仓库严禁提交任何真实密钥、token、内网地址**；示例配置放 `examples/config.example.toml`。
- 提交前自查：`git diff --cached | grep -iE 'api[_-]?key|token|secret'` 应为空（示例占位符除外）。

## Git 规范

- 提交信息用 Conventional Commits：`feat:` / `fix:` / `docs:` / `refactor:` / `chore:` / `ci:` / `test:`，中文描述。
- 分支命名：`feat/<主题>`、`fix/<主题>`、`docs/<主题>`；一个任务一个分支，完成即删。
- `main` 分支任何时刻必须可编译通过；禁止对 `main` 执行 force push / reset --hard。
- PR 自检清单（未完成不得合入）：
  1. `cargo fmt --check && cargo clippy -- -D warnings && cargo test` 全绿；
  2. 涉及架构/行为变化的已同步更新对应 docs；
  3. 无密钥、无生成物、无调试残留。

## AI Agent 工作协议

- 开工前：`git status --short --branch` + `git fetch --prune origin`，确认工作区干净；发现脏工作区先报告，不得擅自清理/stash。
- 实现规格以 `PROMPT.md` 为准；规格与实际实现冲突时在 PR 中说明，不得静默偏离。
- 完成后汇报：改动摘要、验证命令与真实输出、影响面与已知风险。不得汇报未实际执行过的验证结果。
- 平台验证（快捷键、注入、浮层）必须在真实 Hyprland 会话里进行：Omarchy 官方 VM（`scripts/omarchy-e2e`，见 `docs/testing.md`）或实体 Omarchy 机器；只在 headless 环境验证过编译与单测时，PR 中必须标注哪些行为未验证。

## 文档纪律

- 重大架构/选型决策必须落 ADR：`docs/adr/NNNN-kebab-case.md`，含背景、决策、理由、被否决方案；状态 `proposed` → `accepted` / `deprecated`。
- 架构变化必须同 PR 更新 `docs/architecture.md`；README 面向最终用户，保持中文。
