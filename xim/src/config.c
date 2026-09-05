#include "config.h"

#include <ctype.h>
#include <errno.h>
#include <glib/gstdio.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* 受管理的键:顺序即写回顺序;g_suffix 保存行尾注释以便原位保留 */
#define LYY_CFG_KEYS 7
static const char *g_keys[LYY_CFG_KEYS] = {
    "page_size", "mixed_english", "auto_commit_english",
    "chinese_punct", "learning", "font_size", "autostart",
};
static const char *g_key_comments[LYY_CFG_KEYS] = {
    "候选数 1..9",
    "中英混合(无中文命中时给英文词)",
    "高置信英文词遇标点/空格自动直通",
    "中文态空缓冲输出中文标点",
    "用户词学习开关",
    "候选窗字体大小 10..28",
    "开机自启(lyyime-xim)",
};
/* 行尾注释(# 之后)与布尔值写法缓存:load 时记下,save 时复用 */
static char g_suffix[LYY_CFG_KEYS][256];
static const char *g_bool_text[LYY_CFG_KEYS]; /* "true"/"false"(保留用户写法) */

void lyy_config_defaults(LyyConfig *c)
{
    /* 默认值:候选数等与 core Config 的常用取值对齐(docs/ARCHITECTURE.md §4) */
    c->page_size = 5;
    c->mixed_english = 1;
    c->auto_commit_english = 1;
    c->chinese_punct = 1;
    c->learning = 1;
    c->font_size = 14;
    c->autostart = 0;
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

/* 判断一行是否为受管理键;命中返回键序号,*value 指向等号后的值 */
static int key_index_of(const char *line, const char **value_out)
{
    for (int i = 0; i < LYY_CFG_KEYS; i++) {
        size_t klen = strlen(g_keys[i]);
        if (strncmp(line, g_keys[i], klen) != 0)
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

/* 抽取行尾注释(# 起,去行尾空白) */
static void extract_suffix(const char *value, char *out, size_t cap)
{
    const char *hash = strchr(value, '#');
    if (!hash) {
        out[0] = '\0';
        return;
    }
    snprintf(out, cap, "%s", hash);
    char *t = out + strlen(out);
    while (t > out && isspace((unsigned char)t[-1]))
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
    return (idx == 1 || idx == 2 || idx == 3 || idx == 4 || idx == 6);
}

static void apply_value(LyyConfig *c, int idx, const char *v)
{
    switch (idx) {
    case 0: c->page_size = atoi(v); break;
    case 1: c->mixed_english = parse_bool(v, c->mixed_english); break;
    case 2: c->auto_commit_english = parse_bool(v, c->auto_commit_english); break;
    case 3: c->chinese_punct = parse_bool(v, c->chinese_punct); break;
    case 4: c->learning = parse_bool(v, c->learning); break;
    case 5: c->font_size = atoi(v); break;
    case 6: c->autostart = parse_bool(v, c->autostart); break;
    default: break;
    }
}

int lyy_config_load(const char *path, LyyConfig *out)
{
    lyy_config_defaults(out);
    memset(g_suffix, 0, sizeof(g_suffix));
    for (int i = 0; i < LYY_CFG_KEYS; i++)
        g_bool_text[i] = NULL;

    FILE *fp = fopen(path, "r");
    if (!fp)
        return (errno == ENOENT) ? 1 : -1;

    char line[1024];
    while (fgets(line, sizeof(line), fp)) {
        char tmp[1024];
        snprintf(tmp, sizeof(tmp), "%s", line);
        char *p = trim(tmp);
        if (*p == '\0' || *p == '#')
            continue;
        const char *value = NULL;
        int idx = key_index_of(p, &value);
        if (idx < 0)
            continue;
        char vbuf[256];
        snprintf(vbuf, sizeof(vbuf), "%s", value);
        char *v = trim(vbuf);
        /* 记录布尔写法(true/false),写回时保持用户习惯 */
        if (bool_key_index(idx))
            g_bool_text[idx] = (!strcmp(v, "0") || !strcmp(v, "false")) ? "false" : "true";
        /* 记录行尾注释 */
        if (!g_suffix[idx][0])
            extract_suffix(value, g_suffix[idx], sizeof(g_suffix[0]));
        apply_value(out, idx, v);
    }
    fclose(fp);

    /* 边界钳制,避免手改配置把候选窗弄坏 */
    if (out->page_size < 1 || out->page_size > 9)
        out->page_size = 5;
    if (out->font_size < 10 || out->font_size > 28)
        out->font_size = 14;
    return 0;
}

static void write_value(FILE *fp, int idx, const LyyConfig *c)
{
    const char *suffix = g_suffix[idx][0] ? g_suffix[idx] : g_key_comments[idx];
    switch (idx) {
    case 0:
        fprintf(fp, "page_size = %d %s\n", c->page_size, suffix);
        break;
    case 1:
    case 2:
    case 3:
    case 4:
    case 6: {
        int val = (idx == 1) ? c->mixed_english
                : (idx == 2) ? c->auto_commit_english
                : (idx == 3) ? c->chinese_punct
                : (idx == 4) ? c->learning
                             : c->autostart;
        fprintf(fp, "%s = %s %s\n", g_keys[idx],
                g_bool_text[idx] ? g_bool_text[idx]
                                 : (val ? "true" : "false"),
                suffix);
        break;
    }
    case 5:
        fprintf(fp, "font_size = %d %s\n", c->font_size, suffix);
        break;
    default:
        break;
    }
}

int lyy_config_save(const char *path, const LyyConfig *c)
{
    /* 先整读入内存,再截断写回:原位替换受管理键,其余行字节级保留 */
    FILE *in = fopen(path, "r");
    size_t cap = 8192, len = 0;
    char *body = malloc(cap);
    if (!body)
        return -1;
    if (in) {
        char line[1024];
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

    FILE *out = fopen(path, "w");
    if (!out) {
        free(body);
        return -1;
    }

    int seen[LYY_CFG_KEYS] = { 0 };
    char *cursor = body;
    while (*cursor) {
        char *eol = strchr(cursor, '\n');
        char *next = eol ? eol + 1 : cursor + strlen(cursor);
        size_t line_len = (size_t)(next - cursor);

        char tmp[1024];
        size_t copy = line_len < sizeof(tmp) - 1 ? line_len : sizeof(tmp) - 1;
        memcpy(tmp, cursor, copy);
        tmp[copy] = '\0';
        char *p = trim(tmp);

        const char *value = NULL;
        int idx = (*p == '\0' || *p == '#') ? -1 : key_index_of(p, &value);
        if (idx >= 0) {
            seen[idx] = 1;
            write_value(out, idx, c);
        } else {
            fwrite(cursor, 1, line_len, out);
        }
        cursor = next;
    }
    free(body);

    for (int i = 0; i < LYY_CFG_KEYS; i++) {
        if (!seen[i])
            write_value(out, i, c);
    }
    if (fclose(out) != 0)
        return -1;
    return 0;
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
