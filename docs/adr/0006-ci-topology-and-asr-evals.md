# ADR 0006: CI 拓扑、最小提交门与 ASR 准确性评估

- 状态：accepted
- 日期：2026-08-24
- 细化：ADR 0005 第 5 条（真机行为的 CI 归属）

## 背景

ADR 0005 定了分层与 mock 策略。新增约束：真机端到端测试必须在 Omarchy 服务器上执行；要有标准 CI；要用多类型真实音频按场景（coding / 日常聊天 / 写邮件等）评估识别准确性；测试要有通用性，一次性测试不得污染 `tests/`；每次提交跑测试但只跑最小集；测试撰写须严谨、覆盖边界。

## 决策

1. **CI 两拓扑**：PR/push 在 hosted runner 跑 T0+T1（ubuntu 默认 features + Arch 容器 `linux-runtime`）；self-hosted Omarchy runner（label `omarchy`）承接 `daemon` 全量 + T3 真机冒烟 + 准确性评估，仅 `workflow_dispatch`/夜间触发，**不阻塞 PR**。
2. **pre-commit 最小门**：`scripts/pre-commit` = `fmt --check` + clippy + 默认 features 的 `cargo test`（秒级）；`make hooks` 启用（`core.hooksPath scripts`）；Linux 开发机可用 `YUDA_PRECOMMIT_FEATURES=linux-runtime` 提升覆盖。全量永远由 CI 承担。
3. **准确性评估独立成 harness**：`evals/asr/` 下语料清单（`corpus.toml`）进仓库、音频本体不进；指标 CER，按场景阈值断言；只在 `daemon` feature + `--ignored` 下运行，不进 PR 门。
4. **一次性测试去 `.scratch/tests/` 或 `/tmp`**，禁止提交；发现值得保留的，按 `tests/AGENTS.md` 重写为契约测试再进 `tests/`。

## 理由

- self-hosted Omarchy runner 是唯一同时具备真实 Hyprland 会话、音频设备、input 组权限的 CI 节点；hosted runner 无法替代真机层。
- 准确性评估是基准而非契约：结果是分布、依赖大语料与真实模型、天然慢——进 PR 门只会制造 flakiness 与红灯疲劳；独立 harness + 报告对比才是正确形态。
- 提交门若跑全量（尤其 sherpa-onnx static 编译），反馈超过一分钟就会被开发者绕过；最小门保持秒级才会被真正遵守。

## 被否决方案

- **hosted runner 模拟 Wayland/Hyprland**：同 ADR 0005，容器内桌面会话不可靠。
- **语料音频入库**：体积与录音版权问题；清单入库已足够复现评估结构。
- **evals 并入 `tests/`**：混淆契约与基准，拖慢 `cargo test`；用户明确要求分离。
- **pre-commit 跑 `--all-features`**：sherpa-onnx 编译重，违背最小门初衷。

## 影响

- 已新增 `.github/workflows/ci.yml`、`scripts/pre-commit`、`evals/AGENTS.md`、`evals/asr/corpus.example.toml`；Makefile 已增 `test`/`test-linux`/`test-daemon`/`check`/`smoke`/`hooks`。
- `.gitignore` 已排除 `/.scratch/`、`/evals/asr/corpus/`、`/evals/asr/reports/`。
- hosted workflow 在 push/PR 上运行默认 feature 与 `linux-runtime` 门；Omarchy job 在 `workflow_dispatch` 与每日 schedule 运行，使用 `self-hosted` + `omarchy` label，且不阻塞 PR。
- 无 `YUDA_E2E_TRIGGER` 时，scheduled Omarchy run 明确 warning 并跳过需要真人输入的 T3；不以固定文本伪造麦克风链路。配置 trigger 后由 `scripts/hyprland_smoke.sh` 执行真实检查。
- ASR 报告写入 `$RUNNER_TEMP` 并上传为 workflow artifact；manifest 和 enable marker 缺一则 warning 跳过，避免把缺数据伪造成通过。
- Omarchy runner 是仓外基础设施，必须处于已登录 Hyprland 会话；本次已在 Omarchy 手工验证 daemon 监听、全 feature 测试、clippy 与公开 ASR 基线。
- `tests/asr_accuracy.rs` harness 已交付；AISHELL-1 公开语料以 Apache-2.0 来源记录存放在 Omarchy 外部目录，未把音频或报告写入仓库。
- 用户自己的 coding/chat/email 真人录音仍需通过 `scripts/record_asr_corpus.sh` 采集；公开朗读语料只能作为声学/模型基线，不能替代产品场景评估。
