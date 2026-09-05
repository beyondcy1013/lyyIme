import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib

win = Gtk.Window(title="中文输入板 - Ctrl+空格切极点五笔, 打完复制粘贴到ZCode")
win.set_default_size(560, 320)
win.set_keep_above(True)

box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, margin=10)
label = Gtk.Label(label="Ctrl+空格 / Win+空格 切换中英文\n打字 → Ctrl+A 全选 → Ctrl+C 复制 → 去ZCode里 Ctrl+V")
box.pack_start(label, False, False, 0)

tv = Gtk.TextView(wrap_mode=Gtk.WrapMode.WORD_CHAR)
tv.get_buffer().set_text("")
scroll = Gtk.ScrolledWindow()
scroll.set_vexpand(True)
scroll.add(tv)
box.pack_start(scroll, True, True, 0)
win.add(box)

def on_key(w, e):
    if e.keyval == 0xff1b:  # Escape 关闭
        Gtk.main_quit()
    return False

win.connect("key-press-event", on_key)
win.connect("destroy", Gtk.main_quit)
win.show_all()
Gtk.main()
