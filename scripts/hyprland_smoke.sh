#!/usr/bin/env bash
# 真实 Hyprland T3 冒烟：启动 offline daemon，由操作者说一句话并按住默认热键。
# 这是长期可复用的真机检查，不是一次性测试；一次性实验请放 .scratch/tests/。
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        printf '缺少命令：%s\n' "$1" >&2
        exit 2
    }
}

require_command wl-copy
require_command wl-paste
require_command cargo
if ! command -v wtype >/dev/null 2>&1 && ! command -v ydotool >/dev/null 2>&1; then
    printf '%s\n' '缺少 wtype 与 ydotool；至少安装一个 Ctrl+V 后端。' >&2
    exit 2
fi

: "${XDG_RUNTIME_DIR:?需要在 Hyprland 会话中运行：XDG_RUNTIME_DIR 未设置}"
: "${WAYLAND_DISPLAY:?需要在 Hyprland 会话中运行：WAYLAND_DISPLAY 未设置}"

shopt -s nullglob
input_devices=(/dev/input/event*)
if ((${#input_devices[@]} == 0)); then
    printf '%s\n' '未发现 /dev/input/event*；请确认 input 组权限与 evdev 设备。' >&2
    exit 2
fi
for device in "${input_devices[@]}"; do
    if [[ -r "$device" ]]; then
        input_readable=1
        break
    fi
done
if [[ "${input_readable:-0}" != 1 ]]; then
    printf '%s\n' '无法读取 /dev/input/event*；请执行 sudo usermod -aG input $USER 后重新登录。' >&2
    exit 2
fi

model_root="${YUDA_E2E_MODEL_DIR:-$HOME/.local/share/yuda/models}"
if [[ ! -d "$model_root" ]]; then
    printf 'SenseVoice 模型目录不存在：%s\n' "$model_root" >&2
    exit 2
fi

if [[ -n "${YUDA_BIN:-}" ]]; then
    binary="$YUDA_BIN"
else
    cargo build --locked --features daemon --bin yuda
    binary="target/debug/yuda"
fi
if [[ ! -x "$binary" ]]; then
    printf '找不到 daemon 二进制：%s\n' "$binary" >&2
    exit 2
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/yuda-e2e.XXXXXX")
tmp_home="$work_dir/home"
log_file="$work_dir/daemon.log"
before_file="$work_dir/clipboard-before"
after_file="$work_dir/clipboard-after"
types_file="$work_dir/clipboard-types"
mkdir -p "$tmp_home/.config/yuda"

before_mime=''
if wl-paste --list-types >"$types_file" 2>/dev/null; then
    while IFS= read -r mime; do
        case "$mime" in
            text/plain|text/plain\;charset=utf-8|image/*)
                before_mime="$mime"
                break
                ;;
        esac
    done <"$types_file"
fi
if [[ -n "$before_mime" ]]; then
    wl-paste --no-newline --type "$before_mime" >"$before_file" 2>/dev/null || before_mime=''
fi

restore_clipboard() {
    if [[ -n "$before_mime" && -f "$before_file" ]]; then
        wl-copy --type "$before_mime" <"$before_file" >/dev/null 2>&1 || true
    else
        wl-copy --clear >/dev/null 2>&1 || true
    fi
}

daemon_pid=''
cleanup() {
    status=$?
    trap - EXIT INT TERM
    if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
        kill "$daemon_pid" 2>/dev/null || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    restore_clipboard
    rm -rf "$work_dir"
    exit "$status"
}
trap cleanup EXIT INT TERM

cat >"$tmp_home/.config/yuda/config.toml" <<EOF
engine = "offline"
language = "zh"
hotkey = "KEY_RIGHTCTRL"

[offline]
model_dir = "$model_root"
language = "zh"
EOF

HOME="$tmp_home" RUST_LOG="${RUST_LOG:-info}" "$binary" --daemon >"$log_file" 2>&1 &
daemon_pid=$!

log_contains() {
    local needle=$1 line
    [[ -f "$log_file" ]] || return 1
    while IFS= read -r line; do
        case "$line" in
            *"$needle"*) return 0 ;;
        esac
    done <"$log_file"
    return 1
}

ready_deadline=$((SECONDS + 20))
while ((SECONDS < ready_deadline)); do
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
        printf '%s\n' 'daemon 启动失败，日志：' >&2
        cat "$log_file" >&2
        exit 1
    fi
    if log_contains '已监听'; then
        break
    fi
    sleep 1
done
if ! log_contains '已监听'; then
    printf '%s\n' 'daemon 20 秒内未进入监听状态，日志：' >&2
    cat "$log_file" >&2
    exit 1
fi

sentinel="yuda-e2e-${BASHPID}-$(date +%s)"
printf '%s' "$sentinel" | wl-copy --type text/plain;charset=utf-8

printf '%s\n' 'T3 已准备：请在 30 秒内按住右 Ctrl，说一句完整中文（可含 English/Python/JSON），然后松开。'
printf '%s\n' '自动 runner 可设置 YUDA_E2E_TRIGGER，在此处并行驱动真实输入；不能用固定文本替代麦克风。'

if [[ -n "${YUDA_E2E_TRIGGER:-}" ]]; then
    bash -c "$YUDA_E2E_TRIGGER" &
    trigger_pid=$!
else
    trigger_pid=''
fi

success=0
deadline=$((SECONDS + ${YUDA_E2E_TIMEOUT_SECONDS:-30}))
while ((SECONDS < deadline)); do
    if log_contains '中文文本已上屏'; then
        success=1
        break
    fi
    if ! kill -0 "$daemon_pid" 2>/dev/null; then
        break
    fi
    sleep 1
done
if [[ -n "${trigger_pid:-}" ]] && kill -0 "$trigger_pid" 2>/dev/null; then
    kill "$trigger_pid" 2>/dev/null || true
fi

if ((success == 0)); then
    printf '%s\n' '未观察到「中文文本已上屏」；完整 daemon 日志：' >&2
    cat "$log_file" >&2
    exit 1
fi

if [[ -n "$before_mime" ]]; then
    wl-paste --no-newline --type "$before_mime" >"$after_file"
    if ! cmp -s "$before_file" "$after_file"; then
        printf '%s\n' '上屏后剪贴板未恢复原内容。' >&2
        exit 1
    fi
fi

printf '%s\n' 'T3 通过：真实 daemon 监听、录音、SenseVoice 转写、Wayland 上屏成功，原剪贴板已恢复。'
