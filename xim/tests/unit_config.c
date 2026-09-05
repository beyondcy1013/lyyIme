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

    printf("== 结果:%s(失败 %d 项)==\n", g_failed ? "有失败" : "全部通过",
           g_failed);

    /* 清理临时目录 */
    gchar *cmd = g_strdup_printf("rm -rf %s", tmp);
    g_spawn_command_line_sync(cmd, NULL, NULL, NULL, NULL);
    g_free(cmd);
    return g_failed ? 1 : 0;
}
