#include "config.h"

#include <ctype.h>
#include <errno.h>
#include <glib/gstdio.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "keysym_map.h"

/* 受管理的键:顺序即写回顺序;section=NULL 为顶层键,"ai" 为 [ai] 段键。
 * 追加新键放表尾(索引 clamp_key/apply_value/write_value_buf 三处同步)。 */
#define LYY_CFG_KEYS 28
typedef enum { LYY_VT_INT, LYY_VT_BOOL, LYY_VT_STR } LyyValType;
static const struct {
    const char *section;
    const char *key;
    LyyValType type;
    const char *comment;
} g_keys[LYY_CFG_KEYS] = {
    { NULL, "page_size", LYY_VT_INT, "候选数 1..10(数字键 1-9/0,0=第 10 个)" },
    { NULL, "mixed_english", LYY_VT_BOOL, "中英混合(无中文命中时给英文词)" },
    { NULL, "auto_commit_english", LYY_VT_BOOL,
      "高置信英文词遇标点/空格自动直通" },
    { NULL, "chinese_punct", LYY_VT_BOOL, "中文态空缓冲输出中文标点" },
    { NULL, "learning", LYY_VT_BOOL, "用户词学习开关" },
    { NULL, "commit_after_four", LYY_VT_BOOL,
      "四码顶屏(满四码后继续输入字母先上屏五笔首选)" },
    { NULL, "commit_unique_four", LYY_VT_BOOL,
      "恰好四码且候选唯一时免空格直接上屏" },
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
    { NULL, "phrase_hint", LYY_VT_BOOL,
      "词组效率提示(上屏后最近几字有更省键的词组时候选条提示词组与编码)" },
    { NULL, "shot_hotkey", LYY_VT_STR,
      "截屏快捷键(按下拉起框选截屏,存图片目录并复制剪贴板;ctrl/alt/super/shift+键名)" },
    { NULL, "quick_actions_enabled", LYY_VT_BOOL,
      "快速功能键总开关(输入触发词如 peizhi 时候选条提示,数字/点击执行)" },
    { NULL, "stats_enabled", LYY_VT_BOOL,
      "输入停顿显示今日统计(全局,悬浮窗状态行;各输入模式共用同一份数据)" },
    { NULL, "stats_pause_secs", LYY_VT_INT,
      "停顿多少秒后显示今日统计 3..300" },
    { NULL, "stats_idle_exclude_secs", LYY_VT_INT,
      "计入速度的最长停顿秒数,超时的空隙不计时长 5..600" },
    { NULL, "enter_english", LYY_VT_STR,
      "回车上屏英文原串后的去向:temp=临时(默认,保持中文);en=切英文模式" },
    { NULL, "shift_english", LYY_VT_STR,
      "Shift 上屏英文原串后的去向:en=切英文模式(默认);temp=临时(保持中文)" },
    { NULL, "exact_char_freq_rank", LYY_VT_BOOL,
      "精确单字按词频排位(低频字让位高频词组;关闭则恒居首位)" },
    { NULL, "commit_first_at_four", LYY_VT_BOOL,
      "四码首选上屏(满四码且首选是五笔命中、同码无重码时直接上屏首选;有重码待选/顶屏)" },
    { NULL, "custom_query_label", LYY_VT_STR,
      "候选右键·自定义查询菜单名(默认:自定义查询)" },
    { NULL, "custom_query_url", LYY_VT_STR,
      "候选右键·自定义查询网址模板,{q} 为查询词占位符(空=菜单不显示此项)" },
};

/* 内置默认功能键表(合同 §14;与 core Config::default 一致) */
static const LyyQuickAction g_qa_defaults[] = {
    { "peizhi", "打开配置", "@settings" },
    { "shezhi", "设置", "@settings" },
    { "jietu", "截图", "@shot" },
    { "bangzhu", "帮助", "@help" },
};
/* 行尾注释(# 之后)与布尔值写法缓存:load 时记下,save 时复用 */
static char g_suffix[LYY_CFG_KEYS][256];
static int g_bool_numeric[LYY_CFG_KEYS]; /* 用户书写风格:1=数字(1/0),0=单词(true/false) */

