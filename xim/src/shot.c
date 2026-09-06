#include "shot.h"

#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* ---- 资源解析:助手解析顺序(env → 安装位 → exe 同级 → PATH 兜底) ----
 * 与 ai_capture.c lyy_ai_find_helper 同思路;E2E 经 $LYYIME_SHOT 注入桩程序。 */
static int exe_sibling_shot(char *out, size_t cap)
{
    char buf[1024];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
    if (n <= 0)
        return -1;
    buf[n] = '\0';
    char *slash = strrchr(buf, '/');
    if (!slash)
        return -1;
    size_t dirlen = (size_t)(slash - buf) + 1; /* 含目录斜杠 */
    size_t rest = strlen("lyyime-shot");
    if (dirlen + rest + 1 > cap)
        return -1; /* 截断风险:直接放弃该候选路径 */
    memcpy(out, buf, dirlen);
    memcpy(out + dirlen, "lyyime-shot", rest + 1);
    return 0;
}

void lyy_spawn_shot(App *app)
{
    static const char *kInstalled = "/usr/local/bin/lyyime-shot";
    char sibling[1024];
    const char *use = NULL;
    char chosen[1024];

    const char *env = g_getenv("LYYIME_SHOT");
    if (env && env[0] && g_file_test(env, G_FILE_TEST_EXISTS)) {
        snprintf(chosen, sizeof(chosen), "%s", env);
        use = chosen;
    } else if (g_file_test(kInstalled, G_FILE_TEST_EXISTS)) {
        use = kInstalled;
    } else if (exe_sibling_shot(sibling, sizeof(sibling)) == 0 &&
               g_file_test(sibling, G_FILE_TEST_EXISTS)) {
        use = sibling;
    }

    char *argv[2];
    argv[1] = NULL;
    GSpawnFlags flags = G_SPAWN_STDOUT_TO_DEV_NULL | G_SPAWN_STDERR_TO_DEV_NULL;
    if (use) {
        argv[0] = (gchar *)use;
    } else {
        /* 无确切路径:按名字走 PATH(g_spawn 搜索语义);仍失败则提示安装 */
        argv[0] = (gchar *)"lyyime-shot";
        flags |= G_SPAWN_SEARCH_PATH;
    }

    GError *err = NULL;
    if (!g_spawn_async(NULL, argv, NULL, flags, NULL, NULL, NULL, &err)) {
        lyy_log(&app->log, "ERROR 拉起截屏助手失败(%s):%s", argv[0],
                err ? err->message : "?");
        lyy_show_notice(app,
                        "截屏启动失败:未找到 lyyime-shot(scripts/install-all.sh 可安装)");
        lyy_log(&app->log,
                "截屏助手缺失提示已展示(候选条 4 秒)");
        g_clear_error(&err);
        return;
    }
    /* 子进程回收:未用 DO_NOT_REAP_CHILD,GLib 自动收割,无僵尸 */
    lyy_log(&app->log, "已拉起截屏助手:%s", argv[0]);
}
