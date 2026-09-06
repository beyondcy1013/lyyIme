#include "common.h"

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static App g_app; /* 单进程单实例,C 主线程持有 */

App *lyy_app(void)
{
    return &g_app;
}

int lyy_core_ready(App *app)
{
    return app->enabled && !app->degraded && app->core.loaded && app->engine;
}

int lyy_engine_ensure(App *app)
{
    if (!app->core.loaded || app->degraded)
        return -1;
    if (app->engine)
        return 0; /* 已在运行 */
    app->engine = app->core.lyyime_new(app->data_dir);
    if (!app->engine) {
        lyy_core_mark_degraded(app, "lyyime_new 返回 NULL(数据目录不可用?)");
        return -1;
    }
    app->core.lyyime_set_commit_after_four(app->engine, app->config.commit_after_four);
    lyy_log(&app->log, "core 引擎已创建:data_dir=%s", app->data_dir);
    return 0;
}

void lyy_engine_reload(App *app)
{
    if (!app->core.loaded || app->degraded) {
        lyy_log(&app->log, "重载词库跳过:core 未就绪(降级模式)");
        return;
    }
    if (app->engine) {
        app->core.lyyime_free(app->engine);
        app->engine = NULL;
    }
    if (lyy_engine_ensure(app) == 0) {
        lyy_log(&app->log, "词库/配置已重载(引擎重建)");
    }
}

void lyy_core_mark_degraded(App *app, const char *why)
{
    if (app->degraded)
        return;
    app->degraded = 1;
    lyy_log(&app->log, "WARN 进入降级直通模式:%s", why);
    lyy_log(&app->log,
            "修复指引:①用 scripts/build.sh 构建 lyyime-core 得到 "
            "liblyyime_core.so;②或设 LYYIME_CORE_LIB=/路径/liblyyime_core.so;"
            "③或安装到 /usr/local/lib/lyyime/ 后重启 lyyime-xim");
    lyy_app_update_mode_ui(app);
}

void lyy_request_show_settings(App *app)
{
    app->settings_requested = 1;
}

/* 让二次启动"唤起"已存在实例:SIGUSR1 → 主循环弹设置窗 */
static void on_signal(int sig)
{
    App *app = lyy_app();
    if (sig == SIGUSR1)
        app->settings_requested = 1; /* volatile 语义字段,仅置位 */
    else
        app->quit_requested = 1;
}

void lyy_install_signal_handlers(void)
{
    signal(SIGUSR1, on_signal);
    signal(SIGTERM, on_signal);
    signal(SIGINT, on_signal);
}