void lyy_config_defaults(LyyConfig *c)
{
    /* 默认值:候选数等与 core Config 的常用取值对齐(docs/ARCHITECTURE.md §4);
     * AI 默认关闭且字段为空:未配置前 /AI 完全不介入按键 */
    c->page_size = 10;
    c->mixed_english = 1;
    c->auto_commit_english = 1;
    c->chinese_punct = 1;
    c->learning = 1;
    c->commit_after_four = 1;
    c->commit_first_at_four = 1;
    c->commit_unique_four = 1;
    c->phrase_hint = 1;
    c->exact_char_freq_rank = 1;
    c->font_size = 14;
    c->autostart = 0;
    c->ai_enabled = 0;
    c->ai_api_base[0] = '\0';
    c->ai_api_key[0] = '\0';
    c->ai_model[0] = '\0';
    c->ai_system_prompt[0] = '\0';
    c->ai_timeout = 60;
    snprintf(c->coin_hotkey, sizeof(c->coin_hotkey), "%s", "ctrl+equal");
    snprintf(c->shot_hotkey, sizeof(c->shot_hotkey), "%s", "ctrl+alt+a");
    /* 快速功能键(合同 §14):默认开 + 内置表(peizhi/bangzhu) */
    c->quick_actions_enabled = 1;
    memset(c->quick_actions, 0, sizeof(c->quick_actions));
    c->quick_actions_count = (int)(sizeof(g_qa_defaults) / sizeof(g_qa_defaults[0]));
    for (int i = 0; i < c->quick_actions_count; i++)
        c->quick_actions[i] = g_qa_defaults[i];
    /* 输入统计(设置窗统一管理;与悬浮窗默认一致) */
    c->stats_enabled = 1;
    c->stats_pause_secs = 10;
    c->stats_idle_exclude_secs = 30;
    /* 英文上屏去向(§6):回车默认临时(单个英文词),Shift 默认转英文 */
    snprintf(c->enter_english, sizeof(c->enter_english), "%s", "temp");
    snprintf(c->shift_english, sizeof(c->shift_english), "%s", "en");
    /* §15 自定义查询:默认空(url 空 = 候选右键菜单不显示此项) */
    c->custom_query_label[0] = '\0';
    c->custom_query_url[0] = '\0';
}

int lyy_config_ai_active(const LyyConfig *c)
{
    return c->ai_enabled && c->ai_api_base[0] && c->ai_model[0];
}

const char *lyy_en_mode_canon(const char *v, const char *def)
{
    if (!strcmp(v, "temp") || !strcmp(v, "temporary"))
        return "temp";
    if (!strcmp(v, "en") || !strcmp(v, "english") || !strcmp(v, "persist"))
        return "en";
    return def;
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

/* [[quick_actions]] 块内字段识别:命中返回 0=trigger 1=label 2=command,
 * *value_out 指向等号后的值;未命中返回 -1(借 segment/boundary 判断) */
static int qa_field_of(const char *line, const char **value_out)
{
    static const char *names[3] = { "trigger", "label", "command" };
    for (int f = 0; f < 3; f++) {
        size_t klen = strlen(names[f]);
        if (strncmp(line, names[f], klen) != 0)
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
        return f;
    }
    return -1;
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
        if (c->page_size < 1 || c->page_size > 10)
            c->page_size = 10;
        break;
    case 7:
        if (c->font_size < 10 || c->font_size > 28)
            c->font_size = 14;
        break;
    case 14:
        if (c->ai_timeout < 5 || c->ai_timeout > 300)
            c->ai_timeout = 60;
        break;
    case 15:
        if (c->coin_hotkey[0] == '\0')
            snprintf(c->coin_hotkey, sizeof(c->coin_hotkey), "%s",
                     "ctrl+equal");
        break;
    case 17:
        if (c->shot_hotkey[0] == '\0')
            snprintf(c->shot_hotkey, sizeof(c->shot_hotkey), "%s",
                     "ctrl+alt+a");
        break;
    case 18:
        /* 布尔无钳制;总开关缺省由 defaults 给 1 */
        break;
    case 19:
        /* 布尔无钳制;总开关缺省由 defaults 给 1 */
        break;
    case 20:
        if (c->stats_pause_secs < 3 || c->stats_pause_secs > 300)
            c->stats_pause_secs = 10;
        break;
    case 21:
        if (c->stats_idle_exclude_secs < 5 || c->stats_idle_exclude_secs > 600)
            c->stats_idle_exclude_secs = 30;
        break;
    case 22:
    case 23:
        /* 英文上屏去向:取值归一在 apply_value 完成,无额外钳制 */
        break;
    case 24:
        /* 布尔无钳制;缺省由 defaults 给 1 */
        break;
    default:
        break;
    }
}

