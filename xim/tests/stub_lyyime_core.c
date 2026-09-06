/*
 * liblyyime_core 桩库(测试与 E2E 用;gcc -shared -fPIC 直编)
 *
 * 实现 docs/ARCHITECTURE.md §3 C ABI **全符号**,行为确定性(无随机、无环境
 * 依赖),供 xim_e2e.sh 断言。真实词库就绪前临时顶替,集成期由主控以
 * LYYIME_CORE_LIB=<真库> 替换,宿主无感知。
 *
 * 确定性规则(与 tests/e2e/xim_e2e.sh 的断言一一对应):
 *   - 缓冲:连续小写字母 ≤12,Backspace 删尾;Esc/焦点重置清空;
 *   - 候选页:任何非空缓冲都有 5 个候选;
 *       · 缓冲 "nihao" → ["你好","你号","拟好","泥嚎","倪豪"]
 *         注释 = "拼音 nihao"~"拼音 nihao⑤";
 *       · 其它缓冲 k 号候选 = "候选<k+1>"(中文,保证断言可辨 UTF-8 提交通路),
 *         注释 = "<buf>·stub";
 *   - Digit d:有候选 → commit 第 d 个;无候选(空缓冲)→ pass;
 *   - Space:有候选 → commit 首选(顶屏);空缓冲 → pass;
 *   - Enter:有缓冲 → commit 原字母;空 → pass;
 *   - Backspace:删尾 → preedit;空 → pass;
 *   - Esc:清缓冲 → consumed+preedit 清除;空 → pass;
 *   - PageUp/PageDown:翻页 → cands(page 变化);无候选 → pass;
 *   - Punct:中文态空缓冲 → commit 对应中文标点(,→, .→。 ?→? !→! ;→; :→: '→' "→" 等);
 *            有缓冲 → commit 首选 + commit 中文标点;英文态 → pass;
 *   - ShiftPress:切换模式 → mode 效果(m=新模式);
 *   - Other → pass。
 * - JSON 输出严格按 §3 契约:preedit 清空 = {"t":"preedit"}(无 "s" 字段)。
 */
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>

/* ---------- 引擎状态(单实例;宿主保证单线程) ---------- */
typedef struct {
    char buf[16]; /* 小写字母缓冲 */
    int len;
    int mode;     /* 0=中文 1=英文 */
    int page;     /* 当前页(0 基) */
    int pages;
    int coin;     /* 造词模式(合同 §12):0=未进入;2..4=选长(演示串"你好好吗") */
    int commits;  /* 已发生上屏次数(造词历史非空判定) */
} StubEng;

/* 造词演示串(每字 3 字节 UTF-8):选长 n 的选区 = 前 n 字 */
static const char *STUB_COIN_DEMO = "\xE4\xBD\xA0\xE5\xA5\xBD\xE5\xA5\xBD\xE5\x90\x97";

static int coin_bytes(int n)
{
    return n * 3;
}

static const char *g_nihao_cands[5] = { "你好", "你号", "拟好", "泥嚎", "倪豪" };

/* 中文标点映射(§6 默认集的桩版;UTF-8 用转义写死,避免源码字形歧义) */
static const char *punct_map(char c)
{
    switch (c) {
    case ',': return "\xEF\xBC\x8C";       /* ,  U+FF0C */
    case '.': return "\xE3\x80\x82";       /* 。 U+3002 */
    case '?': return "\xEF\xBC\x9F";       /* ?  U+FF1F */
    case '!': return "\xEF\xBC\x81";       /* !  U+FF01 */
    case ';': return "\xEF\xBC\x9B";       /* ;  U+FF1B */
    case ':': return "\xEF\xBC\x9A";       /* :  U+FF1A */
    case '\'': return "\xE2\x80\x98";      /* '  U+2018 */
    case '"': return "\xE2\x80\x9C";  /* " U+201C */
    case '(': return "\xEF\xBC\x88";       /* (  U+FF08 */
    case ')': return "\xEF\xBC\x89";       /* )  U+FF09 */
    case '[': return "\xE3\x80\x90";       /* [  U+3010 */
    case ']': return "\xE3\x80\x91";       /* ]  U+3011 */
    case '{': return "\xEF\xBD\x9B";       /* {  U+FF5B */
    case '}': return "\xEF\xBD\x9D";       /* }  U+FF5D */
    default: return NULL;
    }
}

