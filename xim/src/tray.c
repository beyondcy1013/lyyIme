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

static void on_general_settings(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_request_show_settings(user_data);
}
static void on_general_settings_input(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_request_show_settings_page(user_data, 1);
}
static void on_general_settings_skin(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    lyy_request_show_settings_page(user_data, 6);
}
static int cfg_save_flag(App *app, int which, int on, const char *name)
{
    LyyConfig c;
    if (lyy_config_load(app->config_path, &c) < 0) {
        lyy_show_notice(app, "读取配置失败,改动未保存");
        lyy_log(&app->log, "WARN 菜单切换 %s 中止:读取 %s 失败", name,
                app->config_path);
        return 0;
    }
    int *field = NULL;
    switch (which) {
    case 0: field = &c.chinese_punct; break;
    case 1: field = &c.learning; break;
    case 2: field = &c.next_word_prediction; break;
    case 3: field = &c.quick_actions_enabled; break;
    case 4: field = &c.pinyin_only; break;
    default: return 0;
    }
    *field = on;
    if (lyy_config_save(app->config_path, &c) != 0) {
        lyy_show_notice(app, "配置保存失败,改动未生效");
        lyy_log(&app->log, "WARN 菜单切换 %s 失败:保存 %s 失败", name,
                app->config_path);
        return 0;
    }
    switch (which) {
    case 0: app->config.chinese_punct = on; break;
    case 1: app->config.learning = on; break;
    case 2: app->config.next_word_prediction = on; break;
    case 3: app->config.quick_actions_enabled = on; break;
    case 4: app->config.pinyin_only = on; break;
    }
    lyy_log(&app->log, "菜单切换 %s → %d(已保存)", name, on);
    return 1;
}
static int menu_flag_cur(App *app, int which)
{
    LyyConfig disk;
    if (lyy_config_load(app->config_path, &disk) >= 0) {
        switch (which) {
        case 1: return disk.learning;
        case 2: return disk.next_word_prediction;
        case 3: return disk.quick_actions_enabled;
        case 4: return disk.pinyin_only;
        }
    }
    switch (which) {
    case 1: return app->config.learning;
    case 2: return app->config.next_word_prediction;
    case 3: return app->config.quick_actions_enabled;
    case 4: return app->config.pinyin_only;
    }
    return 0;
}
static int menu_punct_cur(App *app)
{
    if (app->engine && app->core.punct_ok)
        return app->punct_runtime;
    LyyConfig disk;
    if (lyy_config_load(app->config_path, &disk) >= 0)
        return disk.chinese_punct;
    return app->config.chinese_punct;
}
static void on_menu_punct_toggled(GtkCheckMenuItem *item, gpointer user_data)
{
    App *app = user_data;
    int on = gtk_check_menu_item_get_active(item);
    if (on == menu_punct_cur(app))
        return;
    if (!app->core.punct_ok) {
        lyy_show_notice(app, "当前词库核心不支持运行时标点切换");
        gtk_check_menu_item_set_active(item, !on);
        return;
    }
    if (!cfg_save_flag(app, 0, on, "中文标点")) {
        gtk_check_menu_item_set_active(item, !on);
        return;
    }
    if (app->engine)
        app->core.lyyime_set_chinese_punctuation(app->engine, on);
    app->punct_runtime = on;
}
static void on_menu_learn_toggled(GtkCheckMenuItem *item, gpointer user_data)
{
    App *app = user_data;
    int on = gtk_check_menu_item_get_active(item);
    if (on == menu_flag_cur(app, 1))
        return;
    if (!app->core.learn_ok) {
        lyy_show_notice(app, "当前词库核心不支持运行时学习开关");
        gtk_check_menu_item_set_active(item, !on);
        return;
    }
    if (!cfg_save_flag(app, 1, on, "用户词学习")) {
        gtk_check_menu_item_set_active(item, !on);
        return;
    }
    if (app->engine)
        app->core.lyyime_set_learning(app->engine, on);
}
static void on_menu_pred_toggled(GtkCheckMenuItem *item, gpointer user_data)
{
    App *app = user_data;
    int on = gtk_check_menu_item_get_active(item);
    if (on == menu_flag_cur(app, 2))
        return;
    if (!cfg_save_flag(app, 2, on, "上屏后联想")) {
        gtk_check_menu_item_set_active(item, !on);
        return;
    }
    if (app->core.pred_ok && app->engine)
        app->core.lyyime_set_next_word_prediction(app->engine, on);
}
static void on_menu_qa_toggled(GtkCheckMenuItem *item, gpointer user_data)
{
    App *app = user_data;
    int on = gtk_check_menu_item_get_active(item);
    if (on == menu_flag_cur(app, 3))
        return;
    if (!cfg_save_flag(app, 3, on, "快速功能键")) {
        gtk_check_menu_item_set_active(item, !on);
        return;
    }
    if (app->core.qa_ok && app->engine)
        app->core.lyyime_set_quick_actions_enabled(app->engine, on);
}
static void on_menu_scheme_toggled(GtkCheckMenuItem *item, gpointer user_data);
static void scheme_radio_restore(App *app, GtkWidget *item)
{
    GtkWidget *peer = g_object_get_data(G_OBJECT(item), "lyy-peer");
    if (!peer)
        return;
    g_signal_handlers_block_by_func(peer,
                                    (gpointer)on_menu_scheme_toggled, app);
    g_signal_handlers_block_by_func(item,
                                    (gpointer)on_menu_scheme_toggled, app);
    gtk_check_menu_item_set_active(GTK_CHECK_MENU_ITEM(peer), TRUE);
    g_signal_handlers_unblock_by_func(peer,
                                      (gpointer)on_menu_scheme_toggled, app);
    g_signal_handlers_unblock_by_func(item,
                                      (gpointer)on_menu_scheme_toggled, app);
}
static void on_menu_scheme_toggled(GtkCheckMenuItem *item, gpointer user_data)
{
    App *app = user_data;
    if (!gtk_check_menu_item_get_active(item))
        return;
    int want = GPOINTER_TO_INT(g_object_get_data(G_OBJECT(item), "lyy-pure"));
    if (want == menu_flag_cur(app, 4))
        return;
    if (!app->core.pinyin_ok) {
        lyy_show_notice(app, "纯拼音需要更新词库核心(liblyyime_core.so)");
        scheme_radio_restore(app, GTK_WIDGET(item));
        return;
    }
    if (!cfg_save_flag(app, 4, want, "输入方案")) {
        scheme_radio_restore(app, GTK_WIDGET(item));
        return;
    }
    if (app->engine) {
        app->core.lyyime_set_pinyin_only(app->engine, want);
        lyy_compose_clear_ui(app);
    }
    lyy_show_notice(app, want ? "已切换:纯拼音" : "已切换:五笔/拼音混输");
}
void lyy_general_menu_append(App *app, GtkMenuShell *shell)
{
    LyyConfig snap = app->config;
    {
        LyyConfig disk;
        if (lyy_config_load(app->config_path, &disk) >= 0)
            snap = disk;
    }
    GtkWidget *item;
    item = gtk_menu_item_new_with_label("设置…");
    g_signal_connect(item, "activate", G_CALLBACK(on_general_settings), app);
    gtk_menu_shell_append(shell, item);
    item = gtk_menu_item_new_with_label("输入设置…");
    g_signal_connect(item, "activate", G_CALLBACK(on_general_settings_input),
                     app);
    gtk_menu_shell_append(shell, item);
    item = gtk_menu_item_new_with_label("皮肤设置…");
    g_signal_connect(item, "activate", G_CALLBACK(on_general_settings_skin),
                     app);
    gtk_menu_shell_append(shell, item);
    gtk_menu_shell_append(shell, gtk_separator_menu_item_new());
    item = gtk_menu_item_new_with_label("切换 中/EN(Shift 单击)");
    g_signal_connect(item, "activate", G_CALLBACK(on_toggle_mode), app);
    gtk_menu_shell_append(shell, item);
    GtkWidget *mixed =
        gtk_radio_menu_item_new_with_label(NULL, "五笔/拼音混输");
    GtkWidget *pure = gtk_radio_menu_item_new_with_label(
        gtk_radio_menu_item_get_group(GTK_RADIO_MENU_ITEM(mixed)),
        "纯拼音");
    gtk_check_menu_item_set_active(
        GTK_CHECK_MENU_ITEM(snap.pinyin_only ? pure : mixed), TRUE);
    g_object_set_data(G_OBJECT(mixed), "lyy-pure", GINT_TO_POINTER(0));
    g_object_set_data(G_OBJECT(pure), "lyy-pure", GINT_TO_POINTER(1));
    g_object_set_data(G_OBJECT(mixed), "lyy-peer", pure);
    g_object_set_data(G_OBJECT(pure), "lyy-peer", mixed);
    if (!app->core.pinyin_ok) {
        gtk_widget_set_sensitive(pure, FALSE);
        gtk_widget_set_tooltip_text(
            pure, "需要更新词库核心(liblyyime_core.so)才支持纯拼音");
    }
    g_signal_connect(mixed, "toggled", G_CALLBACK(on_menu_scheme_toggled),
                     app);
    g_signal_connect(pure, "toggled", G_CALLBACK(on_menu_scheme_toggled),
                     app);
    gtk_menu_shell_append(shell, mixed);
    gtk_menu_shell_append(shell, pure);
    item = gtk_check_menu_item_new_with_label("中文标点");
    gtk_check_menu_item_set_active(GTK_CHECK_MENU_ITEM(item),
                                   menu_punct_cur(app));
    if (!app->core.punct_ok) {
        gtk_widget_set_sensitive(item, FALSE);
        gtk_widget_set_tooltip_text(
            item, "当前词库核心不支持运行时标点切换");
    }
    g_signal_connect(item, "toggled", G_CALLBACK(on_menu_punct_toggled),
                     app);
    gtk_menu_shell_append(shell, item);
    item = gtk_check_menu_item_new_with_label("用户词学习");
    gtk_check_menu_item_set_active(GTK_CHECK_MENU_ITEM(item),
                                   snap.learning);
    if (!app->core.learn_ok) {
        gtk_widget_set_sensitive(item, FALSE);
        gtk_widget_set_tooltip_text(
            item, "当前词库核心不支持运行时学习开关");
    }
    g_signal_connect(item, "toggled", G_CALLBACK(on_menu_learn_toggled),
                     app);
    gtk_menu_shell_append(shell, item);
    item = gtk_check_menu_item_new_with_label("上屏后联想");
    gtk_check_menu_item_set_active(GTK_CHECK_MENU_ITEM(item),
                                   snap.next_word_prediction);
    if (!app->core.pred_ok)
        gtk_widget_set_sensitive(item, FALSE);
    g_signal_connect(item, "toggled", G_CALLBACK(on_menu_pred_toggled),
                     app);
    gtk_menu_shell_append(shell, item);
    item = gtk_check_menu_item_new_with_label("快速功能键");
    gtk_check_menu_item_set_active(GTK_CHECK_MENU_ITEM(item),
                                   snap.quick_actions_enabled);
    if (!app->core.qa_ok)
        gtk_widget_set_sensitive(item, FALSE);
    g_signal_connect(item, "toggled", G_CALLBACK(on_menu_qa_toggled),
                     app);
    gtk_menu_shell_append(shell, item);
    gtk_menu_shell_append(shell, gtk_separator_menu_item_new());
    item = gtk_menu_item_new_with_label("截屏");
    g_signal_connect(item, "activate", G_CALLBACK(on_screenshot_tool), app);
    gtk_menu_shell_append(shell, item);
    item = gtk_menu_item_new_with_label("重载词库");
    g_signal_connect(item, "activate", G_CALLBACK(on_reload_dict_tool), app);
    gtk_menu_shell_append(shell, item);
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
