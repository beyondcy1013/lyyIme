# -*- coding: utf-8 -*-
"""lyyime_ffi.py —— lyyime-core C ABI 的 ctypes 封装(Mode A 专用)。

合同出处:docs/ARCHITECTURE.md §3(FFI 合同)。本文件逐字实现该合同的
python 侧:库路径探测、原型声明、-needed 缓冲不足重试、effects JSON 解析。

用法::

    from lyyime_ffi import LyyimeFfi, LKEY_CHAR
    ffi = LyyimeFfi(data_dir='/home/me/.local/share/lyyime')
    effects = ffi.process_key(LKEY_CHAR, ord('a'))   # -> [{'t': 'preedit', ...}, ...]

线程模型:全部函数线程不安全,宿主(ibus 引擎主循环)保证单线程调用。
"""

import ctypes
import json
import os
from ctypes import (CFUNCTYPE, POINTER, c_char_p, c_int, c_int64, c_uint32,
                    c_void_p, create_string_buffer)

# ---------------------------------------------------------------------------
# LKey 枚举(与 ARCHITECTURE.md §3 完全一致,勿改动数值)
# ---------------------------------------------------------------------------
LKEY_CHAR = 0        # 小写字母 a–z,码点经 chr 参数传入
LKEY_DIGIT = 1       # 数字 '1'..'9',码点经 chr 参数传入('1'=0x31,core 取 chr-0x30)
LKEY_SPACE = 2
LKEY_ENTER = 3
LKEY_BACKSPACE = 4
LKEY_ESC = 5
LKEY_PAGEUP = 6
LKEY_PAGEDOWN = 7
LKEY_PUNCT = 8       # 标点原字符(半角),码点经 chr 参数传入
LKEY_SHIFTPRESS = 9  # Shift 单击(host 侧完成单击检测后才会送来,见引擎 lyyime.py)
LKEY_OTHER = 10      # 其余键:core 恒回 pass

# 库文件名(liblyyime_core.so 的 basename)
_CORE_LIB_NAME = 'liblyyime_core.so'


class LyyimeFfiError(Exception):
    """lyyime FFI 相关错误基类(带人话提示)。"""


class LibraryNotFoundError(LyyimeFfiError):
    """找不到 liblyyime_core.so;异常文本内含修复指引。"""


def _project_root():
    """返回仓库根目录(ibus-engine/engine/lyyime_ffi.py 上溯两级);取不到则 None。"""
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.abspath(os.path.join(here, '..', '..'))
    return root if os.path.isdir(root) else None


def find_core_library():
    """按合同约定顺序探测 liblyyime_core.so,返回首个存在的绝对路径。

    探测顺序(任务书规定,缺失即跳过):
      1. 环境变量 ``$LYYIME_CORE_LIB`` 指定的确切路径;
      2. /usr/local/lib/lyyime/(集成阶段主控放置真库的约定路径);
      3. /usr/local/lib/;
      4. /usr/lib64/;
      5. 项目 CARGO_TARGET_DIR(/data/cargo-target/local/lyyIme 与
         $CARGO_TARGET_DIR 环境变量,均只找 release 子目录);
      6. 开发态:仓库内 target/release(相对本模块位置推断)。

    :returns: str,存在的 .so 绝对路径
    :raises LibraryNotFoundError: 全部候选都不存在时,附修复指引。
    """
    candidates = []
    env_lib = os.environ.get('LYYIME_CORE_LIB')
    if env_lib:
        # 显式指定的路径具最高权威:存在直接用;不存在立即报错,
        # 不静默落到其它候选(否则用户排查会被误导)。
        env_lib = os.path.abspath(env_lib)
        if os.path.isfile(env_lib):
            return env_lib
        raise LibraryNotFoundError(
            '环境变量 LYYIME_CORE_LIB=%s 指向的文件不存在。\n'
            '修复方法(任选其一):\n'
            '  1. 修正路径:export LYYIME_CORE_LIB=/正确的/liblyyime_core.so\n'
            '  2. 取消指定,按默认顺序探测:unset LYYIME_CORE_LIB\n'
            '  3. 核心尚未构建时先执行 scripts/build.sh;放入系统路径后\n'
            '     记得刷新动态库缓存:sudo ldconfig' % env_lib)
    candidates.append(os.path.join('/usr/local/lib/lyyime', _CORE_LIB_NAME))
    candidates.append(os.path.join('/usr/local/lib', _CORE_LIB_NAME))
    candidates.append(os.path.join('/usr/lib64', _CORE_LIB_NAME))
    # CARGO_TARGET_DIR:AGENTS.MD 硬性规定为 /data/cargo-target/local/lyyIme
    for base in ('/data/cargo-target/local/lyyIme',
                 os.environ.get('CARGO_TARGET_DIR') or ''):
        if base:
            candidates.append(os.path.join(base, 'release', _CORE_LIB_NAME))
    root = _project_root()
    if root:
        candidates.append(os.path.join(root, 'target', 'release', _CORE_LIB_NAME))

    for path in candidates:
        if path and os.path.isfile(path):
            return path

    searched = '\n  '.join(c for c in candidates if c)
    raise LibraryNotFoundError(
        '找不到 Rust 核心库 liblyyime_core.so,lyyIme ibus 引擎无法启动。\n'
        '已按顺序探测以下路径,均不存在:\n  %s\n'
        '修复方法(任选其一):\n'
        '  1. 先构建核心:scripts/build.sh(产物在 $CARGO_TARGET_DIR/release/);\n'
        '  2. 或把 liblyyime_core.so 放到约定路径后执行 ldconfig:\n'
        '       sudo install -Dm755 liblyyime_core.so /usr/local/lib/lyyime/\n'
        '       sudo ldconfig\n'
        '  3. 或临时指定路径:export LYYIME_CORE_LIB=/路径/liblyyime_core.so\n'
        % searched)


