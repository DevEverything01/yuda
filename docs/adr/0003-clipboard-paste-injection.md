# ADR 0003: 上屏主路径——剪贴板 + 模拟 Ctrl+V

- 状态：accepted
- 日期：2026-08-23

## 背景

Wayland 下向任意应用注入文本有三条路：虚拟键盘协议（wtype/ydotool）、剪贴板粘贴、input-method-v2（真 IME 通道）。

## 决策

主路径 = 剪贴板法，严格序列：

1. `wl-paste -n` 快照当前剪贴板（至少保留 text/plain 与 image/* MIME）；
2. `wl-copy` 写入最终文本；
3. `wtype -M ctrl -k v -m ctrl` 合成 Ctrl+V（兜底：`ydotool key 29:1 47:1 47:0 29:0`）；
4. 等待 120ms；
5. 恢复原剪贴板快照。

与 macOS 方案不同：**无需切换输入法**——fcitx5 不拦截 Ctrl+V。

## 理由

- 虚拟键盘协议对 CJK 字符的 keysym 映射不可靠（wtype 打中文会丢字/乱码）；剪贴板粘贴覆盖几乎全部 GTK/Qt/Electron 应用。
- input-method-v2 体验最好（光标处上屏、可撤销、可候选），但开发量高一个量级，留待 Phase 2 键盘一体化时统一解决。

## 被否决方案

- **wtype 直打作为主路径**：CJK 不可靠，仅保留为 ASCII 场景兜底。
- **立即实现 input-method-v2 客户端**：拖慢 MVP 至少一个月。

## 影响

- 已知限制写入 README：个别无 Wayland 剪贴板同步的应用（部分游戏/XWayland 边缘场景）可能收不到粘贴。
- 注入模块必须保证「一次会话只上屏一次」且失败时不得污染剪贴板。
