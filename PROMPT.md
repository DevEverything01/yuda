# Yuda (语打) — Implementation Prompt

> Working name: **Yuda / 语打** (rename freely). Target: Arch Linux / Omarchy, Hyprland on Wayland. Language: Rust, single-binary daemon.

Please implement a Linux (Arch Linux / Omarchy, Hyprland on Wayland) Chinese-first push-to-talk voice input app in Rust (single binary daemon, working name "Yuda / 语打"), with the following requirements:

## 1. Push-to-talk global hotkey

- Hold the hotkey to record, release to stop, transcribe, and inject the text into the currently focused input field.
- Monitor the hotkey globally by reading `/dev/input/event*` directly via the `evdev` crate (works regardless of which window is focused; handle keyboard hotplug via `udev` enumeration + inotify on `/dev/input`). Do NOT use `EVIOCGRAB` (it would block normal typing); the default hotkey must be a key with no default OS-level action.
- Default hotkey: **Right Ctrl**. Fn is supported only when the keyboard controller actually emits an evdev event for it — on most laptops Fn is firmware-level and invisible to the OS; document this and never hardcode Fn as the only option. The hotkey is configurable in the settings window via a "press the key now" recorder.
- Trigger only when the hotkey is pressed and released **alone** (no other key pressed in between), so normal Ctrl/Fn combos are unaffected. Ignore holds shorter than 300 ms (accidental taps).
- Startup check: if `/dev/input` is inaccessible, print a clear error telling the user to run `sudo usermod -aG input $USER` and re-login.

## 2. Chinese-first streaming ASR

- Default language: **Mandarin Chinese with Chinese-English mixed input (中英混输)**, working out of the box.
- **Primary engine — cloud:** Volcano Engine (火山引擎) Doubao bigmodel streaming ASR over WebSocket (`wss://openspeech.bytedance.com/api/v3/sauc/bigmodel`; headers `X-Api-App-Key` / `X-Api-Access-Key` / `X-Api-Resource-Id`, all configurable). Stream 16 kHz mono PCM s16le in ~100 ms frames; enable ITN + punctuation; render partial results live in the overlay; take the final result on key release. Study the PyPI package `doubao-speech` as the reference client implementation for the protocol.
- **Fallback engine — offline:** sherpa-onnx（官方 Rust API / ONNX bindings）+ **SenseVoice-Small int8** 模型包 `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17`，with silero-vad endpointing, fully local CPU inference; model auto-downloads on first use with progress shown in the overlay.
- Engine mode: `cloud` / `offline` / `auto` (default `auto`: cloud with 2 s connect timeout, transparent fallback to offline on network failure; show a small "离线" badge in the overlay while on fallback).
- Language menu (tray + settings): 中文(中英混输) / English / 粤语 / 日本語 / 한국어 — exactly the SenseVoice-supported set; the cloud engine auto-detects within it. Persist the selection in the config file.
- Audio capture: `cpal`, resampled to 16 kHz mono via `rubato`; feed both the RMS meter (for the waveform) and the ASR engine from the same capture stream.

## 3. Recording overlay — frameless capsule, bottom-center, Wayland layer-shell

- GTK4 + `gtk4-layer-shell`, layer = overlay, exclusive-zone = 0, keyboard-mode = none (focus never leaves the target app), anchored bottom-center, 120 px above the bottom edge.
- Capsule: height **56 px**, corner radius **28 px**, background `rgba(20,20,25,0.85)`; blur is delegated to the compositor — set layer namespace `yuda` and document the Hyprland snippet (`layerrule = blur, yuda` and `layerrule = ignorezero, yuda`).
- Left: **5 vertical bar waveform, 44×32 px**, driven by real-time mic RMS (no hardcoded fake animation):
  - bar weights `[0.5, 0.8, 1.0, 0.75, 0.55]` (center-high, sides-low);
  - per-frame envelope smoothing `level += (target - level) * (target > level ? 0.4 : 0.15)` (attack 40%, release 15%);
  - ±4% random jitter per bar per frame at 60 fps;
  - louder speech → taller bars, silence → near-flat.
