#define GDK_DISABLE_DEPRECATION_WARNINGS /* GtkStatusIcon 为任务书指定方案(XEmbed) */
#include "tray.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common.h"
#include "shot.h"

#define DOCTOR_BIN "lyyime-doctor"

/* ---- 通用:GtkTextView 滚动对话框(承载长输出,AGENTS 边界完善红线) ---- */
static void show_text_dialog(GtkWindow *parent, const char *title,
                             const char *text)
{
    GtkWidget *dlg = gtk_dialog_new_with_buttons(
        title, parent, GTK_DIALOG_MODAL | GTK_DIALOG_DESTROY_WITH_PARENT,
        "_关闭", GTK_RESPONSE_CLOSE, NULL);
    gtk_window_set_default_size(GTK_WINDOW(dlg), 680, 440);

    GtkWidget *sw = gtk_scrolled_window_new(NULL, NULL);
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(sw), GTK_POLICY_AUTOMATIC,
                                   GTK_POLICY_AUTOMATIC);
    GtkWidget *view = gtk_text_view_new();
    gtk_text_view_set_editable(GTK_TEXT_VIEW(view), FALSE);
    gtk_text_view_set_wrap_mode(GTK_TEXT_VIEW(view), GTK_WRAP_WORD_CHAR);
    gtk_text_view_set_monospace(GTK_TEXT_VIEW(view), TRUE);
    GtkTextBuffer *buf = gtk_text_view_get_buffer(GTK_TEXT_VIEW(view));
    gtk_text_buffer_set_text(buf, text ? text : "", -1);
    gtk_container_set_border_width(GTK_CONTAINER(sw), 8);
    gtk_box_pack_start(GTK_BOX(gtk_dialog_get_content_area(GTK_DIALOG(dlg))),
                       sw, TRUE, TRUE, 0);
    gtk_widget_show_all(dlg);
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
}

/* ---- 通用:执行命令并捕获输出;返回 0 成功。missing=1 时给出安装提示 ---- */
static int run_command_capture(const char *cmd, gchar **out, int *missing)
{
    GError *err = NULL;
    gchar *stdout_s = NULL, *stderr_s = NULL;
    gint status = 0;
    *missing = 0;
    if (!g_spawn_command_line_sync(cmd, &stdout_s, &stderr_s, &status, &err)) {
        if (err && err->code == G_SPAWN_ERROR_NOENT)
            *missing = 1;
        g_clear_error(&err);
        g_free(stdout_s);
        g_free(stderr_s);
        return -1;
    }
    GString *g = g_string_new("");
    if (stdout_s && stdout_s[0])
        g_string_append(g, stdout_s);
    if (stderr_s && stderr_s[0]) {
        g_string_append(g, "\n[stderr]\n");
        g_string_append(g, stderr_s);
    }
    if (!(status == 0))
        g_string_append_printf(g, "\n[退出码 %d]", status);
    *out = g_string_free(g, FALSE);
    g_free(stdout_s);
    g_free(stderr_s);
    return 0;
}

static void doctor_missing_dialog(GtkWindow *parent)
{
    GtkWidget *dlg = gtk_message_dialog_new(
        parent, GTK_DIALOG_MODAL, GTK_MESSAGE_INFO, GTK_BUTTONS_OK,
        "未找到 %s(lyyIme 诊断/管理工具)。\n\n"
        "安装方式(任选其一):\n"
        "  1. 源码构建:scripts/build.sh(产物 crates/lyyime-doctor)\n"
        "  2. 拷贝到 PATH:cp lyyime-doctor /usr/local/bin/\n\n"
        "安装后即可在此使用“修复输入法 / 输入法管理”。",
        DOCTOR_BIN);
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
}

/* ---- 工具:截屏(拉起 lyyime-shot;热键不可达的英文直通态也能用) ---- */
static void on_screenshot(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    lyy_spawn_shot(app);
}

/* ---- 工具:修复输入法(危险操作,先确认再执行) ---- */
static void on_fix_ime(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    GtkWidget *confirm = gtk_message_dialog_new(
        NULL, GTK_DIALOG_MODAL, GTK_MESSAGE_QUESTION, GTK_BUTTONS_OK_CANCEL,
        "将执行 \"%s check --fix\":\n"
        "诊断并修复输入法环境(XMODIFIERS/autostart/ibus 注册/缓存),\n"
        "可能修改会话配置文件。是否继续?",
        DOCTOR_BIN);
    if (gtk_dialog_run(GTK_DIALOG(confirm)) != GTK_RESPONSE_OK) {
        gtk_widget_destroy(confirm);
        return;
    }
    gtk_widget_destroy(confirm);

    gchar *out = NULL;
    int missing = 0;
    if (run_command_capture(DOCTOR_BIN " check --fix", &out, &missing) != 0) {
        if (missing)
            doctor_missing_dialog(NULL);
        else
            show_text_dialog(NULL, "修复输入法 — 执行失败",
                             "命令执行失败,请检查 lyyime-doctor 是否可用。");
        return;
    }
    show_text_dialog(NULL, "修复输入法 — 执行结果", out);
    g_free(out);
    (void)app;
}

/* ---- 工具:输入法管理(exec lyyime-doctor ime-list 并解析 JSON) ----
 * doctor 合同(docs/ARCHITECTURE.md §9.1):输出对象含
 *   id/name/kind/installed/active/is_default。
 * 此处做容错解析:能解析则提供"设为默认/安装/卸载"操作(全部先确认),
 * 解析失败则原样展示文本并提示用 CLI。 */
typedef struct {
    char id[128];
    char name[192];
    char kind[32];
    int installed, active, is_default;
} ImeEntry;

