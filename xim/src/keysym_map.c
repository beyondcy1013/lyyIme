#include "keysym_map.h"

#include "core_ffi.h"

/* keysymdef.h 按功能宏导出符号组(标准用法) */
#define XK_MISCELLANY
#define XK_XKB_KEYS
#define XK_LATIN1
#include <X11/keysymdef.h>
#include <ctype.h>

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
    /* 大写字母(Shift/capslock 进入):小写化后仍按字母缓冲 */
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

    switch (keysym) {
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
