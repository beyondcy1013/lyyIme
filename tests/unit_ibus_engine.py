#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""unit_ibus_engine.py —— lyyIme ibus 引擎单元测试(不经 dbus/daemon)。

模式说明(借鉴 ibus-table 的 MockEngine 模式,RESEARCH.md §1.5):

* 默认模式:用 gcc 把 tests/stub_lyyime_core.c 编译成 ABI 桩库
  (逐字实现 ARCHITECTURE.md §3 全部导出符号,行为确定性),不经 dbus
  直接实例化 lyyime.EngineLogic,喢单元键序列断言效果流。
* ``--real-core`` 集成模式:LYYIME_CORE_LIB 指向真 liblyyime_core.so 时
  跑核心对接冒烟;真库不存在则跳过并说明。

运行::

    python3 tests/unit_ibus_engine.py            # 桩库模式,全量断言
    LYYIME_CORE_LIB=/路径/liblyyime_core.so \
        python3 tests/unit_ibus_engine.py --real-core   # 真库集成冒烟
"""

import os
import shutil
import subprocess
import sys
import tempfile
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
_PROJECT = os.path.dirname(_HERE)
_ENGINE_DIR = os.path.join(_PROJECT, 'ibus-engine', 'engine')
_STUB_SRC = os.path.join(_HERE, 'stub_lyyime_core.c')
_STUB_BUILD_DIR = os.path.join(_HERE, '_build')
_STUB_SO = os.path.join(_STUB_BUILD_DIR, 'liblyyime_core_stub.so')

# 把引擎目录放进 import 路径(单测不经 dbus,直接 import 逻辑层)
sys.path.insert(0, _ENGINE_DIR)

import lyyime  # noqa: E402
import lyyime_ffi  # noqa: E402


def ensure_stub_library():
    """确保桩库已编译:源码比产物新或产物缺失时重新 gcc 编译。"""
    if not os.path.isfile(_STUB_SRC):
        raise SystemExit('缺少桩库源码:%s' % _STUB_SRC)
    need_build = not os.path.isfile(_STUB_SO) or \
        os.path.getmtime(_STUB_SO) < os.path.getmtime(_STUB_SRC)
    if need_build:
        os.makedirs(_STUB_BUILD_DIR, exist_ok=True)
        gcc = shutil.which('gcc')
        if gcc is None:
            raise SystemExit('未找到 gcc:桩库模式需要 gcc 编译 '
                             'tests/stub_lyyime_core.c(真机集成可用 '
                             'LYYIME_CORE_LIB=... --real-core)')
        subprocess.run(
            [gcc, '-shared', '-fPIC', '-O2', '-Wall', '-Wextra',
             '-o', _STUB_SO, _STUB_SRC],
            check=True)
    return _STUB_SO


class MockHost(object):
    """效果流捕获器:替代 IBus.Engine,记录全部宿主回调(借鉴 MockEngine)。"""

    def __init__(self):
        self.commits = []
        self.preedits = []      # 记录每次 on_preedit(含 None)
        self.candidates = []    # [(cands, page, pages, aux), ...]
        self.modes = []         # 每次 on_mode_changed
        self.notices = []

    def on_commit(self, text):
        self.commits.append(text)

    def on_preedit(self, text):
        self.preedits.append(text)

    def on_candidates(self, cands, page, pages, aux):
        self.candidates.append((cands, page, pages, aux))

    def on_mode_changed(self, mode):
        self.modes.append(mode)

    def on_notice(self, text):
        self.notices.append(text)

    # -- 断言辅助 -------------------------------------------------------
    @property
    def last_preedit(self):
        return self.preedits[-1] if self.preedits else None

    @property
    def last_candidates(self):
        return self.candidates[-1] if self.candidates else None


class LogicTestCase(unittest.TestCase):
    """桩库模式下的逻辑层用例基类。"""

    def make_logic(self, **ffi_kw):
        """新建 (EngineLogic, MockHost, LyyimeFfi);每次用独立引擎实例。"""
        host = MockHost()
        ffi = lyyime_ffi.LyyimeFfi(data_dir=self.tmpdir, lib_path=STUB_SO,
                                   **ffi_kw)
        logic = lyyime.EngineLogic(host=host, ffi=ffi, page_size=5)
        self._ffis.append(ffi)
        return logic, host, ffi

    @classmethod
    def setUpClass(cls):
        cls.tmpdir = tempfile.mkdtemp(prefix='lyyime-unit-')

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmpdir, ignore_errors=True)

    def setUp(self):
        self._ffis = []

    def tearDown(self):
        for ffi in self._ffis:
            ffi.free()

    # -- 按键常量(纯 int,与引擎逻辑层一致) -----------------------------
    K_A, K_B, K_N, K_H, K_I, K_O, K_U = 0x61, 0x62, 0x6e, 0x68, 0x69, 0x6f, 0x75
    K_1 = 0x31
    K_SPACE, K_ESC, K_RET, K_BS = 0x20, 0xff1b, 0xff0d, 0xff08
    K_PGUP, K_PGDN = 0xff55, 0xff56
    K_SHIFT_L, K_SHIFT_R = 0xffe1, 0xffe2
    K_F1 = 0xffbe
    MASK_SHIFT, MASK_CTRL = 1 << 0, 1 << 2
    MASK_RELEASE = 1 << 30

    def press(self, logic, keyval, state=0):
        return logic.process_key_event(keyval, 0, state)

    # ------------------------------------------------------------------
    # 1. nihao → preedit + 候选
    # ------------------------------------------------------------------
    def test_nihao_preedit_and_candidates(self):
        logic, host, _ = self.make_logic()
        for ch in 'nihao':
            ret = self.press(logic, ord(ch))
            self.assertTrue(ret, '字母键应被消费')
        self.assertEqual(host.last_preedit, 'nihao')
        cands, page, pages, aux = host.last_candidates
        self.assertEqual(len(cands), 5)
        self.assertEqual((page, pages), (0, 2))
        self.assertEqual(aux, 'nihao')
        self.assertTrue(all(c[0] and c[1] for c in cands), '候选文本与注释均非空')

    # ------------------------------------------------------------------
    # 2. 数字 1 选词 → commit + preedit 清除
    # ------------------------------------------------------------------
    def test_digit_select_commits_first_candidate(self):
        logic, host, _ = self.make_logic()
        for ch in 'nihao':
            self.press(logic, ord(ch))
        ret = self.press(logic, self.K_1)
        self.assertTrue(ret)
        self.assertEqual(host.commits, ['nihao壹'])
        self.assertIsNone(host.last_preedit, '选词后 preedit 应清除')

    # ------------------------------------------------------------------
    # 3. Space 有缓冲 → commit"栈";空缓冲 → 放行
    # ------------------------------------------------------------------
    def test_space_commits_when_buffered(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_H)
        self.press(logic, self.K_I)
        self.assertTrue(self.press(logic, self.K_SPACE))
        self.assertEqual(host.commits, ['栈'])
        self.assertIsNone(host.last_preedit)

    def test_space_empty_buffer_passes_through(self):
        logic, host, _ = self.make_logic()
        self.assertFalse(self.press(logic, self.K_SPACE))
        self.assertEqual(host.commits, [])

    # ------------------------------------------------------------------
    # 4. Enter 有缓冲 → commit 原始字母(中英混合直通)
    # ------------------------------------------------------------------
    def test_enter_commits_raw_letters(self):
        logic, host, _ = self.make_logic()
        for ch in 'nihao':
            self.press(logic, ord(ch))
        self.assertTrue(self.press(logic, self.K_RET))
        self.assertEqual(host.commits, ['nihao'])

    def test_enter_empty_buffer_passes_through(self):
        logic, _, _ = self.make_logic()
        self.assertFalse(self.press(logic, self.K_RET))

    # ------------------------------------------------------------------
    # 5. Backspace 删尾;Esc 清缓冲
    # ------------------------------------------------------------------
    def test_backspace_shrinks_preedit(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_N)
        self.press(logic, self.K_I)
        self.press(logic, self.K_H)
        self.press(logic, self.K_BS)
        self.assertEqual(host.last_preedit, 'ni')

    def test_escape_clears_buffer_and_consumes(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_N)
        self.assertTrue(self.press(logic, self.K_ESC))
        self.assertIsNone(host.last_preedit)
        # 空缓冲 Esc → 放行
        self.assertFalse(self.press(logic, self.K_ESC))

    # ------------------------------------------------------------------
    # 6. Shift 单击 → 模式切换;组合/期间有键 → 不切换
    # ------------------------------------------------------------------
    def test_shift_click_toggles_mode(self):
        logic, host, _ = self.make_logic()
        ret_press = self.press(logic, self.K_SHIFT_L)
        self.assertTrue(ret_press, 'Shift 单击按下应被吞下')
        ret_release = self.press(logic, self.K_SHIFT_L, self.MASK_RELEASE)
        self.assertTrue(ret_release, 'Shift 单击释放应被消费')
        self.assertEqual(host.modes, [1], '应广播 ModeChanged(1=英文)')

    def test_shift_press_with_buffer_commits_raw_letters(self):
        logic, host, _ = self.make_logic()
        for ch in 'ni':
            self.press(logic, ord(ch))
        self.assertTrue(self.press(logic, self.K_SHIFT_L))
        self.assertEqual(host.commits, ['ni'])
        self.assertIsNone(host.last_preedit)
        self.assertEqual(host.modes, [], '有缓冲 Shift 上屏原串,不切换模式')

    def test_shift_combo_letter_does_not_toggle(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_SHIFT_L)                    # Shift 按下
        ret = self.press(logic, self.K_A, self.MASK_SHIFT)   # Shift+A 组合
        self.assertTrue(ret)
        self.assertEqual(host.last_preedit, 'a', '组合键的字母小写化入缓冲')
        self.assertEqual(host.modes, [], '组合键不得触发切换')
        # Shift 释放:单击已被打断 → 放行且不切换
        self.assertFalse(self.press(logic, self.K_SHIFT_L, self.MASK_RELEASE))
        self.assertEqual(host.modes, [])

    def test_shift_with_ctrl_is_not_a_click(self):
        logic, host, _ = self.make_logic()
        self.assertFalse(self.press(logic, self.K_SHIFT_L, self.MASK_CTRL))
        self.assertFalse(
            self.press(logic, self.K_SHIFT_L, self.MASK_CTRL | self.MASK_RELEASE))
        self.assertEqual(host.modes, [], '带 Ctrl 的 Shift 不算单击')

    def test_any_key_between_press_release_cancels_click(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_SHIFT_L)
        self.press(logic, self.K_B)          # 期间来了别的键
        self.assertFalse(self.press(logic, self.K_SHIFT_L, self.MASK_RELEASE))
        self.assertEqual(host.modes, [])

    def test_shift_click_toggles_back_to_chinese(self):
        logic, host, ffi = self.make_logic()
        self.press(logic, self.K_SHIFT_L)
        self.press(logic, self.K_SHIFT_L, self.MASK_RELEASE)
        self.press(logic, self.K_SHIFT_R)                    # 右 Shift 再单击
        self.press(logic, self.K_SHIFT_R, self.MASK_RELEASE)
        self.assertEqual(host.modes, [1, 0], '两次单击应回到中文')
        self.assertEqual(ffi.mode(), 0)

    # ------------------------------------------------------------------
    # 7. 密码框(InputPurpose PASSWORD/PIN)全放行
    # ------------------------------------------------------------------
    def test_password_purpose_all_keys_pass_through(self):
        logic, host, _ = self.make_logic()
        for purpose in (lyyime.PURPOSE_PASSWORD, lyyime.PURPOSE_PIN):
            logic.input_purpose = purpose
            self.assertFalse(self.press(logic, self.K_N))
            self.assertFalse(self.press(logic, self.K_1))
            self.assertFalse(self.press(logic, self.K_SHIFT_L))
            self.assertFalse(
                self.press(logic, self.K_SHIFT_L, self.MASK_RELEASE))
        self.assertEqual(host.commits + host.modes, [], '密码框零消费')

    # ------------------------------------------------------------------
    # 8. release 事件放行(RESEARCH §1.2)
    # ------------------------------------------------------------------
    def test_letter_release_passes_through(self):
        logic, _, _ = self.make_logic()
        self.assertFalse(
            self.press(logic, self.K_A, self.MASK_RELEASE),
            '字母 release 必须 return False 放行(Qt5 兼容)')

    # ------------------------------------------------------------------
    # 9. 翻页与边界钳制
    # ------------------------------------------------------------------
    def test_page_down_up_boundary(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_N)
        for _ in range(5):  # 连续下翻越界也应钳在最后一页
            self.assertTrue(self.press(logic, self.K_PGDN))
        _, page, pages, _ = host.last_candidates
        self.assertEqual((page, pages), (1, 2), '下翻应钳制在最后一页')
        for _ in range(5):
            self.press(logic, self.K_PGUP)
        _, page, _, _ = host.last_candidates
        self.assertEqual(page, 0, '上翻应钳制在第一页')
        self.assertTrue(self.press(logic, self.K_PGUP))  # 第一页再上翻仍消费
        self.assertEqual(host.last_candidates[1], 0)

    # ------------------------------------------------------------------
    # 10. 英文态字母/空格直通
    # ------------------------------------------------------------------
    def test_english_mode_passes_letters(self):
        logic, host, _ = self.make_logic()
        logic.switch_mode()
        self.assertEqual(host.modes[-1], 1)
        self.assertFalse(self.press(logic, self.K_A), '英文态字母放行')
        self.assertFalse(self.press(logic, self.K_SPACE))
        self.assertIsNone(host.last_preedit)

    # ------------------------------------------------------------------
    # 11. 中文标点:空缓冲出中文标点;有缓冲首选+标点
    # ------------------------------------------------------------------
    def test_punct_empty_buffer_commits_chinese_punct(self):
        logic, host, _ = self.make_logic()
        self.assertTrue(self.press(logic, 0x2e))  # '.'
        self.assertEqual(host.commits, ['。'])

    def test_punct_with_buffer_commits_candidate_plus_punct(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_N)
        self.press(logic, self.K_I)
        self.assertTrue(self.press(logic, 0x2c))  # ','
        self.assertEqual(host.commits, ['ni壹', ','])
        self.assertIsNone(host.last_preedit)

    # ------------------------------------------------------------------
    # 12. 其它键:core 先复位缓冲再放行
    # ------------------------------------------------------------------
    def test_other_key_resets_buffer_then_passes(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_N)
        self.press(logic, self.K_I)
        self.assertFalse(self.press(logic, self.K_F1), 'F1 应放行给应用')
        self.assertIsNone(host.last_preedit, '放行前缓冲应已复位')
        self.assertFalse(host.commits)

    # ------------------------------------------------------------------
    # 13. FFI -needed 缓冲不足重试路径
    # ------------------------------------------------------------------
    def test_process_key_retry_on_small_buffer(self):
        small, host_s, _ = self.make_logic(initial_buf_cap=8)   # 强制 -needed 重试
        big, host_b, _ = self.make_logic(initial_buf_cap=4096)
        for logic, host in ((small, host_s), (big, host_b)):
            for ch in 'nihao':
                logic.process_key_event(ord(ch), 0, 0)
            logic.process_key_event(self.K_1, 0, 0)
        self.assertEqual(host_s.commits, host_b.commits)
        self.assertEqual(host_s.preedits, host_b.preedits)
        self.assertEqual(host_s.candidates, host_b.candidates)

    # ------------------------------------------------------------------
    # 14. 降级态(FFI 不可用)英文直通:一切放行
    # ------------------------------------------------------------------
    def test_degraded_mode_passes_all(self):
        host = MockHost()
        logic = lyyime.EngineLogic(host=host, ffi=None)
        self.assertTrue(logic.degraded)
        for keyval in (self.K_A, self.K_SPACE, self.K_SHIFT_L, self.K_1):
            self.assertFalse(self.press(logic, keyval))
        self.assertFalse(host.commits + host.modes)

    # ------------------------------------------------------------------
    # 15. LyyimeFfi:库缺失时给出可操作的修复指引
    # ------------------------------------------------------------------
    def test_missing_library_raises_with_guidance(self):
        old = os.environ.get('LYYIME_CORE_LIB')
        os.environ['LYYIME_CORE_LIB'] = '/nonexistent/path/liblyyime_core.so'
        try:
            with self.assertRaises(lyyime_ffi.LibraryNotFoundError) as ctx:
                lyyime_ffi.find_core_library()
            msg = str(ctx.exception)
            self.assertIn('ldconfig', msg)
            self.assertIn('LYYIME_CORE_LIB', msg)
            self.assertIn('scripts/build.sh', msg)
        finally:
            if old is None:
                os.environ.pop('LYYIME_CORE_LIB', None)
            else:
                os.environ['LYYIME_CORE_LIB'] = old

    # ------------------------------------------------------------------
    # 16. 桩库导出 §3 全部符号;mode/toggle/reset 行为正确
    # ------------------------------------------------------------------
    def test_stub_exports_and_mode_api(self):
        import ctypes
        lib = ctypes.CDLL(STUB_SO)
        for sym in ('lyyime_new', 'lyyime_free', 'lyyime_reset', 'lyyime_mode',
                    'lyyime_toggle_mode', 'lyyime_process_key', 'lyyime_cand',
                    'lyyime_cand_comment'):
            self.assertTrue(hasattr(lib, sym), '桩库缺少合同符号 %s' % sym)
        _logic, _host, ffi = self.make_logic()
        self.assertEqual(ffi.mode(), 0, '启动默认中文态')
        self.assertEqual(ffi.toggle_mode(), 1)
        self.assertEqual(ffi.mode(), 1)
        ffi.reset()
        self.assertEqual(ffi.mode(), 1, 'reset 不改变模式')

    # ------------------------------------------------------------------
    # 17. effects JSON 与合同格式一致(逐字段抽查)
    # ------------------------------------------------------------------
    def test_effects_json_contract_shape(self):
        _logic, _host, ffi = self.make_logic()
        effects = ffi.process_key(lyyime_ffi.LKEY_CHAR, ord('n'))
        self.assertEqual(effects[0], {'t': 'preedit', 's': 'n'})
        self.assertEqual(effects[1],
                         {'t': 'cands', 'n': 5, 'page': 0, 'pages': 2})
        cand0 = ffi.cand(0)
        self.assertEqual(cand0, 'n壹')
        self.assertEqual(ffi.cand_comment(0), '注壹')
        self.assertEqual(ffi.cand(99), '', '越界候选返回空串')
        ffi.reset()
        mode_eff = ffi.process_key(lyyime_ffi.LKEY_SHIFTPRESS, 0)
        self.assertEqual(mode_eff, [{'t': 'consumed'}])
        pass_eff = ffi.process_key(lyyime_ffi.LKEY_OTHER, 0)
        self.assertEqual(pass_eff, [{'t': 'pass'}])

    # ------------------------------------------------------------------
    # 18. preedit 两种形态:带 s=内容,无 s=清除(集成通报口径)
    # ------------------------------------------------------------------
    def test_preedit_clear_form_without_s_field(self):
        logic, host, _ = self.make_logic()
        self.press(logic, self.K_N)
        self.assertTrue(self.press(logic, self.K_ESC))
        self.assertIsNone(host.last_preedit, 'Esc 清空应把 preedit 置 None')
        # 直接校验分发层对两种形态的处理(真核心清空输出无 s 字段):
        logic._dispatch([{'t': 'preedit'}])
        self.assertIsNone(host.preedits[-1], '无 s 字段应解析为清除')
        logic._dispatch([{'t': 'preedit', 's': 'nihao'}])
        self.assertEqual(host.preedits[-1], 'nihao')
        logic._dispatch([{'t': 'preedit', 's': ''}])
        self.assertIsNone(host.preedits[-1], '空 s 也应解析为清除')


class RealCoreTestCase(unittest.TestCase):
    """--real-core:真 liblyyime_core.so 集成冒烟(库缺失则跳过并说明)。"""

    def test_real_core_smoke(self):
        try:
            lib_path = lyyime_ffi.find_core_library()
        except lyyime_ffi.LibraryNotFoundError as exc:
            self.skipTest(str(exc))
        if os.path.basename(lib_path).endswith('_stub.so'):
            self.skipTest(
                '真库未就绪:当前只找到单元测试桩库(%s)。'
                '集成阶段请先 scripts/build.sh 构建真核心,或 '
                'export LYYIME_CORE_LIB=/路径/liblyyime_core.so' % lib_path)
        data_dir = os.environ.get('LYYIME_DATA_DIR') or os.path.expanduser(
            '~/.local/share/lyyime')
        if not os.path.isdir(data_dir):
            self.skipTest(
                '真库需要词典数据:未找到数据目录 %s,请设置 '
                'LYYIME_DATA_DIR 指向含 wubi.tsv 等词典的目录' % data_dir)
        host = MockHost()
        ffi = lyyime_ffi.LyyimeFfi(data_dir=data_dir, lib_path=lib_path)
        logic = lyyime.EngineLogic(host=host, ffi=ffi, page_size=5)
        try:
            for ch in 'nihao':
                logic.process_key_event(ord(ch), 0, 0)
            # 真核心行为断言从宽:有 preedit/候选即为对接成功
            self.assertTrue(host.preedits, '真核心应返回 preedit 效果')
            self.assertTrue(host.candidates, '真核心应返回候选效果')
            logic.process_key_event(0x20, 0, 0)
            self.assertTrue(host.commits, '空格应有上屏效果')
        finally:
            ffi.free()


def main():
    real_core = '--real-core' in sys.argv[1:]
    ensure_stub_library()
    # 桩库模式默认注入;外部已显式指定 LYYIME_CORE_LIB(真库路径)时不覆盖
    if 'LYYIME_CORE_LIB' not in os.environ:
        os.environ['LYYIME_CORE_LIB'] = STUB_SO
    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    suite.addTests(loader.loadTestsFromTestCase(LogicTestCase))
    if real_core:
        # 真库集成模式:尊重外部 LYYIME_CORE_LIB,未指定时按默认顺序探测
        # (含 /data/cargo-target/local/lyyIme/release);只找到桩库则跳过并说明
        suite.addTests(loader.loadTestsFromTestCase(RealCoreTestCase))
    runner = unittest.TextTestRunner(verbosity=2)
    result = runner.run(suite)
    sys.exit(0 if result.wasSuccessful() else 1)


STUB_SO = ensure_stub_library()

if __name__ == '__main__':
    main()