/* 在一个 JSON 对象文本内提取 "key":"value" / "key":true|false */
static void json_obj_field(const char *obj, const char *key, char *out,
                           size_t cap)
{
    out[0] = '\0';
    char pat[64];
    snprintf(pat, sizeof(pat), "\"%s\"", key);
    const char *p = strstr(obj, pat);
    if (!p)
        return;
    p = strchr(p + strlen(pat), ':');
    if (!p)
        return;
    p++;
    while (*p == ' ')
        p++;
    if (*p == '"') {
        p++;
        size_t w = 0;
        while (*p && *p != '"' && w + 1 < cap)
            out[w++] = *p++;
        out[w] = '\0';
    } else {
        snprintf(out, cap, "%.*s", (int)strcspn(p, ",}"), p);
    }
}

static int parse_ime_list(const char *text, ImeEntry *entries, int max)
{
    int n = 0;
    const char *p = text;
    while (p && (p = strchr(p, '{')) != NULL) {
        const char *end = strchr(p, '}');
        if (!end)
            break;
        char obj[1024];
        size_t len = (size_t)(end - p + 1);
        if (len >= sizeof(obj))
            len = sizeof(obj) - 1;
        memcpy(obj, p, len);
        obj[len] = '\0';

        char id[128];
        json_obj_field(obj, "id", id, sizeof(id));
        if (id[0] && n < max) {
            ImeEntry *e = &entries[n++];
            snprintf(e->id, sizeof(e->id), "%s", id);
            json_obj_field(obj, "name", e->name, sizeof(e->name));
            json_obj_field(obj, "kind", e->kind, sizeof(e->kind));
            char flag[16];
            json_obj_field(obj, "installed", flag, sizeof(flag));
            e->installed = strstr(flag, "true") != NULL;
            json_obj_field(obj, "active", flag, sizeof(flag));
            e->active = strstr(flag, "true") != NULL;
            json_obj_field(obj, "is_default", flag, sizeof(flag));
            e->is_default = strstr(flag, "true") != NULL;
        }
        p = end + 1;
    }
    return n;
}

static void on_manage_ime(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;

    gchar *out = NULL;
    int missing = 0;
    /* 合同未保证 --json 旗标;先试 --json,失败退化为原始输出展示 */
    gchar *cmd = g_strdup_printf("%s ime-list --json", DOCTOR_BIN);
    int rc = run_command_capture(cmd, &out, &missing);
    g_free(cmd);
    if (rc != 0) {
        if (missing)
            doctor_missing_dialog(NULL);
        else
            show_text_dialog(NULL, "输入法管理", "ime-list 执行失败。");
        return;
    }
    if (!out || !out[0]) {
        g_free(out);
        out = NULL;
        cmd = g_strdup_printf(DOCTOR_BIN " ime-list");
        rc = run_command_capture(cmd, &out, &missing);
        g_free(cmd);
        if (rc != 0) {
            if (missing)
                doctor_missing_dialog(NULL);
            else
                show_text_dialog(NULL, "输入法管理", "ime-list 执行失败。");
            return;
        }
    }

    ImeEntry entries[32];
    int n = parse_ime_list(out ? out : "", entries, 32);
    if (n <= 0) {
        GString *g = g_string_new(
            "未能解析 ime-list JSON(可用 CLI 直接操作:\n"
            "lyyime-doctor ime-add/ime-remove/ime-default <id> "
            "[--dry-run|--yes])。\n\n--- 原始输出 ---\n");
        g_string_append(g, out ? out : "");
        show_text_dialog(NULL, "输入法管理", g->str);
        g_string_free(g, TRUE);
        g_free(out);
        return;
    }

    /* 组装展示文本 */
    GString *g = g_string_new("本机输入法(lyyime-doctor ime-list):\n\n");
    for (int i = 0; i < n; i++) {
        g_string_append_printf(g, "  %-24s %-10s %-6s %s%s%s\n",
                               entries[i].id, entries[i].kind,
                               entries[i].installed ? "已装" : "未装",
                               entries[i].name,
                               entries[i].active ? "  [使用中]" : "",
                               entries[i].is_default ? "  [默认]" : "");
    }
    g_string_append(g, "\n操作需二次确认;保护规则由 doctor 保证(绝不误删 "
                       "lyyime 自身或最后一个可用输入法)。\n"
                       "变更类操作 CLI:lyyime-doctor ime-add / ime-remove / "
                       "ime-default <id> --yes。");
    show_text_dialog(NULL, "输入法管理", g->str);
    g_string_free(g, TRUE);
    g_free(out);
}

/* ---- 工具:重载词库 / 打开日志 ---- */
static void on_reload_dict(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    lyy_engine_reload(app);
    GtkWidget *dlg = gtk_message_dialog_new(
        NULL, GTK_DIALOG_MODAL, GTK_MESSAGE_INFO, GTK_BUTTONS_OK,
        "词库与配置已重载(引擎重建)。详见日志:\n%s", app->log.path);
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
}

static void on_open_log(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    gchar *content = NULL;
    if (!g_file_get_contents(app->log.path, &content, NULL, NULL))
        content = g_strdup("(日志为空或不可读)");
    /* 只展示尾部 8000 字符,避免超长卡顿 */
    size_t len = strlen(content);
    const char *tail = (len > 8000) ? content + (len - 8000) : content;
    show_text_dialog(NULL, "xim.log(尾部)", tail);
    g_free(content);
}

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

static void on_settings(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    App *app = user_data;
    lyy_settings_show(&app->settings);
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
        { "截屏", G_CALLBACK(on_screenshot) },
        { "修复输入法…", G_CALLBACK(on_fix_ime) },
        { "输入法管理…", G_CALLBACK(on_manage_ime) },
        { "重载词库", G_CALLBACK(on_reload_dict) },
        { "打开日志", G_CALLBACK(on_open_log) },
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
