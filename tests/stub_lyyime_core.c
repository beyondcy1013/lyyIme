/* -*- coding: utf-8 -*-
 * stub_lyyime_core.c —— lyyime-core C ABI 桩库(gcc 编译,单元测试专用)
 *
 * 目的:在 Rust 核心 crates/lyyime-core 尚未就绪时,给 ibus 引擎
 * (ibus-engine/engine/lyyime.py)与 tests/unit_ibus_engine.py 提供一个
 * **确定性规则**的真 .so,逐字实现 docs/ARCHITECTURE.md §3 的全部导出符号,
 * 使引擎侧可以先开发、先测试、先联调。集成阶段由主控把真库放到约定路径
 * (/usr/local/lib/lyyime/liblyyime_core.so),引擎代码零改动。
 *
 * 编译(由 tests/unit_ibus_engine.py 自动执行):
 *   gcc -shared -fPIC -O2 -o tests/_build/liblyyime_core_stub.so tests/stub_lyyime_core.c
 *
 * 确定性行为规则(全部无随机、无外部 IO,可断言):
 *   - 中文态:字母 a–z 进缓冲(≤12),返回 [preedit][cands n=5 page pages=2];
 *     第 i 个候选文本 = 缓冲串 + 中文数字"壹贰叁肆伍陆柒捌玖"[i],注释 = "注"+同字。
 *   - 数字 1–9:有缓冲 → commit(缓冲串+对应中文数字)并清缓冲;无缓冲 → pass。
 *   - Space:有缓冲 → commit("栈")(桩库约定字样);无缓冲 → pass。
 *   - Enter:有缓冲 → commit(原始字母);无缓冲 → pass。
 *   - Backspace:有缓冲 → 删尾并更新 preedit;无缓冲 → pass。
 *   - Esc:有缓冲 → consumed + 清 preedit;无缓冲 → pass。
 *   - PageUp/PageDown:有缓冲 → 翻页(边界钳制到 0..pages-1)+ cands;无缓冲 → pass。
 *   - 标点:中文态空缓冲 → commit(对应中文标点);有缓冲 → commit(首选)+commit(中文标点);
 *     英文态 → pass。
 *   - ShiftPress(宿主完成"单击"检测后才送来)→ 模式翻转 + {"t":"mode"}。
 *   - 英文态:除 ShiftPress 外一律 pass。
 *   - 其它键:有缓冲先 reset(preedit 清空)再 pass。
 *
 * 重试纪律(重要,真核心同样应遵守):process_key 在宿主缓冲不足返回
 * -needed 时**不得改变任何内部状态**,否则宿主扩容重试会把同一个键
 * 应用两次。本桩库统一采用"先产 JSON → 写入成功才落状态"的写法。
 *
 * preedit 形态(与真核心对齐,集成通报 2026-09-05):清空输出
 * {"t":"preedit"}(无 s 字段);有内容输出 {"t":"preedit","s":"nihao"}。
 * 引擎解析两种形态:无 s/空 s 一律视为清除预编辑。
 *
 * 线程模型:与合同一致——线程不安全,宿主保证单线程调用。
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* 桩库内部状态 */
typedef struct {
    char buf[13];   /* 输入缓冲(≤12 字母 + \0) */
    int  mode;      /* 0=中文 1=英文 */
    int  page;      /* 当前候选页 */
} lyy_stub;

#define STUB_PAGES 5       /* 固定每页 5 个候选 */
#define STUB_PAGE_COUNT 2  /* 总页数固定 2,便于测翻页边界 */

static const char *CJK_DIGIT[8] = {
    "壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌"
};

/* 把 effects JSON 写入宿主缓冲;不够时按合同返回 -needed(不写入) */
static int stub_write_out(const char *json, char *buf, int64_t cap)
{
    int needed = (int)strlen(json) + 1; /* 含 \0 */
    if (buf == NULL || (int64_t)needed > cap) {
        return -needed;
    }
    memcpy(buf, json, (size_t)needed);
    return needed;
}

/* 候选文本/注释的公共实现;cap 不足返回 -needed,越界返回 0 */
static int stub_cand_impl(const lyy_stub *eng, int i, char *buf, int cap,
                          int with_comment)
{
    char text[64];
    if (eng->buf[0] == '\0' || i < 0 || i >= STUB_PAGES) {
        return 0;
    }
    if (with_comment) {
        snprintf(text, sizeof(text), "注%s", CJK_DIGIT[i % 8]);
    } else {
        snprintf(text, sizeof(text), "%s%s", eng->buf, CJK_DIGIT[i % 8]);
    }
    int needed = (int)strlen(text) + 1;
    if (buf == NULL || needed > cap) {
        return -needed;
    }
    memcpy(buf, text, (size_t)needed);
    return needed;
}

