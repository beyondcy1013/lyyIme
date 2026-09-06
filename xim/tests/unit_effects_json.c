/*
 * effects_json 迷你解析器单元测试(headless,无 GTK/X11 依赖)
 * 覆盖:合同六种效果、字段乱序、preedit 无 s(清除预编辑,主控通报形态)、
 *       中文 UTF-8、转义、空数组、坏输入、缓冲截断、多效果序列。
 * 运行:make test(build/tests/unit_effects_json,全绿退出 0)
 */
#include "effects_json.h"

#include <stdio.h>
#include <string.h>

static int g_failed = 0;

#define CHECK(cond, msg)                                                    \
    do {                                                                    \
        if (cond) {                                                         \
            printf("  通过: %s\n", msg);                                    \
        } else {                                                            \
            printf("  失败: %s(行 %d)\n", msg, __LINE__);                  \
            g_failed++;                                                     \
        }                                                                   \
    } while (0)

static int parse_ok(const char *json, LyyEffect *out, int max, int *count)
{
    return lyy_effects_parse(json, out, max, count);
}

int main(void)
{
    LyyEffect eff[16];
    int n = 0;

    printf("== effects_json 单测 ==\n");

    /* 1. 空数组 */
    CHECK(parse_ok("[]", eff, 16, &n) == 0 && n == 0, "空数组解析成功");

    /* 2. commit 中文 UTF-8 */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"commit\",\"s\":\"你好\"}]", eff, 16, &n) == 0 &&
              n == 1 && eff[0].kind == LYY_EFF_COMMIT &&
              strcmp(eff[0].s, "你好") == 0,
          "commit 中文 UTF-8");

    /* 3. preedit 有 s */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"preedit\",\"s\":\"nihao\"}]", eff, 16, &n) == 0 &&
              n == 1 && eff[0].kind == LYY_EFF_PREEDIT &&
              strcmp(eff[0].s, "nihao") == 0,
          "preedit 有内容");

    /* 4. preedit 无 s = 清除预编辑(主控通报形态) */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"preedit\"}]", eff, 16, &n) == 0 && n == 1 &&
              eff[0].kind == LYY_EFF_PREEDIT && eff[0].s[0] == '\0',
          "preedit 无 s = 清除预编辑");

    /* 5. cands n/page/pages */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"cands\",\"n\":5,\"page\":0,\"pages\":3}]",
                   eff, 16, &n) == 0 &&
              n == 1 && eff[0].kind == LYY_EFF_CANDS && eff[0].n == 5 &&
              eff[0].page == 0 && eff[0].pages == 3,
          "cands n/page/pages");

    /* 6. pass / consumed */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"pass\"},{\"t\":\"consumed\"}]", eff, 16, &n) ==
                  0 &&
              n == 2 && eff[0].kind == LYY_EFF_PASS &&
              eff[1].kind == LYY_EFF_CONSUMED,
          "pass/consumed");

    /* 7. mode m */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"mode\",\"m\":1}]", eff, 16, &n) == 0 && n == 1 &&
              eff[0].kind == LYY_EFF_MODE && eff[0].m == 1,
          "mode m=1");

    /* 8. 字段乱序:s 在 t 前 */
    n = -1;
    CHECK(parse_ok("[{\"s\":\"x\",\"t\":\"commit\"}]", eff, 16, &n) == 0 &&
              n == 1 && eff[0].kind == LYY_EFF_COMMIT &&
              strcmp(eff[0].s, "x") == 0,
          "字段乱序解析");

    /* 9. 多效果完整序列(典型:Commit+Preedit 清除) */
    n = -1;
    int rc = parse_ok(
        "[{\"t\":\"commit\",\"s\":\"你号\"},{\"t\":\"preedit\"}]", eff, 16,
        &n);
    CHECK(rc == 0 && n == 2 && eff[0].kind == LYY_EFF_COMMIT &&
              strcmp(eff[0].s, "你号") == 0 && eff[1].kind == LYY_EFF_PREEDIT &&
              eff[1].s[0] == '\0',
          "commit+preedit 清除 序列");

    /* 10. 典型输入序列:preedit+cands+mode 混合 */
    n = -1;
    rc = parse_ok(
        "[{\"t\":\"preedit\",\"s\":\"ni\"},{\"t\":\"cands\",\"n\":5,\"page\":0,\"pages\":2},{\"t\":\"mode\",\"m\":0}]",
        eff, 16, &n);
    CHECK(rc == 0 && n == 3 && eff[0].kind == LYY_EFF_PREEDIT &&
              eff[1].kind == LYY_EFF_CANDS && eff[1].n == 5 &&
              eff[1].pages == 2 && eff[2].kind == LYY_EFF_MODE,
          "三效果混合序列");

    /* 11. 字符串转义 */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"commit\",\"s\":\"a\\nb\\\"c\\\\d\"}]", eff, 16,
                   &n) == 0 &&
              n == 1 && strcmp(eff[0].s, "a\nb\"c\\d") == 0,
          "字符串转义 \\n \\\" \\\\");

    /* 12. 坏输入:截断 */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"comm", eff, 16, &n) == -1, "截断 JSON 判错");

    /* 13. 坏输入:未知 t 值 */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"unknown\"}]", eff, 16, &n) == -1,
          "未知效果类型判错");

    /* 14. 坏输入:非数组 */
    n = -1;
    CHECK(parse_ok("{\"t\":\"pass\"}", eff, 16, &n) == -1, "非数组判错");

    /* 15. 缓冲不足 */
    n = -1;
    LyyEffect one;
    CHECK(parse_ok("[{\"t\":\"pass\"},{\"t\":\"pass\"}]", &one, 1, &n) == -2,
          "缓冲不足返回 -2");

    /* 16. 空白容错 */
    n = -1;
    CHECK(parse_ok("  [ { \"t\" : \"pass\" } ]  ", eff, 16, &n) == 0 &&
              n == 1,
          "空白容错");

    /* 17. hint 效果(合同 §6:词组效率提示) */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"hint\",\"s\":\"词组提示:「你好」可用 wqvb 打出\"}]",
                   eff, 16, &n) == 0 &&
              n == 1 && eff[0].kind == LYY_EFF_HINT &&
              !strcmp(eff[0].s, "词组提示:「你好」可用 wqvb 打出"),
          "hint 效果解析");

    /* 18. notice 效果(合同 §12:造词结果提示) */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"notice\",\"s\":\"已造词:你好(wqvb)\"}]",
                   eff, 16, &n) == 0 &&
              n == 1 && eff[0].kind == LYY_EFF_NOTICE &&
              !strcmp(eff[0].s, "已造词:你好(wqvb)"),
          "notice 效果解析");

    /* 19. action 效果(合同 §14:快速功能键命中,i = 配置下标) */
    n = -1;
    CHECK(parse_ok("[{\"t\":\"action\",\"i\":0},{\"t\":\"preedit\"},{\"t\":\"cands\",\"n\":0,\"page\":0,\"pages\":0}]",
                   eff, 16, &n) == 0 &&
              n == 3 && eff[0].kind == LYY_EFF_ACTION && eff[0].i == 0 &&
              eff[1].kind == LYY_EFF_PREEDIT && eff[2].kind == LYY_EFF_CANDS,
          "action 效果解析(i=0,含清除序列)");
    n = -1;
    CHECK(parse_ok("[{\"t\":\"action\",\"i\":7}]", eff, 16, &n) == 0 &&
              n == 1 && eff[0].kind == LYY_EFF_ACTION && eff[0].i == 7,
          "action 下标任意值");
    /* 未知键仍判违约(新键 i 不放宽其它键) */
    CHECK(parse_ok("[{\"t\":\"action\",\"i\":0,\"x\":1}]", eff, 16, &n) != 0,
          "schema 之外的键仍违约");

    printf("== 结果:%s(失败 %d 项)==\n", g_failed ? "有失败" : "全部通过",
           g_failed);
    return g_failed ? 1 : 0;
}
