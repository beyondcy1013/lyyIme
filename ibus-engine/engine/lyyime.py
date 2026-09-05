#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""lyyime.py —— lyyIme 的 ibus 引擎(Mode A)。

分层设计(借鉴 ibus-table 的 MockEngine 单测模式,出处 RESEARCH.md §1.5):

* ``EngineLogic``:按键逻辑核心,keysym/state → LKey → ctypes FFI → 效果流,
  **不依赖任何 IBus 类型**,可不经 dbus/daemon 直接实例化喢单元键
  (tests/unit_ibus_engine.py)。
* ``LyyimeEngine(IBus.Engine)``:胶水层,把效果流接到
  commit_text / update_preedit_text_with_mode / update_lookup_table /
  update_property 等 IBus API。
* ``EngineFactory(IBus.Factory)``:工厂,写法借鉴 ibus-table factory.py
  (RESEARCH §1.1)。
* ``IMApp``:进程主体,由 ibus-daemon 按 component XML 的 ``<exec>``
  附加 ``--ibus`` 参数拉起。

行为合同:docs/ARCHITECTURE.md §3(FFI)、§6(按键)、§7(Mode A 职责)。
键位放行策略:一律 ``return False`` 放行,**不走 forward_key_event**
(Qt5 的 ibus 输入模块不实现该方法,出处 RESEARCH.md §1.2)。
"""

import argparse
import logging
import logging.handlers
import os
import shutil
import subprocess
import sys
import tomllib

import gi

# import 顺序说明:IBus typelib 在无 DISPLAY 环境也可 import(本机已验证),
# 但保持"require_version 先行"的写法与 ibus-table main.py 一致。
gi.require_version('IBus', '1.0')
from gi.repository import IBus, GLib  # noqa: E402

# 让 ibus-daemon 直接执行本文件时能 import 同目录的 lyyime_ffi
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import lyyime_ffi  # noqa: E402
from lyyime_ffi import (  # noqa: E402
    LKEY_BACKSPACE, LKEY_CHAR, LKEY_DIGIT, LKEY_ENTER, LKEY_ESC,
    LKEY_OTHER, LKEY_PAGEDOWN, LKEY_PAGEUP, LKEY_PUNCT, LKEY_SHIFTPRESS,
    LKEY_SPACE, LyyimeFfi, LyyimeFfiError)

VERSION = '0.1.0'
ENGINE_NAME = 'lyyime'
BUS_NAME = 'org.freedesktop.IBus.Lyyime'

LOGGER = logging.getLogger('lyyime.ibus')
LOGGER.addHandler(logging.NullHandler())  # import 期零输出;main() 里换文件 handler

# ---------------------------------------------------------------------------
# X keysym 常量(纯 int,逻辑层不 import gi,便于无 IBus 单测)
# ---------------------------------------------------------------------------
KSYM_A = 0x61
KSYM_Z = 0x7a
KSYM_1 = 0x31
KSYM_9 = 0x39
KSYM_SPACE = 0x20
KSYM_RETURN = 0xff0d
KSYM_KP_ENTER = 0xff8d
KSYM_BACKSPACE = 0xff08
KSYM_ESCAPE = 0xff1b
KSYM_PAGE_UP = 0xff55
KSYM_PAGE_DOWN = 0xff56
KSYM_SHIFT_L = 0xffe1
KSYM_SHIFT_R = 0xffe2
KSYM_KP_1 = 0xffb1
KSYM_KP_9 = 0xffb9

SHIFT_KEYVALS = (KSYM_SHIFT_L, KSYM_SHIFT_R)

# 修饰位掩码(X/ibus 通用;数值与 IBus.ModifierType 一致)
MASK_SHIFT = 1 << 0
MASK_LOCK = 1 << 1       # CapsLock:仍算普通输入,不拦截
MASK_CTRL = 1 << 2
MASK_ALT = 1 << 3        # MOD1
MASK_SUPER = (1 << 6) | (1 << 26)  # MOD4 | SUPER
MASK_RELEASE = 1 << 30
# 带 Ctrl/Alt/Super 的按键是应用快捷键:不算 Shift 单击、不进缓冲
BLOCKING_MODS = MASK_CTRL | MASK_ALT | MASK_SUPER

# InputPurpose 整型值(与 IBus.InputPurpose 一致;避免逻辑层依赖 gi)
PURPOSE_PASSWORD = 8
PURPOSE_PIN = 9

DEFAULT_PAGE_SIZE = 5
DATA_DIR_DEFAULT = os.path.join(
    os.environ.get('XDG_DATA_HOME') or os.path.expanduser('~/.local/share'),
    'lyyime')


def read_page_size():
    """读 ~/.config/lyyime/config.toml 的 page_size(与 core/app 共用配置)。

    读不到或非法时回退 DEFAULT_PAGE_SIZE,绝不因配置缺失而崩溃。
    """
    default = DEFAULT_PAGE_SIZE
    cfg_path = os.path.join(
        os.environ.get('XDG_CONFIG_HOME') or os.path.expanduser('~/.config'),
        'lyyime', 'config.toml')
    try:
        with open(cfg_path, 'rb') as fh:
            cfg = tomllib.load(fh)
        size = int(cfg.get('page_size', default))
        return min(9, max(1, size))
    except (OSError, ValueError, tomllib.TOMLDecodeError):
        return default


class EngineLogic(object):
    """按键逻辑核心:keysym/state → LKey → FFI 效果流 → 宿主回调。

    与 IBus 类型完全解耦;宿主(host)只需实现这些普通 python 回调:

    * ``on_commit(text)`` —— 上屏文本
    * ``on_preedit(text_or_None)`` —— 预编辑更新(None=清除)
    * ``on_candidates(cands, page, pages, aux)`` —— 候选页更新,
      cands 为 [(文本, 注释), ...];aux 为辅助区输入串
    * ``on_mode_changed(mode)`` —— 模式变化(0=中文 1=英文)
    * ``on_notice(text)`` —— 辅助区临时提示(设置入口缺失等)

    :param host: 上述回调的实现者(IBus 子类或测试 Mock)。
    :param ffi: LyyimeFfi 实例;None = 降级英文直通(FFI 未就绪)。
    :param page_size: 候选页大小(仅展示用,翻页/选词由 core 驱动)。
    """

    def __init__(self, host, ffi=None, page_size=DEFAULT_PAGE_SIZE):
        self.host = host
        self.ffi = ffi
        self.page_size = page_size
        self.input_purpose = 0          # 宿主经 set_content_type 更新
        self.degraded = ffi is None     # True = 英文直通降级态
        self.mode = ffi.mode() if ffi is not None else 0
        # Shift 单击检测状态:按下后记录,期间无其它键且 release 时触发
        self._pending_shift = None
        self._preedit = None

    # ------------------------------------------------------------------
    # 主入口(单测直接调用)
    # ------------------------------------------------------------------
    def process_key_event(self, keyval, keycode, state):
        """处理一个按键事件,返回 True=已消费 / False=放行给应用。

        放行采用 ``return False`` 常规路径(不 forward_key_event,
        Qt5 不支持,出处 RESEARCH.md §1.2)。
        """
        if self.degraded or self.ffi is None:
            return False  # 降级态:一切按键原样放行,绝不卡死输入
        if self.input_purpose in (PURPOSE_PASSWORD, PURPOSE_PIN):
            # 密码框全放行(借鉴 ibus-table table.py:3870 的防护)
            return False
        if state & MASK_RELEASE:
            return self._on_release(keyval)
        return self._on_press(keyval, state)

    # ------------------------------------------------------------------
    # Shift 单击检测(合同 §6:按下后未产生其它键即释放,且无其它修饰)
    # ------------------------------------------------------------------
    def _on_press(self, keyval, state):
        if keyval in SHIFT_KEYVALS:
            if state & BLOCKING_MODS:
                # Shift 参与组合键(Ctrl/Alt/Super+Shift):放行,不算单击
                self._pending_shift = None
                return False
            # 单击按下:先吞下,等待 release;若期间来任何其它键则作废
            self._pending_shift = keyval
            return True
        # 其它键按下:无论成败都取消未决的 Shift 单击
        self._pending_shift = None
        if state & BLOCKING_MODS:
            # 应用快捷键(如 Ctrl+C):先让 core 复位缓冲,再放行
            effects = self.ffi.process_key(LKEY_OTHER, 0)
            return self._dispatch(effects)
        lkey, char = self._map_keyval(keyval)
        effects = self.ffi.process_key(lkey, char)
        return self._dispatch(effects)

    def _on_release(self, keyval):
        if keyval in SHIFT_KEYVALS:
            pending = self._pending_shift
            self._pending_shift = None
            if pending == keyval:
                # 按下后无其它键 → 判定单击:切换中英(合同 §6)
                self.switch_mode()
                return True
            return False  # 与单击无关的 Shift release:放行
        # 非 Shift 键的 release 一律放行(RESEARCH §1.2)
        return False

    # ------------------------------------------------------------------
    # keysym → LKey 映射(合同 §2/§3)
    # ------------------------------------------------------------------
    @staticmethod
    def _map_keyval(keyval):
        """把 keysym 映射为 (LKey, 码点);无法识别时回 LKEY_OTHER。

        注:Digit 的数值经 chr 参数按码点传递('1'=0x31),core 取
        chr-0x30 得 1..9;这是对 §3 "chr 为 Char/Punct 码点" 的补充约定,
        已在 C ABI 桩库与核心开发方对齐。
        """
        if KSYM_A <= keyval <= KSYM_Z:
            return LKEY_CHAR, keyval
        if 0x41 <= keyval <= 0x5a:  # CapsLock/Shift 产生的大写键值:小写化入缓冲
            return LKEY_CHAR, keyval + 0x20
        if KSYM_1 <= keyval <= KSYM_9:
            return LKEY_DIGIT, keyval
        if KSYM_KP_1 <= keyval <= KSYM_KP_9:  # 小键盘数字也可选词
            return LKEY_DIGIT, 0x31 + (keyval - KSYM_KP_1)
        if keyval == KSYM_SPACE:
            return LKEY_SPACE, 0
        if keyval in (KSYM_RETURN, KSYM_KP_ENTER):
            return LKEY_ENTER, 0
        if keyval == KSYM_BACKSPACE:
            return LKEY_BACKSPACE, 0
        if keyval == KSYM_ESCAPE:
            return LKEY_ESC, 0
        if keyval == KSYM_PAGE_UP:
            return LKEY_PAGEUP, 0
        if keyval == KSYM_PAGE_DOWN:
            return LKEY_PAGEDOWN, 0
        # 其余可打印 ASCII 符号(标点、-=、空格之外的)一律按标点送 core
        if 0x21 <= keyval <= 0x7e:
            return LKEY_PUNCT, keyval
        return LKEY_OTHER, 0

    # ------------------------------------------------------------------
    # 效果流分发(合同 §3:顺序即宿主处理顺序)
    # ------------------------------------------------------------------
    def _dispatch(self, effects):
        """逐条执行效果;返回 False 仅当出现 pass(宿主必须放行该键)。"""
        consumed = True
        for eff in effects:
            if not isinstance(eff, dict):
                LOGGER.warning('忽略非法效果项:%r', eff)
                continue
            kind = eff.get('t')
            if kind == 'commit':
                text = eff.get('s') or ''
                if text:
                    self.host.on_commit(text)
            elif kind == 'preedit':
                # 缺 s / 空 s 都视为清除(None)
                self._preedit = eff.get('s') or None
                self.host.on_preedit(self._preedit)
            elif kind == 'cands':
                count = int(eff.get('n') or 0)
                page = int(eff.get('page') or 0)
                pages = int(eff.get('pages') or 1)
                cands = []
                for i in range(count):
                    cands.append((self.ffi.cand(i), self.ffi.cand_comment(i)))
                self.host.on_candidates(cands, page, pages, self._preedit or '')
            elif kind == 'pass':
                consumed = False
            elif kind == 'consumed':
                pass  # 吞键但无可见效果
            elif kind == 'mode':
                self.mode = int(eff.get('m') or 0)
                self.host.on_mode_changed(self.mode)
            else:
                LOGGER.warning('未知效果类型 %r,忽略', kind)
        return consumed

    # ------------------------------------------------------------------
    # 对外操作(属性菜单/焦点切换调用)
    # ------------------------------------------------------------------
    def switch_mode(self):
        """切换中英模式并清空会话状态(与 Shift 单击、托盘菜单共用)。"""
        if self.ffi is None:
            return
        self.ffi.reset()
        self._preedit = None
        self.host.on_preedit(None)
        self.host.on_candidates([], 0, 0, '')
        self.mode = self.ffi.toggle_mode()
        self.host.on_mode_changed(self.mode)

    def reset_session(self):
        """焦点进出/重置:清 core 缓冲与 UI(模式跨焦点保持)。"""
        if self.ffi is not None:
            try:
                self.ffi.reset()
            except LyyimeFfiError:
                LOGGER.exception('reset 失败,忽略')
        self._preedit = None
        self.host.on_preedit(None)
        self.host.on_candidates([], 0, 0, '')
        self.host.on_mode_changed(self.mode)

    def reload_dict(self):
        """重载词库:释放并重建 core Engine(重新加载 data_dir 下的词典)。"""
        data_dir = self.ffi.data_dir if self.ffi is not None else DATA_DIR_DEFAULT
        if self.ffi is not None:
            self.ffi.free()
        self.ffi = None
        self.degraded = True
        self.ffi = LyyimeFfi(data_dir=data_dir)
        self.degraded = False
        self.mode = self.ffi.mode()
        self.reset_session()

    def shutdown(self):
        """引擎销毁时释放 core Engine。"""
        if self.ffi is not None:
            self.ffi.free()
            self.ffi = None
            self.degraded = True


# ===========================================================================
# 以下为 IBus 胶水层(单测不经此层)
# ===========================================================================
def _mode_icon(mode):
    """返回中/英状态图标绝对路径(安装前后都以 icons/ 相对引擎目录解析)。"""
    icon_dir = os.path.normpath(os.path.join(
        os.path.dirname(os.path.abspath(__file__)), '..', 'icons'))
    name = 'lyyime-zh.svg' if mode == 0 else 'lyyime-en.svg'
    path = os.path.join(icon_dir, name)
    return path if os.path.isfile(path) else 'lyyime'


class LyyimeEngine(IBus.Engine):
    """ibus 引擎胶水层:EngineLogic 的效果流 → IBus API。

    健壮性合同(§7):FFI 调用异常时降级英文直通并记日志,绝不卡死按键。
    """

    def __init__(self, bus, engine_name, object_path):
        # connection 必须显式传入:否则 engine 对象不会注册到 ibus 私有总线,
        # daemon 拿到的是死代理,按键会回落 simple 引擎直通(借鉴 table.py:280-288)
        super(LyyimeEngine, self).__init__(connection=bus.get_connection(),
                                           object_path=object_path)
        self._engine_name = engine_name
        self._setup_pid = 0
        self._notice_timeout_id = 0
        # FFI 失败不抛出:进入降级英文直通,引擎照常工作
        try:
            ffi = LyyimeFfi()
            self._logic = EngineLogic(host=self, ffi=ffi,
                                      page_size=read_page_size())
        except Exception:  # noqa: BLE001 —— 降级必须兜住一切初始化异常
            LOGGER.exception('核心库初始化失败,进入英文直通降级模式')
            self._logic = EngineLogic(host=self, ffi=None)
        self._init_properties()
        self.connect('destroy', self._on_destroy_cb)
        LOGGER.info('引擎实例已创建:%s(%s)', engine_name, object_path)

    # ------------------------------------------------------------------
    # 属性菜单(借鉴 ibus-table do_property_activate,RESEARCH §1.4)
    # ------------------------------------------------------------------
    def _init_properties(self):
        """构造托盘属性:中英切换 / 设置 / 工具(子菜单)/ 关于。"""
        mode = self._logic.mode
        self._prop_input_mode = IBus.Property(
            key='InputMode',
            prop_type=IBus.PropType.TOGGLE,
            label=IBus.Text.new_from_string('中英切换(Shift 单击)'),
            symbol=IBus.Text.new_from_string('中' if mode == 0 else 'EN'),
            icon=_mode_icon(mode),
            tooltip=IBus.Text.new_from_string(
                '当前:%s。单击切换中/英文,与 Shift 单击同效。'
                % ('中文' if mode == 0 else '英文')),
            sensitive=True,
            visible=True,
            state=IBus.PropState.UNCHECKED)

        self._prop_setup = IBus.Property(
            key='setup',
            prop_type=IBus.PropType.NORMAL,
            label=IBus.Text.new_from_string('设置'),
            icon='gtk-preferences',
            tooltip=IBus.Text.new_from_string('打开 lyyIme 设置界面'),
            sensitive=True,
            visible=True,
            state=IBus.PropState.UNCHECKED)

        tool_items = (
            ('tools.fix', '修复输入法(预览)', 'system-run',
             '运行 lyyime-doctor fix --all --dry-run 预览修复项'),
            ('tools.ime', '输入法管理', 'preferences-desktop-keyboard',
             '运行 lyyime-doctor ime-list 列出本机输入法(增删/设默认见设置)'),
            ('tools.reload', '重载词库', 'view-refresh',
             '重新加载词典(修改词库后无需重启 ibus)'),
            ('tools.logs', '打开日志目录', 'folder-open',
             '打开 ~/.local/share/lyyime/logs'),
        )
        sub_props = IBus.PropList()
        for key, label, icon, tip in tool_items:
            sub_props.append(IBus.Property(
                key=key,
                prop_type=IBus.PropType.NORMAL,
                label=IBus.Text.new_from_string(label),
                icon=icon,
                tooltip=IBus.Text.new_from_string(tip),
                sensitive=True,
                visible=True,
                state=IBus.PropState.UNCHECKED))
        self._prop_tools = IBus.Property(
            key='tools',
            prop_type=IBus.PropType.MENU,
            label=IBus.Text.new_from_string('工具'),
            icon='applications-system',
            tooltip=IBus.Text.new_from_string('诊断修复 / 输入法管理 / 词库 / 日志'),
            sensitive=True,
            visible=True,
            state=IBus.PropState.UNCHECKED,
            sub_props=sub_props)

        self._prop_about = IBus.Property(
            key='about',
            prop_type=IBus.PropType.NORMAL,
            label=IBus.Text.new_from_string('关于'),
            icon='help-about',
            tooltip=IBus.Text.new_from_string('关于 lyyIme'),
            sensitive=True,
            visible=True,
            state=IBus.PropState.UNCHECKED)

        self._main_props = IBus.PropList()
        self._main_props.append(self._prop_input_mode)
        self._main_props.append(self._prop_setup)
        self._main_props.append(self._prop_tools)
        self._main_props.append(self._prop_about)
        self.register_properties(self._main_props)

    def _update_mode_property(self, mode):
        """按当前模式刷新 InputMode 属性图标(中/EN)。"""
        self._prop_input_mode.set_symbol(
            IBus.Text.new_from_string('中' if mode == 0 else 'EN'))
        self._prop_input_mode.set_icon(_mode_icon(mode))
        self._prop_input_mode.set_tooltip(
            IBus.Text.new_from_string(
                '当前:%s。单击切换中/英文,与 Shift 单击同效。'
                % ('中文' if mode == 0 else '英文')))
        self.update_property(self._prop_input_mode)

    def do_property_activate(self, prop_name, prop_state=IBus.PropState.UNCHECKED):
        """响应托盘/菜单点击。"""
        if prop_name == 'InputMode':
            self._safe(lambda: self._logic.switch_mode())
        elif prop_name == 'setup':
            self._launch_setup()
        elif prop_name == 'tools.fix':
            self._run_in_terminal(
                ['lyyime-doctor', 'fix', '--all', '--dry-run'],
                title='lyyIme 修复输入法(dry-run 预览)')
        elif prop_name == 'tools.ime':
            self._run_in_terminal(
                ['lyyime-doctor', 'ime-list'],
                title='lyyIme 输入法管理(列表)')
        elif prop_name == 'tools.reload':
            def _reload():
                self._logic.reload_dict()
                self.on_notice('词库已重载')
            self._safe(_reload)
        elif prop_name == 'tools.logs':
            log_dir = os.path.join(DATA_DIR_DEFAULT, 'logs')
            os.makedirs(log_dir, exist_ok=True)
            try:
                subprocess.Popen(['xdg-open', log_dir],
                                 stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL)
            except OSError:
                LOGGER.exception('无法打开日志目录 %s', log_dir)
                self.on_notice('无法打开日志目录:%s' % log_dir)
        elif prop_name == 'about':
            self.on_notice(
                'lyyIme 五笔拼音 v%s(Mode A · ibus 引擎)'
                '——五笔/拼音/英文混合输入' % VERSION)

    # ------------------------------------------------------------------
    # 按键与内容类型
    # ------------------------------------------------------------------
    def do_process_key_event(self, keyval, keycode, state):
        """ibus 按键入口:一切异常都降级放行,绝不卡死按键(合同 §7)。"""
        LOGGER.debug('KEY keyval=%s keycode=%s state=%s', keyval, keycode, state)
        try:
            return self._logic.process_key_event(keyval, keycode, state)
        except Exception:  # noqa: BLE001 —— 兜底:异常时英文直通
            LOGGER.exception(
                '处理按键异常(keyval=%s state=%s),放行并保持降级',
                keyval, state)
            return False

    def do_set_content_type(self, purpose, hint):
        """应用声明输入用途:密码框(含 PIN)全放行(借鉴 table.py:3870)。"""
        self._logic.input_purpose = int(purpose)

    def do_focus_in(self):
        self._safe(self._logic.reset_session)

    def do_focus_out(self):
        self._safe(self._logic.reset_session)

    def do_reset(self):
        self._safe(self._logic.reset_session)

    def _on_destroy_cb(self, _widget):
        self._safe(self._logic.shutdown)

    def _safe(self, func):
        """包装宿主侧操作:异常只记日志不外抛(引擎永不崩)。"""
        try:
            func()
        except Exception:  # noqa: BLE001
            LOGGER.exception('引擎操作 %r 失败', getattr(func, '__name__', func))

    # ------------------------------------------------------------------
    # 宿主回调:效果流 → IBus(单测的 Mock 用同名方法替换)
    # ------------------------------------------------------------------
    def on_commit(self, text):
        """上屏文本。"""
        self.commit_text(IBus.Text.new_from_string(text))

    def on_preedit(self, text):
        """更新/清除预编辑(COMPOSITION 模式,带下划线)。"""
        if text:
            attrs = IBus.AttrList()
            attrs.append(IBus.attr_underline_new(
                IBus.AttrUnderline.SINGLE, 0, len(text)))
            ibus_text = IBus.Text.new_from_string(text)
            i = 0
            while attrs.get(i) is not None:
                attr = attrs.get(i)
                ibus_text.append_attribute(attr.get_attr_type(),
                                           attr.get_value(),
                                           attr.get_start_index(),
                                           attr.get_end_index())
                i += 1
            self.update_preedit_text_with_mode(
                ibus_text, len(text), True, IBus.PreeditFocusMode.COMMIT)
        else:
            self.update_preedit_text_with_mode(
                IBus.Text.new_from_string(''), 0, False,
                IBus.PreeditFocusMode.CLEAR)

    def on_candidates(self, cands, page, pages, aux):
        """更新候选窗(LookupTable)与辅助区(输入串+页码)。"""
        if not cands:
            self.hide_lookup_table()
            self.hide_auxiliary_text()
            return
        table = IBus.LookupTable()
        table.set_page_size(max(1, len(cands)))
        table.set_orientation(IBus.Orientation.HORIZONTAL)
        table.set_cursor_visible(True)
        table.set_round(False)  # 翻页边界由 core 钳制,不循环
        for i, (text, comment) in enumerate(cands):
            label = '%s %s' % (text, comment) if comment else text
            table.append_candidate(IBus.Text.new_from_string(label))
            if i < 9:
                table.append_label(IBus.Text.new_from_string(str(i + 1)))
        aux_text = aux or ''
        if pages > 1:
            aux_text = '%s  [%d/%d]' % (aux_text, page + 1, pages) \
                if aux_text else '[%d/%d]' % (page + 1, pages)
        if aux_text:
            self.update_auxiliary_text(
                IBus.Text.new_from_string(aux_text), True)
        else:
            self.hide_auxiliary_text()
        self.update_lookup_table(table, True)

    def on_mode_changed(self, mode):
        """模式变化 → 刷新托盘图标。"""
        self._update_mode_property(mode)

    def on_notice(self, text):
        """辅助区临时提示(约 4 秒后自动清除)。"""
        if self._notice_timeout_id:
            GLib.source_remove(self._notice_timeout_id)
            self._notice_timeout_id = 0
        self.update_auxiliary_text(IBus.Text.new_from_string(text), True)
        self._notice_timeout_id = GLib.timeout_add_seconds(
            4, self._clear_notice)

    def _clear_notice(self):
        self._notice_timeout_id = 0
        self.hide_auxiliary_text()
        return False  # 一次性 timeout

    # ------------------------------------------------------------------
    # 外部命令(设置入口/工具菜单)
    # ------------------------------------------------------------------
    def _launch_setup(self):
        """拉起 lyyime-app 设置窗;不存在时给安装指引(不静默失败)。"""
        app = shutil.which('lyyime-app')
        if not app:
            LOGGER.warning('未找到 lyyime-app,无法打开设置界面')
            self.on_notice('未找到 lyyime-app:设置界面随 lyyIme 应用提供,'
                           '请先安装(见 ibus-engine/README.md)')
            return
        # 防重复拉起(借鉴 ibus-table _start_setup 的 pid 复用思路)
        if self._setup_pid:
            done, _ = os.waitpid(self._setup_pid, os.WNOHANG)
            if done == self._setup_pid:
                self._setup_pid = 0
        if self._setup_pid:
            return
        self._setup_pid = subprocess.Popen(
            [app, '--settings'],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).pid

    def _run_in_terminal(self, argv, title):
        """在终端里运行 doctor 子命令,让用户真实看到 dry-run 预览输出。"""
        doctor = shutil.which(argv[0])
        if doctor is None:
            LOGGER.warning('未找到 %s,请先安装 lyyIme doctor 组件', argv[0])
            self.on_notice('未找到 %s:请先安装 lyyIme 工具组件'
                           '(见 ibus-engine/README.md)' % argv[0])
            return
        terminal = shutil.which('xfce4-terminal')
        if terminal:
            cmd = [terminal, '--title=%s' % title, '-x'] + argv
        else:
            xterm = shutil.which('xterm')
            if not xterm:
                # 无终端可用:前台跑完,输出进日志,辅助区给摘要
                try:
                    done = subprocess.run(
                        argv, capture_output=True, text=True, timeout=120)
                    LOGGER.info('%s 输出:\n%s%s', title,
                                done.stdout, done.stderr)
                    self.on_notice('%s 完成,结果已写入日志' % title)
                except Exception:  # noqa: BLE001
                    LOGGER.exception('运行 %s 失败', argv)
                    self.on_notice('运行 %s 失败,详见日志' % title)
                return
            cmd = [xterm, '-title', title, '-e'] + argv
        try:
            subprocess.Popen(cmd, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
        except OSError:
            LOGGER.exception('启动终端失败:%r', cmd)
            self.on_notice('启动终端失败,无法显示 %s' % title)


class EngineFactory(IBus.Factory):
    """引擎工厂:ibus-daemon 为每个输入上下文调用 do_create_engine。

    写法逐行借鉴 ibus-table factory.py(RESEARCH §1.1)。
    """

    def __init__(self, bus):
        self._bus = bus
        self._engine_id = 0
        self._engines = []  # 持引用防 GC;实例销毁自行释放 FFI
        super(EngineFactory, self).__init__(connection=bus.get_connection(),
                                            object_path=IBus.PATH_FACTORY)

    def do_create_engine(self, engine_name):
        path = '/com/lyyime/IBus/engines/%s/engine/%d' % (
            engine_name.replace(':', '_'), self._engine_id)
        self._engine_id += 1
        engine = LyyimeEngine(self._bus, engine_name, path)
        self._engines.append(engine)
        return engine


class IMApp(object):
    """进程主体:连接 ibus-daemon,注册工厂并进入主循环。"""

    def __init__(self, exec_by_ibus):
        self._mainloop = GLib.MainLoop()
        self._bus = IBus.Bus()
        if not self._bus.is_connected():
            # 直接写 stderr:此时日志系统尚未挂文件 handler,必须让用户看得见
            message = ('无法连接 ibus-daemon,请确认 ibus 正在运行'
                       '(可执行 ibus-daemon -drx 后重试)。')
            print('lyyime: ' + message, file=sys.stderr)
            LOGGER.error(message)
            raise SystemExit(1)
        self._bus.connect('disconnected', self._bus_disconnected_cb)
        self._factory = EngineFactory(self._bus)
        if exec_by_ibus:
            # 由 ibus-daemon 拉起:申请总线名(借鉴 ibus-table main.py)
            self._bus.request_name(BUS_NAME, 0)
        else:
            # 手动运行(未装静态 XML 时):动态注册 component,
            # 正式安装请用 install.sh 的静态 lyyime.xml
            component = IBus.Component(
                name=BUS_NAME,
                description='lyyIme Component',
                version=VERSION,
                license='GPLv3',
                author='lyyIme contributors',
                homepage='',
                textdomain='lyyime')
            component.add_engine(IBus.EngineDesc(
                name=ENGINE_NAME,
                longname='lyyIme 五笔拼音',
                description='lyyIme 五笔/拼音/英文混合输入',
                language='zh_CN',
                license='GPLv3',
                author='lyyIme contributors',
                icon=_mode_icon(0).replace('-zh', ''),
                layout='default',
                symbol='伍'))
            self._bus.register_component(component)

    def run(self):
        LOGGER.info('lyyIme ibus 引擎启动(版本 %s)', VERSION)
        self._mainloop.run()

    def _bus_disconnected_cb(self, _bus):
        LOGGER.info('ibus 连接断开,引擎退出')
        self._mainloop.quit()


def setup_logging(verbose=False):
    """日志写 ~/.local/share/lyyime/logs/ibus.log,按天轮转保留 7 份。

    TimedRotatingFileHandler 模式借鉴 ibus-table(RESEARCH §1.6)。
    """
    log_dir = os.path.join(DATA_DIR_DEFAULT, 'logs')
    os.makedirs(log_dir, exist_ok=True)
    handler = logging.handlers.TimedRotatingFileHandler(
        os.path.join(log_dir, 'ibus.log'),
        when='midnight', backupCount=7, encoding='utf-8')
    handler.setFormatter(logging.Formatter(
        '%(asctime)s %(levelname)s %(name)s %(message)s'))
    LOGGER.addHandler(handler)
    LOGGER.setLevel(logging.DEBUG if verbose else logging.INFO)
    if verbose:
        stream = logging.StreamHandler()
        stream.setFormatter(logging.Formatter('%(levelname)s %(message)s'))
        LOGGER.addHandler(stream)


def parse_args(argv=None):
    """解析命令行:--ibus 由 ibus-daemon 传入;手动运行时不带。"""
    parser = argparse.ArgumentParser(
        prog='ibus-engine-lyyime',
        description='lyyIme 五笔拼音 ibus 引擎(Mode A)')
    parser.add_argument('--ibus', '-i', action='store_true',
                        help='由 ibus-daemon 拉起时自动附加,手动运行勿用')
    parser.add_argument('-v', '--verbose', action='store_true',
                        help='调试日志(DEBUG 级,同时输出到终端)')
    parser.add_argument('--version', action='version',
                        version='lyyime ibus engine %s' % VERSION)
    return parser.parse_args(argv)


def main(argv=None):
    """入口:装日志 → 起 IMApp → 主循环。"""
    args = parse_args(argv)
    setup_logging(verbose=args.verbose)
    app = IMApp(exec_by_ibus=args.ibus)
    try:
        app.run()
    except KeyboardInterrupt:
        LOGGER.info('收到中断,退出')


if __name__ == '__main__':
    main()