def resolve_data_dir():
    """词典目录回退链(与 lyyime-xim main.c resolve_dict_dir 保持一致):

    $LYYIME_DATA_DIR → 用户级 data/ 目录(含 meta.json 才认)
    → /usr/local/share/lyyime/data(install-all.sh 系统安装位)
    → ~/.local/share/lyyime(兜底:历史行为;core 对缺失词库降级为空引擎)。
    """
    env = os.environ.get('LYYIME_DATA_DIR')
    if env:
        return env
    user = os.path.join(os.path.expanduser('~'), '.local', 'share', 'lyyime')
    candidates = []
    xdg = os.environ.get('XDG_DATA_HOME')
    if xdg:
        candidates.append(os.path.join(xdg, 'lyyime', 'data'))
    candidates.append(os.path.join(user, 'data'))
    candidates.append('/usr/local/share/lyyime/data')
    for cand in candidates:
        if os.path.isfile(os.path.join(cand, 'meta.json')):
            return cand
    return user


def _bind_prototypes(lib):
    """给 CDLL 对象声明 §3 合同的函数原型(参数/返回类型)。"""
    # int64_t lyyime_process_key(void*, int, uint32_t, char*, int64_t)
    lib.lyyime_new.argtypes = [c_char_p]
    lib.lyyime_new.restype = c_void_p
    lib.lyyime_free.argtypes = [c_void_p]
    lib.lyyime_free.restype = None
    lib.lyyime_reset.argtypes = [c_void_p]
    lib.lyyime_reset.restype = None
    lib.lyyime_mode.argtypes = [c_void_p]
    lib.lyyime_mode.restype = c_int
    lib.lyyime_toggle_mode.argtypes = [c_void_p]
    lib.lyyime_toggle_mode.restype = c_int
    lib.lyyime_process_key.argtypes = [c_void_p, c_int, c_uint32,
                                       c_char_p, c_int64]
    lib.lyyime_process_key.restype = c_int64
    lib.lyyime_cand.argtypes = [c_void_p, c_int, c_char_p, c_int]
    lib.lyyime_cand.restype = c_int
    lib.lyyime_cand_comment.argtypes = [c_void_p, c_int, c_char_p, c_int]
    lib.lyyime_cand_comment.restype = c_int
    return lib


