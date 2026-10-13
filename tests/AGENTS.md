# tests/ 目录协作规范

本目录**只放跨模块集成测试与 mock 端到端测试**。单元测试留在 `src/` 各模块的 `#[cfg(test)]` 里，不要搬到这里（集成测试只能碰 `pub` API，搬出来会倒逼可见性放宽）。

分层总览见 `docs/testing.md`，决策理由见 `docs/adr/0005-testing-strategy.md`。

## 硬性规则

1. **禁网络**：任何测试不得发起真实网络请求。ASR/LLM 协议一律用进程内 mock server（127.0.0.1 随机端口）。
2. **禁真实设备与外部命令**：不得触碰 `/dev/input`、音频设备、系统剪贴板。`wl-copy`/`wl-paste`/`wtype`/`ydotool` 一律用 PATH 注入的假可执行脚本：临时目录写脚本，把调用 argv 与顺序记录到日志文件，测试结束后断言序列。
3. **临时文件一律用 `tempfile`**，测试结束自动清理；禁止读写真实 `$HOME` / `~/.config/yuda`。
4. **确定性**：禁止依赖真实时序的断言。时序相关只断言**顺序**与**下界**（如「paste 发生在 copy 之后」「延迟 ≥ 配置值」），不断言精确耗时；总等待上限 5s。
5. **每个测试防御一个可观察契约**（行为 / 顺序 / 不变量 / 错误路径）。禁止为覆盖率凑数，禁止断言实现细节（私有结构、中间变量）。
6. **一次性测试不进本目录**：调试/排查/验证想法的临时测试放 `.scratch/tests/` 或 `/tmp`（已 gitignore），用完即删。发现它值得保留 → 按本规范重写为契约测试再提交，禁止原样搬入。
7. **准确性评估不进本目录**：音频本体、报告、一次性评估脚本属于 `evals/`。唯一例外是长期维护的 `tests/asr_accuracy.rs` harness：它必须由 manifest 驱动、标 `#[ignore]`、不携带音频、不读取真实 `~/.config/yuda/config.toml`，且不得混入普通快速测试。

## Feature gate 约定

- 被测模块有 gate，测试就跟着 gate，在 `Cargo.toml` 注册：

  ```toml
  [[test]]
  name = "cloud_protocol"
  required-features = ["linux-runtime"]
  ```

- `injection` 等未 gate 的模块，其集成测试必须在默认 features 下可跑——**默认 features 全绿是底线**（macOS 开发机也要能跑 `cargo test`）。

## 严谨性要求

每个契约测试按这份清单自查边界，缺项需在 PR 说明为什么不需要：

- **空 / 最小输入**：空字符串、空音频、单字符、单事件。
- **上限**：配置或代码里的硬上限（如录音 10min、文本弹性宽度 560px）取等号与超一格各一例。
- **错误路径**：外部命令缺失 / 非零退出、坏 TOML、模型文件缺失、超时——断言错误信息可用而非仅 `is_err()`。
- **Unicode/CJK**：多字节字符边界、中英混输、emoji，禁止按字节断言长度。
- **重复与幂等**：同一操作触发两次的结果（如「一次会话只上屏一次」）。
- **表驱动**：同契约的多组输入输出用表驱动一个测试函数，禁止复制粘贴出 N 个近似测试。

准确性评估的边界要求由 `evals/AGENTS.md` 管理，不在普通契约测试中用 `#[ignore]` 偷渡。

## 命名与结构

- 一个被测契约一个文件：`tests/<area>_<aspect>.rs`（如 `injection_mock.rs`、`cloud_protocol.rs`）。
- 长期 ASR harness 固定命名 `asr_accuracy.rs`，只接受 manifest 输入，禁止在代码内写样本 transcript。
- 测试函数名描述行为而非实现；断言消息用中文写清「期望序列 vs 实际序列」，失败输出必须能直接定位到哪一步不符。
- 共享 mock 工具（假命令生成器等）放 `tests/common/mod.rs`；只有被 ≥2 个测试使用时才抽，单次使用就内联在测试里。

## 提交前验证

```bash
cargo test --locked --features linux-runtime            # 全量（Linux）
cargo test --locked                                     # 默认 features 底线（任意平台）
```

新增测试的 PR 必须注明该测试在哪台机器、哪个 feature 组合下实际跑过；只在本目录改动时不免这条。
