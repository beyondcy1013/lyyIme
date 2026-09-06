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
    CHECK(c.page_size == 5 && c.mixed_english == 1 &&
              c.commit_after_four == 0 && c.font_size == 14 &&
              c.autostart == 0,
          "默认值正确");

    /* 2. 加载不存在的文件 */
    int rc = lyy_config_load(path, &c);
    CHECK(rc == 1, "文件不存在返回 1 并用默认值");

    /* 3. 保存生成文件,含全部管理键与中文注释 */
    c.page_size = 7;
    c.autostart = 1;
    c.commit_after_four = 1;
    CHECK(lyy_config_save(path, &c) == 0, "保存成功");
    char *body = read_all(path);
    CHECK(body && strstr(body, "page_size = 7") && strstr(body, "候选数"),
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
              c2.mixed_english == 1,
          "回读一致");
    g_free(body);
    fp = fopen(path, "w");
    fprintf(fp, "page_size = 99\nfont_size = 3\n");
    fclose(fp);
    lyy_config_load(path, &c2);
    CHECK(c2.page_size == 5 && c2.font_size == 14, "越界值钳制回默认");

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

    printf("== 结果:%s(失败 %d 项)==\n", g_failed ? "有失败" : "全部通过",
           g_failed);

    /* 清理临时目录 */
    gchar *cmd = g_strdup_printf("rm -rf %s", tmp);
    g_spawn_command_line_sync(cmd, NULL, NULL, NULL, NULL);
    g_free(cmd);
    return g_failed ? 1 : 0;
}
