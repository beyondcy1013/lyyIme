#include "effects_json.h"

#include <ctype.h>
#include <stdlib.h>
#include <string.h>

static const char *skip_ws(const char *p)
{
    while (*p && isspace((unsigned char)*p))
        p++;
    return p;
}

/* 解析 JSON 字符串字面量到 out(含最小转义集,见头文件说明) */
static const char *parse_string(const char *p, char *out, size_t cap)
{
    size_t w = 0;
    if (*p != '"')
        return NULL;
    p++;
    while (*p && *p != '"') {
        char ch = *p;
        if (ch == '\\') {
            p++;
            switch (*p) {
            case '"': ch = '"'; break;
            case '\\': ch = '\\'; break;
            case '/': ch = '/'; break;
            case 'b': ch = '\b'; break;
            case 'f': ch = '\f'; break;
            case 'n': ch = '\n'; break;
            case 'r': ch = '\r'; break;
            case 't': ch = '\t'; break;
            default: return NULL; /* 含 \uXXXX:固定 schema 不支持 */
            }
        }
        if (w + 1 >= cap)
            return NULL;
        out[w++] = ch;
        p++;
    }
    if (*p != '"')
        return NULL;
    out[w] = '\0';
    return p + 1;
}

static int kind_of(const char *t)
{
    if (!strcmp(t, "commit")) return LYY_EFF_COMMIT;
    if (!strcmp(t, "preedit")) return LYY_EFF_PREEDIT;
    if (!strcmp(t, "cands")) return LYY_EFF_CANDS;
    if (!strcmp(t, "pass")) return LYY_EFF_PASS;
    if (!strcmp(t, "consumed")) return LYY_EFF_CONSUMED;
    if (!strcmp(t, "notice")) return LYY_EFF_NOTICE;
    if (!strcmp(t, "mode")) return LYY_EFF_MODE;
    return -1;
}

/* 解析一个对象;成功返回结束位置,eff 已填充 */
static const char *parse_effect(const char *p, LyyEffect *eff)
{
    p = skip_ws(p);
    if (*p != '{')
        return NULL;
    p = skip_ws(p + 1);

    memset(eff, 0, sizeof(*eff));
    eff->kind = -1;
    eff->n = eff->page = eff->pages = eff->m = 0;
    eff->s[0] = '\0';
    int have_t = 0;

    if (*p == '}') /* 空对象也算违约(缺 t) */
        return NULL;

    for (;;) {
        p = skip_ws(p);
        char key[32];
        p = parse_string(p, key, sizeof(key));
        if (!p)
            return NULL;
        p = skip_ws(p);
        if (*p != ':')
            return NULL;
        p = skip_ws(p + 1);

        if (!strcmp(key, "t")) {
            char tval[32];
            p = parse_string(p, tval, sizeof(tval));
            if (!p)
                return NULL;
            eff->kind = kind_of(tval);
            if (eff->kind < 0)
                return NULL;
            have_t = 1;
        } else if (!strcmp(key, "s")) {
            p = parse_string(p, eff->s, sizeof(eff->s));
            if (!p)
                return NULL;
        } else if (!strcmp(key, "n") || !strcmp(key, "page") ||
                   !strcmp(key, "pages") || !strcmp(key, "m")) {
            char *end = NULL;
            long v = strtol(p, &end, 10);
            if (end == p || (end && *end != '\0' && !isspace((unsigned char)*end) &&
                             *end != ',' && *end != '}'))
                return NULL;
            if (!strcmp(key, "n")) eff->n = (int)v;
            else if (!strcmp(key, "page")) eff->page = (int)v;
            else if (!strcmp(key, "pages")) eff->pages = (int)v;
            else eff->m = (int)v;
            p = end;
        } else {
            return NULL; /* 固定 schema 之外的键判违约 */
        }

        p = skip_ws(p);
        if (*p == ',') {
            p++;
            continue;
        }
        if (*p == '}') {
            p++;
            break;
        }
        return NULL;
    }
    if (!have_t)
        return NULL;
    return p;
}

int lyy_effects_parse(const char *json, LyyEffect *out, int max_out, int *count)
{
    *count = 0;
    if (!json)
        return -1;
    const char *p = skip_ws(json);
    if (*p != '[')
        return -1;
    p = skip_ws(p + 1);

    if (*p == ']') {
        return 0;
    }

    for (;;) {
        if (*count >= max_out)
            return -2;
        LyyEffect eff;
        p = parse_effect(p, &eff);
        if (!p)
            return -1;
        out[(*count)++] = eff;

        p = skip_ws(p);
        if (*p == ',') {
            p++;
            continue;
        }
        if (*p == ']') {
            return 0;
        }
        return -1;
    }
}

const char *lyy_effect_kind_name(int kind)
{
    switch (kind) {
    case LYY_EFF_COMMIT: return "commit";
    case LYY_EFF_PREEDIT: return "preedit";
    case LYY_EFF_CANDS: return "cands";
    case LYY_EFF_PASS: return "pass";
    case LYY_EFF_CONSUMED: return "consumed";
    case LYY_EFF_NOTICE: return "notice";
    case LYY_EFF_MODE: return "mode";
    default: return "?";
    }
}