- Right: text label, **elastic width 160–560 px**, showing the live partial transcript (older text ellipsized); capsule width animates smoothly (250 ms) as text grows.
- Animations: entry spring **350 ms** (scale 0.9→1.0 + fade-in), width transition **250 ms**, exit scale + fade **220 ms**.
- States: `recording` (waveform live), `transcribing` (cloud final result pending after key release), `refining` (LLM pass, label shows `优化中…`), `error` (red accent + message, auto-dismiss after 1.5 s).

## 4. Text injection — clipboard + simulated paste as the primary path

- Rationale: virtual-keyboard (wtype) CJK injection is unreliable; clipboard paste is the robust Wayland path. Unlike macOS, fcitx5 does NOT intercept Ctrl+V, so no input-method switching dance is needed.
- Sequence: (a) snapshot the current clipboard via `wl-paste -n` (preserve at least `text/plain` and `image/*` MIME types); (b) `wl-copy` the final text; (c) synthesize Ctrl+V via `wtype -M ctrl -k v -m ctrl` (fallback: `ydotool key 29:1 47:1 47:0 29:0`); (d) wait 120 ms; (e) restore the original clipboard snapshot.
- Document the known limitation: rare apps without proper Wayland clipboard sync (some games / XWayland edge cases) may not receive the paste.

## 5. Conservative LLM refinement (optional)

- Any OpenAI-compatible endpoint: configurable `api_base` / `api_key` / `model` in settings (defaults may point at the user's own gateway).
- System prompt, verbatim: "You are a speech-to-text correction filter. Only fix obvious speech-recognition errors: Chinese homophone mistakes (e.g. 他/她/它 by context) and English technical terms mistakenly transcribed as Chinese (配森→Python, 杰森→JSON, 吉特→Git). Never rewrite, polish, translate, add or remove content. If the input looks correct, return it exactly as-is. Return only the corrected text, no explanations."
- Runs after key release when enabled and configured; overlay shows `优化中…` while waiting; **5 s timeout → inject the raw transcript instead**; never block injection on LLM failure.

## 6. Tray, Waybar & settings

- Tray via StatusNotifierItem (`ksni` crate): engine status (云端/离线), language submenu, LLM-refinement on/off toggle, Settings, Quit.
- Waybar: expose state over a Unix socket `$XDG_RUNTIME_DIR/yuda.sock` emitting JSON lines (e.g. `{"text":"🎙 录音中","class":"recording"}`, `{"text":"☁ 云端","class":"idle"}`), plus a documented `custom/yuda` Waybar module config snippet (`return-type: json`, on-click opens settings).
- GTK4 settings window fields: engine mode (cloud/offline/auto); Volcano appid + access token; LLM `api_base` / `api_key` / `model` (the API-key field **must support being fully cleared**); hotkey recorder; language; plus **Test** (runs a fixed sample through the current pipeline and reports end-to-end latency) and **Save** buttons.
- Config at `~/.config/yuda/config.toml` (TOML, `0600` perms; never overwrite an existing config on install/upgrade).

## 7. Daemonization, build & distribution (the Wayland equivalent of LSUIElement)

- Background daemon, no main window; autostart via systemd user unit `yuda.service` (installed, not enabled by default; README documents `systemctl --user enable --now yuda`).
- `cargo build --release` single binary; provide a **Makefile** with `build` / `run` / `install` / `uninstall` / `clean` / `lint` (clippy) / `fmt` targets; `make install` installs binary + systemd unit + default config.
- Provide an **AUR `PKGBUILD`** (`yuda-git`) with deps `wl-clipboard`, `wtype`, `pipewire` (opt-dep `ydotool`); verify it builds in a clean chroot (`extra-x86_64-build`).
- README in Chinese: Omarchy setup walkthrough (input group, deps, Hyprland layerrule blur snippet, Waybar module snippet, Volcano key signup incl. pricing note ≈ ¥2.2/h), screenshots/GIF demo, roadmap noting Phase-2 fcitx5/Rime keyboard integration.
- Acceptance: `cargo clippy -- -D warnings` clean; `cargo test` passing for ASR protocol framing + injection sequencing (mocked); PKGBUILD builds in a clean chroot; end-to-end demo GIF (hold-key → capsule → 上屏) attached.
