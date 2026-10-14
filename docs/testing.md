# 语打 Yuda · 测试体系

> 决策记录：分层与 mock 策略见 ADR 0005；CI 拓扑、提交门与准确性评估见 ADR 0006；Omarchy VM 端到端验收见 ADR 0008。
> 目录规范：`tests/AGENTS.md`（契约测试）、`evals/AGENTS.md`（准确性评估）。

## 分层

| 层 | 内容 | 运行环境 | 触发 |
|---|---|---|---|
| T0 静态检查 | `cargo fmt --check`；`cargo clippy --all-targets -- -D warnings` | 任意 | pre-commit + CI |
| T1 单元测试 | `src/` 模块内 `#[cfg(test)]`，纯逻辑 | 默认 feature 任意平台；`daemon` 需 Linux | pre-commit + CI |
| T2 mock 集成 | `tests/injection_mock.rs`（PATH 注入假命令）、`tests/cloud_protocol.rs`（进程内 mock WebSocket） | 任意 | CI |
| T3 端到端 | `scripts/omarchy-e2e`：Omarchy 官方 VM 镜像 + QMP 快捷键 + PipeWire 虚拟麦克风 | 带 KVM 的 Linux 宿主机 | 手动 / 夜间（`[self-hosted, kvm]`） |
| T4 打包 | `packaging/PKGBUILD` 在净 chroot 中构建（`extra-x86_64-build`） | Arch 打包机 | 发布前手动 |
| 准确性评估 | `tests/asr_accuracy.rs` + `evals/asr/`：真人语料 × 场景 × CER 阈值 | Omarchy 机器 | 手动 / 夜间 |

## 每次提交的最小门

`scripts/pre-commit`（`make hooks` 启用）：`cargo fmt --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`。默认 feature 只含纯逻辑，秒级完成，macOS 也能跑。Linux 开发机可 `export YUDA_PRECOMMIT_FEATURES=daemon` 提升覆盖。

## T1 覆盖

| 模块 | 覆盖的契约 | feature |
|---|---|---|
| `config` | 默认值中文优先且由 Omarchy 驱动；示例配置与默认值一致；0600 round-trip；拼错字段报错；`set_value` 保留注释并拒绝无效修改 | 默认 |
| `ipc` | 命令文本往返；快照字段与 `docs/ipc.md` 一致；回应不含 null | 默认 |
| `session` | 按住流程只上屏一次；<300 ms 丢弃、=300 ms 接受；切换；忙时拒绝；取消后迟到结果作废；录音上限 | 默认 |
| `text` | 语气词只删独立成分；句末语气词保留句号；词库最长匹配且不级联；分段拼接；标点合并；逐字母缩写合并；技术名词写法只在中英混输里生效；中英间距两种模式；时间日期规整不误伤「有一点」「写一个」；zipformer 原始输出整体规整 | 默认 |
| `injection` | 空文本拒绝；Hyprland 粘贴脚本与 Omarchy 通用粘贴一致 | 默认 |
| `edit` | 替换、删除、撤回、追加（含标点名）、英文指令、自由指令；「九点」匹配「9点」；句首「把」被吞掉的指令；找不到目标与需要 LLM 的报错；正文里的「把…改成…」不在听写模式解析 | 默认 |
| `history` | 追加、倒序读取、压缩、0600、跳过坏行 | 默认 |
| `omarchy` | 菜单块插入出厂文件与用户文件、补逗号、幂等、移除还原、拒绝无法安全修改的文件、JSONC 字符串内容不误删 | 默认 |
| `audio` | 电平映射与 30 Hz 节奏；跨块重采样长度；立体声下混 | `audio` |
| `offline` | 静音/底噪低于能量门 | `sensevoice` |
| `cloud_asr` | 帧头编码、分包、服务端消息解码、热词、语言映射 | `cloud` |
| `refine` | 接受小修正、拒绝解释与大幅改写 | `llm` |

## T2

- `tests/injection_mock.rs`：Hyprland 会话走 `hyprctl eval` 并恢复剪贴板；hyprctl 失败回退 wtype；非 Hyprland 走 wtype → ydotool；全部失败时结果留在剪贴板、不恢复；首次复制失败仍恢复原内容；原剪贴板为空时上屏后清空；仅复制模式不模拟按键。
- `tests/llm_mock.rs`：逐字系统提示词与模型名；Bearer 鉴权头，空密钥不发；偏离过大的纠错被拒绝；自由指令改写；超时上限。
- `tests/cloud_protocol.rs`：握手头与 `X-Tt-Logid`；完整请求 JSON；3200 字节分包与递增序号；末包标志与负序号；中间结果；最终结果；错误帧映射；401；连接超时；finish 超时。

