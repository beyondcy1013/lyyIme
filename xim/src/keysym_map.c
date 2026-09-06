#include "keysym_map.h"

#include "core_ffi.h"

/* keysymdef.h 按功能宏导出符号组(标准用法) */
#define XK_MISCELLANY
#define XK_XKB_KEYS
#define XK_LATIN1
#include <X11/keysymdef.h>
#include <xcb/xproto.h>
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int lyy_keysym_is_shift(uint32_t keysym)
{
    return keysym == XK_Shift_L || keysym == XK_Shift_R;
}

void lyy_keysym_map(uint32_t keysym, int *key, uint32_t *chr)
{
    *key = LKEY_OTHER;
    *chr = 0;

    /* 小写字母:chr=码点(§6 宿主负责小写化) */
    if (keysym >= (uint32_t)'a' && keysym <= (uint32_t)'z') {
        *key = LKEY_CHAR;
        *chr = keysym;
        return;
    }
    /* 大写字母(Shift 进入;CapsLock 大写态已在 xim_server 直通,
     * 走不到这里):小写化后仍按字母缓冲 */
    if (keysym >= (uint32_t)'A' && keysym <= (uint32_t)'Z') {
        *key = LKEY_CHAR;
        *chr = (uint32_t)tolower((int)keysym);
        return;
    }
    /* 数字 1–9 选候选;0 无选词语义,交 core 当标点处理 */
    if (keysym >= (uint32_t)'0' && keysym <= (uint32_t)'9') {
        if (keysym == (uint32_t)'0') {
            *key = LKEY_PUNCT;
            *chr = '0';
        } else {
            *key = LKEY_DIGIT;
            *chr = keysym;
        }
        return;
    }
    /* 小键盘数字(NumLock 开启时) */
    if (keysym >= XK_KP_0 && keysym <= XK_KP_9) {
        if (keysym == XK_KP_0) {
            *key = LKEY_PUNCT;
            *chr = '0';
        } else {
            *key = LKEY_DIGIT;
            *chr = (uint32_t)('1' + (keysym - XK_KP_1));
        }
        return;
    }

    /* 方向键:造词模式增减选字(合同 §12);非造词模式 core 按同 Other 处理 */
    switch (keysym) {
    case XK_Left:
        *key = LKEY_LEFT;
        return;
    case XK_Up:
        *key = LKEY_UP;
        return;
    case XK_Right:
        *key = LKEY_RIGHT;
        return;
    case XK_Down:
        *key = LKEY_DOWN;
        return;
    case XK_space:
        *key = LKEY_SPACE;
        return;
    case XK_Return:
    case XK_KP_Enter:
        *key = LKEY_ENTER;
        return;
    case XK_BackSpace:
        *key = LKEY_BACKSPACE;
        return;
    case XK_Escape:
        *key = LKEY_ESC;
        return;
    case XK_minus: /* '-' '=' 翻页(§6);有候选翻页,无候选 core 回 Pass */
        *key = LKEY_PAGEUP;
        return;
    case XK_equal:
        *key = LKEY_PAGEDOWN;
        return;
    default:
        break;
    }

    /* §6 中文标点映射覆盖的半角标点集(交 core 决定中文标点/直通) */
    if ((keysym >= 0x20 && keysym <= 0x7e) &&
        (keysym == (uint32_t)',' || keysym == (uint32_t)'.' ||
         keysym == (uint32_t)'?' || keysym == (uint32_t)'!' ||
         keysym == (uint32_t)';' || keysym == (uint32_t)':' ||
         keysym == (uint32_t)'\'' || keysym == (uint32_t)'"' ||
         keysym == (uint32_t)'(' || keysym == (uint32_t)')' ||
         keysym == (uint32_t)'[' || keysym == (uint32_t)']' ||
         keysym == (uint32_t)'{' || keysym == (uint32_t)'}' ||
         keysym == (uint32_t)'/' || keysym == (uint32_t)'\\' ||
         keysym == (uint32_t)'@' || keysym == (uint32_t)'#' ||
         keysym == (uint32_t)'$' || keysym == (uint32_t)'%' ||
         keysym == (uint32_t)'^' || keysym == (uint32_t)'&' ||
         keysym == (uint32_t)'*' || keysym == (uint32_t)'~' ||
         keysym == (uint32_t)'`' || keysym == (uint32_t)'|' ||
         keysym == (uint32_t)'<' || keysym == (uint32_t)'>')) {
        *key = LKEY_PUNCT;
        *chr = keysym;
        return;
    }
    /* 其余:core 恒回 Pass(§6 末行,有缓冲时 core 先 reset) */
}

/* ---- 造词热键解析(合同 §12;与 Mode A lyyime.py parse_hotkey 同规格)---- */

typedef struct {
    const char *name;
    uint32_t keysym;
} LyyKeyName;

