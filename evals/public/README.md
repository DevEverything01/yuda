# 公开真人语料的模型级评估

`evals/asr/` 的产品场景评估需要你自己的真人录音；这里用公开的真人语料比较**离线模型本身**，用于模型选型（ADR 0009）。音频不进仓库，按需下载。

| 语料 | 内容 | 许可 | 下载（HuggingFace parquet） |
|---|---|---|---|
| ASCEND 测试集 | 真人中英混说对话，1315 句 | CC BY-SA 4.0 | `CAiRE/ASCEND` → `main/test/0000.parquet` |
| FLEURS cmn_hans_cn 测试集 | 真人普通话朗读，945 句 | CC BY 4.0 | `google/fleurs` → `cmn_hans_cn/test/0000.parquet` |

在装好模型的 Linux 机器上：

```bash
mkdir -p ~/asr-bench/corpora && cd ~/asr-bench
curl -fsSL -o corpora/ascend-test.parquet "https://huggingface.co/datasets/CAiRE/ASCEND/resolve/refs%2Fconvert%2Fparquet/main/test/0000.parquet"
curl -fsSL -o corpora/fleurs-cmn-test.parquet "https://huggingface.co/datasets/google/fleurs/resolve/refs%2Fconvert%2Fparquet/cmn_hans_cn/test/0000.parquet"
uv venv .venv && uv pip install --python .venv/bin/python sherpa-onnx soundfile numpy pyarrow
THREADS=4 .venv/bin/python /path/to/yuda/evals/public/eval_corpus.py zipformer ascend
THREADS=4 .venv/bin/python /path/to/yuda/evals/public/eval_corpus.py sensevoice fleurs
```

方法：去标点、英文小写，中文按字、英文按词计算混合错误率（MER）；两个模型都关闭各自的数字规整；参考文本含阿拉伯数字的句子不计入。评估的是模型本身，不含语打的 VAD 分段与文本规整。