/* 受限拷贝:语义同 snprintf(dst,cap,"%s",src),显式精度告知编译器截断边界 */
static void copy_bounded(char *dst, size_t cap, const char *src)
{
    snprintf(dst, cap, "%.*s", (int)cap - 1, src);
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
    case 6: c->commit_unique_four = parse_bool(v, c->commit_unique_four); break;
    case 7: c->font_size = atoi(v); break;
    case 8: c->autostart = parse_bool(v, c->autostart); break;
    case 9: c->ai_enabled = parse_bool(v, c->ai_enabled); break;
    case 10: copy_bounded(c->ai_api_base, sizeof(c->ai_api_base), v); break;
    case 11: copy_bounded(c->ai_api_key, sizeof(c->ai_api_key), v); break;
    case 12: copy_bounded(c->ai_model, sizeof(c->ai_model), v); break;
    case 13:
        copy_bounded(c->ai_system_prompt, sizeof(c->ai_system_prompt), v);
        break;
    case 14: c->ai_timeout = atoi(v); break;
    case 15: copy_bounded(c->coin_hotkey, sizeof(c->coin_hotkey), v); break;
    case 16: c->phrase_hint = parse_bool(v, c->phrase_hint); break;
    case 17: copy_bounded(c->shot_hotkey, sizeof(c->shot_hotkey), v); break;
    case 18:
        c->quick_actions_enabled = parse_bool(v, c->quick_actions_enabled);
        break;
    case 19: c->stats_enabled = parse_bool(v, c->stats_enabled); break;
    case 20: c->stats_pause_secs = atoi(v); break;
    case 21: c->stats_idle_exclude_secs = atoi(v); break;
    case 22:
        copy_bounded(c->enter_english, sizeof(c->enter_english),
                     lyy_en_mode_canon(v, "temp"));
        break;
    case 23:
        copy_bounded(c->shift_english, sizeof(c->shift_english),
                     lyy_en_mode_canon(v, "en"));
        break;
    case 24: c->exact_char_freq_rank = parse_bool(v, c->exact_char_freq_rank); break;
    case 25:
        c->commit_first_at_four = parse_bool(v, c->commit_first_at_four);
        break;
    case 26:
        copy_bounded(c->custom_query_label, sizeof(c->custom_query_label), v);
        break;
    case 27:
        copy_bounded(c->custom_query_url, sizeof(c->custom_query_url), v);
        break;
    default: break;
    }
    clamp_key(c, idx);
}

/* [[quick_actions]] 表头识别:取 "[[" 与 "]]" 之间的表名(§14) */
static int qa_header_of(const char *line, char *out, size_t cap)
{
    if (strncmp(line, "[[", 2) != 0)
        return 0;
    const char *end = strstr(line, "]]");
    if (!end)
        return 0;
    size_t n = (size_t)(end - line - 2);
    if (n == 0 || n >= cap)
        return 0;
    memcpy(out, line + 2, n);
    out[n] = '\0';
    /* 容忍表名两侧空格:[[ quick_actions ]] */
    char *t = trim(out);
    if (t != out)
        memmove(out, t, strlen(t) + 1);
    return 1;
}

/* 触发词合法性:小写字母 1–12 个(与 core QuickAction::trigger_valid 一致) */
static int qa_trigger_valid(const char *s)
{
    size_t n = strlen(s);
    if (n < 1 || n > 12)
        return 0;
    for (size_t i = 0; i < n; i++)
        if (s[i] < 'a' || s[i] > 'z')
            return 0;
    return 1;
}

/* [[quick_actions]] 块内当前条目:首个表头出现时清掉预置默认表(文件表
 * 即最终表),随后每表头开新条目(容量满则丢弃后续条目) */
