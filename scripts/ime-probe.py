#!/usr/bin/env python3
"""真实显示器上的输入法链路探测(lyyIme 运维工具)。

在指定 DISPLAY 上弹出一个小输入框,显式聚焦后由外部用 xdotool 打字,
把 Entry 收到的文本周期性写盘,用于无侵入验证 ibus/XIM 链路是否真正上屏。
窗口 25 秒后自动退出;配合 tests/e2e 的 wait_buffer 式断言使用。

用法:
  python3 scripts/ime-probe.py <缓冲文件路径> [超时秒,默认25]
环境变量:DISPLAY 必须指向目标屏;GTK_IM_MODULE/XMODIFIERS/DBUS_SESSION_BUS_ADDRESS
按"目标应用的真实环境"设置(见 docs/RESEARCH.md §4)。
"""
import os
import sys

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402


def main() -> None:
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    buf_path = sys.argv[1]
    timeout = int(sys.argv[2]) if len(sys.argv) > 2 else 25

    win = Gtk.Window(title="lyyime-probe")
    win.set_default_size(300, 60)
    entry = Gtk.Entry()
    if os.environ.get("LYYIME_PROBE_TRACE") == "1":
        def trace(label, *values):
            print(GLib.get_monotonic_time(), label, *values, flush=True)

        def key_event(_widget, event):
            trace("key", event.type.value_nick, hex(event.keyval), int(event.state))
            return False

        def focus_event(widget, _event):
            trace("focus", widget.has_focus())
            return False

        entry.connect("key-press-event", key_event)
        entry.connect("key-release-event", key_event)
        entry.connect("focus-in-event", focus_event)
        entry.connect("focus-out-event", focus_event)
        entry.connect("preedit-changed", lambda _widget, text: trace("preedit-length", len(text)))
        entry.connect("changed", lambda widget: trace("text-length", len(widget.get_text())))
    win.add(entry)
    win.show_all()

    def dump() -> bool:
        with open(buf_path, "w") as f:
            f.write(entry.get_text())
        return True

    def quit() -> None:
        os._exit(0)

    GLib.timeout_add(300, dump)
    GLib.timeout_add_seconds(timeout, quit)
    Gtk.main()


if __name__ == "__main__":
    main()
