#!/usr/bin/env python3
"""lyyIme 悬浮窗输入 (floatapp) — 类 Windows 万能五笔"外挂悬浮窗"参考实现。

不依赖 ibus/fcitx, 不需要应用任何配合:
  - 在本悬浮窗里打五笔86(极点/海峰码表), 空格/数字选词
  - 选中的中文自动发往"最近使用的应用窗口"
  - 英文打字完全不受影响(不切换系统输入法)

两种发送模式(界面可切):
  - 直输: xdotool keysym 注入, 快, 绝大多数应用可用(实证见 docs/RESEARCH.md §2.1)
  - 粘贴: 剪贴板 + Ctrl+V, 兼容性最强, 适合拒收合成键的应用

真正的"外挂直输"(在应用内直接打字)由 xim/ 的 lyyime-xim (XIM server) 路线承担。
"""
import json
import os
import signal
import subprocess
import sys
import threading
import time

import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gdk, GLib, Pango

from Xlib import X, display as xdisplay

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from dictload import WubiDict

TABLE_DIR = '/usr/share/ibus-table/tables'
CFG_DIR = os.path.expanduser('~/.config/lyyime')
CFG_FILE = os.path.join(CFG_DIR, 'config.json')
FREQ_FILE = os.path.join(CFG_DIR, 'user_freq.json')
PID_FILE = os.path.expanduser('~/.local/share/lyyime/float.pid')


def load_cfg():
    try:
        with open(CFG_FILE) as f:
            return json.load(f)
    except Exception:
        return {}


def save_cfg(cfg):
    os.makedirs(CFG_DIR, exist_ok=True)
    with open(CFG_FILE, 'w') as f:
        json.dump(cfg, f, ensure_ascii=False, indent=1)


class Sender:
    """把文本送达指定窗口: keysym 直输或剪贴板粘贴。"""

    @staticmethod
    def _activate(wid):
        r = subprocess.run(['xdotool', 'windowactivate', '--sync', str(wid)],
                           capture_output=True, timeout=5)
        return r.returncode == 0

    @classmethod
    def send(cls, wid, text, method):
        """返回 (成功?, 错误提示)。粘贴方式须在主线程调用(用 GTK 剪贴板)。"""
        if not wid:
            return False, '还没有目标窗口: 先点一下要输入的应用'
        try:
            if not cls._activate(wid):
                return False, '无法激活目标窗口'
            time.sleep(0.06)
            if method == 'paste':
                cb = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
                cb.set_text(text, -1)
                cb.store()
                time.sleep(0.08)
                subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+v'], timeout=5)
            else:
                subprocess.run(['xdotool', 'type', '--delay', '8', text], timeout=15)
            return True, ''
        except subprocess.TimeoutExpired:
            return False, '发送超时'
        except Exception as e:
            return False, f'发送失败: {e}'


