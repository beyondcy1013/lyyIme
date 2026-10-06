/*
 * 造词热键解析单元测试(合同 §12;需 DISPLAY,经 xvfb-run 运行)
 * 覆盖:默认 ctrl+equal、修饰组合、单字符/字母/数字/F 键/0x keysym、
 *       大小写与空白容错、非法输入拒绝(无修饰/未知键/重复修饰/超长)、
 *       match 精确匹配语义、全局截屏快捷键 GTK accelerator 归一化(§13)。
 * 运行:make test(build/tests/unit_hotkey,全绿退出 0)
 * 附加模式:--global-rollback-test 走真实 xfconf 事务登记+回滚
 *       (需会话总线 + xfconf 服务,由 tests/e2e/global_shot_hotkey_e2e.sh
 *       在隔离环境中调用,非 headless 用例)。
 */
#include "global_hotkey.h"
#include "keysym_map.h"

#include <gtk/gtk.h>
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

int main(int argc, char **argv)
{
    /* accelerator 归一化走 gtk_accelerator_name/parse,需要初始化
     * GTK/X(无显示环境请经 xvfb-run 运行,见 Makefile test 目标) */
    if (!gtk_init_check(&argc, &argv)) {
        fprintf(stderr,
                "unit_hotkey: GTK/X 初始化失败(无 DISPLAY;"
                "请经 xvfb-run -a 运行)\n");
        return 1;
    }

    /* --global-rollback-test:登记一个真实事务再回滚,验证 apply→
     * rollback→commit 全链路把 xfconf 恢复原状(供 E2E 在隔离
     * 会话总线 + xfsettingsd 环境中调用;非 headless 用例) */
    if (argc > 1 && !strcmp(argv[1], "--global-rollback-test")) {
        GError *err = NULL;
        LyyGlobalHotkeyChange *ch =
            lyy_global_shot_apply("ctrl+alt+s", &err);
        if (!ch) {
            fprintf(stderr, "rollback-test: apply 失败:%s\n",
                    err ? err->message : "未知错误");
            g_clear_error(&err);
            return 1;
        }
        GError *rerr = NULL;
        gboolean rolled = lyy_global_shot_rollback(ch, &rerr);
        lyy_global_shot_commit(ch);
        if (!rolled) {
            fprintf(stderr, "rollback-test: rollback 失败:%s\n",
                    rerr ? rerr->message : "未知错误");
            g_clear_error(&rerr);
            return 1;
        }
        printf("global-rollback-test: OK\n");
        return 0;
    }

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

    /* 9. 全局截屏快捷键:shot_hotkey 写法 → GTK accelerator 名
     *    (xfconf 属性名;<Control>/<Primary> 等别名由此归一) */
    {
        gchar *acc = NULL;
        GError *err = NULL;
        acc = lyy_global_shot_accelerator("ctrl+alt+a", &err);
        CHECK(acc && strcmp(acc, "<Primary><Alt>a") == 0,
              "accel 默认 ctrl+alt+a → <Primary><Alt>a");
        g_free(acc);
        g_clear_error(&err);

        acc = lyy_global_shot_accelerator("control+win+f2", &err);
        CHECK(acc && strcmp(acc, "<Primary><Super>F2") == 0,
              "accel 别名归一(control≡ctrl、win≡super)");
        g_free(acc);
        g_clear_error(&err);

        acc = lyy_global_shot_accelerator("ctrl+equal", &err);
        CHECK(acc && strcmp(acc, "<Primary>equal") == 0,
              "accel ctrl+equal → <Primary>equal");
        g_free(acc);
        g_clear_error(&err);

        acc = lyy_global_shot_accelerator("ctrl+return", &err);
        CHECK(acc && strcmp(acc, "<Primary>Return") == 0,
              "accel ctrl+return → <Primary>Return");
        g_free(acc);
        g_clear_error(&err);

        /* pageup 的 accelerator 名按 GTK 键名表产出(Page_Up/Prior),
         * 不锁死拼写:断言能 parse 回同一 (keyval,修饰位) 即可 */
        acc = lyy_global_shot_accelerator("ctrl+pageup", &err);
        if (acc) {
            guint rkey = 0;
            GdkModifierType rmods = 0;
            gtk_accelerator_parse(acc, &rkey, &rmods);
            CHECK(rkey == 0xff55 && rmods == GDK_CONTROL_MASK,
                  "accel ctrl+pageup 往返一致(0xff55 + Ctrl)");
        } else {
            CHECK(0, "accel ctrl+pageup 应解析成功");
        }
        g_free(acc);
        g_clear_error(&err);

        /* 非法写法:无修饰 / 裸键名 / 空串 一律 NULL + error */
        acc = lyy_global_shot_accelerator("a", &err);
        CHECK(acc == NULL && err != NULL, "accel 无修饰拒绝");
        g_free(acc);
        g_clear_error(&err);
        acc = lyy_global_shot_accelerator("equal", &err);
        CHECK(acc == NULL && err != NULL, "accel 裸键名拒绝");
        g_free(acc);
        g_clear_error(&err);
        acc = lyy_global_shot_accelerator("", &err);
        CHECK(acc == NULL && err != NULL, "accel 空串拒绝");
        g_free(acc);
        g_clear_error(&err);
    }

    printf("== 结果:%s(失败 %d 项)==\n", g_failed ? "有失败" : "全部通过",
           g_failed);
    return g_failed ? 1 : 0;
}