static const LyyKeyName LYY_HOTKEY_MODS[] = {
    { "ctrl", XCB_MOD_MASK_CONTROL },
    { "control", XCB_MOD_MASK_CONTROL },
    { "alt", XCB_MOD_MASK_1 },
    { "mod1", XCB_MOD_MASK_1 },
    { "super", XCB_MOD_MASK_4 },
    { "mod4", XCB_MOD_MASK_4 },
    { "win", XCB_MOD_MASK_4 },
    { "shift", XCB_MOD_MASK_SHIFT },
};

static const LyyKeyName LYY_HOTKEY_KEYS[] = {
    { "equal", 0x3d },
    { "=", 0x3d },
    { "minus", 0x2d },
    { "-", 0x2d },
    { "grave", 0x60 },
    { "bracketleft", 0x5b },
    { "[", 0x5b },
    { "bracketright", 0x5d },
    { "]", 0x5d },
    { "semicolon", 0x3b },
    { ";", 0x3b },
    { "apostrophe", 0x27 },
    { "'", 0x27 },
    { "comma", 0x2c },
    { ",", 0x2c },
    { "period", 0x2e },
    { ".", 0x2e },
    { "slash", 0x2f },
    { "/", 0x2f },
    { "backslash", 0x5c },
    { "\\", 0x5c },
    { "space", 0x20 },
    { "tab", 0xff09 },
    { "return", 0xff0d },
    { "enter", 0xff0d },
    { "escape", 0xff1b },
    { "esc", 0xff1b },
    { "left", 0xff51 },
    { "up", 0xff52 },
    { "right", 0xff53 },
    { "down", 0xff54 },
    { "insert", 0xff63 },
    { "delete", 0xffff },
    { "del", 0xffff },
    { "home", 0xff50 },
    { "end", 0xff57 },
    { "pageup", 0xff55 },
    { "pagedown", 0xff56 },
    { "print", 0xff61 },
};

int lyy_hotkey_parse(const char *spec, uint32_t *mods, uint32_t *keysym)
{
    if (!spec || !*spec || !mods || !keysym)
        return 0;

    /* 复制成可改写的缓冲,按 '+' 切分(最多 5 段:4 修饰 + 1 键) */
    char buf[128];
    size_t len = strlen(spec);
    if (len >= sizeof(buf))
        return 0;
    memcpy(buf, spec, len + 1);

    uint32_t m = 0;
    char *tokens[6] = { 0 };
    int ntok = 0;
    char *save = NULL;
    for (char *tok = strtok_r(buf, "+", &save); tok && ntok < 6;
         tok = strtok_r(NULL, "+", &save)) {
        /* 小写化 + 去首尾空白 */
        while (*tok == ' ' || *tok == '\t')
            tok++;
        char *end = tok + strlen(tok);
        while (end > tok && (end[-1] == ' ' || end[-1] == '\t'))
            *--end = '\0';
        for (char *q = tok; *q; q++)
            *q = (char)tolower((unsigned char)*q);
        if (*tok == '\0')
            return 0;
        if (ntok >= 5)
            return 0;
        tokens[ntok++] = tok;
    }
    if (ntok < 2) /* 至少一个修饰 + 一个键 */
        return 0;

    for (int i = 0; i < ntok - 1; i++) {
        size_t cnt = sizeof(LYY_HOTKEY_MODS) / sizeof(LYY_HOTKEY_MODS[0]);
        uint32_t bit = 0;
        for (size_t k = 0; k < cnt; k++) {
            if (!strcmp(tokens[i], LYY_HOTKEY_MODS[k].name)) {
                bit = LYY_HOTKEY_MODS[k].keysym;
                break;
            }
        }
        if (!bit || (m & bit)) /* 未知或重复修饰 */
            return 0;
        m |= bit;
    }

    const char *key = tokens[ntok - 1];
    uint32_t sym = 0;
    size_t klen = strlen(key);
    if (klen == 1 && ((key[0] >= 'a' && key[0] <= 'z') ||
                      (key[0] >= '0' && key[0] <= '9'))) {
        sym = (uint32_t)key[0];
    } else if (klen >= 2 && key[0] == 'f' && klen <= 3) {
        char *end = NULL;
        long n = strtol(key + 1, &end, 10);
        if (end && *end == '\0' && n >= 1 && n <= 24)
            sym = (uint32_t)(0xffbe + n - 1);
    } else if (klen > 2 && key[0] == '0' && key[1] == 'x') {
        char *end = NULL;
        unsigned long v = strtoul(key + 2, &end, 16);
        if (end && *end == '\0' && v > 0 && v <= 0xffffff)
            sym = (uint32_t)v;
    } else {
        size_t cnt = sizeof(LYY_HOTKEY_KEYS) / sizeof(LYY_HOTKEY_KEYS[0]);
        for (size_t k = 0; k < cnt; k++) {
            if (!strcmp(key, LYY_HOTKEY_KEYS[k].name)) {
                sym = LYY_HOTKEY_KEYS[k].keysym;
                break;
            }
        }
    }
    if (sym == 0 || m == 0)
        return 0;
    *mods = m;
    *keysym = sym;
    return 1;
}

