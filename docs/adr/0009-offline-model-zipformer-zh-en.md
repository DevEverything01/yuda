# ADR 0009: 中英离线识别改用 zipformer 中英模型，SenseVoice 负责粤日韩

- 状态：accepted
- 日期：2026-10-14
- 细化：ADR 0002（混合 ASR）中的离线模型选择

## 背景

对标豆包输入法时，中英混输里的技术名词是最明显的差距。用 edge-tts 合成 8 句含技术名词的中英混输语音（Python、JSON、API、GitHub、Rust、Docker、Node、React、Zoom、MacBook、PR、main、CI、Omarchy、Hyprland、Lua），加上 SenseVoice 自带的中文、英文测试音频，在 Omarchy VM（6 vCPU，无 GPU，2 线程）里横向评测 sherpa-onnx 官方发布的离线模型：

| 模型 | 大小 | RTF | 技术名词表现 |
|---|---|---|---|
| SenseVoice-Small int8（原默认） | 230 MB | 0.060 | GitHub → gek、Rust → lastst、Zoom → zom、Hyprland → heland；英文全小写 |
| sense-voice-funasr-nano int8 | 254 MB | 0.061 | JSON → j、API → ap、GitHub → gub；数字不规整 |
| **x-asr zipformer transducer zh-en punct int8** | 168 MB | **0.041** | Python、JSON、GitHub、Rust、Zoom、MacBook、main、CI、Omarchy、Lua 正确；缩写偶尔逐字母输出（A P I） |
| FireRed2-CTC zh_en int8 | 611 MB | 0.436 | 大量漏字（PI、J N、DOER） |
| Qwen3-ASR 0.6B int8 | 711 MB | 0.393 | 多数正确，但 Docker → Doctor、Node → Note；每句约 2 秒，加载 9.5 秒 |

### 真人语料复核

合成语音只能说明技术名词的趋势。之后用两套公开的**真人录音**语料复核（脚本 `evals/public/eval_corpus.py`，用法见同目录 README。方法：去标点、英文小写，中文按字、英文按词计算混合错误率 MER；两个模型都关闭各自的数字规整；参考文本含阿拉伯数字的句子不计入，避免写法差异）：

| 语料 | 内容 | zipformer 中英 | SenseVoice-Small |
|---|---|---|---|
| ASCEND 测试集（HKUST，CC BY-SA 4.0），1315 句，0.92 小时 | 真人中英混说对话 · 全部 | **12.2%** | 13.7% |
| | 　中英混说（373 句） | **11.8%** | 12.9% |
| | 　纯英文（257 句） | **23.6%** | 37.0% |
| | 　纯中文（685 句） | 9.6% | **8.6%** |
| FLEURS cmn_hans_cn 测试集（Google，CC BY 4.0），750 句，2.36 小时 | 真人普通话朗读 | **4.00%** | 4.17% |

真人语料上 zipformer 整体更好，英文与中英混说明显更好；纯中文对话略差约 1 个百分点，朗读普通话略好。RTF 0.03–0.05（4 线程）。

### daemon 分段路径复核（2026-10-14）

上表是整句一次解码，评估模型本身。daemon 实际走 VAD 分段（ADR 0007 第 6 条）。用同一脚本的 `SEGMENT=vad` 模式复核：句前加 1 秒静音，模拟按下快捷键后才开口；分段方式与 `src/offline.rs` 一致。

| 语料（zipformer） | 整句解码（模型上限） | 旧分段 | 新分段 |
|---|---|---|---|
| ASCEND 全部 | 12.2% | 12.8% | 12.6% |
| 　中英混说 | 11.8% | 12.7% | 12.3% |
| FLEURS 朗读普通话 | 3.95% | **5.73%** | **4.10%** |

旧分段有两个问题：
- **只解码 VAD 句段本身。** Silero VAD 的句段起点随 512 样本窗口与语音开头的相位变化，实测比语音开头晚 0.14 s，第一个字被切掉。VM 声卡链路验收里，「把九点改成十点」变成了「九点改成十点」。
- **VAD 漏判的轻声被丢弃。** 停顿后小声补的半句没有被判为语音，就不会出现在最终结果里，实时预览里却能看到。

新分段的规则：VAD 只决定在哪里切，不决定丢掉什么。
- 句段前后各补 0.2 s 再解码。
- 与上一段之间的音频超过能量门（-45 dBFS）时，并入下一段一起解码。
- 松手时，最后一段之后的音频超过能量门时也解码。

## 决策

1. 中文（中英混输）与英文默认使用 `sherpa-onnx-x-asr-zipformer-transducer-zh-en-punct-int8-2026-06-03`。
2. 粤语、日语、韩语使用 SenseVoice-Small（x-asr 只支持中英）。离线引擎按目录文件识别模型族，按语言选择模型；同一时间只加载一个。
3. 文本规整补齐 zipformer 的输出差异：逐字母缩写合并（A P I → API）、技术名词规范写法、中英间距（默认不加空格）、只在时间日期语境里的数字规整（下午三点 → 下午3点）。
4. 不采用官方 `itn_zh_number.fst`：它把所有「一」改成「1」（写1个、跑1下）。
5. 安装器下载两个模型；已有配置里中文模型仍是旧默认值（SenseVoice）时自动迁移。

## 理由

- 技术名词正确率显著提升，同时更快（RTF 0.041 对 0.060），松手到上屏仍约 250 ms。
- transducer 结构为将来的离线热词（contextual biasing）留出空间；SenseVoice 不支持热词。
- Qwen3-ASR 质量接近但慢 10 倍，不适合按住说话的即时体验。

## 被否决的方案

- **继续用 SenseVoice + 更大的替换词库**：误识别（gek、lastst）无法靠大小写修正。
- **Qwen3-ASR 作为默认**：延迟 2 秒级，模型 700 MB。
- **两遍识别（SenseVoice 预览 + Qwen3 定稿）**：松手后要等 2 秒，违背即时上屏。

## 影响

- 下载体积从约 160 MB 增加到约 290 MB（两个模型）。
- 合成语音只用于选型对比与端到端功能测试；真人语料的准确率评估（ADR 0006）仍待采集。
- 已知误识别：Hyprland → Hipalent；zh.wav 的「开放」被识别为「菜饭」/「开饭」。用户可用 `[text.replacements]` 词库修正。
