/* 主窗口实现(纯代码构建;模块合同见 mainwin.h) */
#include "mainwin.h"

#include <stdio.h>
#include <string.h>

#include "common.h"
#include "tools.h"

/* ---- 状态行:版本 · 启用/停用 · 中/EN · 引擎态 ---- */
void lyy_mainwin_refresh(MainWin *mw)
{
    if (!mw->built || !mw->lbl_status)
        return;
    App *app = lyy_app();
    const char *mode =
        (app->tray.mode == LYY_MODE_EN) ? "英文 EN" : "中文";
    const char *eng = app->degraded
                          ? "引擎降级(直通)"
                          : (app->core.loaded ? "引擎正常" : "引擎未加载");
    gchar *s = app->enabled
                   ? g_strdup_printf("v%s · 启用 · %s · %s", LYY_APP_VERSION,
                                     mode, eng)
                   : g_strdup_printf("v%s · 已停用(全部直通) · %s",
                                     LYY_APP_VERSION, eng);
    gtk_label_set_text(GTK_LABEL(mw->lbl_status), s);
    g_free(s);
}

/* ---- 入口/工具回调(签名对齐 GTK clicked;App 经 lyy_app() 取单例) ---- */
static void on_settings(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;
    lyy_settings_show(&lyy_app()->settings);
}

static void on_float(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;
    lyy_tools_float_window(lyy_app());
}

static void on_screenshot(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;
    lyy_tools_screenshot(lyy_app());
}

static void on_fix_ime(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;
    lyy_tools_fix_ime(lyy_app());
}

static void on_manage_ime(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;
    lyy_tools_manage_ime(lyy_app());
}

static void on_reload_dict(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;
    lyy_tools_reload_dict(lyy_app());
}

static void on_open_log(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    (void)user_data;
    lyy_tools_open_log(lyy_app());
}

void lyy_mainwin_show(MainWin *mw)
{
    if (!mw->built || !mw->window)
        return;
    lyy_mainwin_refresh(mw);
    gtk_widget_show_all(mw->window);
    gtk_window_present(GTK_WINDOW(mw->window));
}