int lyy_hotkey_match(uint32_t mods, uint32_t keysym, uint32_t hotkey_mods,
                     uint32_t hotkey_keysym)
{
    return mods == hotkey_mods && keysym == hotkey_keysym;
}

/* ---- 规范化与冲突升级(合同 §12.3/§13;与 lyyime-core src/hotkey.rs 同规格)----
 * 规范化:别名归一(control≡ctrl、mod1≡alt、mod4/win≡super、"="≡equal 等)
 * + 修饰定序(ctrl+alt+super+shift)+ 键名标准形,用于冲突比较与设置窗回显;
 * 升级:组合被占用时按 原组合 → +Alt → +Alt+Shift 逐级尝试空闲组合。 */

/* keysym → 标准键名(与 lyy_hotkey_parse 的键名解析互逆,别名取正名;
 * 表外 keysym 写作 0x 十六进制,保证任意合法解析结果都可回显) */
static void hotkey_key_label(uint32_t sym, char *buf, size_t cap)
{
    if ((sym >= 'a' && sym <= 'z') || (sym >= '0' && sym <= '9')) {
        snprintf(buf, cap, "%c", (char)sym);
        return;
    }
    if (sym >= 0xffbe && sym <= 0xffc9) { /* F1–F12 */
        snprintf(buf, cap, "f%u", (unsigned)(sym - 0xffbe + 1));
        return;
    }
    if (sym >= 0xffca && sym <= 0xffd5) { /* F13–F24(与 parse 同段连续映射) */
        snprintf(buf, cap, "f%u", (unsigned)(sym - 0xffca + 13));
        return;
    }
    size_t cnt = sizeof(LYY_HOTKEY_KEYS) / sizeof(LYY_HOTKEY_KEYS[0]);
    for (size_t k = 0; k < cnt; k++) {
        if (LYY_HOTKEY_KEYS[k].keysym == sym) {
            snprintf(buf, cap, "%s", LYY_HOTKEY_KEYS[k].name);
            return;
        }
    }
    snprintf(buf, cap, "0x%lx", (unsigned long)sym);
}

/* (修饰位, keysym) → 规范化串;缓冲不足返回 0 */
static int hotkey_canon_from(uint32_t mods, uint32_t sym, char *out, size_t cap)
{
    static const struct {
        uint32_t bit;
        const char *name;
    } MODS[] = {
        { XCB_MOD_MASK_CONTROL, "ctrl" },
        { XCB_MOD_MASK_1, "alt" },
        { XCB_MOD_MASK_4, "super" },
        { XCB_MOD_MASK_SHIFT, "shift" },
    };
    size_t off = 0;
    for (size_t i = 0; i < sizeof(MODS) / sizeof(MODS[0]); i++) {
        if (!(mods & MODS[i].bit))
            continue;
        int n = snprintf(out + off, cap - off, "%s%s", off ? "+" : "",
                         MODS[i].name);
        if (n < 0 || (size_t)n >= cap - off)
            return 0;
        off += (size_t)n;
    }
    char key[32];
    hotkey_key_label(sym, key, sizeof(key));
    int n = snprintf(out + off, cap - off, "%s%s", off ? "+" : "", key);
    if (n < 0 || (size_t)n >= cap - off)
        return 0;
    return 1;
}

int lyy_hotkey_canon(const char *spec, char *out, size_t cap)
{
    uint32_t mods = 0, sym = 0;
    if (!lyy_hotkey_parse(spec, &mods, &sym))
        return 0;
    return hotkey_canon_from(mods, sym, out, cap);
}

int lyy_hotkey_escalate(const char *spec, const char *occupied, char *out,
                        size_t cap)
{
    uint32_t mods = 0, sym = 0, om = 0, os = 0;
    if (!lyy_hotkey_parse(spec, &mods, &sym))
        return 0;
    /* occupied 写法非法不构成占用(宿主对非法热键本就不拦截) */
    int occ_ok = lyy_hotkey_parse(occupied, &om, &os);
    /* 阶梯:原组合 → +Alt → +Alt+Shift;已含的修饰自动跳过
     * (候选与前一等级重合时必然同样被占,continue 语义天然去重) */
    for (int level = 0; level < 3; level++) {
        uint32_t cand = mods;
        if (level >= 1)
            cand |= XCB_MOD_MASK_1;
        if (level >= 2)
            cand |= XCB_MOD_MASK_SHIFT;
        if (occ_ok && cand == om && sym == os)
            continue;
        return hotkey_canon_from(cand, sym, out, cap);
    }
    return 0;
}
