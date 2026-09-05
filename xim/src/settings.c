#define GDK_DISABLE_DEPRECATION_WARNINGS /* GtkStatusIcon 为任务书指定方案(XEmbed),
                                             GTK3.14 起标记弃用但功能完好,此处有意使用 */
#include "settings.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common.h"

/* ---- 构建控件状态 ←→ 配置 ---- */
static void ui_from_config(SettingsUi *ui)
{
    const LyyConfig *c = &lyy_app()->config;
    gtk_spin_button_set_value(GTK_SPIN_BUTTON(ui->spin_page), c->page_size);
    gtk_spin_button_set_value(GTK_SPIN_BUTTON(ui->spin_font), c->font_size);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_mixed),
                                 c->mixed_english);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_auto),
                                 c->auto_commit_english);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_punct),
                                 c->chinese_punct);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_learn),
                                 c->learning);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_autostart),
                                 c->autostart);
}

static void config_from_ui(SettingsUi *ui, LyyConfig *c)
{
    c->page_size =
        (int)gtk_spin_button_get_value(GTK_SPIN_BUTTON(ui->spin_page));
    c->font_size =
        (int)gtk_spin_button_get_value(GTK_SPIN_BUTTON(ui->spin_font));
    c->mixed_english =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_mixed));
    c->auto_commit_english =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_auto));
    c->chinese_punct =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_punct));
    c->learning =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_learn));
    c->autostart =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_autostart));
}

static void on_ok(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    SettingsUi *ui = user_data;
    App *app = lyy_app();

    LyyConfig c = app->config;
    config_from_ui(ui, &c);
    app->config = c;
    if (lyy_config_save(app->config_path, &c) != 0) {
        lyy_log(&app->log, "ERROR 配置保存失败:%s", app->config_path);
    }
    /* 开机自启:落/删 ~/.config/autostart/lyyime-xim.desktop */
    if (lyy_config_apply_autostart(c.autostart) != 0)
        lyy_log(&app->log, "WARN 开机自启项写入失败");

    /* 保存即生效:core 引擎重建(重读词库与配置)+ 候选窗字体即时刷新 */
    lyy_engine_reload(app);
    lyy_candwin_set_font_size(&app->candwin, c.font_size);
    lyy_log(&app->log,
            "设置已保存并生效:page_size=%d mixed=%d auto=%d punct=%d learn=%d font=%d autostart=%d",
            c.page_size, c.mixed_english, c.auto_commit_english,
            c.chinese_punct, c.learning, c.font_size, c.autostart);
    gtk_widget_hide(ui->window);
}

static void on_cancel(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    SettingsUi *ui = user_data;
    ui_from_config(ui); /* 还原显示 */
    gtk_widget_hide(ui->window);
}

void lyy_settings_init(SettingsUi *ui, const char *ui_dir)
{
    memset(ui, 0, sizeof(*ui));
    if (ui_dir)
        snprintf(ui->ui_dir, sizeof(ui->ui_dir), "%s", ui_dir);

    char path[1200];
    GError *err = NULL;
    GtkBuilder *builder = gtk_builder_new();

    /* 资源目录解析:LYYIME_RES_DIR 环境变量 → 安装位 → 源码树(开发态) */
    const char *dirs[3];
    dirs[0] = getenv("LYYIME_RES_DIR");
    dirs[1] = "/usr/local/share/lyyime/res";
    dirs[2] = LYY_SRC_RES_DIR;

    const char *file = NULL;
    for (int i = 0; i < 3 && !file; i++) {
        if (!dirs[i] || !dirs[i][0])
            continue;
        snprintf(path, sizeof(path), "%s/settings.ui", dirs[i]);
        if (g_file_test(path, G_FILE_TEST_EXISTS))
            file = path;
    }
    if (!file || !gtk_builder_add_from_file(builder, file, &err)) {
        lyy_log(&lyy_app()->log, "ERROR 设置界面加载失败(%s): %s",
                file ? file : "(未找到)",
                err ? err->message : "未知错误");
        g_clear_error(&err);
        g_object_unref(builder);
        return;
    }
    snprintf(ui->ui_dir, sizeof(ui->ui_dir), "%s", file);
    *(strrchr(ui->ui_dir, '/')) = '\0';

    ui->window = GTK_WIDGET(gtk_builder_get_object(builder, "settings_window"));
    ui->spin_page =
        GTK_WIDGET(gtk_builder_get_object(builder, "spin_page_size"));
    ui->spin_font =
        GTK_WIDGET(gtk_builder_get_object(builder, "spin_font_size"));
    ui->chk_mixed =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_mixed_english"));
    ui->chk_auto = GTK_WIDGET(
        gtk_builder_get_object(builder, "chk_auto_commit_english"));
    ui->chk_punct =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_chinese_punct"));
    ui->chk_learn =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_learning"));
    ui->chk_autostart =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_autostart"));

    if (!ui->window || !ui->spin_page || !ui->spin_font || !ui->chk_mixed ||
        !ui->chk_auto || !ui->chk_punct || !ui->chk_learn ||
        !ui->chk_autostart) {
        lyy_log(&lyy_app()->log, "ERROR 设置界面缺少控件(%s)", file);
        g_object_unref(builder);
        return;
    }
    g_signal_connect(ui->window, "delete-event",
                     G_CALLBACK(gtk_widget_hide_on_delete), NULL);
    GtkWidget *ok = GTK_WIDGET(gtk_builder_get_object(builder, "btn_ok"));
    GtkWidget *cancel =
        GTK_WIDGET(gtk_builder_get_object(builder, "btn_cancel"));
    if (ok)
        g_signal_connect(ok, "clicked", G_CALLBACK(on_ok), ui);
    if (cancel)
        g_signal_connect(cancel, "clicked", G_CALLBACK(on_cancel), ui);

    ui->built = 1;
    ui_from_config(ui);
    g_object_unref(builder); /* gtk_builder 保活控件引用,g_object_unref 安全 */
}

void lyy_settings_show(SettingsUi *ui)
{
    if (!ui->built) {
        GtkWidget *dlg = gtk_message_dialog_new(
            NULL, GTK_DIALOG_MODAL, GTK_MESSAGE_WARNING, GTK_BUTTONS_OK,
            "设置界面未能加载。\n"
            "排查:检查 %s/settings.ui 是否存在\n"
            "(安装后位于 /usr/local/share/lyyime/res/;开发态位于 xim/res/)。",
            ui->ui_dir[0] ? ui->ui_dir : LYY_SRC_RES_DIR);
        gtk_dialog_run(GTK_DIALOG(dlg));
        gtk_widget_destroy(dlg);
        return;
    }
    ui_from_config(ui);
    gtk_window_present(GTK_WINDOW(ui->window));
}