class LyyimeFfi(object):
    """liblyyime_core.so 的进程内封装:一个实例 = 一个 core Engine。

    :param data_dir: 词典/用户数据目录;None 时按 resolve_data_dir() 回退链解析。
    :param lib_path: 显式指定 .so 路径(测试桩用);None 时自动探测。
    :param initial_buf_cap: effects JSON 首次调用的缓冲字节数(仅测试用,
        调小可强制走 -needed 重试路径)。

    建议用 with 语句或显式 free();实例被回收时也会兜底 free。
    """

    def __init__(self, data_dir=None, lib_path=None, initial_buf_cap=4096):
        self._initial_buf_cap = max(16, int(initial_buf_cap))
        if lib_path is None:
            lib_path = find_core_library()
        # RTLD_LOCAL:不把核心符号泄进主进程命名空间,避免与宿主冲突
        self._lib = _bind_prototypes(ctypes.CDLL(lib_path, mode=ctypes.RTLD_LOCAL))
        self._lib_path = lib_path
        if data_dir is None:
            data_dir = resolve_data_dir()
        self.data_dir = data_dir
        if not isinstance(data_dir, bytes):
            data_dir = data_dir.encode('utf-8')
        self._eng = self._lib.lyyime_new(data_dir)
        if not self._eng:
            raise LyyimeFfiError(
                'lyyime_new(%r) 返回 NULL:核心初始化失败。\n'
                '请检查目录是否可写、词典文件是否就位(可运行 lyyime-doctor check)。'
                % self.data_dir)
        self._closed = False

    # -- 生命周期 ----------------------------------------------------------
    def free(self):
        """释放 core Engine;幂等,可安全重复调用。"""
        if not self._closed and getattr(self, '_eng', None):
            self._lib.lyyime_free(self._eng)
        self._closed = True
        self._eng = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.free()
        return False

    def __del__(self):
        try:
            self.free()
        except Exception:  # 解释器退出期的兜底,忽略一切清理错误
            pass

    def _check_open(self):
        if self._closed or not self._eng:
            raise LyyimeFfiError('引擎已释放(free 之后不可再用),请重新创建 LyyimeFfi。')

    # -- 合同函数 ----------------------------------------------------------
    def reset(self):
        """清空输入缓冲(焦点切换时宿主调用);不改变中英模式。"""
        self._check_open()
        self._lib.lyyime_reset(self._eng)

    def mode(self):
        """返回当前模式:int,0=中文 1=英文。"""
        self._check_open()
        return int(self._lib.lyyime_mode(self._eng))

    def toggle_mode(self):
        """切换中英模式,返回切换后的模式(0=中文 1=英文)。"""
        self._check_open()
        return int(self._lib.lyyime_toggle_mode(self._eng))

    def process_key(self, key_id, char=0):
        """喂一个抽象键,返回效果流列表 [{t,s,n,page,pages,m}, ...]。

        :param key_id: LKEY_* 常量之一。
        :param char: Char/Digit/Punct 的码点(int);其余键填 0。
        :raises LyyimeFfiError: JSON 解析失败或核心返回异常时。
        """
        self._check_open()
        cap = self._initial_buf_cap
        for _attempt in range(4):
            buf = create_string_buffer(cap)
            needed = int(self._lib.lyyime_process_key(
                self._eng, int(key_id), int(char & 0xFFFFFFFF), buf,
                c_int64(cap)))
            if needed < 0:
                # 合同:缓冲不足时不写入,返回 -needed;据此扩容重试
                cap = -needed
                continue
            raw = buf.value  # 到 \0 截断
            if not raw:
                return []
            try:
                effects = json.loads(raw.decode('utf-8'))
            except (ValueError, UnicodeDecodeError) as exc:
                raise LyyimeFfiError(
                    'process_key 返回的 effects JSON 解析失败(%s):%r'
                    % (exc, raw[:200]))
            if not isinstance(effects, list):
                raise LyyimeFfiError(
                    'process_key 返回的 effects 不是 JSON 数组:%r' % raw[:200])
            return effects
        raise LyyimeFfiError('process_key 缓冲扩容重试 4 次仍未成功,核心行为异常。')

    def _fetch_cstr(self, cfunc, index):
        """lyyime_cand / lyyime_cand_comment 的公共实现(含 -needed 重试)。"""
        self._check_open()
        cap = self._initial_buf_cap
        for _attempt in range(4):
            buf = create_string_buffer(cap)
            ret = int(cfunc(self._eng, int(index), buf, cap))
            if ret < 0:
                cap = -ret
                continue
            if ret == 0:
                return ''
            return buf.value.decode('utf-8', errors='replace')
        raise LyyimeFfiError('候选缓冲扩容重试 4 次仍未成功,核心行为异常。')

    def cand(self, index):
        """取当前页第 index 个候选文本(0 起;越界返回空串)。"""
        return self._fetch_cstr(self._lib.lyyime_cand, index)

    def cand_comment(self, index):
        """取当前页第 index 个候选的注释(码型提示;越界返回空串)。"""
        return self._fetch_cstr(self._lib.lyyime_cand_comment, index)
