/*
 * 造词热键解析单元测试(headless;合同 §12)
 * 覆盖:默认 ctrl+equal、修饰组合、单字符/字母/数字/F 键/0x keysym、
 *       大小写与空白容错、非法输入拒绝(无修饰/未知键/重复修饰/超长)、
 *       match 精确匹配语义。
 * 运行:make test(build/tests/unit_hotkey,全绿退出 0)
 */
#include "keysym_map.h"

#include <stdio.h>
#include <string.h>

static int g_failed = 0;

#define CHECK(cond, msg)                                                   \
    do {                                                                   \
        if (cond)                                                          \
            printf("  通过: %s\n", msg);                                   \
        else {                                                             \
            printf("  失败: %s(行 %d)\n", msg, __LINE__);                 \
            g_failed++;                                                    \
        }                                                                  \
    } while (0)

int main(void)
{
    printf("== 造词热键解析单测 ==\n");
    uint32_t mods = 0, sym = 0;

    /* 1. 默认键 */
    CHECK(lyy_hotkey_parse("ctrl+equal", &mods, &sym) == 1 &&
              mods == 0x04 && sym == 0x3d,
          "ctrl+equal → (0x04, 0x3d)");

    /* 2. 大小写/空白容错与 '=' 字面量 */
    CHECK(lyy_hotkey_parse("  Ctrl +=  ", &mods, &sym) == 1 && sym == 0x3d,
          "空白与大写容错");
    CHECK(lyy_hotkey_parse("ctrl+=", &mods, &sym) == 1 && sym == 0x3d,
          "ctrl+=(= 字面量别名)");

    /* 3. 各修饰 */
    CHECK(lyy_hotkey_parse("alt+comma", &mods, &sym) == 1 && mods == 0x08 &&
              sym == 0x2c,
          "alt+comma");
    CHECK(lyy_hotkey_parse("super+f12", &mods, &sym) == 1 && mods == 0x40 &&
              sym == 0xffbe + 11,
          "super+f12");
    CHECK(lyy_hotkey_parse("shift+minus", &mods, &sym) == 1 && mods == 0x01 &&
              sym == 0x2d,
          "shift+minus");
    CHECK(lyy_hotkey_parse("control+win+space", &mods, &sym) == 1 &&
              mods == (0x04 | 0x40) && sym == 0x20,
          "多修饰组合");

    /* 4. 键名形态 */
    CHECK(lyy_hotkey_parse("ctrl+w", &mods, &sym) == 1 && sym == 'w',
          "单字母");
    CHECK(lyy_hotkey_parse("ctrl+5", &mods, &sym) == 1 && sym == '5',
          "数字");
    CHECK(lyy_hotkey_parse("ctrl+0x2f", &mods, &sym) == 1 && sym == 0x2f,
          "0x 十六进制 keysym");
    CHECK(lyy_hotkey_parse("ctrl+pagedown", &mods, &sym) == 1 &&
              sym == 0xff56,
          "功能键名");

    /* 5. 非法输入一律拒绝(宿主回退默认) */
    CHECK(lyy_hotkey_parse("equal", &mods, &sym) == 0, "无修饰拒绝");
    CHECK(lyy_hotkey_parse("", &mods, &sym) == 0, "空串拒绝");
    CHECK(lyy_hotkey_parse(NULL, &mods, &sym) == 0, "NULL 拒绝");
    CHECK(lyy_hotkey_parse("ctrl", &mods, &sym) == 0, "只有修饰拒绝");
    CHECK(lyy_hotkey_parse("ctrl+notakey!", &mods, &sym) == 0,
          "未知键名拒绝");
    CHECK(lyy_hotkey_parse("ctrl+ctrl+w", &mods, &sym) == 0,
          "重复修饰拒绝");
    CHECK(lyy_hotkey_parse("hyper+w", &mods, &sym) == 0, "未知修饰拒绝");
    CHECK(lyy_hotkey_parse("ctrl+f25", &mods, &sym) == 0, "F 越界拒绝");
    CHECK(lyy_hotkey_parse("ctrl+", &mods, &sym) == 0, "尾随 + 拒绝");

    /* 6. match:修饰精确匹配(Lock/NumLock 由调用方先行剔除) */
    CHECK(lyy_hotkey_match(0x04, 0x3d, 0x04, 0x3d) == 1, "match 命中");
    CHECK(lyy_hotkey_match(0x04 | 0x01, 0x3d, 0x04, 0x3d) == 0,
          "多出 Shift 修饰不命中");
    CHECK(lyy_hotkey_match(0x00, 0x3d, 0x04, 0x3d) == 0,
          "缺少修饰不命中");
    CHECK(lyy_hotkey_match(0x04, 0x2d, 0x04, 0x3d) == 0,
          "键不符不命中");

    /* 7. 规范化(合同 §12.3/§13):别名归一 + 修饰定序 + 键名标准形 */
    char canon[128];
    CHECK(lyy_hotkey_canon("ctrl+equal", canon, sizeof(canon)) == 1 &&
              strcmp(canon, "ctrl+equal") == 0,
          "canon 基本形");
    CHECK(lyy_hotkey_canon("Ctrl + =", canon, sizeof(canon)) == 1 &&
              strcmp(canon, "ctrl+equal") == 0,
          "canon 别名归一(= ≡ equal、大小写/空白容错)");
    CHECK(lyy_hotkey_canon("shift+mod4+A", canon, sizeof(canon)) == 1 &&
              strcmp(canon, "super+shift+a") == 0,
          "canon 修饰定序(ctrl+alt+super+shift)");
    CHECK(lyy_hotkey_canon("control+0x31", canon, sizeof(canon)) == 1 &&
              strcmp(canon, "ctrl+1") == 0,
          "canon 键名标准形(control ≡ ctrl、0x → 字符)");
    CHECK(lyy_hotkey_canon("equal", canon, sizeof(canon)) == 0,
          "canon 非法写法拒绝");
    CHECK(lyy_hotkey_canon("ctrl+pagedown", canon, sizeof(canon)) == 1 &&
              strcmp(canon, "ctrl+pagedown") == 0,
          "canon 功能键名保持");

    /* 8. 冲突自动升级:原组合 → +Alt → +Alt+Shift 逐级避让 */
    char next[128];
    CHECK(lyy_hotkey_escalate("ctrl+equal", "ctrl+alt+a", next,
                              sizeof(next)) == 1 &&
              strcmp(next, "ctrl+equal") == 0,
          "原组合空闲时原样返回");
    CHECK(lyy_hotkey_escalate("ctrl+equal", "ctrl+equal", next,
                              sizeof(next)) == 1 &&
              strcmp(next, "ctrl+alt+equal") == 0,
          "被占 → +Alt(用户示例 CTRL+= → CTRL+ALT+=)");
    CHECK(lyy_hotkey_escalate("ctrl+equal", "Ctrl + =", next,
                              sizeof(next)) == 1 &&
              strcmp(next, "ctrl+alt+equal") == 0,
          "别名写法同样构成占用");
    CHECK(lyy_hotkey_escalate("alt+a", "alt+a", next, sizeof(next)) == 1 &&
              strcmp(next, "alt+shift+a") == 0,
          "已含 Alt 的组合下一级直接 +Shift");
    CHECK(lyy_hotkey_escalate("ctrl+equal", "乱写", next, sizeof(next)) == 1 &&
              strcmp(next, "ctrl+equal") == 0,
          "occupied 非法不构成占用");
    CHECK(lyy_hotkey_escalate("ctrl+alt+shift+a", "ctrl+alt+shift+a", next,
                              sizeof(next)) == 0,
          "三级全占用返回 0(交由调用方人话提示)");
    CHECK(lyy_hotkey_escalate("乱写", "", next, sizeof(next)) == 0,
          "spec 非法返回 0");

    printf("== 结果:%s(失败 %d 项)==\n", g_failed ? "有失败" : "全部通过",
           g_failed);
    return g_failed ? 1 : 0;
}