static LyyQuickAction *qa_current(LyyConfig *c, int *in_qa, int *overflow)
{
    if (!*in_qa && c->quick_actions_count > 0) {
        /* 首个表头:丢弃 defaults 预置的默认表,从 0 开始收录文件条目 */
        memset(c->quick_actions, 0, sizeof(c->quick_actions));
        c->quick_actions_count = 0;
    }
    if (*overflow || c->quick_actions_count >= LYY_QA_MAX) {
        *overflow = 1;
        *in_qa = 1;
        return NULL;
    }
    LyyQuickAction *a = &c->quick_actions[c->quick_actions_count];
    memset(a, 0, sizeof(*a));
    c->quick_actions_count++;
    *in_qa = 1;
    return a;
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
    int in_qa = 0;      /* 正处于 [[quick_actions]] 块内 */
    int qa_overflow = 0; /* 条目超上限:继续消费但不入库 */
    while (fgets(line, sizeof(line), fp)) {
        char tmp[2048];
        snprintf(tmp, sizeof(tmp), "%s", line);
        char *p = trim(tmp);
        if (*p == '\0' || *p == '#')
            continue;
        char sec[64];
        if (qa_header_of(p, sec, sizeof(sec))) {
            /* 数组表头:[[quick_actions]] 开新条目;其它 [[表]] 视为未知段 */
            if (!strcmp(sec, "quick_actions")) {
                qa_current(out, &in_qa, &qa_overflow);
            } else {
                in_qa = 0;
            }
            continue;
        }
        if (section_of(p, sec, sizeof(sec))) {
            in_qa = 0;
            snprintf(section, sizeof(section), "%s", sec);
            continue;
        }
        if (in_qa) {
            /* 块内键:trigger/label/command;其它键按未知行原样保留(块结束) */
            const char *value = NULL;
            int field = qa_field_of(p, &value);
            if (field >= 0) {
                LyyQuickAction *a = qa_overflow ? NULL
                    : &out->quick_actions[out->quick_actions_count - 1];
                char vbuf[2048], sbuf[256];
                split_value(value, vbuf, sizeof(vbuf), sbuf, sizeof(sbuf));
                if (a) {
                    if (field == 0)
                        copy_bounded(a->trigger, sizeof(a->trigger), vbuf);
                    else if (field == 1)
                        copy_bounded(a->label, sizeof(a->label), vbuf);
                    else
                        copy_bounded(a->command, sizeof(a->command), vbuf);
                }
                continue;
            }
            in_qa = 0; /* 非块内键:块结束(该行走普通解析) */
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
    /* 无 [[quick_actions]] 块(或全部非法):回退内置默认表 */
    int valid = 0;
    for (int i = 0; i < out->quick_actions_count; i++)
        if (qa_trigger_valid(out->quick_actions[i].trigger) &&
            out->quick_actions[i].label[0] && out->quick_actions[i].command[0])
            out->quick_actions[valid++] = out->quick_actions[i];
    if (valid == 0) {
        out->quick_actions_count =
            (int)(sizeof(g_qa_defaults) / sizeof(g_qa_defaults[0]));
        for (int i = 0; i < out->quick_actions_count; i++)
            out->quick_actions[i] = g_qa_defaults[i];
    } else {
        out->quick_actions_count = valid;
    }
    return 0;
}

int lyy_config_resolve_hotkey_conflicts(LyyConfig *c, char *note, size_t cap)
{
    if (note && cap)
        note[0] = '\0';
    char coin[128], shot[128];
    /* 任一写法非法:宿主按各自合同回退默认并日志,不参与冲突 */
    if (!lyy_hotkey_canon(c->coin_hotkey, coin, sizeof(coin)) ||
        !lyy_hotkey_canon(c->shot_hotkey, shot, sizeof(shot)))
        return 0;
    if (strcmp(coin, shot) != 0)
        return 0;
    /* 占用同一组合:截屏热键逐级让位(工具键让位打字键,合同 §13) */
    char next[128];
    if (lyy_hotkey_escalate(c->shot_hotkey, c->coin_hotkey, next,
                            sizeof(next))) {
        snprintf(c->shot_hotkey, sizeof(c->shot_hotkey), "%s", next);
        if (note && cap)
            snprintf(note, cap,
                     "截屏快捷键 %s 与造词快捷键冲突,已自动改为 %s"
                     "(可在设置中修改)",
                     shot, next);
        return 1;
    }
    if (note && cap)
        snprintf(note, cap,
                 "截屏快捷键 %s 与造词快捷键冲突且无法自动升级,请修改其中一项",
                 shot);
    return -1;
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
    case 6: val = c->commit_unique_four; break;
    case 7: val = c->font_size; break;
    case 8: val = c->autostart; break;
    case 9: val = c->ai_enabled; break;
    case 10: sval = c->ai_api_base; break;
    case 11: sval = c->ai_api_key; break;
    case 12: sval = c->ai_model; break;
    case 13: sval = c->ai_system_prompt; break;
    case 14: val = c->ai_timeout; break;
    case 15: sval = c->coin_hotkey; break;
    case 16: val = c->phrase_hint; break;
    case 17: sval = c->shot_hotkey; break;
    case 18: val = c->quick_actions_enabled; break;
    case 19: val = c->stats_enabled; break;
    case 20: val = c->stats_pause_secs; break;
    case 21: val = c->stats_idle_exclude_secs; break;
    case 22: sval = c->enter_english; break;
    case 23: sval = c->shift_english; break;
    case 24: val = c->exact_char_freq_rank; break;
    case 25: val = c->commit_first_at_four; break;
    case 26: sval = c->custom_query_label; break;
    case 27: sval = c->custom_query_url; break;
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

/* 把 [[quick_actions]] 全部条目写入 buf(表头 + 3 行/条;§14) */
static int write_qa_blocks(Buf *b, const LyyConfig *c)
{
    for (int i = 0; i < c->quick_actions_count; i++) {
        const LyyQuickAction *a = &c->quick_actions[i];
        if (buf_append_str(b, "[[quick_actions]]\n") != 0 ||
            buf_append_str(b, "trigger = \"") != 0 ||
            buf_append_str(b, a->trigger) != 0 ||
            buf_append_str(b, "\" # 触发词(1-12 个小写字母)\n") != 0 ||
            buf_append_str(b, "label = \"") != 0 ||
            buf_append_str(b, a->label) != 0 ||
            buf_append_str(b, "\" # 候选展示文本\n") != 0 ||
            buf_append_str(b, "command = \"") != 0 ||
            buf_append_str(b, a->command) != 0 ||
            buf_append_str(b, "\" # @settings/@help 或 shell 命令\n") != 0)
            return -1;
    }
    return 0;
}

int lyy_config_save(const char *path, const LyyConfig *c)
{
    /* 整读入内存:原位替换受管理键,其余行字节级保留;[ai] 段缺失键按
     * "插入位"(段头/最后一个 ai 键之后)补齐,无段则在尾部新建;
     * [[quick_actions]] 块(§14)原位重写为当前表,文件里没有则追加文件尾。 */
    /* 落盘前先压紧条目(非法触发词/空字段的条目不入盘) */
    LyyConfig cc = *c;
    int valid = 0;
    for (int i = 0; i < c->quick_actions_count; i++)
        if (qa_trigger_valid(c->quick_actions[i].trigger) &&
            c->quick_actions[i].label[0] && c->quick_actions[i].command[0])
            cc.quick_actions[valid++] = c->quick_actions[i];
    cc.quick_actions_count = valid;
    c = &cc;

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
    int qa_written = 0;  /* [[quick_actions]] 块已重写(首个块位置) */
    int in_qa_save = 0;  /* 正在跳过旧 [[quick_actions]] 块的行 */
    const char *qa_dummy = NULL;
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
            if (qa_header_of(p, sec, sizeof(sec)) &&
                !strcmp(sec, "quick_actions")) {
                /* §14:首个块位置重写为当前表,后续旧块整块跳过。
                 * 数组表头同样是"段头":首个块位置兼作顶层补齐键的插入点,
                 * 保证 quick_actions_enabled 等顶层键落在块之前(合法 TOML)。 */
                if (sec_off == (size_t)-1)
                    sec_off = out.len;
                if (!qa_written) {
                    if (write_qa_blocks(&out, c) != 0)
                        goto out;
                    qa_written = 1;
                }
                in_qa_save = 1;
                handled = 1;
            } else if (qa_header_of(p, sec, sizeof(sec))) {
                if (sec_off == (size_t)-1)
                    sec_off = out.len;
                in_qa_save = 0; /* 其它 [[表]]:未知段,原样保留 */
            } else if (section_of(p, sec, sizeof(sec))) {
                if (sec_off == (size_t)-1)
                    sec_off = out.len; /* 本行是首个段头,记下插入点 */
                snprintf(section, sizeof(section), "%s", sec);
                if (!strcmp(sec, "ai")) {
                    ai_hdr_seen = 1;
                    ai_ins_off = out.len + line_len; /* 段头原样保留后再插入 */
                }
                was_ai = !strcmp(section, "ai");
            } else if (in_qa_save && qa_field_of(p, &qa_dummy) >= 0) {
                handled = 1; /* 旧块内 trigger/label/command 行:跳过 */
            } else if (in_qa_save) {
                in_qa_save = 0; /* 块结束 */
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

    /* 文件里本没有 [[quick_actions]] 块:整表追加到文件尾(§14) */
    if (!qa_written && c->quick_actions_count > 0) {
        if (write_qa_blocks(&final, c) != 0)
            goto out;
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
