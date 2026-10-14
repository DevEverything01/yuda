# 语打 Yuda

**Omarchy 上的中文语音输入：按住说话，松手上屏。** 对标豆包输入法的语音体验，以 Omarchy 原生听写后端的形式接入——快捷键、菜单、顶栏、主题都和 Omarchy 一体。

- 按住 **Right Alt** 或 **F9** 说话，松开即上屏；**Super + Ctrl + X** 切换录音。
- 说错了？按住 **Shift + F9** 说「把九点改成十点」「删掉上一句」，上一段直接改好。
- 屏幕底部的录音胶囊实时显示波形和识别中的文字，颜色跟随 Omarchy 主题。
- 云端用火山引擎豆包语音大模型（与豆包输入法同款）；断网或未配置时自动用本地中英模型，**不联网也能用**，技术名词（Python、JSON、GitHub）照样识别。

| 录音中（Tokyo Night） | 录音中（Catppuccin Latte） | 语音修改 | 云端不可用，自动离线 |
|---|---|---|---|
| ![录音胶囊](docs/images/capsule-tokyo-night.png) | ![浅色主题](docs/images/capsule-catppuccin-latte.png) | ![修改徽标](docs/images/capsule-edit.png) | ![离线徽标](docs/images/capsule-offline.png) |

| Setup › Defaults › Dictation | Setup › Yuda 语打 |
|---|---|
| ![默认听写后端](docs/images/menu-default-dictation.png) | ![语打设置菜单](docs/images/menu-yuda.png) |

以上截图来自 Omarchy 官方 edge VM 镜像中的真实运行（`scripts/omarchy-e2e`）。

## 与豆包输入法对照

| 能力 | 语打 | 说明 |
|---|---|---|
| 按住说话、松手上屏 | ✓ | Omarchy 统一听写快捷键；按住少于 300 ms 视为误触 |
| 点按切换录音 | ✓ | Super + Ctrl + X，或点击顶栏麦克风 |
| 边说边出字 | ✓ | 离线：VAD 分句 + 尾段 0.4 s 刷新；云端：流式结果 |
| 自动标点、数字规整 | ✓ | 「下午三点」→「下午3点」；「有一点」「写一个」不受影响 |
| 中英混输、技术名词 | ✓ | 实测「我们用Python写一个解析JSON的脚本，下午3点发给你。」；`A P I` 合并为 `API` |
| 真人语音准确率 | ✓ | 离线模型在公开真人语料上：中英混说对话 MER 11.8%，普通话朗读 CER 4.0%（ADR 0009） |
| 粤语 / English / 日本語 / 한국어 | ✓ | Setup › Yuda 语打 › 识别语言 |
| 去口语化（嗯、呃） | ✓ | 只删不可能是正文的语气词，「额外」「好啊」不受影响 |
| 个人词库 | ✓ | `[text.replacements]`，例如 `"配森" = "Python"` |
| 热词 | ✓ | 云端引擎 `hotwords` |
| AI 纠错 | ✓ | 任意 OpenAI 兼容端点；只修同音字和误识别的术语，超时直接上屏原文 |
| 豆包大模型识别 | ◐ | 按官方协议实现，本地协议测试与 401 降级路径已验证；填入密钥后用 `yuda cloud-test` 一条命令联调 |
| 离线识别 | ✓ | 中英：zipformer 中英模型；粤日韩：SenseVoice-Small；本机 CPU，松手到上屏实测约 0.25 秒 |
| 识别历史、重新上屏 | ✓ | 只存本机，`yuda history`、菜单「历史记录 · 重新上屏」 |
| 终端里也能上屏 | ✓ | 与 Omarchy 的 Super+V 同一路径，终端自动用 Ctrl+Shift+V |
| 不污染剪贴板 | ✓ | 上屏后恢复原剪贴板；临时内容标记为敏感，不进剪贴板历史 |
| 语音修改上一段 | ✓ | 按住 Shift + F9：「把 A 改成 B」「删掉 X」「删掉上一句」「最后加上问号」；开启 AI 纠错后支持「改得正式一点」「翻译成英文」 |

## 安装

需要 **Omarchy 4 的 edge 版本**（带可插拔听写后端；`omarchy-dictation-use` 命令存在即可）。稳定版 4.0.4 还没有这个接口，安装器会提示先更新。

从源码安装：

```bash
git clone https://github.com/DevEverything01/yuda.git
cd yuda
omarchy-pkg-add base-devel rust clang cmake alsa-lib pkgconf
make build
sudo make install PREFIX=/usr
omarchy-install-dictation-yuda
```

`packaging/PKGBUILD` 是 `yuda-git` 的 AUR 包定义（尚未发布到 AUR），也可以在 `packaging/` 目录里用 `makepkg -si` 安装。

`omarchy-install-dictation-yuda` 会：

1. 写入默认配置 `~/.config/yuda/config.toml`（权限 0600，已有配置不覆盖）；
2. 下载离线模型：中英 zipformer（约 130 MB）、粤日韩 SenseVoice-Small（约 160 MB）与 Silero VAD，全部 sha256 校验；
3. 询问是否填写火山引擎密钥（可跳过）；
4. 启用 `yuda.service`；
5. 安装录音浮层插件 `yuda.dictation`、合并 Omarchy 菜单条目，并安装语音修改快捷键 `~/.config/yuda/shortcuts.lua`；
6. 把 Omarchy 默认听写后端切换为语打。

之后也可以在 Omarchy 菜单 **Setup › Defaults › Dictation** 里在 Superwhisper、Voxtype 和 **Yuda 语打** 之间切换。