static int has_cands(const StubEng *e)
{
    return e->len > 0;
}

/* 生成第 page 页候选文本(0 基页) */
static void cand_text(const StubEng *e, int i, char *out, int cap)
{
    if (strcmp(e->buf, "nihao") == 0)
        snprintf(out, (size_t)cap, "%s", g_nihao_cands[i]);
    else
        snprintf(out, (size_t)cap, "候选%d", i + 1);
}

static void cand_comment_of(const StubEng *e, int i, char *out, int cap)
{
    if (strcmp(e->buf, "nihao") == 0)
        snprintf(out, (size_t)cap, "拼音 nihao %d/5", i + 1);
    else
        snprintf(out, (size_t)cap, "%s·stub %d/5", e->buf, i + 1);
}

/* ---------- §3 FFI ---------- */

void *lyyime_new(const char *data_dir)
{
    /* 桩:不读词典;data_dir 仅记录(避免未用告警) */
    (void)data_dir;
    StubEng *e = calloc(1, sizeof(StubEng));
    if (!e)
        return NULL;
    e->mode = 0;
    return e;
}

void lyyime_free(void *eng)
{
    free(eng);
}

void lyyime_reset(void *eng)
{
    StubEng *e = eng;
    e->len = 0;
    e->buf[0] = '\0';
    e->page = 0;
    e->pages = 0;
    e->coin = 0; /* 焦点切换/清缓冲均取消造词模式 */
}

int lyyime_mode(void *eng)
{
    return ((StubEng *)eng)->mode;
}

int lyyime_toggle_mode(void *eng)
{
    StubEng *e = eng;
    e->mode = !e->mode;
    return e->mode;
}

int lyyime_set_commit_after_four(void *eng, int enabled)
{
    (void)eng;
    return enabled ? 1 : 0;
}

int lyyime_set_commit_unique_four(void *eng, int enabled)
{
    (void)eng;
    return enabled ? 1 : 0;
}

int lyyime_set_phrase_hint(void *eng, int enabled)
{
    (void)eng;
    return enabled ? 1 : 0;
}

/* 效果 JSON 追加小工具:返回写入后总长(不含 \0),截断返回 -1 */
typedef struct {
    char *buf;
    int cap;
    int len;
    int truncated;
} JsonBuf;

static void jb_init(JsonBuf *j, char *buf, int cap)
{
    j->buf = buf;
    j->cap = cap;
    j->len = 0;
    j->truncated = 0;
    if (cap > 0)
        buf[0] = '\0';
}

static void jb_append(JsonBuf *j, const char *s)
{
    size_t l = strlen(s);
    if (j->len + l + 1 > (size_t)j->cap) {
        j->truncated = 1;
        return;
    }
    memcpy(j->buf + j->len, s, l);
    j->len += (int)l;
    j->buf[j->len] = '\0';
}

static void jb_printf(JsonBuf *j, const char *fmt, ...)
    __attribute__((format(printf, 2, 3)));
