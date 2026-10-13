# evals/ 目录协作规范

本目录放**持续性评估（standing evals）**：基准、准确性度量——衡量质量水位，不是逐提交的通过/失败契约。契约测试在 `tests/`（见 `tests/AGENTS.md`），二者不得混放。

## ASR 准确性评估（`evals/asr/`）

用真实音频语料衡量识别准确率，按场景分层。目标：发现「哪个场景退了」，而不是一个总均分。

### 语料组织

- **音频本体不进仓库**（体积 + 录音版权）：放 `evals/asr/corpus/<scenario>/*.wav`，已被 `.gitignore` 排除；Omarchy runner 与开发机各自维护本地副本。
- **清单进仓库**：`evals/asr/corpus.toml`，每条样本声明 `file` / `scenario` / `lang` / `transcript` / 可选 `tags` 与 `max_cer`；示例见 `corpus.example.toml`。
- `transcript` 必须**人工校对**过，是 ground truth；机翻/识别结果禁止当参考答案，合成音不能冒充真人录音。
- 场景标签自由扩展，但基线清单强制覆盖：`coding`（口述代码/技术术语中英混输）、`chat`（日常口语）、`email`（书面长句）。新场景 = 加清单条目 + 建子目录，**不改 harness 代码**。

### 断言与阈值

- 指标用 **CER**（字符级编辑距离 / 参考字符数），中英混输场景按 Unicode 字符计，不按字节或英文单词计。
- 每条样本对 `max_cer` 断言；每个场景对平均 CER 出报告。阈值写在清单里，收紧/放宽都要在 PR 说明理由。
- 基线边界 tag 必须包含：`short`（<2s）、`long`（接近 10min 录音上限）、`technical`、`numbers-punctuation`、`noise`、`silence-trim`。

### 运行

```bash
YUDA_ASR_CONFIG=/path/config.toml cargo test --locked --features sensevoice --test asr_accuracy -- --ignored --nocapture
```

- `tests/asr_accuracy.rs` 是长期维护的 manifest-driven harness，不是一次性测试；它按语言复用 recognizer、按样本算 Unicode-safe CER、检查场景与边界 tag，并写出报告。
- Omarchy runner 只有在同时存在 `evals/asr/corpus.toml` 和 `$HOME/.config/yuda/asr-evals.enabled` 时才由 CI 调用；没有真实录音时只 warning 跳过，不冒充通过。
- 报告写 `evals/asr/reports/<date>.md`（gitignored），含每场景平均 CER 与失败样本明细。
- 音频缺失、manifest 不完整、路径越界、模型初始化失败都必须失败；不要把缺数据改成成功路径。
- 由 CI 的 `omarchy-e2e` job（手动/夜间）执行。回归判断依赖报告与阈值，不把大语料塞进普通 `cargo test`。

### 为什么不在普通测试

准确性评估依赖大文件语料与真实模型，运行慢、结果是分布而非布尔；长期 harness 源码虽位于 `tests/asr_accuracy.rs` 以复用 Cargo test 入口，但音频/报告与普通契约测试严格分离。调试一次性脚本仍只能放 `.scratch/tests/` 或 `/tmp`，不能进仓库。

## 可选公开语料

- **AISHELL-1**：官方 OpenSLR SLR33 页面标注 Apache-2.0，16 kHz 普通话、400 名说话人、带人工 transcript；适合作为中文清晰语音基线。来源：`https://www.openslr.org/33/`。
- **Mozilla Common Voice Chinese (China)**：Mozilla Data Collective 页面标注为中文大陆朗读语音；Common Voice 数据集许可为 CC0-1.0。适合作为公开日常朗读基线，不等同于 coding/email 用户场景。来源：`https://mozilladatacollective.com/datasets/cmqim47x700tunq074za20dq1`。
- 公开语料只能覆盖“真实录音 + transcript + 声学条件”基线；coding 中英混输、邮件口述和当前用户麦克风链路仍需真人补录，不能用公开朗读数据冒充。
- 下载到 Omarchy 的持久目录（例如 `~/.local/share/yuda/evals/public/`），不进入 git；外部 manifest 应记录来源、许可证、下载日期、分片或样本 ID。仓库内 `corpus.toml` 保持产品域真人录音的 metadata contract，不嵌入公开音频。

## 录音采集

在 Omarchy 真人录音机上运行下面的可重复采集器；它读取 `corpus.toml`，按场景逐条录音，全部文件非空后才创建 enable marker：

```bash
scripts/record_asr_corpus.sh
```

重录已有文件才显式设置 `YUDA_ASR_OVERWRITE=1`。采集前检查 `wpctl get-volume @DEFAULT_SOURCE@`，确认实际麦克风未静音；采集后人工复核每条 `transcript` 与音频内容。
