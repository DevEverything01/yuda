# 语打 Yuda 协作指南

本文件是本仓库的唯一协作标准，所有人类贡献者与 AI Agent 都必须遵守。任何操作前重新执行 Git 检查，不要依赖记忆中的仓库状态。

## 项目定位

语打（Yuda）是 Omarchy（Arch Linux + Hyprland, Wayland）上的**中文-first 语音输入法**：按住说话，松手上屏，对标豆包输入法的语音体验。开源，目标 AUR 分发。

- 产品定义与实现规格：`PROMPT.md`（唯一权威实现提示词）
- 调研结论：`docs/research.md`
- 架构总览：`docs/architecture.md`
- 里程碑：`docs/roadmap.md`
- 重大决策记录：`docs/adr/`

## 目标平台与技术栈

- **唯一目标平台**：Arch Linux / Omarchy，Hyprland（Wayland）。不考虑 X11；不为 GNOME/KDE 做专门适配（能用是运气，不修专属 bug）。
- **语言**：Rust stable，单二进制 daemon。
- **关键依赖方向**：`evdev`（全局热键）、`cpal` + `rubato`（采集与重采样）、`tokio`（异步运行时）、`gtk4` + `gtk4-layer-shell`（悬浮条与设置窗）、`ksni`（托盘）、`sherpa-rs`（离线 ASR）、`wl-clipboard`/`wtype`（上屏，外部命令）。
- 新增第三方 crate 前必须在 PR 描述里说明理由；优先成熟、维护活跃的 crate。

## 目录结构约定

```
yuda/
├── PROMPT.md            # 实现提示词（产品规格的权威来源）
├── README.md            # 面向用户的中文 README
├── AGENTS.md            # 本文件
├── docs/                # 调研、架构、路线图、ADR
│   └── adr/             # 架构决策记录，编号递增，不可复用
├── packaging/           # PKGBUILD 等打包文件（makepkg 的 src/ 会与 Rust src/ 冲突，严禁放仓库根目录构建）
├── examples/            # config.example.toml 等示例（不含真实密钥）
└── src/                 # Rust 源码（首个代码 PR 建立）
```

`target/`、`models/`、`.env`、日志、截图等生成物一律不提交。

## 代码规范

- `cargo fmt` 格式化；`cargo clippy -- -D warnings` 必须零警告通过。
- 错误处理：库代码用 `thiserror`，应用层用 `anyhow`；**生产路径禁止 `unwrap()`/`expect()`**（测试与 `main` 早期初始化除外）；禁止无注释论证的 `unsafe`。
- 日志统一用 `tracing`，禁止散落 `println!`/`eprintln!`。
- 平台行为红线：
  - 悬浮条必须 `keyboard-mode = none`，**永远不抢焦点**。
  - 上屏唯一主路径 = 剪贴板 + 模拟 Ctrl+V（剪贴板须先快照、用后恢复）；wtype 直打仅作 ASCII 兜底。
  - 热键监听走 evdev 只读，**禁止 `EVIOCGRAB`**。
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
- 平台验证（热键、注入、悬浮条）必须在真实 Hyprland 会话里进行；headless 环境只能验证编译与单测，需在 PR 中明确标注哪些行为未做真机验证。

## 文档纪律

- 重大架构/选型决策必须落 ADR：`docs/adr/NNNN-kebab-case.md`，含背景、决策、理由、被否决方案；状态 `proposed` → `accepted` / `deprecated`。
- 架构变化必须同 PR 更新 `docs/architecture.md`；README 面向最终用户，保持中文。
