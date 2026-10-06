/*
 * lyyime-xim 入口(Mode B 独立外挂,docs/ARCHITECTURE.md §8)
 *
 * 职责:
 *   - CLI(--help/--version/--settings/--mainwin)与"人话"错误提示;
 *   - 单实例(pidfile ~/.local/share/lyyime/xim.pid;二次启动 SIGUSR1 弹
 *     设置窗 / SIGUSR2 弹主窗口后自身退出);
 *   - 信号:SIGTERM/SIGINT 优雅退出(关 XIM server、删 pidfile);
 *   - 装配:日志 → 配置 → core FFI(dlopen)→ XIM server → 候选窗 → 托盘
 *     → 设置窗 → 主窗口 → GLib 主循环(xcb fd 融入,单线程,满足 §3 线程约定)。
 */
#include <ctype.h>
#include <errno.h>
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
#ifdef __linux__
#include <sys/mman.h>
#include <sys/resource.h>
#endif
#include <xcb/xcb.h>
#include <xcb/xcb_aux.h>

#include "common.h"

/* 对输入法进程实施内存锁定，防止在高 I/O 和内存压力下被换出到 Swap 造成按键卡顿 */
static void lock_process_memory(LyyLog *log)
{
#ifdef __linux__
    struct rlimit rlim = {
        .rlim_cur = RLIM_INFINITY,
        .rlim_max = RLIM_INFINITY,
    };
    (void)setrlimit(RLIMIT_MEMLOCK, &rlim);
    if (mlockall(MCL_CURRENT | MCL_FUTURE) == 0) {
        lyy_log(log, "mlockall 内存锁定成功：已防止进程换出到 Swap");
    } else {
        lyy_log(log, "WARN mlockall 内存锁定未生效(errno=%d): 输入法在极高内存压力下可能被置换到 Swap", errno);
    }
#else
    (void)log;
#endif
}

