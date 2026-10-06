/*
 * config(TOML 平面子集)单元测试(headless)
 * 覆盖:默认值、加载(存在/不存在)、保存后在位更新、未知行与注释保留、
 *       布尔写法保持与翻转落盘、非法值钳制、autostart 开关(写/删 desktop 文件)。
 * 运行:make test(build/tests/unit_config,全绿退出 0)
 */
#include "config.h"

#include <glib/gstdio.h>
#include <stdio.h>
#include <stdlib.h>
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

static char *read_all(const char *path)
{
    gchar *s = NULL;
    gsize len = 0;
    if (!g_file_get_contents(path, &s, &len, NULL))
        return NULL;
    return s; /* g_malloc 家族,用 g_free 释放 */
}

int main(void)
{
    char dir[] = "/tmp/lyyime-unit-config-XXXXXX";
    char *tmp = mkdtemp(dir);
    if (!tmp) {
        perror("mkdtemp");
        return 1;
    }
    char path[512];
    snprintf(path, sizeof(path), "%s/config.toml", tmp);

    printf("== config 单测 ==\n");

    /* 1. 默认值 */
    LyyConfig c;
    lyy_config_defaults(&c);
    CHECK(c.page_size == 10 && c.mixed_english == 1 &&
              c.commit_after_four == 1 && c.commit_unique_four == 1 &&
              c.commit_first_at_four == 1 &&
              c.phrase_hint == 1 && c.exact_char_freq_rank == 1 &&
              c.next_word_prediction == 0 &&
              c.font_size == 14 && c.autostart == 0,
          "默认值正确");

    /* 2. 加载不存在的文件 */
    int rc = lyy_config_load(path, &c);
    CHECK(rc == 1, "文件不存在返回 1 并用默认值");

    /* 3. 保存生成文件,含全部管理键与中文注释 */
    c.page_size = 7;
    c.autostart = 1;
    c.commit_after_four = 1;
    c.commit_unique_four = 0;
    c.commit_first_at_four = 0;
    c.phrase_hint = 0;
    c.next_word_prediction = 0;
    CHECK(lyy_config_save(path, &c) == 0, "保存成功");
    char *body = read_all(path);
    CHECK(body && strstr(body, "page_size = 7") && strstr(body, "候选数") &&
              strstr(body, "commit_unique_four = false") &&
              strstr(body, "commit_first_at_four = false") &&
              strstr(body, "phrase_hint = false") &&
              strstr(body, "next_word_prediction = false"),
          "保存内容含键与注释");

    /* 4. 在文件中插入未知行/注释/[section],再保存,必须原样保留 */
    FILE *fp = fopen(path, "a");
    fprintf(fp,
            "# 用户手写注释\n"
            "[doctor]\n"
            "custom_unknown_key = 42\n");
    fclose(fp);
    c.page_size = 9;
    CHECK(lyy_config_save(path, &c) == 0, "再次保存成功");
    g_free(body);
    body = read_all(path);
    CHECK(body != NULL && strstr(body, "# 用户手写注释") &&
              strstr(body, "[doctor]") &&
              strstr(body, "custom_unknown_key = 42") &&
              strstr(body, "page_size = 9"),
          "未知行、注释、section 原样保留,已知键在位更新");

    /* 5. 布尔写法保持(true/false) */
    CHECK(strstr(body, "mixed_english = true") != NULL, "布尔值写法保持");

    /* 6. 回读一致性 + 非法值钳制 */
    LyyConfig c2;
    CHECK(lyy_config_load(path, &c2) == 0 && c2.page_size == 9 &&
              c2.mixed_english == 1 && c2.phrase_hint == 0 &&
              c2.next_word_prediction == 0,
          "回读一致");
    g_free(body);
    fp = fopen(path, "w");
    fprintf(fp, "page_size = 99\nfont_size = 3\n");
    fclose(fp);
    lyy_config_load(path, &c2);
    CHECK(c2.page_size == 10 && c2.font_size == 14, "越界值钳制回默认");

    /* 7. 布尔值翻转必须落盘:文件已有 false,置 1 后保存必须写回 true */
    fp = fopen(path, "w");
    fprintf(fp, "autostart = false\n");
    fclose(fp);
    LyyConfig c3;
    lyy_config_load(path, &c3);
    CHECK(c3.autostart == 0, "读到已有 false 值");
    c3.autostart = 1;
    CHECK(lyy_config_save(path, &c3) == 0, "翻转后保存成功");
    body = read_all(path);
    CHECK(body && strstr(body, "autostart = true"),
          "已有 false 值翻转后写回 true(不得冻结旧值)");
    g_free(body);

    /* 8. 数字写法保持:autostart = 0 翻转后写 1(而非 true) */
    fp = fopen(path, "w");
    fprintf(fp, "autostart = 0\n");
    fclose(fp);
    lyy_config_load(path, &c3);
    c3.autostart = 1;
    lyy_config_save(path, &c3);
    body = read_all(path);
    CHECK(body && strstr(body, "autostart = 1") &&
              !strstr(body, "autostart = true"),
          "数字写法保持且值更新");
    g_free(body);

    /* 9. [ai] 段:带转义字符串读写回环 + ai_active 判定 */
    fp = fopen(path, "w");
    fprintf(fp,
            "[ai]\n"
            "enabled = true\n"
            "api_base = \"https://api.deepseek.com/v1\"\n"
            "api_key = \"sk-\\\"x\\\"\"\n"
            "model = \"deepseek-chat\"\n"
            "timeout = 45\n");
    fclose(fp);
    LyyConfig c4;
    CHECK(lyy_config_load(path, &c4) == 0, "加载 [ai] 段");
    CHECK(c4.ai_enabled == 1 &&
              strcmp(c4.ai_api_base, "https://api.deepseek.com/v1") == 0 &&
              strcmp(c4.ai_api_key, "sk-\"x\"") == 0 &&
              strcmp(c4.ai_model, "deepseek-chat") == 0 &&
              c4.ai_timeout == 45,
          "[ai] 段字符串反转义与布尔/整数读取");
    CHECK(lyy_config_ai_active(&c4) == 1, "启用+地址+模型配齐后 ai_active");
    snprintf(c4.ai_model, sizeof(c4.ai_model), "glm-4.7");
    CHECK(lyy_config_save(path, &c4) == 0, "[ai] 保存");
    body = read_all(path);
    CHECK(body && strstr(body, "model = \"glm-4.7\"") &&
              strstr(body, "api_key = \"sk-\\\"x\\\"\"") &&
              strstr(body, "[ai]"),
          "[ai] 键在位更新且字符串值转义写回");
    g_free(body);
    LyyConfig c5;
    lyy_config_load(path, &c5);
    CHECK(strcmp(c5.ai_model, "glm-4.7") == 0, "[ai] 回读一致");

    /* 10. 其它段内的同名键不得误配,原样保留 */
    fp = fopen(path, "a");
    fprintf(fp, "[other]\nenabled = 7\napi_base = \"x\"\n");
    fclose(fp);
    c4.ai_enabled = 0;
    CHECK(lyy_config_save(path, &c4) == 0, "含 [other] 段保存");
    body = read_all(path);
    CHECK(body && strstr(body, "enabled = 7\n") &&
              strstr(body, "api_base = \"x\"\n") &&
              strstr(body, "enabled = false"),
          "[other] 段同名键保留,[ai] 段在位更新");
    g_free(body);

    /* 11. 缺失 [ai] 键自动建段补齐(追加在已有内容之后,不插队文件头) */
    fp = fopen(path, "w");
    fprintf(fp, "page_size = 6 # 手写注释\n");
    fclose(fp);
    LyyConfig c6;
    lyy_config_defaults(&c6);
    /* 模拟真实流程:先 load(填行尾注释缓存)再改 AI 字段保存 */
    CHECK(lyy_config_load(path, &c6) == 0 && c6.page_size == 6,
          "建段用例先读原文件");
    c6.ai_enabled = 1;
    snprintf(c6.ai_api_base, sizeof(c6.ai_api_base), "http://127.0.0.1:9/v1");
    snprintf(c6.ai_model, sizeof(c6.ai_model), "m1");
    CHECK(lyy_config_save(path, &c6) == 0, "无 [ai] 段保存");
    body = read_all(path);
    char *ai_hdr = body ? strstr(body, "[ai]") : NULL;
    CHECK(ai_hdr != NULL && strstr(ai_hdr, "enabled = true") &&
              strstr(ai_hdr, "api_base = \"http://127.0.0.1:9/v1\"") &&
              strstr(ai_hdr, "model = \"m1\"") &&
              ai_hdr > strstr(body, "page_size = 6"),
          "[ai] 段自动创建且位于文件尾、键齐全");
    CHECK(body && strstr(body, "page_size = 6 # 手写注释"),
          "已有行与注释原样保留");
    g_free(body);
    LyyConfig c7;
    lyy_config_load(path, &c7);
    CHECK(c7.ai_enabled == 1 && strcmp(c7.ai_model, "m1") == 0 &&
              c7.page_size == 6,
          "自动建段后回读一致");

    /* 12. 保存产物为合法 TOML 形态(行尾注释带 #,历史缺陷归一) */
    fp = fopen(path, "w");
    fprintf(fp, "page_size = 5 候选数 1..9\n"); /* 历史无 # 写法 */
    fclose(fp);
    LyyConfig c8;
    CHECK(lyy_config_load(path, &c8) == 0 && c8.page_size == 5,
          "历史无#注释行可读");
    CHECK(lyy_config_save(path, &c8) == 0, "归一保存");
    body = read_all(path);
    CHECK(body && strstr(body, "page_size = 5 # 候选数 1..9"),
          "写回行尾注释带 #(tomllib 兼容)");
    g_free(body);

    /* 13. 造词热键(coin_hotkey):默认值 / 加载 / 保存回读 / 段前插入 */
    CHECK(strcmp(c.coin_hotkey, "ctrl+equal") == 0, "coin_hotkey 默认 ctrl+equal");
    fp = fopen(path, "w");
    fprintf(fp, "[ai]\nmodel = \"m2\"\n"); /* 只有 [ai] 段的文件 */
    fclose(fp);
    LyyConfig c9;
    CHECK(lyy_config_load(path, &c9) == 0 &&
              strcmp(c9.coin_hotkey, "ctrl+equal") == 0,
          "缺 coin_hotkey 时用默认");
    snprintf(c9.coin_hotkey, sizeof(c9.coin_hotkey), "%s", "alt+comma");
    CHECK(lyy_config_save(path, &c9) == 0, "保存自定义造词热键");
    body = read_all(path);
    CHECK(body != NULL, "保存产物可读");
    /* 顶层键必须插到 [ai] 段头之前,否则下次读取会被当成段内键 */
    CHECK(body && strstr(body, "coin_hotkey = \"alt+comma\"") &&
              body < strstr(body, "coin_hotkey") &&
              strstr(body, "[ai]") > strstr(body, "coin_hotkey") &&
              strstr(body, "model = \"m2\""),
          "coin_hotkey 插到 [ai] 段头之前且段内容保留");
    g_free(body);
    LyyConfig c10;
    lyy_config_load(path, &c10);
    CHECK(strcmp(c10.coin_hotkey, "alt+comma") == 0 &&
              strcmp(c10.ai_model, "m2") == 0,
          "造词热键回读一致(不落段内)");
    /* 空字符串在钳制后回退默认 */
    snprintf(c10.coin_hotkey, sizeof(c10.coin_hotkey), "%s", "");
    CHECK(lyy_config_save(path, &c10) == 0, "保存空热键");
    LyyConfig c11;
    lyy_config_load(path, &c11);
    CHECK(strcmp(c11.coin_hotkey, "ctrl+equal") == 0,
          "空热键回退默认 ctrl+equal");

    /* 14. 截屏快捷键(shot_hotkey,合同 §13):默认值 / 保存回读 / 空串回退 */
    CHECK(strcmp(c.shot_hotkey, "ctrl+alt+a") == 0,
          "shot_hotkey 默认 ctrl+alt+a");
    snprintf(c10.shot_hotkey, sizeof(c10.shot_hotkey), "%s", "ctrl+shift+x");
    CHECK(lyy_config_save(path, &c10) == 0, "保存自定义截屏热键");
    LyyConfig c12;
    lyy_config_load(path, &c12);
    CHECK(strcmp(c12.shot_hotkey, "ctrl+shift+x") == 0,
          "截屏热键回读一致(不落段内)");
    snprintf(c12.shot_hotkey, sizeof(c12.shot_hotkey), "%s", "");
    CHECK(lyy_config_save(path, &c12) == 0, "保存空截屏热键");
    LyyConfig c13;
    lyy_config_load(path, &c13);
    CHECK(strcmp(c13.shot_hotkey, "ctrl+alt+a") == 0,
          "空截屏热键回退默认 ctrl+alt+a");

    /* 15. 热键冲突自动升级(合同 §13):两键占用同一组合 → 截屏热键
     * 按 原组合→+Alt→+Alt+Shift 逐级让位并写入人话说明 */
    char note[512];
    LyyConfig c14;
    lyy_config_defaults(&c14);
    CHECK(lyy_config_resolve_hotkey_conflicts(&c14, note, sizeof(note)) == 0 &&
              note[0] == '\0',
          "无冲突时不动配置");
    snprintf(c14.coin_hotkey, sizeof(c14.coin_hotkey), "%s", "ctrl+alt+a");
    /* 与截屏默认 ctrl+alt+a 同组合 → 截屏升一级(+Shift,Alt 已含) */
    CHECK(lyy_config_resolve_hotkey_conflicts(&c14, note, sizeof(note)) == 1 &&
              strcmp(c14.shot_hotkey, "ctrl+alt+shift+a") == 0 &&
              strstr(note, "ctrl+alt+shift+a") != NULL,
          "冲突时截屏热键逐级让位(+Shift)");
    /* 别名写法同样判冲突(Ctrl + = ≡ ctrl+equal)→ 截屏升一级(+Alt) */
    snprintf(c14.coin_hotkey, sizeof(c14.coin_hotkey), "%s", "Ctrl + =");
    snprintf(c14.shot_hotkey, sizeof(c14.shot_hotkey), "%s", "ctrl+equal");
    CHECK(lyy_config_resolve_hotkey_conflicts(&c14, note, sizeof(note)) == 1 &&
              strcmp(c14.shot_hotkey, "ctrl+alt+equal") == 0,
          "别名写法判冲突并升级(+Alt)");
    /* 顶格组合(ctrl+alt+shift)冲突无级可升:配置不变,返回 -1 */
    snprintf(c14.coin_hotkey, sizeof(c14.coin_hotkey), "%s",
             "ctrl+alt+shift+a");
    snprintf(c14.shot_hotkey, sizeof(c14.shot_hotkey), "%s",
             "ctrl+alt+shift+a");
    CHECK(lyy_config_resolve_hotkey_conflicts(&c14, note, sizeof(note)) ==
                  -1 &&
              strstr(note, "无法自动升级") != NULL,
          "阶梯用尽给出人话说明");
    /* 任一写法非法:不参与冲突(宿主按各自合同回退默认) */
    snprintf(c14.coin_hotkey, sizeof(c14.coin_hotkey), "%s", "a");
    CHECK(lyy_config_resolve_hotkey_conflicts(&c14, note, sizeof(note)) == 0,
          "非法写法不参与冲突");

    /* 16. 快速功能键(合同 §14):默认表 / [[quick_actions]] 解析 / 保存回读 */
    LyyConfig q0;
    lyy_config_defaults(&q0);
    CHECK(q0.quick_actions_enabled == 1 && q0.quick_actions_count == 4 &&
              strcmp(q0.quick_actions[0].trigger, "peizhi") == 0 &&
              strcmp(q0.quick_actions[0].label, "打开配置") == 0 &&
              strcmp(q0.quick_actions[0].command, "@settings") == 0 &&
              strcmp(q0.quick_actions[1].trigger, "shezhi") == 0 &&
              strcmp(q0.quick_actions[1].command, "@settings") == 0 &&
              strcmp(q0.quick_actions[2].trigger, "jietu") == 0 &&
              strcmp(q0.quick_actions[2].label, "截图") == 0 &&
              strcmp(q0.quick_actions[2].command, "@shot") == 0 &&
              strcmp(q0.quick_actions[3].trigger, "bangzhu") == 0 &&
              strcmp(q0.quick_actions[3].command, "@help") == 0,
          "快速功能键默认表(peizhi/shezhi/jietu/bangzhu)");

    /* 带 [[quick_actions]] 块的文件:解析出条目与开关 */
    FILE *fqa = fopen(path, "w");
    fprintf(fqa,
            "quick_actions_enabled = false\n"
            "[[quick_actions]]\n"
            "trigger = \"rizhi\"\n"
            "label = \"看日志\"\n"
            "command = \"tail -f xim.log\"\n"
            "[[quick_actions]]\n"
            "trigger = \"bad1\"\n"
            "label = \"非法\"\n"
            "command = \"x\"\n");
    fclose(fqa);
    LyyConfig q1;
    CHECK(lyy_config_load(path, &q1) == 0 && q1.quick_actions_enabled == 0,
          "[[quick_actions]] 文件解析:开关为 false");
    CHECK(q1.quick_actions_count == 1 &&
              strcmp(q1.quick_actions[0].trigger, "rizhi") == 0 &&
              strcmp(q1.quick_actions[0].label, "看日志") == 0 &&
              strcmp(q1.quick_actions[0].command, "tail -f xim.log") == 0,
          "非法触发词条目被剔除,合法条目保留");
    /* 全部条目非法 → 回退内置默认表 */
    FILE *fqa2 = fopen(path, "w");
    fprintf(fqa2, "[[quick_actions]]\ntrigger = \"BAD\"\nlabel = \"x\"\ncommand = \"y\"\n");
    fclose(fqa2);
    LyyConfig q2;
    lyy_config_load(path, &q2);
    CHECK(q2.quick_actions_count == 4 &&
              strcmp(q2.quick_actions[0].trigger, "peizhi") == 0 &&
              strcmp(q2.quick_actions[2].trigger, "jietu") == 0,
          "全部条目非法回退默认表(4 条)");

    /* 保存回读:[[quick_actions]] 块按当前表重写,块内键可再解析 */
    LyyConfig q3 = q0;
    snprintf(q3.quick_actions[0].command, sizeof(q3.quick_actions[0].command),
             "%s", "xfce4-terminal");
    CHECK(lyy_config_save(path, &q3) == 0, "保存含 [[quick_actions]] 的配置");
    char *qbody = read_all(path);
    CHECK(qbody && strstr(qbody, "[[quick_actions]]") &&
              strstr(qbody, "trigger = \"peizhi\"") &&
              strstr(qbody, "command = \"xfce4-terminal\""),
          "保存产物含 [[quick_actions]] 块");
    g_free(qbody);
    LyyConfig q4;
    CHECK(lyy_config_load(path, &q4) == 0 &&
              q4.quick_actions_count == 4 &&
              strcmp(q4.quick_actions[0].command, "xfce4-terminal") == 0 &&
              strcmp(q4.quick_actions[2].command, "@shot") == 0,
          "保存后回读一致");

    /* 17. 输入统计(stats_* 顶层键):默认值 / 解析 / 钳制 / 保存回读 */
    LyyConfig s0;
    lyy_config_defaults(&s0);
    CHECK(s0.stats_enabled == 1 && s0.stats_pause_secs == 10 &&
              s0.stats_idle_exclude_secs == 30,
          "统计三项默认值(开 / 10 / 30)");

    FILE *fst = fopen(path, "w");
    fprintf(fst,
            "page_size = 6\n"
            "stats_enabled = false\n"
            "stats_pause_secs = 42\n"
            "stats_idle_exclude_secs = 77\n"
            "# 用户手写注释\n");
    fclose(fst);
    LyyConfig s1;
    CHECK(lyy_config_load(path, &s1) == 0 && s1.stats_enabled == 0 &&
              s1.stats_pause_secs == 42 && s1.stats_idle_exclude_secs == 77 &&
              s1.page_size == 6,
          "统计三键解析(其余键不受影响)");

    /* 越界钳制回默认 */
    FILE *fst2 = fopen(path, "w");
    fprintf(fst2,
            "stats_enabled = true\n"
            "stats_pause_secs = 9999\n"
            "stats_idle_exclude_secs = 1\n"
            "# 用户手写注释\n");
    fclose(fst2);
    LyyConfig s2;
    lyy_config_load(path, &s2);
    CHECK(s2.stats_pause_secs == 10 && s2.stats_idle_exclude_secs == 30,
          "统计秒数越界钳制回默认");

    /* 保存回读:三键落盘为顶层键,未知行与注释保留 */
    LyyConfig s3 = s0;
    s3.stats_enabled = 0;
    s3.stats_pause_secs = 15;
    s3.stats_idle_exclude_secs = 45;
    CHECK(lyy_config_save(path, &s3) == 0, "保存含统计键的配置");
    char *sbody = read_all(path);
    CHECK(sbody && strstr(sbody, "stats_enabled = false") &&
              strstr(sbody, "stats_pause_secs = 15") &&
              strstr(sbody, "stats_idle_exclude_secs = 45") &&
              strstr(sbody, "# 用户手写注释"),
          "统计键原位更新且未知行保留");
    g_free(sbody);
    LyyConfig s4;
    CHECK(lyy_config_load(path, &s4) == 0 && s4.stats_enabled == 0 &&
              s4.stats_pause_secs == 15 && s4.stats_idle_exclude_secs == 45,
          "统计键保存后回读一致");

    /* 18. 英文上屏去向(enter_english / shift_english,§6):
     * 默认值 / 同义词归一 / 未知值回退 / 保存回读 */
    CHECK(strcmp(c.enter_english, "temp") == 0 &&
              strcmp(c.shift_english, "en") == 0,
          "英文上屏去向默认值(回车 temp / Shift en)");
    CHECK(strcmp(lyy_en_mode_canon("temporary", "en"), "temp") == 0 &&
              strcmp(lyy_en_mode_canon("english", "temp"), "en") == 0 &&
              strcmp(lyy_en_mode_canon("persist", "temp"), "en") == 0 &&
              strcmp(lyy_en_mode_canon("junk", "en"), "en") == 0,
          "取值归一:同义词识别,未知回退默认");
    FILE *fen = fopen(path, "w");
    fprintf(fen,
            "enter_english = \"en\"\n"
            "shift_english = \"temporary\"\n");
    fclose(fen);
    LyyConfig e1;
    CHECK(lyy_config_load(path, &e1) == 0 &&
              strcmp(e1.enter_english, "en") == 0 &&
              strcmp(e1.shift_english, "temp") == 0,
          "两键解析并归一(en/temporary)");
    FILE *fen2 = fopen(path, "w");
    fprintf(fen2, "enter_english = \"junk\"\nshift_english = \"\"\n");
    fclose(fen2);
    LyyConfig e2;
    CHECK(lyy_config_load(path, &e2) == 0 &&
              strcmp(e2.enter_english, "temp") == 0 &&
              strcmp(e2.shift_english, "en") == 0,
          "未知/空取值按各键默认回退");
    /* 保存回读:带引号字符串落盘,回读一致 */
    snprintf(e1.enter_english, sizeof(e1.enter_english), "%s", "en");
    snprintf(e1.shift_english, sizeof(e1.shift_english), "%s", "temp");
    CHECK(lyy_config_save(path, &e1) == 0, "保存英文上屏去向");
    char *ebody = read_all(path);
    CHECK(ebody && strstr(ebody, "enter_english = \"en\"") &&
              strstr(ebody, "shift_english = \"temp\""),
          "两键以带引号字符串写回");
    g_free(ebody);
    LyyConfig e3;
    CHECK(lyy_config_load(path, &e3) == 0 &&
              strcmp(e3.enter_english, "en") == 0 &&
              strcmp(e3.shift_english, "temp") == 0,
          "英文上屏去向保存后回读一致");

    /* 19. 精确单字按词频排位(exact_char_freq_rank,§5):
     * 默认开 / 文件解析 / 翻转落盘 / 回读一致 */
    FILE *ffr = fopen(path, "w");
    fprintf(ffr, "exact_char_freq_rank = false\n");
    fclose(ffr);
    LyyConfig f1;
    CHECK(lyy_config_load(path, &f1) == 0 && f1.exact_char_freq_rank == 0,
          "exact_char_freq_rank = false 可读");
    f1.exact_char_freq_rank = 1;
    CHECK(lyy_config_save(path, &f1) == 0, "翻转后保存成功");
    char *fbody = read_all(path);
    CHECK(fbody && strstr(fbody, "exact_char_freq_rank = true"),
          "翻转后写回 true");
    g_free(fbody);
    LyyConfig f2;
    CHECK(lyy_config_load(path, &f2) == 0 && f2.exact_char_freq_rank == 1,
          "exact_char_freq_rank 回读一致");

    /* 20. 菜单触发(menu_trigger_*):默认值 / 解析钳制 / 保存回读 /
     * 黑名单逐项精确匹配与未知 id 保留 */
    CHECK(c.menu_trigger_enabled == 1 && c.menu_trigger_key == 7 &&
              strcmp(c.menu_trigger_disabled, "fix_ime") == 0,
          "菜单触发默认值(开/F7/黑名单=fix_ime)");
    FILE *fmt = fopen(path, "w");
    fprintf(fmt,
            "menu_trigger_enabled = false\n"
            "menu_trigger_key = 8\n"
            "menu_trigger_disabled = \"help, future_entry\"\n"
            "# 手写注释保留\n");
    fclose(fmt);
    LyyConfig m1;
    CHECK(lyy_config_load(path, &m1) == 0 && m1.menu_trigger_enabled == 0 &&
              m1.menu_trigger_key == 8 &&
              strcmp(m1.menu_trigger_disabled, "help, future_entry") == 0,
          "菜单触发三键解析(含未知 id)");
    /* 键越界钳制回默认 7 */
    FILE *fmt2 = fopen(path, "w");
    fprintf(fmt2, "menu_trigger_key = 99\nmenu_trigger_key2 = 5\n");
    fclose(fmt2);
    LyyConfig m2;
    CHECK(lyy_config_load(path, &m2) == 0 && m2.menu_trigger_key == 7,
          "确认键越界钳制回 7(未知近名键不动)");
    /* 保存回读 + 注释保留 */
    m1.menu_trigger_enabled = 1;
    snprintf(m1.menu_trigger_disabled, sizeof(m1.menu_trigger_disabled),
             "%s", "settings, unknown_x");
    CHECK(lyy_config_save(path, &m1) == 0, "菜单触发配置保存");
    char *mbody = read_all(path);
    CHECK(mbody && strstr(mbody, "menu_trigger_enabled = true") &&
              strstr(mbody, "menu_trigger_key = 8") &&
              strstr(mbody, "menu_trigger_disabled = \"settings, unknown_x\"") &&
              strstr(mbody, "menu_trigger_key2 = 5"),
          "菜单触发键在位写回,未知键保留");
    g_free(mbody);
    LyyConfig m3;
    CHECK(lyy_config_load(path, &m3) == 0 && m3.menu_trigger_enabled == 1 &&
              m3.menu_trigger_key == 8 &&
              strcmp(m3.menu_trigger_disabled, "settings, unknown_x") == 0,
          "菜单触发保存后回读一致");

    /* csv 逐项精确匹配:token 整段相等,非子串 */
    CHECK(lyy_config_csv_contains("help,settings", "help") &&
              !lyy_config_csv_contains("help,settings", "hel") &&
              !lyy_config_csv_contains("help,settings", "helps") &&
              lyy_config_csv_contains(" a , b ", "b") &&
              !lyy_config_csv_contains("", "x") &&
              !lyy_config_csv_contains(NULL, "x"),
          "csv 精确匹配(含 trim/空/NULL 边界)");
    /* 黑名单合并:勾选项 + 未知 id 保留,目录项未勾选则移除 */
    char merged[256];
    CHECK(lyy_config_merge_menu_disabled(
              merged, sizeof(merged), "help, old_gone, fix_ime",
              "help,settings,fix_ime", "settings") == 0 &&
              strcmp(merged, "settings,old_gone") == 0,
          "黑名单合并:勾选入列,未知 id 保留,未勾目录项移除");
    CHECK(lyy_config_merge_menu_disabled(merged, sizeof(merged), "", "a,b",
                                         "b") == 0 &&
              strcmp(merged, "b") == 0,
          "黑名单合并:空基数");

    /* 未知 id 长 token(>127 字节)逐字节保留:不得静默截断 */
    char longtok[256];
    memset(longtok, 'x', 200);
    longtok[200] = '\0';
    {
        char base2[600];
        snprintf(base2, sizeof(base2), "help, %s", longtok);
        char merged2[LYY_CFG_STR_CMD];
        CHECK(lyy_config_merge_menu_disabled(merged2, sizeof(merged2), base2,
                                             "help,settings", "settings") ==
                      0 &&
                  strlen(merged2) == strlen("settings,") + 200 &&
                  strcmp(merged2 + strlen("settings,"), longtok) == 0,
              "黑名单合并:>127 字节未知 id 原样保留");
    }

    /* 合并结果超字段容量:整体失败,调用方中止保存(不静默丢安全开关) */
    {
        char base3[600];
        memset(base3, 'y', 520);
        base3[520] = '\0'; /* 单 token 已超 LYY_CFG_STR_CMD(512)容量 */
        char merged3[LYY_CFG_STR_CMD];
        CHECK(lyy_config_merge_menu_disabled(merged3, sizeof(merged3), base3,
                                             "a,b", "a") != 0,
              "黑名单合并:单 token 超容量整体失败");
        /* 部分失败回滚:out 不得留半截串 */
        CHECK(merged3[0] == '\0', "黑名单合并失败:输出不回留残串");
        /* 小 out 容量的多 token 溢出同判定 */
        char tiny[32];
        CHECK(lyy_config_merge_menu_disabled(
                  tiny, sizeof(tiny),
                  "abcdefghij,klmno,pqrst,uvwxy,zzzzz", "z", "z") != 0,
              "黑名单合并:多 token 累加溢出亦失败");
    }

    /* 21. 皮肤(skin 顶层键):默认 system / 有效 id 回读 / 旧配置缺键回退 /
     * 未知与空值回退 system / 补齐的顶层键落在 [ai]/[[quick_actions]] 之前 */
    CHECK(strcmp(c.skin, "system") == 0, "skin 默认 system");

    FILE *fsk = fopen(path, "w");
    fprintf(fsk, "skin = \"sakura\"\n");
    fclose(fsk);
    LyyConfig k1;
    CHECK(lyy_config_load(path, &k1) == 0 &&
              strcmp(k1.skin, "sakura") == 0,
          "skin = \"sakura\" 解析回读");

    FILE *fsk2 = fopen(path, "w");
    fprintf(fsk2, "page_size = 6\n"); /* 旧版配置:无 skin 键 */
    fclose(fsk2);
    LyyConfig k2;
    CHECK(lyy_config_load(path, &k2) == 0 && strcmp(k2.skin, "system") == 0 &&
              k2.page_size == 6,
          "旧配置缺 skin 回退 system");

    FILE *fsk3 = fopen(path, "w");
    fprintf(fsk3, "skin = \"no-such-skin\"\n");
    fclose(fsk3);
    LyyConfig k3;
    CHECK(lyy_config_load(path, &k3) == 0 &&
              strcmp(k3.skin, "system") == 0,
          "未知 skin id 回退 system");
    FILE *fsk4 = fopen(path, "w");
    fprintf(fsk4, "skin = \"\"\n");
    fclose(fsk4);
    LyyConfig k4;
    CHECK(lyy_config_load(path, &k4) == 0 && strcmp(k4.skin, "system") == 0,
          "空 skin 回退 system");

    /* skin 与 [ai]/[[quick_actions]] 同文件共存解析 */
    FILE *fsk5 = fopen(path, "w");
    fprintf(fsk5,
            "skin = \"business-navy\"\n"
            "[ai]\n"
            "model = \"m9\"\n"
            "[[quick_actions]]\n"
            "trigger = \"rizhi\"\n"
            "label = \"看日志\"\n"
            "command = \"x\"\n");
    fclose(fsk5);
    LyyConfig k5;
    CHECK(lyy_config_load(path, &k5) == 0 &&
              strcmp(k5.skin, "business-navy") == 0 &&
              k5.quick_actions_count == 1,
          "skin 与 [ai]/[[quick_actions]] 同文件解析");
    snprintf(k5.skin, sizeof(k5.skin), "%s", "sakura");
    CHECK(lyy_config_save(path, &k5) == 0, "skin 在位保存");
    char *skbody = read_all(path);
    CHECK(skbody && strstr(skbody, "skin = \"sakura\""),
          "skin 在位写回 sakura");
    g_free(skbody);
    LyyConfig k5b;
    CHECK(lyy_config_load(path, &k5b) == 0 &&
              strcmp(k5b.skin, "sakura") == 0,
          "skin 保存后回读一致(sakura)");

    /* 缺 skin 的文件:补齐的顶层键必须落在首个段头之前(否则下次读取
     * 会被并入段内失效);未知行原样保留 */
    FILE *fsk6 = fopen(path, "w");
    fprintf(fsk6,
            "page_size = 6\n"
            "weird_line_keep = 1\n"
            "[ai]\n"
            "model = \"m9\"\n"
            "[[quick_actions]]\n"
            "trigger = \"rizhi\"\n"
            "label = \"看日志\"\n"
            "command = \"x\"\n");
    fclose(fsk6);
    LyyConfig k6;
    CHECK(lyy_config_load(path, &k6) == 0, "缺 skin 文件读取");
    CHECK(lyy_config_save(path, &k6) == 0, "缺 skin 文件保存补齐");
    skbody = read_all(path);
    char *pskin = skbody ? strstr(skbody, "skin = \"system\"") : NULL;
    char *pai = skbody ? strstr(skbody, "[ai]") : NULL;
    char *pqa = skbody ? strstr(skbody, "[[quick_actions]]") : NULL;
    CHECK(pskin && pai && pqa && strstr(skbody, "weird_line_keep = 1") &&
              pskin < pai && pskin < pqa,
          "补齐 skin 落在 [ai]/[[quick_actions]] 之前,未知行保留");
    g_free(skbody);
    LyyConfig k7;
    CHECK(lyy_config_load(path, &k7) == 0 &&
              strcmp(k7.skin, "system") == 0 &&
              k7.quick_actions_count == 1 &&
              strcmp(k7.ai_model, "m9") == 0,
          "补齐后回读:skin=system,[[quick_actions]]/[ai] 不丢");

    /* 非法值兜底:写回时经注册表归一(程序内被改坏的值不原样落盘) */
    snprintf(k7.skin, sizeof(k7.skin), "%s", "corrupted-id");
    CHECK(lyy_config_save(path, &k7) == 0, "非法 skin 保存归一");
    skbody = read_all(path);
    CHECK(skbody && strstr(skbody, "skin = \"system\"") &&
              !strstr(skbody, "corrupted-id"),
          "非法 skin 写回时归一 system");
    g_free(skbody);

    /* 上屏后联想开关注入:文件缺省(空文件)读默认 0;显式 false 读 0;
     * 显式 true 读 1;翻转写回 true(布尔写回必经注册表归一,不得冻结旧值) */
    fp = fopen(path, "w");
    fclose(fp); /* 空文件:键缺失 */
    LyyConfig cp;
    lyy_config_defaults(&cp);
    CHECK(cp.next_word_prediction == 0, "联想默认关");
    CHECK(lyy_config_load(path, &cp) == 0 && cp.next_word_prediction == 0,
          "键缺失读到默认 0");
    fp = fopen(path, "w");
    fprintf(fp, "next_word_prediction = false\n");
    fclose(fp);
    lyy_config_defaults(&cp);
    CHECK(lyy_config_load(path, &cp) == 0 && cp.next_word_prediction == 0,
          "文件 false 读到 0");
    cp.next_word_prediction = 1;
    CHECK(lyy_config_save(path, &cp) == 0, "联想翻转后保存成功");
    body = read_all(path);
    CHECK(body && strstr(body, "next_word_prediction = true"),
          "联想 false→true 翻转写回");
    g_free(body);
    LyyConfig cp2;
    CHECK(lyy_config_load(path, &cp2) == 0 && cp2.next_word_prediction == 1,
          "联想回读为 1");

    /* 22. 中文标点(chinese_punct)与旧别名 cn_punct:默认开 /
     * 别名读取 / 规范键无论书写先后都优先 / 保存归一为一条规范键 */
    CHECK(c.chinese_punct == 1, "chinese_punct 默认开");
    fp = fopen(path, "w");
    fprintf(fp, "cn_punct = false\n");
    fclose(fp);
    LyyConfig p1;
    CHECK(lyy_config_load(path, &p1) == 0 && p1.chinese_punct == 0,
          "旧别名 cn_punct=false 读取生效");
    /* 两键并存任一书写顺序:规范键(设置窗写盘键)优先 */
    fp = fopen(path, "w");
    fprintf(fp, "cn_punct = true\nchinese_punct = false\n");
    fclose(fp);
    lyy_config_load(path, &p1);
    CHECK(p1.chinese_punct == 0, "别名在前:规范键 chinese_punct 仍优先");
    fp = fopen(path, "w");
    fprintf(fp, "chinese_punct = false\ncn_punct = true\n");
    fclose(fp);
    lyy_config_load(path, &p1);
    CHECK(p1.chinese_punct == 0, "规范键在前:其后的别名行被忽略");
    /* 保存归一:两种拼写收敛为一条规范键,别名行不再出现,其它键保留 */
    p1.chinese_punct = 1;
    p1.page_size = 8;
    CHECK(lyy_config_save(path, &p1) == 0, "标点键归一保存");
    body = read_all(path);
    char *pl = body ? strstr(body, "chinese_punct =") : NULL;
    CHECK(body && pl && !strstr(pl + 1, "chinese_punct =") &&
              !strstr(body, "cn_punct") && strstr(body, "page_size = 8"),
          "两种拼写收敛为一条规范键,其余键不受影响");
    g_free(body);
    LyyConfig p2;
    CHECK(lyy_config_load(path, &p2) == 0 && p2.chinese_punct == 1 &&
              p2.page_size == 8,
          "归一保存后回读一致");

    /* 23. 保留键 Ctrl+.(中英文标点切换):造词/截屏配成它时
     * 逐级让位(+alt → +alt+shift),两侧同占时互不冲突 */
    LyyConfig c15;
    lyy_config_defaults(&c15);
    snprintf(c15.coin_hotkey, sizeof(c15.coin_hotkey), "%s", "ctrl+period");
    CHECK(lyy_config_resolve_hotkey_conflicts(&c15, note, sizeof(note)) == 1 &&
              strcmp(c15.coin_hotkey, "ctrl+alt+period") == 0 &&
              strstr(note, "标点切换") != NULL,
          "造词占用保留键 → +alt 让位并说明");
    /* 两侧都配成保留键:各让一级且互不冲突 */
    lyy_config_defaults(&c15);
    snprintf(c15.coin_hotkey, sizeof(c15.coin_hotkey), "%s", "ctrl+period");
    snprintf(c15.shot_hotkey, sizeof(c15.shot_hotkey), "%s", "ctrl+period");
    CHECK(lyy_config_resolve_hotkey_conflicts(&c15, note, sizeof(note)) == 1 &&
              strcmp(c15.coin_hotkey, "ctrl+alt+period") == 0 &&
              strcmp(c15.shot_hotkey, "ctrl+alt+shift+period") == 0,
          "两侧同占保留键:分别让位且不互撞");
    /* 让位候选被另一侧已占组合挡住 → 升两级 */
    lyy_config_defaults(&c15);
    snprintf(c15.coin_hotkey, sizeof(c15.coin_hotkey), "%s", "ctrl+period");
    snprintf(c15.shot_hotkey, sizeof(c15.shot_hotkey), "%s",
             "ctrl+alt+period");
    CHECK(lyy_config_resolve_hotkey_conflicts(&c15, note, sizeof(note)) == 1 &&
              strcmp(c15.coin_hotkey, "ctrl+alt+shift+period") == 0 &&
              strcmp(c15.shot_hotkey, "ctrl+alt+period") == 0,
          "让位候选被占时升两级(避开另一侧)");
    /* 别名写法(Ctrl + .)同样判保留 */
    lyy_config_defaults(&c15);
    snprintf(c15.coin_hotkey, sizeof(c15.coin_hotkey), "%s", "Ctrl + .");
    CHECK(lyy_config_resolve_hotkey_conflicts(&c15, note, sizeof(note)) == 1 &&
              strcmp(c15.coin_hotkey, "ctrl+alt+period") == 0,
          "别名写法同样判保留并让位");

    CHECK(c.pinyin_only == 0, "pinyin_only 默认 0(五笔/拼音混输)");
    fp = fopen(path, "w");
    fprintf(fp,
            "pinyin_only = true\n"
            "page_size = 6\n"
            "# 方案行旁注释\n");
    fclose(fp);
    LyyConfig sc1;
    CHECK(lyy_config_load(path, &sc1) == 0 && sc1.pinyin_only == 1 &&
              sc1.page_size == 6,
          "pinyin_only = true 解析(其余键不受影响)");
    fp = fopen(path, "w");
    fprintf(fp, "pinyin_only = false\n");
    fclose(fp);
    CHECK(lyy_config_load(path, &sc1) == 0 && sc1.pinyin_only == 0,
          "pinyin_only = false 读取生效");
    sc1.pinyin_only = 1;
    sc1.page_size = 5;
    CHECK(lyy_config_save(path, &sc1) == 0, "方案切换保存成功");
    body = read_all(path);
    CHECK(body && strstr(body, "pinyin_only = true") &&
              strstr(body, "page_size = 5"),
          "方案翻转写回 true,其余键在位更新");
    g_free(body);
    LyyConfig sc2;
    CHECK(lyy_config_load(path, &sc2) == 0 && sc2.pinyin_only == 1 &&
              sc2.page_size == 5,
          "方案键保存后回读一致");
    fp = fopen(path, "w");
    fprintf(fp,
            "weird_line_keep = 1\n"
            "[ai]\n"
            "model = \"m9\"\n");
    fclose(fp);
    LyyConfig sc3;
    CHECK(lyy_config_load(path, &sc3) == 0 && sc3.pinyin_only == 0,
          "旧配置缺 pinyin_only 回退混输");
    CHECK(lyy_config_save(path, &sc3) == 0, "缺键文件保存补齐");
    body = read_all(path);
    char *ppo = body ? strstr(body, "pinyin_only = false") : NULL;
    char *pai2 = body ? strstr(body, "[ai]") : NULL;
    CHECK(ppo && pai2 && ppo < pai2 &&
              strstr(body, "weird_line_keep = 1"),
          "补齐 pinyin_only 落在 [ai] 段头之前,未知行保留");
    g_free(body);
    LyyConfig sc4;
    CHECK(lyy_config_load(path, &sc4) == 0 && sc4.pinyin_only == 0 &&
              strcmp(sc4.ai_model, "m9") == 0,
          "补齐后回读:方案=混输,[ai] 段不丢");
    printf("== 结果:%s(失败 %d 项)==\n", g_failed ? "有失败" : "全部通过",
           g_failed);

    /* 清理临时目录 */
    gchar *cmd = g_strdup_printf("rm -rf %s", tmp);
    g_spawn_command_line_sync(cmd, NULL, NULL, NULL, NULL);
    g_free(cmd);
    return g_failed ? 1 : 0;
}
