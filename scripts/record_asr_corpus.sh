#!/usr/bin/env bash
# 交互式采集 manifest 中的真人语料；音频落在 gitignored corpus/，不进 tests/。
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null 2>&1 || { printf '%s\n' '需要 python3（解析 corpus.toml）。' >&2; exit 2; }
command -v pw-record >/dev/null 2>&1 || { printf '%s\n' '需要 pw-record（PipeWire 录音）。' >&2; exit 2; }

manifest="${YUDA_ASR_MANIFEST:-evals/asr/corpus.toml}"
[[ -f "$manifest" ]] || { printf '找不到 manifest：%s\n' "$manifest" >&2; exit 2; }
manifest_dir=$(dirname "$manifest")
overwrite="${YUDA_ASR_OVERWRITE:-0}"
record_pid=''

cleanup_recording() {
    if [[ -n "$record_pid" ]] && kill -0 "$record_pid" 2>/dev/null; then
        kill -INT "$record_pid" 2>/dev/null || true
        wait "$record_pid" 2>/dev/null || true
    fi
}
trap cleanup_recording EXIT INT TERM

sample_count=0
while IFS=$'\t' read -r file scenario transcript; do
    [[ -n "$file" ]] || continue
    case "$file" in
        /*|../*|*/../*|*/..)
            printf '拒绝越界语料路径：%s\n' "$file" >&2
            exit 2
            ;;
    esac
    path="$manifest_dir/$file"
    mkdir -p "$(dirname "$path")"
    sample_count=$((sample_count + 1))

    if [[ -s "$path" && "$overwrite" != 1 ]]; then
        printf '[%d] 已存在，跳过：%s (%s)\n' "$sample_count" "$path" "$scenario"
        continue
    fi
    rm -f "$path"
    printf '\n[%d] 场景：%s\n文件：%s\n参考文本：%s\n' "$sample_count" "$scenario" "$path" "$transcript"
    printf '%s\n' '请确认输入设备已选中且未静音；回车开始录音。'
    read -r
    pw-record --target @DEFAULT_SOURCE@ --rate 16000 --channels 1 --format s16 "$path" >/dev/null 2>&1 &
    record_pid=$!
    printf '%s\n' '请真人朗读上面的参考文本；说完后回车停止。'
    read -r
    kill -INT "$record_pid" 2>/dev/null || true
    wait "$record_pid" 2>/dev/null || true
    record_pid=''
    [[ -s "$path" ]] || { printf '录音为空：%s\n' "$path" >&2; exit 1; }
done < <(python3 - "$manifest" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as handle:
    manifest = tomllib.load(handle)
for sample in manifest.get("sample", []):
    print(f"{sample['file']}\t{sample['scenario']}\t{sample['transcript']}")
PY
)

((sample_count > 0)) || { printf '%s\n' 'manifest 没有样本。' >&2; exit 2; }

marker="${YUDA_ASR_ENABLE_FILE:-$HOME/.config/yuda/asr-evals.enabled}"
mkdir -p "$(dirname "$marker")"
printf 'enabled\n' >"$marker"
printf '已完成 %d 条真人录音，评估 enable marker：%s\n' "$sample_count" "$marker"
