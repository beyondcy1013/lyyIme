#!/usr/bin/env python3
"""lyyime-core FFI ctypes 冒烟测试(ARCHITECTURE.md §10)。

用法:
    python3 crates/lyyime-core/ffi-test.py
    LYYIME_CORE_SO=/path/to/liblyyime_core.so python3 ...

流程:加载 liblyyime_core.so → 用 tests/fixtures 建引擎 →
喂 "nihao<space>" → 断言效果流 JSON 与候选/注释/-needed 约定。
HOME 会被重定向到临时目录,不污染真实用户词典。
"""

import ctypes
import json
import os
import sys
import tempfile
from pathlib import Path

# ---- 常量(与合同 §3、ffi.rs 一致) ----
LKEY_CHAR, LKEY_DIGIT, LKEY_SPACE, LKEY_ENTER = 0, 1, 2, 3
LKEY_BACKSPACE, LKEY_ESC, LKEY_PAGEUP, LKEY_PAGEDOWN = 4, 5, 6, 7
LKEY_PUNCT, LKEY_SHIFTPRESS, LKEY_OTHER = 8, 9, 10

SCRIPT_DIR = Path(__file__).resolve().parent
FIXTURES = SCRIPT_DIR / "tests" / "fixtures"


def load_lib() -> ctypes.CDLL:
    so = os.environ.get("LYYIME_CORE_SO")
    if not so:
        target = os.environ.get(
            "CARGO_TARGET_DIR", "/data/cargo-target/local/lyyIme"
        )
        so = str(Path(target) / "debug" / "liblyyime_core.so")
    if not Path(so).exists():
        print(f"找不到 liblyyime_core.so({so}),请先 cargo build 或设置 LYYIME_CORE_SO")
        sys.exit(1)
    lib = ctypes.CDLL(so)
    # void* lyyime_new(const char*); 其余按合同声明
    lib.lyyime_new.argtypes = [ctypes.c_char_p]
    lib.lyyime_new.restype = ctypes.c_void_p
    lib.lyyime_free.argtypes = [ctypes.c_void_p]
    lib.lyyime_reset.argtypes = [ctypes.c_void_p]
    lib.lyyime_mode.argtypes = [ctypes.c_void_p]
    lib.lyyime_mode.restype = ctypes.c_int
    lib.lyyime_toggle_mode.argtypes = [ctypes.c_void_p]
    lib.lyyime_toggle_mode.restype = ctypes.c_int
    lib.lyyime_process_key.argtypes = [
        ctypes.c_void_p, ctypes.c_int, ctypes.c_uint32,
        ctypes.c_char_p, ctypes.c_int64,
    ]
    lib.lyyime_process_key.restype = ctypes.c_int64
    lib.lyyime_cand.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_int]
    lib.lyyime_cand.restype = ctypes.c_int
    lib.lyyime_cand_comment.argtypes = lib.lyyime_cand.argtypes
    lib.lyyime_cand_comment.restype = ctypes.c_int
    return lib


def feed(lib, eng, key_id: int, chr_: int = 0) -> list:
    """喂一个键,解析效果流 JSON;容量不足时按 -needed 扩容重试。"""
    cap = 4096
    while True:
        buf = ctypes.create_string_buffer(cap)
        n = lib.lyyime_process_key(eng, key_id, chr_, buf, cap)
        if n >= 0:
            return json.loads(buf.value.decode("utf-8"))
        cap = -n  # -needed 约定


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="lyyime-ffi-smoke-") as home:
        os.environ["HOME"] = home  # 封闭学习词落盘
        lib = load_lib()
        eng = lib.lyyime_new(str(FIXTURES).encode())
        assert eng, "lyyime_new 失败"
        try:
            # 1. 打 nihao → 空格顶屏"你好"
            effects = []
            for ch in "nihao":
                effects = feed(lib, eng, LKEY_CHAR, ord(ch))
            assert {"t": "preedit", "s": "nihao"} in effects, effects
            assert any(e["t"] == "cands" and e["n"] > 0 for e in effects), effects

            effects = feed(lib, eng, LKEY_SPACE)
            assert effects[0] == {"t": "commit", "s": "你好"}, effects

            # 2. 候选与注释:重新打 nihao 后读第 0 个候选
            for ch in "nihao":
                feed(lib, eng, LKEY_CHAR, ord(ch))
            buf = ctypes.create_string_buffer(256)
            n = lib.lyyime_cand(eng, 0, buf, 256)
            assert buf.value.decode() == "你好", buf.value
            n = lib.lyyime_cand_comment(eng, 0, buf, 256)
            assert buf.value.decode() == "ni hao", buf.value

            # 3. -needed 约定:容量不足返回负值
            n = lib.lyyime_process_key(eng, LKEY_ESC, 0, None, 0)
            assert n < 0, f"容量不足应返回 -needed,得到 {n}"

            # 4. 模式切换
            assert lib.lyyime_mode(eng) == 0
            assert lib.lyyime_toggle_mode(eng) == 1
            assert lib.lyyime_mode(eng) == 1
            effects = feed(lib, eng, LKEY_CHAR, ord("n"))
            assert effects == [{"t": "pass"}], "英文态应直通"
            assert lib.lyyime_toggle_mode(eng) == 0
        finally:
            lib.lyyime_free(eng)
    print("FFI ctypes 冒烟通过:效果流 JSON / 候选注释 / -needed / 模式切换 全部符合合同 §3")
    return 0


if __name__ == "__main__":
    sys.exit(main())