static void candwin_general_append(GtkMenuShell *shell, void *user_data)
{
    lyy_general_menu_append((App *)user_data, shell);
}
static void candwin_settings_show(void *user_data)
{
    lyy_settings_show(&((App *)user_data)->settings);
}
static void usage(FILE *out)
{
    fprintf(out,
            "%s — lyyIme 独立输入法外挂(Mode B,XIM server)\n"
            "用法: lyyime-xim [选项]\n"
            "  --settings   显示设置窗口(若已在运行则唤起已存在实例)\n"
            "  --settings-page N\n"
            "               显示设置窗口并切到第 N 页(0 基;供菜单触发等\n"
            "               内置路径直达子页)\n"
            "  --mainwin    显示主窗口(门面:输入设置/直输模式/工具箱)\n"
            "  --sync-shot-hotkey\n"
            "               按当前 config.toml 的 shot_hotkey 登记 XFCE\n"
            "               全局截屏快捷键(安装/修复用;不启动输入法)\n"
            "  --remove-shot-hotkey\n"
            "               移除已登记的全局截屏快捷键(卸载用)\n"
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

/* --settings-page 页码解析:严格 strtol,合法范围 0..6(设置窗 7 页);
 * 尾部允许空白(请求文件是 "N\n" 文本);非法/缺值/越界返回 -1。 */
static int parse_settings_page(const char *s, int *out)
{
    if (!s)
        return -1;
    char *end = NULL;
    errno = 0;
    long v = strtol(s, &end, 10);
    if (errno != 0 || end == s)
        return -1;
    while (*end && isspace((unsigned char)*end))
        end++;
    if (*end != '\0' || v < 0 || v > 6)
        return -1;
    *out = (int)v;
    return 0;
}

/* 主循环 200ms 轮询一次信号置位(Signal→GTK 单线程桥) */
static gboolean on_flags_tick(gpointer user_data)
{
    App *app = user_data;
    if (app->settings_requested) {
        app->settings_requested = 0;
        int page = app->settings_page;
        app->settings_page = -1;
        /* 唤起路径页码经请求文件传递(SIGUSR1 不带参数):仅本进程未自带
         * 页码时才采纳文件值,防遗留文件盖掉本次启动 --settings-page;
         * 文件无论采纳与否都清掉 */
        if (app->settings_page_req[0]) {
            if (page < 0) {
                gchar *txt = NULL;
                if (g_file_get_contents(app->settings_page_req, &txt, NULL,
                                        NULL)) {
                    int p = -1;
                    if (parse_settings_page(txt, &p) == 0)
                        page = p;
                    g_free(txt);
                }
            }
            g_unlink(app->settings_page_req);
        }
        lyy_settings_show_page(&app->settings, page);
    }
    if (app->mainwin_requested) {
        app->mainwin_requested = 0;
        lyy_mainwin_show(&app->mainwin);
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

/* --sync-shot-hotkey / --remove-shot-hotkey:安装期全局截屏快捷键登记与
 * 移除(合同 §13)。在 pidfile/单实例/日志之前处理:绝不向已运行实例发
 * 信号、不启动输入法引擎。成功打印到 stdout,失败 stderr + 非零退出。 */
static int cli_shot_hotkey(App *app, int do_sync)
{
    if (!do_sync) {
        /* 移除只动自有绑定,不需要读配置 */
        GError *err = NULL;
        if (lyy_global_shot_remove(&err)) {
            printf("已移除全局截屏快捷键绑定。\n");
            return 0;
        }
        fprintf(stderr, "移除全局截屏快捷键失败:%s\n",
                err ? err->message : "未知错误");
        g_clear_error(&err);
        return 1;
    }

    /* sync:读配置(含热键冲突自愈)→ GTK/X11 环境 → 事务化登记 */
    LyyConfig c;
    if (lyy_config_load(app->config_path, &c) != 0)
        fprintf(stderr, "警告:配置读取失败,使用默认值:%s\n",
                app->config_path);
    {
        char note[512];
        if (lyy_config_resolve_hotkey_conflicts(&c, note,
                                                sizeof(note)) != 0)
            fprintf(stderr, "%s\n", note);
    }
    if (!gtk_init_check(NULL, NULL)) {
        fprintf(stderr,
                "错误:无法连接 X 显示(GTK 初始化失败),未登记全局快捷键\n");
        return 1;
    }
    GError *err = NULL;
    LyyGlobalHotkeyChange *change =
        lyy_global_shot_apply(c.shot_hotkey, &err);
    if (!change) {
        fprintf(stderr, "登记全局截屏快捷键失败:%s\n",
                err ? err->message : "未知错误");
        g_clear_error(&err);
        return 1;
    }
    lyy_global_shot_commit(change);
    printf("全局截屏快捷键已登记:%s → /usr/local/bin/lyyime-shot\n",
           c.shot_hotkey);
    return 0;
}

int main(int argc, char *argv[])
{
    App *app = lyy_app();
    app->settings_page = -1; /* -1 = 未指定子页 */
    lyy_ai_init(&app->ai); /* /AI 触发会话资源(任何路径退出统一 clear) */
    int shot_cli = 0;      /* 0=常规启动 1=--sync-shot-hotkey 2=--remove */
    int shot_cli_dup = 0;  /* sync/remove 同时出现=参数冲突 */

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
        if (!strcmp(argv[i], "--settings-page") ||
            !strncmp(argv[i], "--settings-page=", 16)) {
            const char *val = NULL;
            if (argv[i][15] == '=') {
                val = argv[i] + 16;
            } else if (i + 1 < argc) {
                val = argv[++i];
            }
            int page = -1;
            if (!val || parse_settings_page(val, &page) != 0) {
                fprintf(stderr,
                        "--settings-page 需要合法页码(0..6,0 基)\n");
                return 2;
            }
            app->settings_page = page;
            app->settings_requested = 2;
            continue;
        }
        if (!strcmp(argv[i], "--mainwin")) {
            app->mainwin_requested = 2; /* 启动即弹主窗口(或唤起已有实例) */
            continue;
        }
        if (!strcmp(argv[i], "--sync-shot-hotkey")) {
            if (shot_cli)
                shot_cli_dup = 1;
            shot_cli = 1;
            continue;
        }
        if (!strcmp(argv[i], "--remove-shot-hotkey")) {
            if (shot_cli)
                shot_cli_dup = 1;
            shot_cli = 2;
            continue;
        }
        fprintf(stderr, "未知参数:%s(见 --help)\n", argv[i]);
        return 2;
    }
    /* 全局快捷键登记/移除是独立维护命令:互斥且不与窗口唤起混用 */
    if (shot_cli_dup) {
        fprintf(stderr,
                "--sync-shot-hotkey 与 --remove-shot-hotkey 互斥\n");
        return 2;
    }
    if (shot_cli && (app->settings_requested || app->mainwin_requested)) {
        fprintf(stderr,
                "--%s 不能与 --settings/--mainwin 同时使用\n",
                shot_cli == 1 ? "sync-shot-hotkey" : "remove-shot-hotkey");
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
    /* --settings-page 唤起已存在实例的页码请求文件(信号不带参数) */
    snprintf(app->settings_page_req, sizeof(app->settings_page_req),
             "%s/lyyime/settings-page.req", data_dir);
    g_mkdir_with_parents(app->data_dir, 0755);

    /* 全局截屏快捷键维护命令:配置路径就绪后、日志/pidfile/单实例之前
     * 处理——装/卸操作绝不向已运行实例发信号、不拉起输入法引擎 */
    if (shot_cli)
        return cli_shot_hotkey(app, shot_cli == 1);

    /* 日志先行(排障红线) */
    char log_path[1024];
    snprintf(log_path, sizeof(log_path), "%s/lyyime/logs", data_dir);
    g_mkdir_with_parents(log_path, 0755);
    snprintf(log_path, sizeof(log_path), "%s/lyyime/logs/xim.log", data_dir);
    lyy_log_open(&app->log, log_path);
    lyy_log(&app->log, "==== %s %s 启动(pid=%ld) ====", LYY_APP_NAME,
            LYY_APP_VERSION, (long)getpid());

    /* 单实例:已存在实例 → 唤起信号(--mainwin 走 SIGUSR2 弹主窗口,
     * 其余 SIGUSR1 弹设置窗),自身退出 */
    long alive_pid = 0;
    if (pidfile_alive(pidfile, &alive_pid)) {
        int want_mainwin = app->mainwin_requested == 2;
        lyy_log(&app->log, "已存在实例(pid=%ld),发送唤起信号后退出", alive_pid);
        /* 设置唤起(含 --settings 与 --settings-page):信号不带参数,页码
         * 经请求文件**原子**传递;普通 --settings 写 -1 显式清掉可能遗留
         * 的旧页码。写失败不发信号,避免向用户假报成功。 */
        if (!want_mainwin) {
            char req[16];
            snprintf(req, sizeof(req), "%d\n", app->settings_page);
            if (!g_file_set_contents(app->settings_page_req, req, -1, NULL)) {
                fprintf(stderr, "无法写入设置页请求文件:%s\n",
                        app->settings_page_req);
                lyy_log(&app->log,
                        "ERROR 设置页请求文件写入失败,唤起信号未发");
                lyy_log_close(&app->log);
                return 1;
            }
        }
        kill((pid_t)alive_pid, want_mainwin ? SIGUSR2 : SIGUSR1);
        fprintf(stderr, "lyyime-xim 已在运行(pid=%ld),已唤起其%s窗口。\n",
                alive_pid, want_mainwin ? "主" : "设置");
        lyy_log_close(&app->log);
        return 0;
    }
    write_pidfile(pidfile);
    lock_process_memory(&app->log);

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

    /* 全局截屏快捷键(合同 §13):经 XFCE xfconf 登记/校正桌面级绑定,
     * 对所有输入法与无输入法环境生效;失败仅告警,不阻断打字服务 */
    {
        GError *gh_err = NULL;
        LyyGlobalHotkeyChange *ghc =
            lyy_global_shot_apply(app->config.shot_hotkey, &gh_err);
        if (ghc) {
            lyy_global_shot_commit(ghc);
            lyy_log(&app->log, "全局截屏快捷键已登记:%s",
                    app->config.shot_hotkey);
        } else {
            lyy_log(&app->log, "WARN 全局截屏快捷键登记失败:%s",
                    gh_err ? gh_err->message : "未知错误");
            g_clear_error(&gh_err);
        }
    }

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

    /* 候选窗 / 托盘 / 设置窗 / 主窗口 */
    {
        xcb_screen_t *screen =
            xcb_aux_get_screen(app->xim.conn, app->xim.screen_no);
        lyy_candwin_init(&app->candwin, app->xim.conn, screen->root,
                         res_dir_default, app->config.font_size);
        lyy_candwin_set_skin(&app->candwin, app->config.skin);
        /* 候选窗行点击(§14 鼠标点选):桥到 core select_candidate;
         * 右键菜单(§15):状态查询 + 操作回调桥到 core cand_op */
        app->candwin.on_click = lyy_candwin_row_clicked;
        app->candwin.click_user_data = app;
        lyy_candwin_set_op_fns(&app->candwin, lyy_candwin_op_state,
                               lyy_candwin_op, app);
        lyy_candwin_set_general_fn(&app->candwin, candwin_general_append,
                                   app);
        /* 表头齿轮左键 → 本进程设置窗口(不另起 --settings 进程) */
        lyy_candwin_set_settings_fn(&app->candwin, candwin_settings_show,
                                    app);
        /* §15 自定义查询(菜单第 4 项):启动注入;设置保存后 settings.c
         * 再调一次即时生效 */
        lyy_candwin_set_query(&app->candwin, app->config.custom_query_label,
                              app->config.custom_query_url);
    }
    {
        /* 图标目录:安装位 SVG 在 icons/(res/ 只放 settings.ui/candidate.css);
         * 优先用"确认存在 zh.svg"的目录,避免安装版托盘/主窗口图标破图 */
        char icon_dir[1024];
        snprintf(icon_dir, sizeof(icon_dir), "%s", res_dir_default);
        char probe[1200];
        snprintf(probe, sizeof(probe), "%s/zh.svg", icon_dir);
        if (!g_file_test(probe, G_FILE_TEST_EXISTS) &&
            g_file_test("/usr/local/share/lyyime/icons/zh.svg",
                        G_FILE_TEST_EXISTS))
            snprintf(icon_dir, sizeof(icon_dir),
                     "/usr/local/share/lyyime/icons");
        lyy_tray_init(&app->tray, icon_dir);
        lyy_mainwin_init(&app->mainwin, icon_dir);
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
    if (app->mainwin_requested == 2)
        app->mainwin_requested = 1; /* 启动即弹主窗口 */

    g_main_loop_run(loop);

    /* 优雅退出:关 server → 释放引擎 → 清 pidfile */
    lyy_log(&app->log, "退出:清理 XIM server 与引擎");
    lyy_xim_shutdown(&app->xim);
    if (app->engine && app->core.loaded)
        app->core.lyyime_free(app->engine);
    if (app->menu_trigger && app->core.mt_ok)
        app->core.lyyime_menu_trigger_free(app->menu_trigger);
    lyy_ai_clear(&app->ai);
    g_unlink(pidfile);
    lyy_log(&app->log, "==== 退出完成 ====");
    lyy_log_close(&app->log);
    return 0;
}