/* 半角 → 中文标点(桩库用静态表;真核心还做引号配对,桩库从简) */
static const char *stub_cn_punct(uint32_t ch)
{
    switch (ch) {
    case ',': return ",";
    case '.': return "。";
    case '?': return "?";
    case '!': return "!";
    case ';': return ";";
    case ':': return ":";
    case '\'': return "'";
    case '"': return "\"";
    case '(': return "(";
    case ')': return ")";
    case '[': return "【";
    case ']': return "】";
    case '{': return "{";
    case '}': return "}";
    default:  return NULL; /* 未收录:宿主按 pass 处理 */
    }
}

/* ------------------------------ 合同导出符号 ------------------------------ */

void *lyyime_new(const char *data_dir)
{
    lyy_stub *eng = (lyy_stub *)calloc(1, sizeof(lyy_stub));
    (void)data_dir; /* 桩库不读词典,忽略数据目录(允许 NULL) */
    if (eng) {
        eng->mode = 0; /* 启动即中文态 */
        eng->page = 0;
        eng->buf[0] = '\0';
    }
    return (void *)eng;
}

void lyyime_free(void *eng)
{
    free(eng);
}

void lyyime_reset(void *eng)
{
    lyy_stub *e = (lyy_stub *)eng;
    if (!e) return;
    e->buf[0] = '\0';
    e->page = 0;
}

int lyyime_mode(void *eng)
{
    lyy_stub *e = (lyy_stub *)eng;
    return e ? e->mode : 0;
}

int lyyime_toggle_mode(void *eng)
{
    lyy_stub *e = (lyy_stub *)eng;
    if (!e) return 0;
    e->mode = e->mode ? 0 : 1;
    return e->mode;
}

