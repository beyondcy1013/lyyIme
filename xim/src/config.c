#include "config.h"

#include <ctype.h>
#include <errno.h>
#include <glib/gstdio.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* 受管理的键:顺序即写回顺序;section=NULL 为顶层键,"ai" 为 [ai] 段键 */
#define LYY_CFG_KEYS 15
typedef enum { LYY_VT_INT, LYY_VT_BOOL, LYY_VT_STR } LyyValType;
static const struct {
    const char *section;
    const char *key;
    LyyValType type;
    const char *comment;
} g_keys[LYY_CFG_KEYS] = {
    { NULL, "page_size", LYY_VT_INT, "候选数 1..9" },
    { NULL, "mixed_english", LYY_VT_BOOL, "中英混合(无中文命中时给英文词)" },
    { NULL, "auto_commit_english", LYY_VT_BOOL,
      "高置信英文词遇标点/空格自动直通" },
    { NULL, "chinese_punct", LYY_VT_BOOL, "中文态空缓冲输出中文标点" },
    { NULL, "learning", LYY_VT_BOOL, "用户词学习开关" },
    { NULL, "commit_after_four", LYY_VT_BOOL,
      "满足四码后继续输入字母先上屏当前选中" },
    { NULL, "font_size", LYY_VT_INT, "候选窗字体大小 10..28" },
    { NULL, "autostart", LYY_VT_BOOL, "开机自启(lyyime-xim)" },
    { "ai", "enabled", LYY_VT_BOOL, "AI 助手开关(中文态 /AI+提示词,回车调用)" },
    { "ai", "api_base", LYY_VT_STR,
      "OpenAI 兼容 API 地址(如 https://api.deepseek.com/v1)" },
    { "ai", "api_key", LYY_VT_STR, "API 密钥(本地服务如 Ollama 可留空)" },
    { "ai", "model", LYY_VT_STR, "模型名(如 deepseek-chat)" },
    { "ai", "system_prompt", LYY_VT_STR, "系统提示词(可选;留空用程序内置)" },
    { "ai", "timeout", LYY_VT_INT, "AI 请求超时秒数 5..300" },
    { NULL, "coin_hotkey", LYY_VT_STR,
      "造词快捷键(上屏后按此键造词,方向键增减选字;ctrl/alt/super/shift+键名)" },
};
/* 行尾注释(# 之后)与布尔值写法缓存:load 时记下,save 时复用 */
static char g_suffix[LYY_CFG_KEYS][256];
static int g_bool_numeric[LYY_CFG_KEYS]; /* 用户书写风格:1=数字(1/0),0=单词(true/false) */

void lyy_config_defaults(LyyConfig *c)
{
    /* 默认值:候选数等与 core Config 的常用取值对齐(docs/ARCHITECTURE.md §4);
     * AI 默认关闭且字段为空:未配置前 /AI 完全不介入按键 */
    c->page_size = 5;
    c->mixed_english = 1;
    c->auto_commit_english = 1;
    c->chinese_punct = 1;
    c->learning = 1;
    c->commit_after_four = 0;
    c->font_size = 14;
    c->autostart = 0;
    c->ai_enabled = 0;
    c->ai_api_base[0] = '\0';
    c->ai_api_key[0] = '\0';
    c->ai_model[0] = '\0';
    c->ai_system_prompt[0] = '\0';
    c->ai_timeout = 60;
    snprintf(c->coin_hotkey, sizeof(c->coin_hotkey), "%s", "ctrl+equal");
}

int lyy_config_ai_active(const LyyConfig *c)
{
    return c->ai_enabled && c->ai_api_base[0] && c->ai_model[0];
}

static char *trim(char *s)
{
    while (isspace((unsigned char)*s))
        s++;
    char *end = s + strlen(s);
    while (end > s && isspace((unsigned char)end[-1]))
        *--end = '\0';
    return s;
}

/* 判断一行(已 trim)是否为 [section] 头;命中写 out 返回 1 */
static int section_of(const char *line, char *out, size_t cap)
{
    if (*line != '[')
        return 0;
    const char *end = strchr(line, ']');
    if (!end)
        return 0;
    size_t n = (size_t)(end - line - 1);
    if (n == 0 || n >= cap)
        return 0;
    memcpy(out, line + 1, n);
    out[n] = '\0';
    return 1;
}

