"""在公开真人语料上计算混合错误率（MER：中文按字、英文按词）。

用法：THREADS=4 python eval_corpus.py <模型名> <语料名> [最多条数]
模型：sensevoice / zipformer；语料：ascend / fleurs

SEGMENT=vad 时改走 daemon 的分段路径（与 src/offline.rs 的 StreamingSession 一致）：句前加
LEAD 秒静音（默认 1.0，模拟按下快捷键后才开口），Silero VAD 切句后分段解码。
SEGMENT=vad-legacy 是 2026-10-14 之前的分段方式（句段不补边距，VAD 漏掉的音频直接丢弃），
只用于对比。默认 SEGMENT=whole：整句一次解码，评估模型本身。
"""
import io, os, re, sys, time, json, unicodedata
import numpy as np, soundfile as sf, pyarrow.parquet as pq, sherpa_onnx as so

root = os.path.expanduser(os.environ.get("ASR_BENCH", "~/asr-bench"))
models = os.path.expanduser(os.environ.get("YUDA_MODELS", "~/.local/share/yuda/models"))
threads = int(os.environ.get("THREADS", "4"))
model_name, corpus = sys.argv[1], sys.argv[2]
limit = int(sys.argv[3]) if len(sys.argv) > 3 else None

def build():
    if model_name == "sensevoice":
        d = f"{models}/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
        return so.OfflineRecognizer.from_sense_voice(model=f"{d}/model.int8.onnx", tokens=f"{d}/tokens.txt", num_threads=threads, language="zh", use_itn=False)
    d = f"{models}/sherpa-onnx-x-asr-zipformer-transducer-zh-en-punct-int8-2026-06-03"
    return so.OfflineRecognizer.from_transducer(encoder=f"{d}/encoder-epoch-99-avg-1.int8.onnx", decoder=f"{d}/decoder-epoch-99-avg-1.onnx", joiner=f"{d}/joiner-epoch-99-avg-1.int8.onnx", tokens=f"{d}/tokens.txt", num_threads=threads)

segment = os.environ.get("SEGMENT", "whole")
lead = float(os.environ.get("LEAD", "1.0"))
SR = 16000
SEGMENT_PADDING = SR // 5
MIN_SPEECH_DBFS = -45.0

def peak_dbfs(x):
    w = SR // 10
    peaks = [np.sqrt(np.mean(x[i:i + w] ** 2)) for i in range(0, len(x), w)]
    return 20 * np.log10(max(max(peaks, default=0.0), 1e-9))

def vad_decode(rec, audio):
    """src/offline.rs 的 StreamingSession：512 样本一窗送入 VAD，句段解码，松手时处理剩余音频。"""
    cfg = so.VadModelConfig()
    cfg.silero_vad.model = f"{models}/silero_vad.onnx"
    cfg.silero_vad.threshold = 0.5
    cfg.silero_vad.min_silence_duration = 0.5
    cfg.silero_vad.min_speech_duration = 0.25
    cfg.silero_vad.window_size = 512
    cfg.sample_rate = SR
    cfg.num_threads = 1
    vad = so.VoiceActivityDetector(cfg, buffer_size_in_seconds=120)
    audio = np.concatenate([np.zeros(int(lead * SR), dtype=np.float32), audio])
    legacy = segment == "vad-legacy"
    texts, prev_end = [], 0

    def decode(a, b):
        s = rec.create_stream()
        s.accept_waveform(SR, audio[a:b])
        rec.decode_stream(s)
        texts.append(s.result.text.strip())

    def drain():
        nonlocal prev_end
        while not vad.empty():
            seg = vad.front
            start, end = seg.start, seg.start + len(seg.samples)
            if legacy:
                a, b = max(start, prev_end), end
            else:
                a = min(max(start - SEGMENT_PADDING, prev_end), len(audio))
                if peak_dbfs(audio[prev_end:a]) >= MIN_SPEECH_DBFS:
                    a = prev_end
                b = max(min(end + SEGMENT_PADDING, len(audio)), a)
            decode(a, b)
            prev_end = max(prev_end, b)
            vad.pop()

    whole = len(audio) // 512 * 512
    for i in range(0, whole, 512):
        vad.accept_waveform(audio[i:i + 512])
        drain()
    if whole < len(audio):
        vad.accept_waveform(audio[whole:])
    vad.flush()
    drain()
    if legacy:
        if not texts and peak_dbfs(audio) >= MIN_SPEECH_DBFS:
            decode(0, len(audio))
    elif peak_dbfs(audio[prev_end:]) >= MIN_SPEECH_DBFS:
        decode(prev_end, len(audio))
    return " ".join(texts)

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
    t0 = time.time()
    if segment == "whole":
        stream = rec.create_stream()
        stream.accept_waveform(sr, audio)
        rec.decode_stream(stream)
        text = stream.result.text
    else:
        assert sr == SR, sr
        text = vad_decode(rec, audio.astype(np.float32))
    decode_seconds += time.time() - t0
    audio_seconds += len(audio) / sr
    hyp, ref = tokens(text), tokens(ref_text)
    if not ref:
        skipped += 1
        continue
    group = row.get("language", "zh") if corpus == "ascend" else "zh"
    for key in (group, "all"):
        errors, count, n = totals.get(key, (0, 0, 0))
        totals[key] = (errors + distance(ref, hyp), count + len(ref), n + 1)
    if group == "mixed" and len(examples) < 8:
        examples.append((ref_text, text))
summary = {k: {"utterances": n, "ref_tokens": c, "mer": round(e / c, 4)} for k, (e, c, n) in totals.items()}
print(json.dumps({"model": model_name, "corpus": corpus, "segment": segment, "skipped_with_digits_or_empty": skipped, "audio_hours": round(audio_seconds / 3600, 3), "rtf": round(decode_seconds / audio_seconds, 3), "results": summary}, ensure_ascii=False))
for ref, hyp in examples:
    print("  ref:", ref, "\n  hyp:", hyp)