## 使用

| 操作 | 效果 |
|---|---|
| 按住 Right Alt 或 F9 | 录音；松开后识别并上屏 |
| Super + Ctrl + X | 开始 / 结束录音 |
| 点击顶栏麦克风 | 同上 |
| 按住 Shift + F9 | 说一条修改指令，作用于上一段（见下） |
| Super + Ctrl + Shift + X | 开始 / 结束修改指令 |
| Setup › Yuda 语打 › 历史记录 | 选一条重新上屏 |

### 语音修改

上一段文字上屏后（10 分钟内、焦点还在同一个窗口），按住 **Shift + F9** 说：

| 说 | 效果 |
|---|---|
| 把九点改成十点 / 把张三换成李四 | 替换（「把」可以省略；「九点」能匹配识别出的「9点」） |
| 删掉「其实」 / 把其实删掉 | 删除词语 |
| 删掉上一句 / 撤销 | 撤回整段 |
| 最后加上问号 / 开头加上你好 | 追加（「句号」「逗号」「问号」自动变成标点） |
| 改得正式一点 / 翻译成英文 | 开启 AI 纠错后交给 LLM 改写 |

语打会删掉上一段（按字符数退格），再粘贴改好的版本，可以连续修改。命令行等价：`yuda edit 把9点改成10点`。

> **请在听写之后、自己打字之前使用。** 语打只知道它自己上屏了什么，看不到你之后打的字或移动的光标；如果中间已经手动输入或换了光标位置，退格会删到你自己的文字。焦点切到别的窗口时语打会拒绝修改。

在 Right Alt 是 AltGr 的键盘布局上，用 F9。

## 设置

Omarchy 菜单 **Setup › Yuda 语打**：

- **识别引擎**：自动（云端优先，2 秒连不上用离线）/ 云端 / 离线
- **识别语言**：中文（中英混输）/ English / 粤语 / 日本語 / 한국어（粤日韩自动切换到 SenseVoice 模型）
- **去除语气词**、**AI 纠错**：开关
- **编辑配置**：打开 `~/.config/yuda/config.toml`，所有选项都有中文注释（见 [examples/config.example.toml](examples/config.example.toml)）
- **重启语打**

命令行同样可用：`yuda config set engine offline`、`yuda config get language`。修改后 daemon 自动重新加载。

## 豆包云端识别（可选）

1. 在[火山引擎控制台](https://console.volcengine.com/speech/app)开通「豆包语音 · 大模型流式语音识别」，创建应用。
2. 旧版控制台：把 **APP ID** 填到 `cloud.app_key`，**Access Token** 填到 `cloud.access_key`；新版控制台只有一个 **API Key**：`app_key` 留空，`access_key` 填 API Key。
3. `resource_id` 默认 `volc.bigasr.sauc.duration`（小时版）；并发版改为 `volc.bigasr.sauc.concurrent`。

计费按识别时长，参考价约 ¥2.2/小时，以火山引擎官网为准。没有密钥时语打始终使用离线模型。

填好后运行 `yuda cloud-test` 验证：它把一段测试语音按实时速度送到豆包大模型，打印握手耗时、服务端日志 ID 与识别结果；密钥错误时会显示 HTTP 401 和日志 ID，便于在火山引擎控制台排查。

## 隐私

- 离线模式下音频只在本机内存中处理，不落盘。
- 云端模式把录音发送到火山引擎；AI 纠错把识别文本发送到你配置的端点。
- 识别历史只存本机 `~/.local/state/yuda/history.jsonl`（0600），可在配置中关闭。

## 故障排查

```bash
yuda doctor                         # 检查配置、模型、服务、Omarchy 集成
yuda status                         # 当前状态
journalctl --user -u yuda -f        # daemon 日志
```

- **没有反应**：确认 `omarchy default dictation` 输出 `yuda`，`systemctl --user status yuda` 在运行。
- **识别为空**：「没有识别到语音」说明麦克风没有声音，检查 PipeWire 默认输入设备。
- **某个应用收不到文字**：语打会把结果留在剪贴板并通知，按 Super+V 粘贴；个别游戏或 XWayland 程序不支持 Wayland 剪贴板同步。

卸载：`omarchy-remove-dictation-yuda`（Omarchy 菜单 Remove › Dictation › Yuda 语打）。

## 非 Omarchy 的 Hyprland

在配置里设置 `trigger.source = "evdev"`，语打会自己监听 `trigger.evdev_key`（默认右 Ctrl，需要 `sudo usermod -aG input $USER` 后重新登录），然后 `systemctl --user enable --now yuda`。此模式没有录音浮层。

## 开发

| 文档 | 内容 |
|---|---|
| [AGENTS.md](AGENTS.md) | 协作规范（人类与 AI Agent） |
| [PROMPT.md](PROMPT.md) | 产品规格 |
| [docs/architecture.md](docs/architecture.md) | 架构与模块 |
| [docs/ipc.md](docs/ipc.md) | daemon ↔ 插件 ↔ 适配器协议 |
| [docs/testing.md](docs/testing.md) | 测试分层与 Omarchy VM 端到端验收 |
| [docs/adr/](docs/adr/) | 架构决策记录 |

```bash
make check                          # fmt + clippy + 默认特性测试（任意平台）
cargo test --features daemon        # 全量（Linux）
scripts/omarchy-e2e                 # 在 Omarchy VM 中端到端验收（KVM 宿主机）
```

## 许可

[MIT](LICENSE)