static void jb_printf(JsonBuf *j, const char *fmt, ...)
{
    char tmp[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(tmp, sizeof(tmp), fmt, ap);
    va_end(ap);
    jb_append(j, tmp);
}

static void cands_page_of(const StubEng *e, int *page, int *pages)
{
    *page = e->page;
    *pages = e->len > 0 ? 1 : 0; /* 桩:单页 5 候选 */
}

int64_t lyyime_process_key(void *eng, int key_id, uint32_t chr, char *buf,
                           int64_t buf_cap)
{
    StubEng *e = eng;
    JsonBuf j;
    jb_init(&j, buf, (int)buf_cap);

    /* 先在栈上组包,再按 §3 协议决定返回值(不足时返回 -needed) */
    char tmp[2048];
    JsonBuf t;
    jb_init(&t, tmp, (int)sizeof(tmp));
    jb_append(&t, "[");

    /* ---- 造词模式(合同 §12):桩库演示语义,优先于普通路径 ----
     * 选区固定取演示串前 n 字(2..4);效果形态与真核心一致。 */
    if (e->coin > 0) {
        int grow = (key_id == 13 || key_id == 14); /* →/↑ 多选一字 */
        int shrink = (key_id == 12 || key_id == 15 || key_id == 4);
        if (key_id == 11) { /* 热键重按:重置选长 */
            e->coin = 2;
        } else if (grow) {
            if (e->coin < 4)
                e->coin++;
        } else if (shrink) {
            if (e->coin > 2)
                e->coin--;
        } else if (key_id == 3 || key_id == 2) { /* Enter/Space 存词 */
            e->coin = 0;
            jb_printf(&t, "{\"t\":\"notice\",\"s\":\"已造词:你好(wqvb),可直接用该编码打出\"},");
            jb_append(&t, "{\"t\":\"preedit\"}");
            goto finish;
        } else if (key_id == 5) { /* Esc 取消 */
            e->coin = 0;
            jb_append(&t, "{\"t\":\"consumed\"},{\"t\":\"preedit\"}");
            goto finish;
        } else { /* 其余键:退出造词,继续普通路径 */
            e->coin = 0;
        }
        if (e->coin > 0) {
            jb_printf(&t, "{\"t\":\"preedit\",\"s\":\"造词:%.*s\"},",
                      coin_bytes(e->coin), STUB_COIN_DEMO);
            jb_printf(&t,
                      "{\"t\":\"cands\",\"n\":1,\"page\":0,\"pages\":1},");
            goto finish;
        }
    }

    switch (key_id) {
    case 11: { /* COIN:进入造词(需有上屏历史;英文态直通) */
        if (e->mode != 0) {
            jb_append(&t, "{\"t\":\"pass\"}");
        } else if (e->commits == 0) {
            jb_printf(&t, "{\"t\":\"notice\",\"s\":\"造词:还没有可造词的上屏汉字,请先输入中文\"},");
            jb_append(&t, "{\"t\":\"consumed\"}");
        } else {
            e->coin = 2;
            jb_printf(&t, "{\"t\":\"preedit\",\"s\":\"造词:%.*s\"},",
                      coin_bytes(e->coin), STUB_COIN_DEMO);
            jb_printf(&t,
                      "{\"t\":\"cands\",\"n\":1,\"page\":0,\"pages\":1},");
        }
        break;
    }
    case 0: { /* CHAR */
        if (e->len < 12 && chr >= 'a' && chr <= 'z') {
            e->buf[e->len++] = (char)chr;
            e->buf[e->len] = '\0';
            e->page = 0;
        }
        jb_printf(&t, "{\"t\":\"preedit\",\"s\":\"%s\"},", e->buf);
        if (has_cands(e)) {
            int page = 0, pages = 0;
            cands_page_of(e, &page, &pages);
            jb_printf(&t, "{\"t\":\"cands\",\"n\":5,\"page\":%d,\"pages\":%d},",
                      page, pages);
        }
        break;
    }
    case 1: { /* DIGIT */
        int d = (int)chr - '0'; /* 1..9 */
        if (has_cands(e) && d >= 1 && d <= 5) {
            char word[64];
            cand_text(e, d - 1, word, (int)sizeof(word));
            jb_printf(&t, "{\"t\":\"commit\",\"s\":\"%s\"},", word);
            jb_append(&t, "{\"t\":\"preedit\"}");
            e->len = 0;
            e->buf[0] = '\0';
            e->page = 0;
            e->commits++;
        } else {
            jb_append(&t, "{\"t\":\"pass\"}");
        }
        break;
    }
    case 2: { /* SPACE */
        if (has_cands(e)) {
            char word[64];
            cand_text(e, 0, word, (int)sizeof(word));
            jb_printf(&t, "{\"t\":\"commit\",\"s\":\"%s\"},", word);
            jb_append(&t, "{\"t\":\"preedit\"}");
            e->len = 0;
            e->buf[0] = '\0';
            e->page = 0;
            e->commits++;
        } else {
            jb_append(&t, "{\"t\":\"pass\"}");
        }
        break;
    }
    case 3: { /* ENTER */
        if (e->len > 0) {
            jb_printf(&t, "{\"t\":\"commit\",\"s\":\"%s\"},", e->buf);
            jb_append(&t, "{\"t\":\"preedit\"}");
            e->len = 0;
            e->buf[0] = '\0';
            e->page = 0;
            e->commits++;
        } else {
            jb_append(&t, "{\"t\":\"pass\"}");
        }
        break;
    }
    case 4: { /* BACKSPACE */
        if (e->len > 0) {
            e->buf[--e->len] = '\0';
            e->page = 0;
            if (e->len > 0) {
                jb_printf(&t, "{\"t\":\"preedit\",\"s\":\"%s\"},", e->buf);
                int page = 0, pages = 0;
                cands_page_of(e, &page, &pages);
                jb_printf(&t,
                          "{\"t\":\"cands\",\"n\":5,\"page\":%d,\"pages\":%d},",
                          page, pages);
            } else {
                jb_append(&t, "{\"t\":\"preedit\"},");
            }
        } else {
            jb_append(&t, "{\"t\":\"pass\"}");
        }
        break;
    }
    case 5: { /* ESC */
        if (e->len > 0) {
            lyyime_reset(e);
            jb_append(&t, "{\"t\":\"consumed\"},{\"t\":\"preedit\"}");
        } else {
            jb_append(&t, "{\"t\":\"pass\"}");
        }
        break;
    }
    case 6:   /* PAGEUP */
    case 7: { /* PAGEDOWN */
        if (has_cands(e)) {
            jb_append(&t, "{\"t\":\"consumed\"}"); /* 桩单页:翻页无效果变化 */
        } else {
            jb_append(&t, "{\"t\":\"pass\"}");
        }
        break;
    }
    case 8: { /* PUNCT */
        const char *cn = punct_map((char)chr);
        if (e->mode == 1 || !cn) {
            jb_append(&t, "{\"t\":\"pass\"}");
        } else if (e->len > 0) {
            char word[64];
            cand_text(e, 0, word, (int)sizeof(word));
            jb_printf(&t, "{\"t\":\"commit\",\"s\":\"%s\"},", word);
            jb_printf(&t, "{\"t\":\"commit\",\"s\":\"%s\"},", cn);
            jb_append(&t, "{\"t\":\"preedit\"}");
            e->len = 0;
            e->buf[0] = '\0';
        } else {
            jb_printf(&t, "{\"t\":\"commit\",\"s\":\"%s\"}", cn);
        }
        break;
    }
    case 9: { /* SHIFTPRESS */
        if (e->len > 0) {
            jb_printf(&t, "{\"t\":\"commit\",\"s\":\"%s\"},", e->buf);
            jb_append(&t, "{\"t\":\"preedit\"}");
            e->len = 0;
            e->buf[0] = '\0';
            e->page = 0;
        } else {
            jb_append(&t, "{\"t\":\"consumed\"}");
        }
        break;
    }
    case 10: /* OTHER */
    default:
        jb_append(&t, "{\"t\":\"pass\"}");
        break;
    }

finish:
    /* 去掉末尾悬挂逗号(各分支按需追加逗号,统一在收口前清理) */
    if (t.len > 0 && tmp[t.len - 1] == ',') {
        tmp[t.len - 1] = '\0';
        t.len--;
    }
    jb_append(&t, "]");
    if (t.truncated)
        return -1;

    /* §3 协议:返回所需字节数(含\0);不足时返回 -needed 且不写入 */
    int64_t need = (int64_t)strlen(tmp) + 1;
    if (buf_cap >= need) {
        memcpy(buf, tmp, (size_t)need);
        return need;
    }
    return -need;
}

int lyyime_cand(void *eng, int i, char *buf, int cap)
{
    StubEng *e = eng;
    if (i < 0)
        return -1;
    if (e->coin > 0) { /* 造词:单候选 = 选区文本 */
        if (i >= 1)
            return -1;
        int need = coin_bytes(e->coin) + 1;
        if (cap < need)
            return -need;
        snprintf(buf, (size_t)cap, "%.*s", coin_bytes(e->coin),
                 STUB_COIN_DEMO);
        return need;
    }
    if (i >= 5 || e->len == 0)
        return -1;
    char word[64];
    cand_text(e, i, word, (int)sizeof(word));
    int64_t need = (int64_t)strlen(word) + 1;
    if (cap < need)
        return -(int)need;
    memcpy(buf, word, (size_t)need);
    return (int)need;
}

int lyyime_cand_comment(void *eng, int i, char *buf, int cap)
{
    StubEng *e = eng;
    if (i < 0)
        return -1;
    if (e->coin > 0) { /* 造词:注释 = 演示编码 */
        if (i >= 1)
            return -1;
        const char *code = "wqvb";
        int need = (int)strlen(code) + 1;
        if (cap < need)
            return -need;
        memcpy(buf, code, (size_t)need);
        return need;
    }
    if (i >= 5 || e->len == 0)
        return -1;
    char c[128];
    cand_comment_of(e, i, c, (int)sizeof(c));
    int64_t need = (int64_t)strlen(c) + 1;
    if (cap < need)
        return -(int)need;
    memcpy(buf, c, (size_t)need);
    return (int)need;
}

/* ---------- §14 快速功能键(桩:注入/执行合同;行为确定性) ---------- */

/* 桩配置表:注入即替换;action_command 原样返回;select_candidate 对
 * 缓冲 "qa" 生成单条功能候选(index 0)供点选路径断言 */
#define STUB_QA_MAX 8
typedef struct {
    char trigger[64];
    char label[128];
    char command[512];
} StubQa;
typedef struct {
    int enabled;
    int count;
    StubQa items[STUB_QA_MAX];
} StubQaStore;

static StubQaStore g_qa = { .enabled = 1, .count = 0 };

int lyyime_set_quick_actions_enabled(void *eng, int enabled)
{
    (void)eng;
    g_qa.enabled = enabled ? 1 : 0;
    return g_qa.enabled;
}

void lyyime_clear_quick_actions(void *eng)
{
    (void)eng;
    g_qa.count = 0;
}

int lyyime_add_quick_action(void *eng, const char *trigger, const char *label,
                            const char *command)
{
    (void)eng;
    if (!trigger || !label || !command || g_qa.count >= STUB_QA_MAX)
        return -1;
    StubQa *a = &g_qa.items[g_qa.count++];
    snprintf(a->trigger, sizeof(a->trigger), "%s", trigger);
    snprintf(a->label, sizeof(a->label), "%s", label);
    snprintf(a->command, sizeof(a->command), "%s", command);
    return 0;
}

int lyyime_action_command(void *eng, int i, char *buf, int cap)
{
    (void)eng;
    if (i < 0 || i >= g_qa.count)
        return cap > 0 ? 1 : -1; /* 越界:空串(含 \0 需 1 字节) */
    int need = (int)strlen(g_qa.items[i].command) + 1;
    if (cap < need)
        return -need;
    memcpy(buf, g_qa.items[i].command, (size_t)need);
    return need;
}

int64_t lyyime_select_candidate(void *eng, int idx, char *buf, int64_t buf_cap)
{
    StubEng *e = eng;
    if (!e || e->len == 0)
        return -1; /* 无候选:consumed 由调用方语义处理(返回异常) */
    /* 桩行为:缓冲恰为注入过的触发词 → action 效果;否则按普通候选上屏 */
    for (int i = 0; i < g_qa.count; i++) {
        if (strcmp(e->buf, g_qa.items[i].trigger) == 0) {
            if (idx != 0)
                return -1;
            const char *tpl = "[{\"t\":\"action\",\"i\":%d},{\"t\":\"preedit\"},{\"t\":\"cands\",\"n\":0,\"page\":0,\"pages\":0}]";
            char tmp[256];
            int need = snprintf(tmp, sizeof(tmp), tpl, i) + 1;
            if (buf_cap >= need) {
                memcpy(buf, tmp, (size_t)need);
                e->len = 0;
                e->buf[0] = '\0';
                return need;
            }
            return -need;
        }
    }
    /* 普通候选点选:commit 第 idx 个(越界 consumed) */
    char word[64];
    if (idx < 0 || idx >= 5)
        return -1;
    cand_text(e, idx, word, (int)sizeof(word));
    const char *tpl = "[{\"t\":\"commit\",\"s\":\"%s\"},{\"t\":\"preedit\"},{\"t\":\"cands\",\"n\":0,\"page\":0,\"pages\":0}]";
    char tmp[256];
    int need = snprintf(tmp, sizeof(tmp), tpl, word) + 1;
    if (buf_cap >= need) {
        memcpy(buf, tmp, (size_t)need);
        e->len = 0;
        e->buf[0] = '\0';
        return need;
    }
    return -need;
}