/* 判断受管理键:须与当前 section 匹配;命中返回键序号,*value 指向等号后的值 */
static int key_index_of(const char *line, const char *section,
                        const char **value_out)
{
    for (int i = 0; i < LYY_CFG_KEYS; i++) {
        const char *ksec = g_keys[i].section;
        if ((ksec == NULL) != (section[0] == '\0'))
            continue;
        if (ksec != NULL && strcmp(ksec, section) != 0)
            continue;
        size_t klen = strlen(g_keys[i].key);
        if (strncmp(line, g_keys[i].key, klen) != 0)
            continue;
        const char *p = line + klen;
        while (*p == ' ' || *p == '\t')
            p++;
        if (*p != '=')
            continue;
        p++;
        while (*p == ' ' || *p == '\t')
            p++;
        *value_out = p;
        return i;
    }
    return -1;
}

/* 把 = 之后的原始串拆成值与行尾注释(写回时规范化为 ` # 注释`)。
 * 带引号:取首个引号段并反转义(不支持的 \x 序列原样保留);裸值:取首个
 * 空白/# 前的 token,其余并入注释(兼容历史无 # 的行,如 `page_size = 5 候选数 1..9`)。 */
static void split_value(const char *raw, char *val, size_t vcap, char *suffix,
                        size_t scap)
{
    val[0] = '\0';
    suffix[0] = '\0';
    while (*raw == ' ' || *raw == '\t')
        raw++;
    if (*raw == '"') {
        raw++;
        size_t n = 0;
        while (*raw && *raw != '"' && n + 1 < vcap) {
            if (*raw == '\\' && raw[1]) {
                raw++;
                char c = *raw++;
                switch (c) {
                case 'n': val[n++] = '\n'; break;
                case 't': val[n++] = '\t'; break;
                case 'r': val[n++] = '\r'; break;
                case 'b': val[n++] = '\b'; break;
                case 'f': val[n++] = '\f'; break;
                default: val[n++] = c; break; /* \" \\ \/ 及未知转义取本字 */
                }
            } else {
                val[n++] = *raw++;
            }
        }
        val[n] = '\0';
    } else {
        size_t n = 0;
        while (*raw && *raw != ' ' && *raw != '\t' && *raw != '#' &&
               n + 1 < vcap)
            val[n++] = *raw++;
        val[n] = '\0';
    }
    /* 余下部分归行尾注释 */
    const char *rest = raw;
    if (*rest == '"')
        rest++;
    while (*rest == ' ' || *rest == '\t')
        rest++;
    if (*rest == '#') {
        snprintf(suffix, scap, "%s", rest);
    } else if (*rest) {
        snprintf(suffix, scap, "# %s", rest);
    }
    char *t = suffix + strlen(suffix);
    while (t > suffix && isspace((unsigned char)t[-1]))
        *--t = '\0';
}

static int parse_bool(const char *v, int def)
{
    if (strcmp(v, "true") == 0 || strcmp(v, "1") == 0)
        return 1;
    if (strcmp(v, "false") == 0 || strcmp(v, "0") == 0)
        return 0;
    return def;
}

static int bool_key_index(int idx)
{
    return g_keys[idx].type == LYY_VT_BOOL;
}

static void clamp_key(LyyConfig *c, int idx)
{
    switch (idx) {
    case 0:
        if (c->page_size < 1 || c->page_size > 9)
            c->page_size = 5;
        break;
    case 6:
        if (c->font_size < 10 || c->font_size > 28)
            c->font_size = 14;
        break;
    case 13:
        if (c->ai_timeout < 5 || c->ai_timeout > 300)
            c->ai_timeout = 60;
        break;
    case 14:
        if (c->coin_hotkey[0] == '\0')
            snprintf(c->coin_hotkey, sizeof(c->coin_hotkey), "%s",
                     "ctrl+equal");
        break;
    default:
        break;
    }
}

static void apply_value(LyyConfig *c, int idx, const char *v)
{
    switch (idx) {
    case 0: c->page_size = atoi(v); break;
    case 1: c->mixed_english = parse_bool(v, c->mixed_english); break;
    case 2: c->auto_commit_english = parse_bool(v, c->auto_commit_english); break;
    case 3: c->chinese_punct = parse_bool(v, c->chinese_punct); break;
    case 4: c->learning = parse_bool(v, c->learning); break;
    case 5: c->commit_after_four = parse_bool(v, c->commit_after_four); break;
    case 6: c->font_size = atoi(v); break;
    case 7: c->autostart = parse_bool(v, c->autostart); break;
    case 8: c->ai_enabled = parse_bool(v, c->ai_enabled); break;
    case 9: snprintf(c->ai_api_base, sizeof(c->ai_api_base), "%s", v); break;
    case 10: snprintf(c->ai_api_key, sizeof(c->ai_api_key), "%s", v); break;
    case 11: snprintf(c->ai_model, sizeof(c->ai_model), "%s", v); break;
    case 12:
        snprintf(c->ai_system_prompt, sizeof(c->ai_system_prompt), "%s", v);
        break;
    case 13: c->ai_timeout = atoi(v); break;
    case 14: snprintf(c->coin_hotkey, sizeof(c->coin_hotkey), "%s", v); break;
    default: break;
    }
    clamp_key(c, idx);
}