/* ---- 构建 ---- */
void lyy_mainwin_init(MainWin *mw, const char *icon_dir)
{
    memset(mw, 0, sizeof(*mw));

    GtkWidget *win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(win), "lyyIme 输入法");
    gtk_window_set_default_size(GTK_WINDOW(win), 400, -1);
    gtk_window_set_resizable(GTK_WINDOW(win), FALSE);
    gtk_window_set_position(GTK_WINDOW(win), GTK_WIN_POS_CENTER);
    gtk_container_set_border_width(GTK_CONTAINER(win), 14);
    g_signal_connect(win, "delete-event",
                     G_CALLBACK(gtk_widget_hide_on_delete), NULL);

    GtkWidget *root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_container_add(GTK_CONTAINER(win), root);

    /* 头部:中英图标 + 标题 + 状态行 */
    GtkWidget *head = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 12);
    gtk_box_pack_start(GTK_BOX(root), head, FALSE, FALSE, 0);
    char icon_path[1200];
    snprintf(icon_path, sizeof(icon_path), "%s/%s",
             (icon_dir && icon_dir[0]) ? icon_dir : LYY_SRC_RES_DIR, "zh.svg");
    GtkWidget *img = gtk_image_new_from_file(icon_path);
    gtk_image_set_pixel_size(GTK_IMAGE(img), 44);
    gtk_box_pack_start(GTK_BOX(head), img, FALSE, FALSE, 0);
    GtkWidget *hv = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_box_pack_start(GTK_BOX(head), hv, FALSE, FALSE, 0);
    GtkWidget *title = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(title), "<big><b>lyyIme 输入法</b></big>");
    gtk_widget_set_halign(title, GTK_ALIGN_START);
    gtk_box_pack_start(GTK_BOX(hv), title, FALSE, FALSE, 0);
    mw->lbl_status = gtk_label_new(NULL);
    gtk_widget_set_halign(mw->lbl_status, GTK_ALIGN_START);
    gtk_widget_set_sensitive(mw->lbl_status, FALSE); /* 灰字状态行 */
    gtk_box_pack_start(GTK_BOX(hv), mw->lbl_status, FALSE, FALSE, 0);

    gtk_box_pack_start(
        GTK_BOX(root),
        gtk_separator_new(GTK_ORIENTATION_HORIZONTAL), FALSE, FALSE, 0);

    /* ── 输入 ── */
    GtkWidget *sec_in = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(sec_in), "<b>输入</b>");
    gtk_widget_set_halign(sec_in, GTK_ALIGN_START);
    gtk_box_pack_start(GTK_BOX(root), sec_in, FALSE, FALSE, 0);

    GtkWidget *grid_in = gtk_grid_new();
    gtk_grid_set_column_spacing(GTK_GRID(grid_in), 8);
    gtk_grid_set_row_spacing(GTK_GRID(grid_in), 8);
    gtk_box_pack_start(GTK_BOX(root), grid_in, FALSE, FALSE, 0);

    GtkWidget *btn_settings = gtk_button_new_with_label("输入设置…");
    gtk_widget_set_tooltip_text(
        btn_settings, "候选数/字体/标点/学习/四码/快捷键/开机自启/AI 助手");
    g_signal_connect(btn_settings, "clicked", G_CALLBACK(on_settings), NULL);
    gtk_grid_attach(GTK_GRID(grid_in), btn_settings, 0, 0, 1, 1);

    GtkWidget *btn_float = gtk_button_new_with_label("直输模式…");
    gtk_widget_set_tooltip_text(
        btn_float, "打开悬浮独立输入窗(lyyime-float):窗口里打字,"
                   "直输/粘贴发送到最近使用的应用");
    g_signal_connect(btn_float, "clicked", G_CALLBACK(on_float), NULL);
    gtk_grid_attach(GTK_GRID(grid_in), btn_float, 1, 0, 1, 1);

    /* ── 工具箱(与托盘「工具」子菜单共用 tools.c) ── */
    GtkWidget *sec_tools = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(sec_tools), "<b>工具箱</b>");
    gtk_widget_set_halign(sec_tools, GTK_ALIGN_START);
    gtk_box_pack_start(GTK_BOX(root), sec_tools, FALSE, FALSE, 0);

    GtkWidget *grid_tools = gtk_grid_new();
    gtk_grid_set_column_spacing(GTK_GRID(grid_tools), 8);
    gtk_grid_set_row_spacing(GTK_GRID(grid_tools), 8);
    gtk_box_pack_start(GTK_BOX(root), grid_tools, FALSE, FALSE, 0);

    struct {
        const char *label;
        const char *tip;
        GCallback cb;
    } tools[] = {
        { "截屏", "全屏/框选截屏(lyyime-shot)", G_CALLBACK(on_screenshot) },
        { "修复输入法…", "诊断并修复输入法环境(doctor check --fix)",
          G_CALLBACK(on_fix_ime) },
        { "输入法管理…", "查看本机输入法/变更指引(doctor ime-list)",
          G_CALLBACK(on_manage_ime) },
        { "重载词库", "重建 core 引擎,重读词库与配置",
          G_CALLBACK(on_reload_dict) },
        { "打开日志", "查看 xim.log 尾部", G_CALLBACK(on_open_log) },
    };
    for (unsigned long i = 0; i < sizeof(tools) / sizeof(tools[0]); i++) {
        GtkWidget *b = gtk_button_new_with_label(tools[i].label);
        if (tools[i].tip)
            gtk_widget_set_tooltip_text(b, tools[i].tip);
        g_signal_connect(b, "clicked", tools[i].cb, NULL);
        gtk_grid_attach(GTK_GRID(grid_tools), b, (int)(i % 2), (int)(i / 2),
                        1, 1);
    }

    mw->window = win;
    mw->built = 1;
    lyy_mainwin_refresh(mw);
    /* 初始不显示:托盘常驻即可,主窗口按需唤起(菜单/--mainwin/SIGUSR2) */
}
