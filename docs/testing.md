# 语打 Yuda · 测试体系

> 决策记录：分层与 mock 策略见 `docs/adr/0005-testing-strategy.md`，CI 拓扑 / 提交门 / 准确性评估见 `docs/adr/0006-ci-topology-and-asr-evals.md`。
> 目录强制规范：`tests/AGENTS.md`（契约测试）、`evals/AGENTS.md`（准确性评估）。
> 本文档区分【现状】与【待办】；没有真实语料或未实现业务模块的部分明确标记，不以跳过代替通过。

## 分层总览

| 层 | 内容 | 运行环境 | 触发 |
|---|---|---|---|
| T0 静态检查 | `cargo fmt --check`；`cargo clippy -- -D warnings` | 任意 | pre-commit + CI 每个 job |
| T1 单元测试 | `src/` 模块内 `#[cfg(test)]`，只测纯逻辑 | 按 feature gate 分平台 | pre-commit（最小集）+ CI |
| T2 mock 集成 | `tests/`：外部命令 PATH 注入、协议 mock server | 任意（gated 部分 Linux） | CI |
| T3 真机冒烟 | `scripts/hyprland_smoke.sh`：真实热键 + 录音 + 上屏 + 剪贴板往返断言 | **Omarchy 服务器**（真实 Hyprland 会话） | CI `omarchy-e2e` job，手动/夜间触发；无 `YUDA_E2E_TRIGGER` 时明确 warning 跳过非交互输入 |
| T4 打包验证 | `extra-x86_64-build` 净 chroot 构建 PKGBUILD | Arch 打包机 | 发布前手动 |
| 准确性评估 | `evals/asr/` + `tests/asr_accuracy.rs`：真实音频 × 场景 × CER 阈值 | Omarchy 服务器（需本地语料和模型） | CI `omarchy-e2e` job，显式启用后手动/夜间 |

## 每次提交的最小门【现状】

