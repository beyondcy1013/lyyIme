#define GDK_DISABLE_DEPRECATION_WARNINGS /* GtkStatusIcon 为任务书指定方案(XEmbed) */
#include "tray.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common.h"
#include "tools.h"

/* 工具动作(截屏/修复输入法/输入法管理/重载词库/日志/直输模式)的实现
 * 在 tools.c:主窗口「工具箱」与本菜单共用,勿在此重复实现。 */

/* ---- 中英切换(与 Shift 单击同一路径) ---- */
static void on_toggle_mode(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    if (!app->enabled || app->degraded)
        return;
    int english = 1;
    if (app->core.loaded && app->engine)
        english = app->core.lyyime_mode(app->engine) == 1;
    lyy_xim_set_trigger(app, english); /* 当前英文 → 切中文;反之亦然 */
    lyy_log(&app->log, "托盘切换模式 → %s", english ? "中文" : "英文");
}

static void on_enable_toggled(GtkWidget *widget, gpointer user_data)
{
    App *app = user_data;
    int active = gtk_check_menu_item_get_active(GTK_CHECK_MENU_ITEM(widget));
    lyy_xim_set_enabled(app, active);
}

static void on_mainwin(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    lyy_mainwin_show(&app->mainwin);
}

static void on_settings(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    lyy_settings_show(&app->settings);
}

/* 工具菜单项 → tools.c 动作(activate 回调签名;app 经 user_data) */
static void on_float_tool(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_tools_float_window(user_data);
}

static void on_screenshot_tool(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_tools_screenshot(user_data);
}

static void on_fix_ime_tool(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_tools_fix_ime(user_data);
}

static void on_manage_ime_tool(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_tools_manage_ime(user_data);
}

static void on_reload_dict_tool(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_tools_reload_dict(user_data);
}

static void on_open_log_tool(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_tools_open_log(user_data);
}

static void on_quit(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    app->quit_requested = 1;
}

/* 左键单击图标 = 切换中英(对齐搜狗/万能五笔习惯) */
static void on_status_activate(GtkStatusIcon *icon, gpointer user_data)
{
    (void)icon;
    on_toggle_mode(NULL, user_data);
}

/* GSourceFunc 包装(g_object_unref 返回 void,不能直接转型,避免
   -Wcast-function-type) */
static gboolean menu_unref_later(gpointer data)
{
    g_object_unref(data);
    return G_SOURCE_REMOVE;
}

static void on_status_popup(GtkStatusIcon *icon, guint button, guint activate_time,
                            gpointer user_data)
{
    App *app = user_data;
    (void)icon;

    GtkWidget *menu = gtk_menu_new();

    GtkWidget *mainwin = gtk_menu_item_new_with_label("主窗口…");
    g_signal_connect(mainwin, "activate", G_CALLBACK(on_mainwin), app);
    gtk_menu_shell_append(GTK_MENU_SHELL(menu), mainwin);

    GtkWidget *enable = gtk_check_menu_item_new_with_label("启用输入法");
    gtk_check_menu_item_set_active(GTK_CHECK_MENU_ITEM(enable), app->enabled);
    g_signal_connect(enable, "toggled", G_CALLBACK(on_enable_toggled), app);
    gtk_menu_shell_append(GTK_MENU_SHELL(menu), enable);

    GtkWidget *toggle = gtk_menu_item_new_with_label("切换 中/EN(Shift 单击)");
    g_signal_connect(toggle, "activate", G_CALLBACK(on_toggle_mode), app);
    gtk_menu_shell_append(GTK_MENU_SHELL(menu), toggle);

    gtk_menu_shell_append(GTK_MENU_SHELL(menu), gtk_separator_menu_item_new());

    GtkWidget *settings = gtk_menu_item_new_with_label("设置…");
    g_signal_connect(settings, "activate", G_CALLBACK(on_settings), app);
    gtk_menu_shell_append(GTK_MENU_SHELL(menu), settings);

    GtkWidget *tools = gtk_menu_item_new_with_label("工具");
    gtk_menu_shell_append(GTK_MENU_SHELL(menu), tools);
    GtkWidget *submenu = gtk_menu_new();
    gtk_menu_item_set_submenu(GTK_MENU_ITEM(tools), submenu);

    struct {
        const char *label;
        GCallback cb;
    } tool_items[] = {
        { "直输模式…", G_CALLBACK(on_float_tool) },
        { "截屏", G_CALLBACK(on_screenshot_tool) },
        { "修复输入法…", G_CALLBACK(on_fix_ime_tool) },
        { "输入法管理…", G_CALLBACK(on_manage_ime_tool) },
        { "重载词库", G_CALLBACK(on_reload_dict_tool) },
        { "打开日志", G_CALLBACK(on_open_log_tool) },
    };
    for (unsigned long i = 0; i < sizeof(tool_items) / sizeof(tool_items[0]);
         i++) {
        GtkWidget *it = gtk_menu_item_new_with_label(tool_items[i].label);
        g_signal_connect(it, "activate", tool_items[i].cb, app);
        gtk_menu_shell_append(GTK_MENU_SHELL(submenu), it);
    }

    gtk_menu_shell_append(GTK_MENU_SHELL(menu), gtk_separator_menu_item_new());

    GtkWidget *quit = gtk_menu_item_new_with_label("退出");
    g_signal_connect(quit, "activate", G_CALLBACK(on_quit), app);
    gtk_menu_shell_append(GTK_MENU_SHELL(menu), quit);

    gtk_widget_show_all(menu);
    gtk_menu_popup(GTK_MENU(menu), NULL, NULL,
                   gtk_status_icon_position_menu, icon, button,
                   activate_time);
    /* 弹出菜单生命周期:ref_sink 后挂 30s 兜底回收(菜单关闭早于此也不泄漏) */
    g_object_ref_sink(menu);
    g_timeout_add_seconds(30, menu_unref_later, menu);
}

static void tray_refresh_icon(Tray *tray)
{
    char path[1200];
    const char *name;
    if (!tray->active)
        name = "en.svg"; /* 停用:EN 灰态示意(同图,tooltip 说明) */
    else
        name = tray->mode == LYY_MODE_EN ? "en.svg" : "zh.svg";
    snprintf(path, sizeof(path), "%s/%s", tray->icon_dir[0] ? tray->icon_dir
                                                           : LYY_SRC_RES_DIR,
             name);
    gtk_status_icon_set_from_file(tray->icon, path);

    const char *mode_text =
        !tray->active ? "已停用(全部直通)"
                      : (tray->mode == LYY_MODE_EN ? "英文 EN(Shift 单击切换)"
                                                   : "中文(Shift 单击切换)");
    gchar *tip = g_strdup_printf("lyyIme 输入法:%s", mode_text);
    gtk_status_icon_set_tooltip_text(tray->icon, tip);
    g_free(tip);
}

void lyy_tray_init(Tray *tray, const char *icon_dir)
{
    memset(tray, 0, sizeof(*tray));
    if (icon_dir)
        snprintf(tray->icon_dir, sizeof(tray->icon_dir), "%s", icon_dir);
    tray->icon = gtk_status_icon_new();
    tray->mode = LYY_MODE_ZH;
    tray->active = 1;
    tray_refresh_icon(tray);
    g_signal_connect(tray->icon, "activate", G_CALLBACK(on_status_activate),
                     lyy_app());
    g_signal_connect(tray->icon, "popup-menu", G_CALLBACK(on_status_popup),
                     lyy_app());
}

void lyy_tray_set_mode(Tray *tray, int mode, int active)
{
    tray->mode = mode;
    tray->active = active;
    tray_refresh_icon(tray);
}
