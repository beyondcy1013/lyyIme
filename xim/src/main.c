/*
 * lyyime-xim 入口(Mode B 独立外挂,docs/ARCHITECTURE.md §8)
 *
 * 职责:
 *   - CLI(--help/--version/--settings)与"人话"错误提示;
 *   - 单实例(pidfile ~/.local/share/lyyime/xim.pid;二次启动 SIGUSR1 唤起
 *     已存在实例弹出设置窗后自身退出);
 *   - 信号:SIGTERM/SIGINT 优雅退出(关 XIM server、删 pidfile);
 *   - 装配:日志 → 配置 → core FFI(dlopen)→ XIM server → 候选窗 → 托盘
 *     → 设置窗 → GLib 主循环(xcb fd 融入,单线程,满足 §3 线程约定)。
 */
#include <glib.h>
#include <glib/gstdio.h>
#include <locale.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>
#include <xcb/xcb.h>
#include <xcb/xcb_aux.h>

#include "common.h"

static void usage(FILE *out)
{
    fprintf(out,
            "%s — lyyIme 独立输入法外挂(Mode B,XIM server)\n"
            "用法: lyyime-xim [选项]\n"
            "  --settings   显示设置窗口(若已在运行则唤起已存在实例)\n"
            "  --version    显示版本\n"
            "  --help       显示本帮助\n\n"
            "环境变量:\n"
            "  LYYIME_CORE_LIB  指定 liblyyime_core.so 路径(默认按内置顺序搜索)\n"
            "  LYYIME_RES_DIR   指定资源目录(图标/设置界面/候选窗样式)\n\n"
            "接入方式:应用环境变量 XMODIFIERS=@im=%s + GTK_IM_MODULE=xim\n",
            LYY_APP_NAME, LYY_XIM_SERVER_NAME);
}

/* ---- 单实例:pidfile 存活检查 + 唤起 ---- */
static int pidfile_alive(const char *path, long *pid_out)
{
    FILE *fp = fopen(path, "r");
    if (!fp)
        return 0;
    long pid = 0;
    int ok = fscanf(fp, "%ld", &pid) == 1;
    fclose(fp);
    if (!ok || pid <= 0)
        return 0;
    if (kill((pid_t)pid, 0) != 0)
        return 0; /* 进程不存在(残留 pidfile) */
    *pid_out = pid;
    return 1;
}

static void write_pidfile(const char *path)
{
    FILE *fp = fopen(path, "w");
    if (fp) {
        fprintf(fp, "%ld\n", (long)getpid());
        fclose(fp);
    }
}

/* 主循环 200ms 轮询一次信号置位(Signal→GTK 单线程桥) */
static gboolean on_flags_tick(gpointer user_data)
{
    App *app = user_data;
    if (app->settings_requested) {
        app->settings_requested = 0;
        lyy_settings_show(&app->settings);
    }
    if (app->quit_requested) {
        g_main_loop_quit(app->loop);
    }
    return G_SOURCE_CONTINUE;
}

/* 词典目录回退链,与 ibus 引擎 lyyime_ffi.resolve_data_dir 保持一致:
 * $LYYIME_DATA_DIR → 用户级 lyyime/data(含 meta.json)→
 * /usr/local/share/lyyime/data → ~/.local/share/lyyime(兜底) */
static void resolve_dict_dir(char *out, size_t cap, const char *user_data_dir)
{
    const char *env = g_getenv("LYYIME_DATA_DIR");
    if (env && *env) {
        snprintf(out, cap, "%s", env);
        return;
    }
    char cand[1024];
    const char *xdg = g_getenv("XDG_DATA_HOME");
    if (xdg && *xdg) {
        snprintf(cand, sizeof(cand), "%s/lyyime/data/meta.json", xdg);
        if (g_file_test(cand, G_FILE_TEST_EXISTS)) {
            snprintf(cand, sizeof(cand), "%s/lyyime/data", xdg);
            snprintf(out, cap, "%s", cand);
            return;
        }
    }
    snprintf(cand, sizeof(cand), "%s/lyyime/data/meta.json", user_data_dir);
    if (g_file_test(cand, G_FILE_TEST_EXISTS)) {
        snprintf(cand, sizeof(cand), "%s/lyyime/data", user_data_dir);
        snprintf(out, cap, "%s", cand);
        return;
    }
    if (g_file_test("/usr/local/share/lyyime/data/meta.json",
                    G_FILE_TEST_EXISTS)) {
        snprintf(out, cap, "%s", "/usr/local/share/lyyime/data");
        return;
    }
    snprintf(out, cap, "%s/lyyime", user_data_dir);
}