int64_t lyyime_process_key(void *eng, int key_id, uint32_t chr,
                           char *buf, int64_t buf_cap)
{
    lyy_stub *e = (lyy_stub *)eng;
    char json[512];
    char newbuf[13];
    int newpage;
    int newmode;
    int r;
    const char *punct;
    size_t len;

    if (!e) {
        return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
    }

    switch (key_id) {
    case 0: /* LKEY_CHAR:小写字母入缓冲 */
        if (e->mode != 0) { /* 英文态直通 */
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        snprintf(newbuf, sizeof(newbuf), "%s", e->buf);
        len = strlen(newbuf);
        if (len < 12 && chr >= 'a' && chr <= 'z') {
            newbuf[len] = (char)chr;
            newbuf[len + 1] = '\0';
        }
        newpage = 0;
        snprintf(json, sizeof(json),
                 "[{\"t\":\"preedit\",\"s\":\"%s\"},"
                 "{\"t\":\"cands\",\"n\":%d,\"page\":%d,\"pages\":%d}]",
                 newbuf, STUB_PAGES, newpage, STUB_PAGE_COUNT);
        r = stub_write_out(json, buf, buf_cap);
        if (r < 0) return r; /* 缓冲不足:不落状态,宿主扩容后重试 */
        strcpy(e->buf, newbuf);
        e->page = newpage;
        return r;

    case 1: /* LKEY_DIGIT:数字选词(chr 为 '1'..'9' 码点) */
        if (e->mode != 0 || e->buf[0] == '\0' || chr < '1' || chr > '9') {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        snprintf(json, sizeof(json),
                 "[{\"t\":\"commit\",\"s\":\"%s%s\"},"
                 "{\"t\":\"preedit\"}]",
                 e->buf, CJK_DIGIT[(chr - '1') % 8]);
        r = stub_write_out(json, buf, buf_cap);
        if (r < 0) return r;
        e->buf[0] = '\0';
        e->page = 0;
        return r;

    case 2: /* LKEY_SPACE:有缓冲顶屏(commit 桩库约定字样"栈");空缓冲 pass */
        if (e->mode != 0 || e->buf[0] == '\0') {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        r = stub_write_out("[{\"t\":\"commit\",\"s\":\"栈\"},"
                           "{\"t\":\"preedit\"}]", buf, buf_cap);
        if (r < 0) return r;
        e->buf[0] = '\0';
        e->page = 0;
        return r;

    case 3: /* LKEY_ENTER:有缓冲 commit 原始字母(中英混合直通);空 pass */
        if (e->mode != 0 || e->buf[0] == '\0') {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        snprintf(json, sizeof(json),
                 "[{\"t\":\"commit\",\"s\":\"%s\"},{\"t\":\"preedit\"}]",
                 e->buf);
        r = stub_write_out(json, buf, buf_cap);
        if (r < 0) return r;
        e->buf[0] = '\0';
        e->page = 0;
        return r;

    case 4: /* LKEY_BACKSPACE:删尾;空 pass */
        if (e->mode != 0 || e->buf[0] == '\0') {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        snprintf(newbuf, sizeof(newbuf), "%s", e->buf);
        newbuf[strlen(newbuf) - 1] = '\0';
        newpage = 0;
        snprintf(json, sizeof(json), "[{\"t\":\"preedit\",\"s\":\"%s\"}]", newbuf);
        r = stub_write_out(json, buf, buf_cap);
        if (r < 0) return r;
        strcpy(e->buf, newbuf);
        e->page = newpage;
        return r;

    case 5: /* LKEY_ESC:清缓冲(consumed);空 pass */
        if (e->mode != 0 || e->buf[0] == '\0') {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        r = stub_write_out("[{\"t\":\"consumed\"},{\"t\":\"preedit\"}]",
                           buf, buf_cap);
        if (r < 0) return r;
        e->buf[0] = '\0';
        e->page = 0;
        return r;

    case 6: /* LKEY_PAGEUP */
    case 7: /* LKEY_PAGEDOWN:有候选翻页(边界钳制);无缓冲 pass */
        if (e->mode != 0 || e->buf[0] == '\0') {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        if (key_id == 6) {
            newpage = (e->page > 0) ? e->page - 1 : 0;
        } else {
            newpage = (e->page < STUB_PAGE_COUNT - 1) ? e->page + 1
                                                      : STUB_PAGE_COUNT - 1;
        }
        snprintf(json, sizeof(json),
                 "[{\"t\":\"cands\",\"n\":%d,\"page\":%d,\"pages\":%d}]",
                 STUB_PAGES, newpage, STUB_PAGE_COUNT);
        r = stub_write_out(json, buf, buf_cap);
        if (r < 0) return r;
        e->page = newpage;
        return r;

    case 8: /* LKEY_PUNCT:中文标点规则 */
        if (e->mode != 0) {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        punct = stub_cn_punct(chr);
        if (!punct) {
            return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
        }
        if (e->buf[0] == '\0') { /* 空缓冲:直接出中文标点 */
            snprintf(json, sizeof(json), "[{\"t\":\"commit\",\"s\":\"%s\"}]", punct);
            return stub_write_out(json, buf, buf_cap); /* 无状态变化 */
        }
        /* 有缓冲:首选 + 中文标点一起上屏 */
        snprintf(json, sizeof(json),
                 "[{\"t\":\"commit\",\"s\":\"%s%s\"},"
                 "{\"t\":\"commit\",\"s\":\"%s\"},"
                 "{\"t\":\"preedit\"}]",
                 e->buf, CJK_DIGIT[0], punct);
        r = stub_write_out(json, buf, buf_cap);
        if (r < 0) return r;
        e->buf[0] = '\0';
        e->page = 0;
        return r;

    case 9: /* LKEY_SHIFTPRESS:有缓冲上屏英文原串;空缓冲吞键 */
        if (e->buf[0] != '\0') {
            snprintf(json, sizeof(json),
                     "[{\"t\":\"commit\",\"s\":\"%s\"},{\"t\":\"preedit\"}]",
                     e->buf);
            r = stub_write_out(json, buf, buf_cap);
            if (r < 0) return r;
            e->buf[0] = '\0';
            e->page = 0;
            return r;
        }
        return stub_write_out("[{\"t\":\"consumed\"}]", buf, buf_cap);

    default: /* LKEY_OTHER:有缓冲先 reset(preedit 清空)再 pass */
        if (e->buf[0] != '\0') {
            r = stub_write_out("[{\"t\":\"preedit\"},{\"t\":\"pass\"}]",
                               buf, buf_cap);
            if (r < 0) return r;
            e->buf[0] = '\0';
            e->page = 0;
            return r;
        }
        return stub_write_out("[{\"t\":\"pass\"}]", buf, buf_cap);
    }
}

int lyyime_cand(void *eng, int i, char *buf, int cap)
{
    lyy_stub *e = (lyy_stub *)eng;
    if (!e) return 0;
    return stub_cand_impl(e, i, buf, cap, 0);
}

int lyyime_cand_comment(void *eng, int i, char *buf, int cap)
{
    lyy_stub *e = (lyy_stub *)eng;
    if (!e) return 0;
    return stub_cand_impl(e, i, buf, cap, 1);
}
