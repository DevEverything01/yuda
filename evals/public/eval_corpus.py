"""在公开真人语料上计算混合错误率（MER：中文按字、英文按词）。

用法：THREADS=4 python eval_corpus.py <模型名> <语料名> [最多条数]
模型：sensevoice / zipformer；语料：ascend / fleurs
"""
import io, os, re, sys, time, json, unicodedata
import numpy as np, soundfile as sf, pyarrow.parquet as pq, sherpa_onnx as so

root = os.path.expanduser("~/asr-bench")
models = os.path.expanduser("~/.local/share/yuda/models")
threads = int(os.environ.get("THREADS", "4"))
model_name, corpus = sys.argv[1], sys.argv[2]
limit = int(sys.argv[3]) if len(sys.argv) > 3 else None

def build():
    if model_name == "sensevoice":
        d = f"{models}/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
        return so.OfflineRecognizer.from_sense_voice(model=f"{d}/model.int8.onnx", tokens=f"{d}/tokens.txt", num_threads=threads, language="zh", use_itn=False)
    d = f"{models}/sherpa-onnx-x-asr-zipformer-transducer-zh-en-punct-int8-2026-06-03"
    return so.OfflineRecognizer.from_transducer(encoder=f"{d}/encoder-epoch-99-avg-1.int8.onnx", decoder=f"{d}/decoder-epoch-99-avg-1.onnx", joiner=f"{d}/joiner-epoch-99-avg-1.int8.onnx", tokens=f"{d}/tokens.txt", num_threads=threads)

def tokens(text):
    text = unicodedata.normalize("NFKC", text).lower()
    text = "".join(" " if unicodedata.category(c)[0] in "PSZ" and c != "'" else c for c in text)
    out = []
    for word in text.split():
        buf = ""
        for c in word:
            if "一" <= c <= "鿿" or "㐀" <= c <= "䶿":
                if buf: out.append(buf); buf = ""
                out.append(c)
            else:
                buf += c
        if buf: out.append(buf.strip("'"))
    return [t for t in out if t]

def distance(a, b):
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i] + [0] * len(b)
        for j, y in enumerate(b, 1):
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y))
        prev = cur
    return prev[-1]

path = {"ascend": "corpora/ascend-test.parquet", "fleurs": "corpora/fleurs-cmn-test.parquet"}[corpus]
table = pq.read_table(f"{root}/{path}")
rows = table.to_pylist()[: limit or None]
rec = build()
totals, examples = {}, []
audio_seconds = decode_seconds = 0.0
skipped = 0
for row in rows:
    ref_text = row["raw_transcription"] if corpus == "fleurs" else row["transcription"]
    if re.search(r"[0-9]", ref_text):
        skipped += 1
        continue
    audio, sr = sf.read(io.BytesIO(row["audio"]["bytes"]), dtype="float32")
    if audio.ndim > 1:
        audio = audio.mean(axis=1)
    stream = rec.create_stream()
    stream.accept_waveform(sr, audio)
    t0 = time.time(); rec.decode_stream(stream); decode_seconds += time.time() - t0
    audio_seconds += len(audio) / sr
    hyp, ref = tokens(stream.result.text), tokens(ref_text)
    if not ref:
        skipped += 1
        continue
    group = row.get("language", "zh") if corpus == "ascend" else "zh"
    for key in (group, "all"):
        errors, count, n = totals.get(key, (0, 0, 0))
        totals[key] = (errors + distance(ref, hyp), count + len(ref), n + 1)
    if group == "mixed" and len(examples) < 8:
        examples.append((ref_text, stream.result.text))
summary = {k: {"utterances": n, "ref_tokens": c, "mer": round(e / c, 4)} for k, (e, c, n) in totals.items()}
print(json.dumps({"model": model_name, "corpus": corpus, "skipped_with_digits_or_empty": skipped, "audio_hours": round(audio_seconds / 3600, 3), "rtf": round(decode_seconds / audio_seconds, 3), "results": summary}, ensure_ascii=False))
for ref, hyp in examples:
    print("  ref:", ref, "\n  hyp:", hyp)