int main(int argc, char *argv[])
{
    App *app = lyy_app();
    lyy_ai_init(&app->ai); /* /AI 触发会话资源(任何路径退出统一 clear) */

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--help") || !strcmp(argv[i], "-h")) {
            usage(stdout);
            return 0;
        }
        if (!strcmp(argv[i], "--version")) {
            printf("%s %s\n", LYY_APP_NAME, LYY_APP_VERSION);
            return 0;
        }
        if (!strcmp(argv[i], "--settings")) {
            app->settings_requested = 2; /* 启动即弹设置(或唤起已有实例) */
            continue;
        }
        fprintf(stderr, "未知参数:%s(见 --help)\n", argv[i]);
        return 2;
    }

    /* GTK/XIM 需要 locale(GTK 界面与 COMPOUND_TEXT 客户端转码) */
    setlocale(LC_ALL, "");

    if (!getenv("DISPLAY")) {
        fprintf(stderr,
                "错误:未检测到 X 显示环境(变量 DISPLAY 未设置)。\n"
                "lyyime-xim 是 X11 输入法外挂,请在图形会话中启动,"
                "或先 export DISPLAY=:0\n");
        return 1;
    }

    /* 目录与文件布局(XDG):配置/数据/日志 */
    const char *cfg_dir = g_get_user_config_dir();
    const char *data_dir = g_get_user_data_dir();
    snprintf(app->config_path, sizeof(app->config_path),
             "%s/lyyime/config.toml", cfg_dir);
    /* 词典目录回退链(与 ibus 引擎 lyyime_ffi.resolve_data_dir 一致):
     * $LYYIME_DATA_DIR → 用户级 data/(含 meta.json)→ /usr/local/share/lyyime/data
     * → ~/.local/share/lyyime(用户词/历史行为兜底;core 对缺失词库降级为空引擎) */
    resolve_dict_dir(app->data_dir, sizeof(app->data_dir), data_dir);
    char pidfile[1024], res_dir_default[1024];
    snprintf(pidfile, sizeof(pidfile), "%s/lyyime/xim.pid", data_dir);
    g_mkdir_with_parents(app->data_dir, 0755);

    /* 日志先行(排障红线) */
    char log_path[1024];
    snprintf(log_path, sizeof(log_path), "%s/lyyime/logs", data_dir);
    g_mkdir_with_parents(log_path, 0755);
    snprintf(log_path, sizeof(log_path), "%s/lyyime/logs/xim.log", data_dir);
    lyy_log_open(&app->log, log_path);
    lyy_log(&app->log, "==== %s %s 启动(pid=%ld) ====", LYY_APP_NAME,
            LYY_APP_VERSION, (long)getpid());

    /* 单实例:已存在实例 → SIGUSR1 唤起(弹设置窗),自身退出 */
    long alive_pid = 0;
    if (pidfile_alive(pidfile, &alive_pid)) {
        lyy_log(&app->log, "已存在实例(pid=%ld),发送唤起信号后退出", alive_pid);
        kill((pid_t)alive_pid, SIGUSR1);
        fprintf(stderr, "lyyime-xim 已在运行(pid=%ld),已唤起其设置窗口。\n",
                alive_pid);
        lyy_log_close(&app->log);
        return 0;
    }
    write_pidfile(pidfile);

    /* 配置(共享 config.toml,保留未知行与注释) */
    if (lyy_config_load(app->config_path, &app->config) != 0)
        lyy_log(&app->log, "WARN 配置读取失败,使用默认值:%s", app->config_path);
    /* 热键冲突自动升级(合同 §13):加载即自愈,截屏热键让位并留痕日志 */
    {
        char hk_note[512];
        if (lyy_config_resolve_hotkey_conflicts(&app->config, hk_note,
                                                sizeof(hk_note)) != 0)
            lyy_log(&app->log, "%s", hk_note);
    }
    /* 造词/截屏热键解析(设置保存后由 settings.c 再次刷新) */
    lyy_app_reload_hotkey(app);

    /* GTK 初始化(候选窗/托盘/设置窗依赖) */
    gtk_init(&argc, &argv);

    /* core FFI:失败不退出,进入降级直通并给出修复指引 */
    if (lyy_core_ffi_load(&app->core) != 0) {
        lyy_log(&app->log,
                "ERROR 未找到可用的 liblyyime_core.so。\n"
                "  修复指引:\n"
                "  ① 在项目根执行 scripts/build.sh 构建 lyyime-core;\n"
                "  ② 或设置环境变量 LYYIME_CORE_LIB=/路径/liblyyime_core.so;\n"
                "  ③ 或安装到 /usr/local/lib/lyyime/liblyyime_core.so。\n"
                "  在 core 就绪前,lyyime-xim 以直通模式运行(按键原样送达应用)。");
        app->degraded = 1;
    }
    if (app->core.loaded)
        lyy_log(&app->log, "core 库已加载:%s", app->core.lib_path);

    /* 资源目录(图标/设置 ui/候选窗 css):env → 安装位 → 源码树(编译期注入) */
    const char *res_env = getenv("LYYIME_RES_DIR");
    if (res_env && res_env[0]) {
        snprintf(res_dir_default, sizeof(res_dir_default), "%s", res_env);
    } else if (g_file_test("/usr/local/share/lyyime/icons",
                           G_FILE_TEST_EXISTS)) {
        snprintf(res_dir_default, sizeof(res_dir_default),
                 "/usr/local/share/lyyime/res");
    } else {
        snprintf(res_dir_default, sizeof(res_dir_default), "%s",
                 LYY_SRC_RES_DIR);
    }

    /* XIM server(先起:候选窗初始化需要 xcb 连接与根窗口) */
    app->enabled = 1;
    if (lyy_xim_init(&app->xim, app) != 0) {
        fprintf(stderr,
                "错误:XIM server 启动失败(同名 server 已在运行?)。\n"
                "排查:pgrep -af lyyime-xim;日志:%s\n",
                app->log.path);
        lyy_log_close(&app->log);
        g_unlink(pidfile);
        return 1;
    }

    /* core 引擎实例 */
    lyy_engine_ensure(app);

    /* 候选窗 / 托盘 / 设置窗 */
    {
        xcb_screen_t *screen =
            xcb_aux_get_screen(app->xim.conn, app->xim.screen_no);
        lyy_candwin_init(&app->candwin, app->xim.conn, screen->root,
                         res_dir_default, app->config.font_size);
        /* 候选窗行点击(§14 鼠标点选):桥到 core select_candidate */
        app->candwin.on_click = lyy_candwin_row_clicked;
        app->candwin.click_user_data = app;
    }
    {
        char icon_dir[1024];
        snprintf(icon_dir, sizeof(icon_dir), "%s", res_dir_default);
        lyy_tray_init(&app->tray, icon_dir);
    }
    lyy_settings_init(&app->settings, res_dir_default);
    lyy_app_update_mode_ui(app);

    /* 信号 + 唤起处理 */
    lyy_install_signal_handlers();
    GMainLoop *loop = g_main_loop_new(NULL, FALSE);
    app->loop = loop;
    g_timeout_add(200, on_flags_tick, app);

    lyy_log(&app->log, "初始化完成,进入主循环(降级=%d)", app->degraded);
    if (app->degraded)
        fprintf(stderr,
                "警告:liblyyime_core.so 未就绪,当前为直通模式(可收键不上字)。"
                "详见 %s\n", app->log.path);
    if (app->settings_requested == 2)
        app->settings_requested = 1; /* 启动即弹设置 */

    g_main_loop_run(loop);

    /* 优雅退出:关 server → 释放引擎 → 清 pidfile */
    lyy_log(&app->log, "退出:清理 XIM server 与引擎");
    lyy_xim_shutdown(&app->xim);
    if (app->engine && app->core.loaded)
        app->core.lyyime_free(app->engine);
    lyy_ai_clear(&app->ai);
    g_unlink(pidfile);
    lyy_log(&app->log, "==== 退出完成 ====");
    lyy_log_close(&app->log);
    return 0;
}
