#!/usr/bin/env python3
"""lyyIme 词典加载：从 ibus-table SQLite 码表构建内存前缀索引。

支持 ibus-table 1.17 schema: phrases(id, tabkeys, phrase, freq, user_freq)。
极点86/海峰86 均可加载。前缀索引一次性建好，查询 O(1)。
"""
import sqlite3
import threading


class WubiDict:
    def __init__(self, db_path: str):
        self.db_path = db_path
        self.exact = {}      # code -> [(freq, phrase), ...] 频率降序
        self.prefix = {}     # 任意前缀 -> [(freq, phrase), ...]
        self.user_freq = {}  # phrase -> 加权(学习)
        self.lock = threading.Lock()
        self._load()

    def _load(self):
        con = sqlite3.connect(f"file:{self.db_path}?mode=ro", uri=True)
        try:
            rows = con.execute("SELECT tabkeys, phrase, freq FROM phrases").fetchall()
        finally:
            con.close()
        exact = {}
        prefix = {}
        for code, phrase, freq in rows:
            code = code.lower()
            exact.setdefault(code, []).append((freq, phrase))
            for i in range(1, len(code) + 1):
                prefix.setdefault(code[:i], []).append((freq, phrase))
        for d in (exact, prefix):
            for v in d.values():
                v.sort(key=lambda t: -t[0])
        self.exact, self.prefix = exact, prefix

    def set_user_freq(self, user_freq: dict):
        with self.lock:
            self.user_freq = dict(user_freq or {})

    def _score(self, freq, phrase, is_exact):
        return freq + self.user_freq.get(phrase, 0) * 10_000_000 + (1 << 62 if is_exact else 0)

    def lookup(self, code: str, limit: int = 45):
        """返回候选列表 [(phrase, score)]，精确匹配优先，其后按频率。"""
        code = code.lower().strip()
        if not code:
            return []
        with self.lock:
            seen, merged = set(), []
            for freq, phrase in self.exact.get(code, []):
                if phrase not in seen:
                    seen.add(phrase)
                    merged.append((self._score(freq, phrase, True), phrase))
            for freq, phrase in self.prefix.get(code, []):
                if phrase not in seen:
                    seen.add(phrase)
                    merged.append((self._score(freq, phrase, False), phrase))
        merged.sort(key=lambda t: -t[0])
        return [(p, f) for f, p in merged[:limit]]

    def learn(self, phrase: str):
        with self.lock:
            self.user_freq[phrase] = self.user_freq.get(phrase, 0) + 1