int lyy_config_load(const char *path, LyyConfig *out)
{
    lyy_config_defaults(out);
    memset(g_suffix, 0, sizeof(g_suffix));
    for (int i = 0; i < LYY_CFG_KEYS; i++)
        g_bool_numeric[i] = 0;

    FILE *fp = fopen(path, "r");
    if (!fp)
        return (errno == ENOENT) ? 1 : -1;

    char line[2048];
    char section[64] = "";
    while (fgets(line, sizeof(line), fp)) {
        char tmp[2048];
        snprintf(tmp, sizeof(tmp), "%s", line);
        char *p = trim(tmp);
        if (*p == '\0' || *p == '#')
            continue;
        char sec[64];
        if (section_of(p, sec, sizeof(sec))) {
            snprintf(section, sizeof(section), "%s", sec);
            continue;
        }
        const char *value = NULL;
        int idx = key_index_of(p, section, &value);
        if (idx < 0)
            continue;
        char vbuf[2048], sbuf[256];
        split_value(value, vbuf, sizeof(vbuf), sbuf, sizeof(sbuf));
        /* 记录布尔书写风格(1/0 或 true/false),写回时保持用户习惯 */
        if (bool_key_index(idx))
            g_bool_numeric[idx] =
                (!strcmp(vbuf, "0") || !strcmp(vbuf, "1"));
        /* 记录行尾注释 */
        if (!g_suffix[idx][0] && sbuf[0])
            snprintf(g_suffix[idx], sizeof(g_suffix[0]), "%s", sbuf);
        apply_value(out, idx, vbuf);
    }
    fclose(fp);
    return 0;
}

/* ---- 保存:内存缓冲 + [ai] 段插入位 ---- */
typedef struct {
    char *p;
    size_t len, cap;
} Buf;

static int buf_append_n(Buf *b, const char *s, size_t n)
{
    if (b->len + n + 1 > b->cap) {
        size_t ncap = b->cap ? b->cap : 8192;
        while (b->len + n + 1 > ncap)
            ncap *= 2;
        char *np = realloc(b->p, ncap);
        if (!np)
            return -1;
        b->p = np;
        b->cap = ncap;
    }
    memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = '\0';
    return 0;
}

static int buf_append_str(Buf *b, const char *s)
{
    return buf_append_n(b, s, strlen(s));
}

/* 组 `key = ` 前缀(INT/BOOL 用);返回 0 成功 */
static int buf_key_eq(Buf *b, int idx)
{
    if (buf_append_str(b, g_keys[idx].key) != 0)
        return -1;
    return buf_append_str(b, " = ");
}

/* 组规范化行尾注释:无则用默认注释;统一 ` # xxx` 形态(含前导空格) */
static int buf_suffix(Buf *b, int idx)
{
    char piece[512];
    const char *s = g_suffix[idx][0] ? g_suffix[idx] : g_keys[idx].comment;
    if (s[0] != '#')
        snprintf(piece, sizeof(piece), " # %s", s);
    else
        snprintf(piece, sizeof(piece), " %s", s);
    return buf_append_str(b, piece) || buf_append_str(b, "\n") ? -1 : 0;
}

