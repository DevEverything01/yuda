-- 语打（Yuda）附加快捷键 —— 由 omarchy-install-dictation-yuda 安装并管理。
-- 想自定义就删掉上面这行，安装器以后不会再覆盖本文件。
--
-- Omarchy 只在默认听写后端是 yuda 时加载本文件（default/hypr/dictation-backend.lua）。
-- 听写本身仍用 Omarchy 的快捷键：按住 Right Alt / F9，或 Super + Ctrl + X 切换。

-- 语音修改上一段：按住 Shift + F9 说「把九点改成十点」「删掉上一句」「最后加上问号」，松开执行。
o.bind_hold(
  "SHIFT + F9",
  "Voice edit the last dictation (Yuda)", "yuda start --edit",
  "Apply the voice edit (Yuda)", "yuda stop"
)
o.bind("SUPER + CTRL + SHIFT + X", "Toggle voice edit of the last dictation (Yuda)", "yuda toggle --edit")