class LyyImeApp:
    PAGE = 9
    MAXC = 45

    def __init__(self):
        self.cfg = load_cfg()
        self.cfg.setdefault('dict', 'jidian86')
        self.cfg.setdefault('send', 'type')
        self.dict = None
        self.cands = []
        self.page = 0
        self.code = ''
        self.last_target = (0, '')
        self.self_xid = 0
        self.xdisp = xdisplay.Display()
        os.makedirs(CFG_DIR, exist_ok=True)
        try:
            with open(FREQ_FILE) as f:
                self.user_freq = json.load(f)
        except Exception:
            self.user_freq = {}

        self.build_ui()
        self.apply_css()
        self.reload_dict()
        self.win.show_all()
        self.self_xid = self.win.get_window().get_xid()
        GLib.timeout_add(250, self.poll_target)
        GLib.timeout_add(5000, self.reassert_above)
        GLib.idle_add(self.focus_entry)

    # ---------- UI ----------
    def build_ui(self):
        self.win = Gtk.Window(title='lyyIme 悬浮输入法')
        self.win.set_decorated(False)
        self.win.set_keep_above(True)
        self.win.set_skip_taskbar_hint(True)
        self.win.set_skip_pager_hint(True)
        self.win.set_resizable(False)
        self.win.connect('destroy', self.on_quit)

        v = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4,
                    margin=6, margin_top=4)

        # 标题行: 拖动手柄 + 发送模式切换 + 菜单/隐藏
        title = Gtk.EventBox()
        th = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        grip = Gtk.Label(label='⠿ lyyIme')
        grip.get_style_context().add_class('grip')
        th.pack_start(grip, False, False, 0)
        self.send_combo = Gtk.ComboBoxText()
        self.send_combo.append('type', '直输模式')
        self.send_combo.append('paste', '粘贴模式')
        self.send_combo.set_active(0 if self.cfg['send'] == 'type' else 1)
        self.send_combo.connect('changed', self.on_send_changed)
        th.pack_start(self.send_combo, False, False, 0)
        menu_btn = Gtk.Button(label='☰')
        menu_btn.set_relief(Gtk.ReliefStyle.NONE)
        menu_btn.connect('clicked', self.on_menu)
        th.pack_end(menu_btn, False, False, 0)
        hide_btn = Gtk.Button(label='—')
        hide_btn.set_relief(Gtk.ReliefStyle.NONE)
        hide_btn.connect('clicked', lambda b: self.win.hide())
        th.pack_end(hide_btn, False, False, 0)
        title.add(th)
        title.connect('button-press-event', self.on_title_press)
        v.pack_start(title, False, False, 0)

        # 输入行
        self.entry = Gtk.Entry()
        self.entry.set_placeholder_text('打五笔: 空格上屏 · 数字选词 · Esc清空')
        self.entry.connect('changed', self.on_entry_changed)
        self.entry.connect('key-press-event', self.on_entry_key)
        v.pack_start(self.entry, False, False, 0)

        # 候选行
        self.cand_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=2)
        self.cand_btns = []
        for i in range(self.PAGE):
            b = Gtk.Button(label='')
            b.get_style_context().add_class('cand')
            b.set_no_show_all(True)
            b.connect('clicked', self.on_cand_clicked, i)
            self.cand_box.pack_start(b, False, False, 0)
            self.cand_btns.append(b)
        self.prev_btn = Gtk.Button(label='◀')
        self.prev_btn.set_no_show_all(True)
        self.prev_btn.connect('clicked', lambda b: self.flip(-1))
        self.next_btn = Gtk.Button(label='▶')
        self.next_btn.set_no_show_all(True)
        self.next_btn.connect('clicked', lambda b: self.flip(1))
        self.cand_box.pack_end(self.next_btn, False, False, 0)
        self.cand_box.pack_end(self.prev_btn, False, False, 0)
        v.pack_start(self.cand_box, False, False, 0)

        # 状态行
        self.status = Gtk.Label(label='正在加载码表…')
        self.status.set_ellipsize(Pango.EllipsizeMode.START)
        self.status.set_name('status')
        self.status.set_halign(Gtk.Align.START)
        v.pack_start(self.status, False, False, 0)

        self.win.add(v)
        pos = self.cfg.get('position')
        if pos:
            self.win.move(pos[0], pos[1])

        # 托盘
        self.tray = Gtk.StatusIcon(icon_name='input-keyboard')
        self.tray.set_tooltip_text('lyyIme 悬浮输入法')
        self.tray.connect('activate', self.on_tray_toggle)
        self.tray.connect('popup-menu', lambda *a: self.on_menu(None))

    def apply_css(self):
        css = b"""
        window { background: #fbfbf2; border: 1px solid #999; }
        .cand { font-size: 15px; padding: 1px 5px; background: transparent;
                border: none; box-shadow: none; }
        .grip { color: #567; font-weight: bold; padding-right: 6px; }
        #status { color: #6a6a5a; font-size: 10px; }
        """
        prov = Gtk.CssProvider()
        prov.load_from_data(css)
        Gtk.StyleContext.add_provider_for_screen(
            Gdk.Screen.get_default(), prov, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)

    def on_tray_toggle(self, icon):
        if self.win.get_visible():
            self.save_position()
            self.win.hide()
        else:
            self.win.show_all()
            self.reassert_above()
            self.focus_entry()

    def reassert_above(self):
        try:
            self.win.set_keep_above(True)
        except Exception:
            pass
        return True

    # ---------- 菜单 ----------
    def on_menu(self, btn):
        m = Gtk.Menu()
        for name, val in (('使用极点86码表', 'jidian86'), ('使用海峰86码表', 'haifeng86')):
            it = Gtk.CheckMenuItem(label=name)
            it.set_active(self.cfg['dict'] == val)
            it.connect('activate', self.on_pick_dict, val)
            m.append(it)
        m.append(Gtk.SeparatorMenuItem())
        it = Gtk.MenuItem(label='显示系统打字板(复制中转)')
        it.connect('activate', lambda *_: self.launch_pad())
        m.append(it)
        it = Gtk.MenuItem(label='退出')
        it.connect('activate', lambda *_: self.on_quit())
        m.append(it)
        m.show_all()
        m.popup_at_pointer(None)

    def on_pick_dict(self, item, val):
        if self.cfg['dict'] != val:
            self.cfg['dict'] = val
            save_cfg(self.cfg)
            self.reload_dict()

    def launch_pad(self):
        subprocess.Popen(['setsid', 'python3',
                          os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                       'chinese_pad.py')],
                         start_new_session=True)

    # ---------- 码表 ----------
    def reload_dict(self):
        self.status.set_text('码表加载中…')

        def work():
            path = os.path.join(TABLE_DIR, f"wubi-{self.cfg['dict']}.db")
            try:
                d = WubiDict(path)
                d.set_user_freq(self.user_freq)
                GLib.idle_add(self._dict_ready, d, None)
            except Exception as e:
                GLib.idle_add(self._dict_ready, None, str(e))
        threading.Thread(target=work, daemon=True).start()

    def _dict_ready(self, d, err):
        if err:
            self.status.set_text(f'码表加载失败: {err}')
            return False
        self.dict = d
        self.status.set_text(self._status_text())
        self.refresh_cands()
        return False

    def _status_text(self):
        t = self.last_target[1][:30] or '(无)'
        return f'目标: {t}  |  码表 {self.cfg["dict"]}'

    # ---------- 目标窗口追踪 ----------
    def poll_target(self):
        try:
            root = self.xdisp.screen().root
            prop = root.get_full_property(
                self.xdisp.intern_atom('_NET_ACTIVE_WINDOW'), X.AnyPropertyType)
            wid = int(prop.value[0]) if prop and prop.value else 0
            if wid and wid != self.self_xid:
                title = self.get_title(wid)
                if (wid, title) != self.last_target:
                    self.last_target = (wid, title)
                    if self.dict:
                        self.status.set_text(self._status_text())
        except Exception:
            pass
        return True

    def get_title(self, wid):
        try:
            w = self.xdisp.create_resource_object('window', wid)
            for atom in ('_NET_WM_NAME', 'WM_NAME'):
                p = w.get_full_property(self.xdisp.intern_atom(atom), 0)
                if p is not None and p.value:
                    v = p.value
                    if isinstance(v, (bytes, bytearray)):
                        return bytes(v).decode('utf-8', 'ignore')
                    return str(v)
        except Exception:
            pass
        return '?'

    # ---------- 候选 ----------
    def on_entry_changed(self, e):
        self.code = e.get_text().strip()
        self.refresh_cands()

    def refresh_cands(self):
        code = getattr(self, 'code', '')
        self.cands = self.dict.lookup(code, self.MAXC) if (self.dict and code) else []
        self.page = min(self.page, max(0, (len(self.cands) - 1) // self.PAGE))
        self.render_cands()

    def render_cands(self):
        n = len(self.cands)
        start = self.page * self.PAGE
        for i, b in enumerate(self.cand_btns):
            j = start + i
            if j < n:
                b.set_label(f"{i + 1}{self.cands[j][0][:8]}")
                b.show()
            else:
                b.hide()
        multi = n > self.PAGE
        self.prev_btn.set_visible(multi)
        self.next_btn.set_visible(multi)
        if multi:
            self.prev_btn.set_sensitive(self.page > 0)
            self.next_btn.set_sensitive((self.page + 1) * self.PAGE < n)

    def flip(self, d):
        np_ = self.page + d
        if 0 <= np_ * self.PAGE < max(len(self.cands), 1):
            self.page = np_
            self.render_cands()

    def on_cand_clicked(self, btn, i):
        j = self.page * self.PAGE + i
        if j < len(self.cands):
            self.commit(self.cands[j][0])

    # ---------- 按键 ----------
    def on_entry_key(self, w, ev):
        name = Gdk.keyval_name(ev.keyval)
        s = ev.string or ''
        if name == 'space':
            if self.cands:
                self.commit(self.cands[self.page * self.PAGE][0])
            return True
        if s in '123456789':
            j = self.page * self.PAGE + int(s) - 1
            if j < len(self.cands):
                self.commit(self.cands[j][0])
            return True
        if name == 'Return':
            if self.code:
                self.commit(self.code)   # 原样上屏(英文)
            return True
        if name == 'Escape':
            self.entry.set_text('')
            self.page = 0
            return True
        if name in ('minus', 'Page_Up'):
            self.flip(-1)
            return True
        if name in ('equal', 'Page_Down'):
            self.flip(1)
            return True
        return False

    def commit(self, text):
        wid = self.last_target[0]
        if not wid:
            self.status.set_text('⚠ 先点一下要输入字的应用窗口, 再回来打字')
            return
        ok, err = Sender.send(wid, text, self.cfg['send'])
        if ok:
            self.dict.learn(text)
            self.user_freq[text] = self.user_freq.get(text, 0) + 1
            try:
                with open(FREQ_FILE, 'w') as f:
                    json.dump(self.user_freq, f, ensure_ascii=False)
            except Exception:
                pass
        else:
            self.status.set_text(f'⚠ {err}')
        self.entry.set_text('')
        self.page = 0
        subprocess.Popen(['xdotool', 'windowactivate', str(self.self_xid)],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        GLib.idle_add(self.focus_entry)

    def focus_entry(self):
        self.entry.grab_focus()
        return False

    def on_send_changed(self, combo):
        self.cfg['send'] = combo.get_active_id() or 'type'
        save_cfg(self.cfg)

    # ---------- 拖动/退出 ----------
    def on_title_press(self, w, ev):
        if ev.button == 1:
            self.win.begin_move_drag(ev.button, int(ev.x_root), int(ev.y_root), ev.time)
            return True
        return False

    def save_position(self):
        try:
            x, y = self.win.get_position()
            self.cfg['position'] = [x, y]
            save_cfg(self.cfg)
        except Exception:
            pass

    def on_quit(self, *a):
        self.save_position()
        Gtk.main_quit()


def pidfile_alive(path):
    """读 pidfile 并确认进程存活;残留/非法内容视为不存在。"""
    try:
        with open(path) as f:
            pid = int(f.read().strip())
    except Exception:
        return 0
    if pid <= 0:
        return 0
    try:
        os.kill(pid, 0)
    except OSError:
        return 0
    return pid


def write_pidfile(path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        f.write(f'{os.getpid()}\n')


def main():
    # WM_CLASS 与桌面入口 StartupWMClass 对应(窗口归组/再次唤起识别)
    GLib.set_prgname('lyyime-float')
    # 单实例:已有悬浮窗在跑 → SIGUSR1 唤起(显示+聚焦)后自身退出,避免重复开窗
    alive = pidfile_alive(PID_FILE)
    if alive and alive != os.getpid():
        try:
            os.kill(alive, signal.SIGUSR1)
            print(f'lyyime-float 已在运行(pid={alive}),已唤起其悬浮窗。',
                  file=sys.stderr)
            return
        except OSError:
            pass
    write_pidfile(PID_FILE)

    app = LyyImeApp()

    def on_wake():
        if not app.win.get_visible():
            app.win.show_all()
        app.win.present()
        app.reassert_above()
        app.focus_entry()
        return False

    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, on_wake)
    GLib.timeout_add(3000, lambda: (app.save_position(), False)[1])
    try:
        Gtk.main()
    finally:
        try:
            os.remove(PID_FILE)
        except OSError:
            pass


if __name__ == '__main__':
    main()