## T3：Omarchy VM 端到端

宿主机（例：印度机器 `hostdzire-in`，8 vCPU / 11 GB，无 GPU）：

```bash
scripts/omarchy-vm fetch      # 下载并校验 nightly.omarchy.org 的 edge 镜像（约 4.5 GB）
scripts/omarchy-e2e           # 构建、安装、跑全部场景；截图与日志在 target/e2e/
scripts/omarchy-e2e --no-build
scripts/omarchy-vm ssh        # 进入 VM（用户/密码 omarchy）
scripts/omarchy-vm screenshot /tmp/x.png
```

场景（详见脚本头部）：A 按住 F9 → foot 终端；B 按住 Right Alt → GTK4 窗口；C Super+Ctrl+X 切换 → 粤语（切换语言后使用 SenseVoice）；D 轻点 F9 不上屏；E 静音不上屏；F 无效云端密钥 → 自动离线；G 按住 Shift+F9 说「把九点改成十点」；H Super+Ctrl+Shift+X 说「删掉上一句」；I 自然语音中英混输（技术名词）；J `yuda edit 最后加上问号`；K AI 自由指令（模拟 OpenAI 兼容端点）。另有剪贴板历史、修饰键是否卡住（Herdr 进程数）与松手到上屏时延（< 3 s）三项全局检查。G、H、I 的语音由宿主机 edge-tts 生成，生成失败时记为 SKIP。

最近一次结果（2026-10-14，印度宿主机，Omarchy edge `4.0.0.r6942.gf5006bd`，Hyprland 0.56.2，安装路径与软件包相同）：集成检查 4 项、场景 A–K 与三项全局检查全部通过，连续多轮稳定；松手到上屏 255 / 254 / 346 / 246 ms。`makepkg` 构建的 `yuda-git` 软件包经 `pacman -U` 安装后同样全部通过。另行手工验证：从旧版 `b3b779a` 升级（`/usr/local` 安装的旧二进制与旧单元、挂在 `default.target` 的旧链接、旧键名与占位密钥的配置），安装器清理残留、迁移配置后服务正常、`yuda doctor` 全部通过、端到端 A–K 全部通过；Omarchy 菜单两处条目与勾选状态、`omarchy-menu-select` 切换语言后 daemon 热加载、卸载后重装、Catppuccin Latte 主题下胶囊配色跟随。

模拟声卡：`YUDA_VM_AUDIO=pa scripts/omarchy-vm up` 给 VM 加一块 Intel HDA 声卡（`hda-micro`），`YUDA_E2E_MIC=hda scripts/omarchy-e2e` 让测试语音从宿主机 PulseAudio 经 QEMU 声卡模拟、内核 `snd-hda-intel` 驱动、ALSA、PipeWire 进入语打。2026-10-14 这条链路上全部场景通过，松手到上屏 161–182 ms；daemon 日志「麦克风已出声」显示按键到第一段音频 43–67 ms。

未覆盖：真实麦克风硬件、实体键盘布局（AltGr）、GPU 渲染下的动画性能——在实体 Omarchy 机器上抽查。云端识别没有真实密钥时只能由 T2 覆盖协议。

## 准确性评估

`evals/asr/corpus.toml` 清单 + 本机音频（不入库），`tests/asr_accuracy.rs` 用与 daemon 相同的 VAD 分段路径逐条计算 CER：

```bash
YUDA_ASR_CONFIG=/path/config.toml cargo test --locked --features sensevoice --test asr_accuracy -- --ignored --nocapture
```

真人语料用 `scripts/record_asr_corpus.sh` 采集。公开语料只能作为声学基线：ASCEND（真人中英混说对话）与 FLEURS（真人普通话朗读）的模型级结果见 ADR 0009。

## 一次性测试

调试用的临时测试放 `.scratch/tests/` 或 `/tmp`（已 gitignore），用完即删；值得保留的按 `tests/AGENTS.md` 重写为契约测试。

## 命令速查

```bash
make check                                   # fmt + clippy + 默认 feature 测试
cargo test --locked --features daemon        # 全量（Linux）
scripts/perf_smoke.sh                        # Linux 全量冒烟 + 关键边界
scripts/omarchy-e2e                          # T3（KVM 宿主机）
```
