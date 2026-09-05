/*
 * keysym → LKey 映射(与 docs/ARCHITECTURE.md §6 按键行为规范完全一致)
 *
 * 映射表(Shift 列已由调用方按事件 state 选好 col,Shift+1 得 '!' 这样的
 * 上档字符由 keysym 本身携带):
 *   XK_a..XK_z / XK_A..XK_Z      → LKEY_CHAR(小写化,§6"a–z 进缓冲")
 *   XK_1..XK_9、XK_KP_1..KP_9    → LKEY_DIGIT
 *   XK_0、XK_KP_0                → LKEY_PUNCT '0'(交由 core 决定直通与否)
 *   XK_space                     → LKEY_SPACE
 *   XK_Return、XK_KP_Enter       → LKEY_ENTER
 *   XK_BackSpace                 → LKEY_BACKSPACE
 *   XK_Escape                    → LKEY_ESC
 *   XK_minus / XK_equal          → LKEY_PAGEUP / LKEY_PAGEDOWN(§6 翻页)
 *   §6 标点集 `, . ? ! ; : ' " ( ) [ ] { }` 及常用半角符号 → LKEY_PUNCT
 *   Shift_L / Shift_R            → 由 xim_server 的触发键逻辑单独处理(不入 core)
 *   其余(功能键/方向键/组合键)  → LKEY_OTHER(core 恒回 Pass,§6 末行)
 *
 * keysym 常量取自 X11 keysymdef.h(仅头文件常量,不引入 Xlib 链接依赖)。
 */
#ifndef LYY_KEYSYM_MAP_H_
#define LYY_KEYSYM_MAP_H_

#include <stdint.h>

/* 输出参数:*key = LKEY_*(core_ffi.h),*chr = CHAR/DIGIT/PUNCT 码点(其余 0) */
void lyy_keysym_map(uint32_t keysym, int *key, uint32_t *chr);

/* Shift_L / Shift_R(触发键,两个 Shift,任务书 §8) */
int lyy_keysym_is_shift(uint32_t keysym);

#endif /* LYY_KEYSYM_MAP_H_ */