static int write_value_buf(Buf *b, int idx, const LyyConfig *c)
{
    int val = 0;
    const char *sval = NULL;
    switch (idx) {
    case 0: val = c->page_size; break;
    case 1: val = c->mixed_english; break;
    case 2: val = c->auto_commit_english; break;
    case 3: val = c->chinese_punct; break;
    case 4: val = c->learning; break;
    case 5: val = c->commit_after_four; break;
    case 6: val = c->font_size; break;
    case 7: val = c->autostart; break;
    case 8: val = c->ai_enabled; break;
    case 9: sval = c->ai_api_base; break;
    case 10: sval = c->ai_api_key; break;
    case 11: sval = c->ai_model; break;
    case 12: sval = c->ai_system_prompt; break;
    case 13: val = c->ai_timeout; break;
    case 14: sval = c->coin_hotkey; break;
    default: return 0;
    }
    if (g_keys[idx].type == LYY_VT_STR) {
        if (buf_key_eq(b, idx) != 0)
            return -1;
        if (buf_append_str(b, "\"") != 0)
            return -1;
        for (const char *p = sval ? sval : ""; *p; p++) {
            if (*p == '"' || *p == '\\') {
                char esc[2] = { '\\', *p };
                if (buf_append_n(b, esc, 2) != 0)
                    return -1;
            } else if (buf_append_n(b, p, 1) != 0) {
                return -1;
            }
        }
        if (buf_append_str(b, "\"") != 0)
            return -1;
        return buf_suffix(b, idx);
    }
    if (g_keys[idx].type == LYY_VT_BOOL) {
        if (buf_key_eq(b, idx) != 0)
            return -1;
        if (buf_append_str(b, g_bool_numeric[idx] ? (val ? "1" : "0")
                                                  : (val ? "true" : "false")) != 0)
            return -1;
        return buf_suffix(b, idx);
    }
    char num[32];
    snprintf(num, sizeof(num), "%d", val);
    if (buf_key_eq(b, idx) != 0)
        return -1;
    if (buf_append_str(b, num) != 0)
        return -1;
    return buf_suffix(b, idx);
}

int lyy_config_save(const char *path, const LyyConfig *c)
{
    /* 整读入内存:原位替换受管理键,其余行字节级保留;[ai] 段缺失键按
     * "插入位"(段头/最后一个 ai 键之后)补齐,无段则在尾部新建。 */
    FILE *in = fopen(path, "r");
    size_t cap = 8192, len = 0;
    char *body = malloc(cap);
    if (!body)
        return -1;
    if (in) {
        char line[2048];
        while (fgets(line, sizeof(line), in)) {
            size_t llen = strlen(line);
            if (len + llen + 1 > cap) {
                cap *= 2;
                char *nb = realloc(body, cap);
                if (!nb) {
                    free(body);
                    fclose(in);
                    return -1;
                }
                body = nb;
            }
            memcpy(body + len, line, llen);
            len += llen;
        }
        fclose(in);
    }
    body[len] = '\0';

    Buf out = { 0 }, block = { 0 }, final = { 0 };
    int seen[LYY_CFG_KEYS] = { 0 };
    size_t ai_ins_off = (size_t)-1;  /* -1=文件尾(尚无 [ai] 段时) */
    size_t sec_off = (size_t)-1;     /* 首个 [section] 行偏移:顶层补齐键须插在其前 */
    int ai_hdr_seen = 0;
    int rc = -1;
    char section[64] = "";
    char *cursor = body;
    while (*cursor) {
        char *eol = strchr(cursor, '\n');
        char *next = eol ? eol + 1 : cursor + strlen(cursor);
        size_t line_len = (size_t)(next - cursor);

        char tmp[2048];
        size_t copy = line_len < sizeof(tmp) - 1 ? line_len : sizeof(tmp) - 1;
        memcpy(tmp, cursor, copy);
        tmp[copy] = '\0';
        char *p = trim(tmp);

        int handled = 0;
        int was_ai = !strcmp(section, "ai");
        if (*p && *p != '#') {
            char sec[64];
            if (section_of(p, sec, sizeof(sec))) {
                if (sec_off == (size_t)-1)
                    sec_off = out.len; /* 本行是首个段头,记下插入点 */
                snprintf(section, sizeof(section), "%s", sec);
                if (!strcmp(sec, "ai")) {
                    ai_hdr_seen = 1;
                    ai_ins_off = out.len + line_len; /* 段头原样保留后再插入 */
                }
                was_ai = !strcmp(section, "ai");
            } else {
                const char *value = NULL;
                int idx = key_index_of(p, section, &value);
                if (idx >= 0) {
                    seen[idx] = 1;
                    if (write_value_buf(&out, idx, c) != 0)
                        goto out;
                    if (g_keys[idx].section &&
                        !strcmp(g_keys[idx].section, "ai"))
                        ai_ins_off = out.len;
                    was_ai = 1;
                    handled = 1;
                }
            }
        }
        if (was_ai && ai_ins_off != (size_t)-1)
            ai_ins_off = out.len;
        if (!handled && buf_append_n(&out, cursor, line_len) != 0)
            goto out;
        cursor = next;
    }

    /* 未出现的键补齐:顶层键插到首个段头之前(段内追加会被解析进段,
     * 下次读取失效);[ai] 键插入段内(缺段先建头) */
    int ai_missing = 0;
    for (int i = 0; i < LYY_CFG_KEYS; i++)
        if (!seen[i] && g_keys[i].section && !strcmp(g_keys[i].section, "ai"))
            ai_missing = 1;
    int top_missing = 0;
    for (int i = 0; i < LYY_CFG_KEYS; i++)
        if (!seen[i] && !(g_keys[i].section && g_keys[i].section[0]))
            top_missing = 1;
    if (top_missing) {
        Buf tops = { 0 };
        for (int i = 0; i < LYY_CFG_KEYS; i++) {
            if (seen[i] || (g_keys[i].section && g_keys[i].section[0]))
                continue;
            if (write_value_buf(&tops, i, c) != 0) {
                free(tops.p);
                goto out;
            }
        }
        if (sec_off == (size_t)-1 || sec_off > out.len) {
            /* 无任何段:直接追加 */
            if (buf_append_n(&out, tops.p, tops.len) != 0) {
                free(tops.p);
                goto out;
            }
        } else {
            /* 插到首个段头之前 */
            if (buf_append_n(&final, out.p, sec_off) != 0 ||
                buf_append_n(&final, tops.p, tops.len) != 0 ||
                buf_append_n(&final, out.p + sec_off, out.len - sec_off) != 0) {
                free(tops.p);
                goto out;
            }
            free(out.p);
            out = final;
            final.p = NULL;
            final.len = final.cap = 0;
        }
        size_t tops_len = tops.len;
        free(tops.p);
        /* 插入使后续内容整体后移:[ai] 段插入点随之平移 */
        if (sec_off != (size_t)-1 && ai_ins_off != (size_t)-1)
            ai_ins_off += tops_len;
    }
    if (ai_missing) {
        if (!ai_hdr_seen) {
            if (buf_append_str(&block, "[ai] # AI 助手(中文态 /AI 触发)\n") != 0)
                goto out;
        }
        for (int i = 0; i < LYY_CFG_KEYS; i++) {
            if (seen[i] || !g_keys[i].section ||
                strcmp(g_keys[i].section, "ai"))
                continue;
            if (write_value_buf(&block, i, c) != 0)
                goto out;
        }
        if (ai_ins_off == (size_t)-1 || ai_ins_off > out.len)
            ai_ins_off = out.len;
        if (buf_append_n(&final, out.p, ai_ins_off) != 0 ||
            buf_append_n(&final, block.p, block.len) != 0 ||
            buf_append_n(&final, out.p + ai_ins_off,
                         out.len - ai_ins_off) != 0)
            goto out;
    } else {
        final = out;
        out.p = NULL; /* 所有权移交 final,避免双重释放 */
    }

    FILE *fo = fopen(path, "w");
    if (!fo)
        goto out;
    if (final.len && fwrite(final.p, 1, final.len, fo) != final.len) {
        fclose(fo);
        goto out;
    }
    rc = (fclose(fo) == 0) ? 0 : -1;

out:
    free(body);
    free(out.p);
    free(block.p);
    free(final.p);
    return rc;
}