`scripts/pre-commit`（`make hooks` 一次性启用）：

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked                  # 默认 features，秒级
```

- 设计目标：反馈 < 1 分钟，全量永远由 CI 承担。门太重就会被绕过，不如小而必跑。
- Linux 开发机提升覆盖：`export YUDA_PRECOMMIT_FEATURES=linux-runtime`（gated 模块一并跑）。
- 临时跳过有 `git commit --no-verify`，但 CI 红灯照样拦——别把跳过当常态。

## CI 拓扑【现状：hosted 层已配置；Omarchy 层已手工验证】

`.github/workflows/ci.yml`：

| job | runner | 内容 | 阻塞 PR |
|---|---|---|---|
| `fmt` | ubuntu | `cargo fmt --check` | 是 |
| `default-features` | ubuntu | clippy + `cargo test`（默认 features） | 是 |
| `linux-runtime` | Arch 容器 | clippy + `cargo test --features linux-runtime` | 是 |
| `omarchy-e2e` | **self-hosted, label `omarchy`** | `daemon` 全量 + `perf_smoke.sh` + 有触发器时 T3 + 已启用时准确性评估 | 否（仅手动/夜间触发） |

Omarchy 会话已手工验证 runner 所需的 `WAYLAND_DISPLAY`、`HYPRLAND_INSTANCE_SIGNATURE`、`XDG_RUNTIME_DIR` 与 DBUS 环境，并实际启动 daemon；GitHub self-hosted runner 的注册与 online 状态属于部署方基础设施，workflow 要求 `self-hosted` + `omarchy` label。无 `YUDA_E2E_TRIGGER` 的 scheduled run 只跳过需要真人输入的 T3，不伪造通过。

ASR job 只有同时存在 manifest 和 `$HOME/.config/yuda/asr-evals.enabled` 才执行；报告写入 `$RUNNER_TEMP` 并作为 workflow artifact 上传。没有实际录音时显示 warning 并跳过，不伪造通过。`YUDA_ASR_MANIFEST`、`YUDA_ASR_MODEL_DIR` 等通过 repository variables 注入。

## ASR 准确性评估【现状：harness 已交付；公开基线已在 Omarchy 跑通】
- 位置 `evals/asr/`，与普通 `tests/` 契约分离：评估是基准不是契约，结果是分布和报告。
- 清单 `evals/asr/corpus.toml` 已覆盖 `coding` / `chat` / `email`，并强制要求 `short` / `long` / `technical` / `numbers-punctuation` / `noise` / `silence-trim` 六类 tag。
- 音频本体放 `evals/asr/corpus/<scenario>/`，gitignore；必须是真人录音且 `transcript` 人工校对，禁止合成音或把识别结果当 ground truth。公开 AISHELL-1 音频仅存放在 Omarchy 外部目录，不进仓库。
- `tests/asr_accuracy.rs` 按语言复用 recognizer，逐样本算 Unicode-safe CER、按 `max_cer` 失败，生成按场景汇总报告；文件缺失、manifest 越界路径、缺场景/边界 tag 都直接报错。
- 运行：`YUDA_ASR_CONFIG=/path/config.toml cargo test --locked --features daemon --test asr_accuracy -- --ignored --nocapture`；不设配置时使用 `Config::default()`。

2026-08-24 在 Omarchy 使用 AISHELL-1 / OpenSLR SLR33（Apache-2.0，外部目录）跑通公开声学基线：`chat` 5 条平均 CER `0.1069`、`coding` 3 条 `0.0748`、`email` 4 条 `0.1621`，12 条全部通过清单阈值。该基线是朗读语音，不代表用户的 coding 中英混输、邮件口述或当前麦克风链路；后者仍需 `scripts/record_asr_corpus.sh` 采集真人样本。

## 一次性测试纪律【现状】

调试/排查/验证想法的临时测试放 `.scratch/tests/` 或 `/tmp`（已 gitignore），用完即删；值得保留的按 `tests/AGENTS.md` 重写为契约测试再提交。禁止把一次性测试原样塞进 `tests/`。

## T1 覆盖盘点（2026-08-24）

| 模块 | 已有用例 | 所需 feature |
|---|---|---|
| `app` | 状态机：录音到 Ready 且只注入一次、键盘文本进 Ready、空文本拒注入、错误可复位 | 默认 |
| `config` | 默认值中文优先 + TOML round-trip、空 LLM key 序列化保留、离线模型路径拼接 | 默认 |
| `injection` | 上屏序列含快照/恢复步骤、ClipboardOnly 兜底显式化（纯函数） | 默认 |
| `ui` | idle/recording 快照内容 | 默认 |
| `audio` | PcmBuffer 容量与预分配、WAV 头与采样写入 | `linux-runtime` |
| `hotkey` | HotkeyGate 单独按下接受、夹键取消 | `linux-runtime` |
| `offline_asr` | SenseVoice 语言集合校验、模型缺失报错 | `sensevoice` |

【边界覆盖现状】

- `hotkey` 已覆盖 `<300ms` 与 `=300ms` 的共享判定函数边界。
- `config` 已覆盖保存后 Unix 权限 `0600` 与损坏 TOML 的路径化错误信息。
- `injection` 的真实外部命令缺失 / 非零退出由 `tests/injection_mock.rs` 的 PATH mock 覆盖，并在默认 feature 门执行。

## T2 mock 集成【现状：注入契约已提交；云端协议待业务模块】

- `tests/injection_mock.rs` 已覆盖 PATH 注入假 `wl-copy`/`wl-paste`/`wtype`/`ydotool`、Unicode 文本、严格顺序、wtype→ydotool 回退、首个 copy 失败仍恢复；默认 features 可跑。
- `tests/cloud_protocol.rs` 待云端 ASR 模块落地后交付，必须用本地 mock WebSocket server 覆盖握手头、~100ms 分帧、final 解析、2s 超时回退。

## 命令速查

```bash
make check                     # fmt + lint + 默认 features test
make hooks                     # 启用 pre-commit 最小门
cargo test --locked --features linux-runtime          # T1+T2，Linux
scripts/perf_smoke.sh                        # Linux 本机全量冒烟
scripts/hyprland_smoke.sh                    # T3，Omarchy 真机；需人工说话
scripts/record_asr_corpus.sh                 # 采集 manifest 驱动的真人音频（不入库）
YUDA_ASR_CONFIG=/path/config.toml cargo test --locked --features daemon --test asr_accuracy -- --ignored --nocapture
extra-x86_64-build                           # T4，发布前
```
