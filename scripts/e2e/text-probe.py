#!/usr/bin/env python3
"""GTK4 文本探针：一个普通（非终端）窗口，文本框内容每次变化都写入文件。

用法：text-probe.py <标题> <输出文件>
端到端测试用它验证 Ctrl+V 上屏路径；GTK 走 text-input-v3，与 fcitx5 共存。
"""

import sys

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402

title, output = sys.argv[1], sys.argv[2]


def on_activate(app):
    window = Gtk.ApplicationWindow(application=app, title=title)
    window.set_default_size(640, 240)
    view = Gtk.TextView(wrap_mode=Gtk.WrapMode.WORD_CHAR)
    buffer = view.get_buffer()

    def save(_buffer):
        start, end = buffer.get_bounds()
        with open(output, "w", encoding="utf-8") as handle:
            handle.write(buffer.get_text(start, end, False))

    buffer.connect("changed", save)
    window.set_child(view)
    window.present()
    view.grab_focus()


app = Gtk.Application(application_id="dev.yuda.TextProbe")
app.connect("activate", on_activate)
app.run([])