int lyy_config_apply_autostart(int enable)
{
    const char *cfg = g_get_user_config_dir();
    char dir[1024], file[1280];
    snprintf(dir, sizeof(dir), "%s/autostart", cfg ? cfg : ".");
    snprintf(file, sizeof(file), "%s/lyyime-xim.desktop", dir);
    if (mkdir(dir, 0755) != 0 && errno != EEXIST)
        return -1;

    if (!enable) {
        if (g_unlink(file) != 0 && errno != ENOENT)
            return -1;
        return 0;
    }
    /* Exec 取正在运行的二进制路径(开发态指向 build/bin,安装后为 /usr/local/bin) */
    char exe[1024];
    ssize_t n = readlink("/proc/self/exe", exe, sizeof(exe) - 1);
    if (n <= 0)
        snprintf(exe, sizeof(exe), "/usr/local/bin/lyyime-xim");
    else
        exe[n] = '\0';

    FILE *fp = fopen(file, "w");
    if (!fp)
        return -1;
    fprintf(fp,
            "[Desktop Entry]\n"
            "Type=Application\n"
            "Name=lyyIme 输入法(XIM 外挂)\n"
            "Comment=lyyIme 独立输入法外挂(Mode B)\n"
            "Exec=%s\n"
            "X-GNOME-Autostart-enabled=true\n",
            exe);
    fclose(fp);
    return 0;
}
