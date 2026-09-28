#define GDK_DISABLE_DEPRECATION_WARNINGS /* GtkStatusIcon 为任务书指定方案(XEmbed),
                                             GTK3.14 起标记弃用但功能完好,此处有意使用 */
#include "settings.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "ai_capture.h"
#include "common.h"
#include "keysym_map.h"

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
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_commit_four),
                                 c->commit_after_four);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_commit_unique_four),
                                 c->commit_unique_four);
    gtk_toggle_button_set_active(
        GTK_TOGGLE_BUTTON(ui->chk_commit_first_at_four),
        c->commit_first_at_four);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_phrase_hint),
                                 c->phrase_hint);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_exact_freq_rank),
                                 c->exact_char_freq_rank);
    /* 英文上屏去向(§6):0=临时 temp,1=切英文模式 en(两行同一映射) */
    gtk_combo_box_set_active(
        GTK_COMBO_BOX(ui->combo_enter_en),
        strcmp(c->enter_english, "en") == 0 ? 1 : 0);
    gtk_combo_box_set_active(
        GTK_COMBO_BOX(ui->combo_shift_en),
        strcmp(c->shift_english, "en") == 0 ? 1 : 0);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_autostart),
                                 c->autostart);
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_quick_actions),
                                 c->quick_actions_enabled);
    gtk_entry_set_text(GTK_ENTRY(ui->ent_coin_hotkey), c->coin_hotkey);
    gtk_entry_set_text(GTK_ENTRY(ui->ent_shot_hotkey), c->shot_hotkey);
    /* AI 助手([ai] 段) */
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_ai_enabled),
                                 c->ai_enabled);
    gtk_entry_set_text(GTK_ENTRY(ui->ent_ai_base), c->ai_api_base);
    gtk_entry_set_text(GTK_ENTRY(ui->ent_ai_key), c->ai_api_key);
    gtk_entry_set_text(GTK_ENTRY(ui->ent_ai_model), c->ai_model);
    gtk_entry_set_text(GTK_ENTRY(ui->ent_ai_prompt), c->ai_system_prompt);
    gtk_spin_button_set_value(GTK_SPIN_BUTTON(ui->spin_ai_timeout),
                              c->ai_timeout);
    /* 输入统计(config.toml 顶层 stats_* 键,悬浮窗文件监视即时生效) */
    gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_stats_enabled),
                                 c->stats_enabled);
    gtk_spin_button_set_value(GTK_SPIN_BUTTON(ui->spin_stats_pause),
                              c->stats_pause_secs);
    gtk_spin_button_set_value(GTK_SPIN_BUTTON(ui->spin_stats_idle),
                              c->stats_idle_exclude_secs);
    /* 自定义查询(§15 候选右键菜单第 4 项) */
    gtk_entry_set_text(GTK_ENTRY(ui->ent_cq_label), c->custom_query_label);
    gtk_entry_set_text(GTK_ENTRY(ui->ent_cq_url), c->custom_query_url);
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
    c->commit_after_four =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_commit_four));
    c->commit_unique_four = gtk_toggle_button_get_active(
        GTK_TOGGLE_BUTTON(ui->chk_commit_unique_four));
    c->commit_first_at_four = gtk_toggle_button_get_active(
        GTK_TOGGLE_BUTTON(ui->chk_commit_first_at_four));
    c->phrase_hint =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_phrase_hint));
    c->exact_char_freq_rank = gtk_toggle_button_get_active(
        GTK_TOGGLE_BUTTON(ui->chk_exact_freq_rank));
    snprintf(c->enter_english, sizeof(c->enter_english), "%s",
             gtk_combo_box_get_active(GTK_COMBO_BOX(ui->combo_enter_en)) == 1
                 ? "en"
                 : "temp");
    snprintf(c->shift_english, sizeof(c->shift_english), "%s",
             gtk_combo_box_get_active(GTK_COMBO_BOX(ui->combo_shift_en)) == 1
                 ? "en"
                 : "temp");
    c->autostart =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_autostart));
    c->quick_actions_enabled = gtk_toggle_button_get_active(
        GTK_TOGGLE_BUTTON(ui->chk_quick_actions));
    snprintf(c->coin_hotkey, sizeof(c->coin_hotkey), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_coin_hotkey)));
    g_strstrip(c->coin_hotkey);
    snprintf(c->shot_hotkey, sizeof(c->shot_hotkey), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_shot_hotkey)));
    g_strstrip(c->shot_hotkey);
    /* AI 助手([ai] 段);字段越界由 config 钳制语义兜底 */
    c->ai_enabled =
        gtk_toggle_button_get_active(GTK_TOGGLE_BUTTON(ui->chk_ai_enabled));
    snprintf(c->ai_api_base, sizeof(c->ai_api_base), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_ai_base)));
    g_strstrip(c->ai_api_base);
    snprintf(c->ai_api_key, sizeof(c->ai_api_key), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_ai_key)));
    g_strstrip(c->ai_api_key);
    snprintf(c->ai_model, sizeof(c->ai_model), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_ai_model)));
    g_strstrip(c->ai_model);
    snprintf(c->ai_system_prompt, sizeof(c->ai_system_prompt), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_ai_prompt)));
    c->ai_timeout =
        (int)gtk_spin_button_get_value(GTK_SPIN_BUTTON(ui->spin_ai_timeout));
    /* 输入统计(config.toml 顶层 stats_* 键) */
    c->stats_enabled = gtk_toggle_button_get_active(
        GTK_TOGGLE_BUTTON(ui->chk_stats_enabled));
    c->stats_pause_secs =
        (int)gtk_spin_button_get_value(GTK_SPIN_BUTTON(ui->spin_stats_pause));
    c->stats_idle_exclude_secs = (int)gtk_spin_button_get_value(
        GTK_SPIN_BUTTON(ui->spin_stats_idle));
    /* 自定义查询(§15);留空 = 菜单不显示此项 */
    snprintf(c->custom_query_label, sizeof(c->custom_query_label), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_cq_label)));
    g_strstrip(c->custom_query_label);
    snprintf(c->custom_query_url, sizeof(c->custom_query_url), "%s",
             gtk_entry_get_text(GTK_ENTRY(ui->ent_cq_url)));
    g_strstrip(c->custom_query_url);
}

/* ---- 快捷键弹窗提示(写法非法/冲突避让结果,人话) ---- */
static void hotkey_msg_dialog(SettingsUi *ui, GtkMessageType type,
                              char *body)
{
    GtkWidget *dlg = gtk_message_dialog_new(GTK_WINDOW(ui->window),
                                            GTK_DIALOG_MODAL, type,
                                            GTK_BUTTONS_OK, "%s", body);
    gtk_window_set_title(GTK_WINDOW(dlg), "lyyIme 快捷键设置");
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
    g_free(body);
}

/* ---- 快捷键校验 + 冲突自动升级(合同 §12.3/§13)----
 * 返回 TRUE=可保存。两条规则:
 *  ① 写法非法(缺修饰/未知键)→ 报错并把该输入框还原为保存值,FALSE;
 *  ② 两键占用同一组合 → 刚改动的一侧按 原组合→+Alt→+Alt+Shift 逐级避让,
 *     最终值**直接回写输入框**(所见即所得)并弹窗说明;三级全占用 →
 *     报错还原该侧,FALSE。 */
static gboolean hotkeys_validate_and_resolve(SettingsUi *ui, LyyConfig *c,
                                             const LyyConfig *saved)
{
    char coin[128], shot[128];
    if (!lyy_hotkey_canon(c->coin_hotkey, coin, sizeof(coin))) {
        hotkey_msg_dialog(
            ui, GTK_MESSAGE_ERROR,
            g_strdup_printf("造词快捷键「%s」写法不合法:需至少一个修饰"
                            "(ctrl/alt/super/shift)+ 键名,如 ctrl+equal。"
                            "已还原为原值。",
                            c->coin_hotkey));
        gtk_entry_set_text(GTK_ENTRY(ui->ent_coin_hotkey),
                           saved->coin_hotkey);
        snprintf(c->coin_hotkey, sizeof(c->coin_hotkey), "%s",
                 saved->coin_hotkey);
        return FALSE;
    }
    if (!lyy_hotkey_canon(c->shot_hotkey, shot, sizeof(shot))) {
        hotkey_msg_dialog(
            ui, GTK_MESSAGE_ERROR,
            g_strdup_printf("截屏快捷键「%s」写法不合法:需至少一个修饰"
                            "(ctrl/alt/super/shift)+ 键名,如 ctrl+alt+a。"
                            "已还原为原值。",
                            c->shot_hotkey));
        gtk_entry_set_text(GTK_ENTRY(ui->ent_shot_hotkey), saved->shot_hotkey);
        snprintf(c->shot_hotkey, sizeof(c->shot_hotkey), "%s",
                 saved->shot_hotkey);
        return FALSE;
    }
    if (strcmp(coin, shot) != 0)
        return TRUE; /* 无冲突,原样保存 */

    /* 谁刚改动谁让位;两侧同改/都未改(手改配置文件后直接保存)则截屏让位 */
    char coin_saved[128] = "", shot_saved[128] = "";
    lyy_hotkey_canon(saved->coin_hotkey, coin_saved, sizeof(coin_saved));
    lyy_hotkey_canon(saved->shot_hotkey, shot_saved, sizeof(shot_saved));
    int edited_coin = strcmp(coin, coin_saved) != 0;
    int edited_shot = strcmp(shot, shot_saved) != 0;
    int coin_yields = edited_coin && !edited_shot;

    const char *target = coin_yields ? c->coin_hotkey : c->shot_hotkey;
    const char *other = coin_yields ? c->shot_hotkey : c->coin_hotkey;
    const char *tname = coin_yields ? "造词快捷键" : "截屏快捷键";
    const char *oname = coin_yields ? "截屏快捷键" : "造词快捷键";
    GtkWidget *tent = coin_yields ? ui->ent_coin_hotkey : ui->ent_shot_hotkey;
    const char *tsaved = coin_yields ? saved->coin_hotkey : saved->shot_hotkey;
    char *tbuf = coin_yields ? c->coin_hotkey : c->shot_hotkey;
    size_t tcap = sizeof(c->coin_hotkey);

    char next[128];
    if (lyy_hotkey_escalate(target, other, next, sizeof(next))) {
        char tprev[LYY_CFG_STR_BASE];
        snprintf(tprev, sizeof(tprev), "%s", target); /* 改写前留底供日志 */
        snprintf(tbuf, tcap, "%s", next);
        gtk_entry_set_text(GTK_ENTRY(tent), next); /* 所见即所得 */
        hotkey_msg_dialog(
            ui, GTK_MESSAGE_INFO,
            g_strdup_printf("「%s」%s 与「%s」冲突,已自动改为 %s"
                            "(原组合 → +Alt → +Alt+Shift 逐级避让)。",
                            tname, tprev, oname, next));
        lyy_log(&lyy_app()->log, "快捷键冲突自动升级:%s %s → %s", tname,
                tprev, next);
        return TRUE;
    }
    hotkey_msg_dialog(
        ui, GTK_MESSAGE_ERROR,
        g_strdup_printf("「%s」%s 与「%s」冲突,+Alt/+Shift 逐级避让后仍被"
                        "占用,已还原为原值;请手动指定其它组合。",
                        tname, target, oname));
    gtk_entry_set_text(GTK_ENTRY(tent), tsaved);
    snprintf(tbuf, tcap, "%s", tsaved);
    return FALSE;
}

static void on_ok(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    SettingsUi *ui = user_data;
    App *app = lyy_app();

    LyyConfig c = app->config;
    config_from_ui(ui, &c);
    /* 快捷键校验 + 冲突自动升级(合同 §13):非法/无法避让时已弹窗还原,
     * 不保存不关窗 */
    if (!hotkeys_validate_and_resolve(ui, &c, &app->config))
        return;
    app->config = c;
    if (lyy_config_save(app->config_path, &c) != 0) {
        lyy_log(&app->log, "ERROR 配置保存失败:%s", app->config_path);
    }
    /* 开机自启:落/删 ~/.config/autostart/lyyime-xim.desktop */
    if (lyy_config_apply_autostart(c.autostart) != 0)
        lyy_log(&app->log, "WARN 开机自启项写入失败");

    /* 保存即生效:core 引擎重建(重读词库与配置)+ 候选窗字体即时刷新;
     * AI 配置由 ai_capture 每键实时读取,同样立即生效 */
    lyy_engine_reload(app);
    lyy_app_reload_hotkey(app); /* 造词/截屏热键即时生效 */
    lyy_candwin_set_font_size(&app->candwin, c.font_size);
    /* §15 自定义查询(候选右键菜单第 4 项)即时生效 */
    lyy_candwin_set_query(&app->candwin, c.custom_query_label,
                          c.custom_query_url);
    lyy_log(&app->log,
            "设置已保存并生效:page_size=%d mixed=%d auto=%d punct=%d learn=%d four=%d first4=%d unique4=%d hint=%d freq_rank=%d enter_en=%s shift_en=%s font=%d autostart=%d qa=%d(%d条) ai=%d base=%s model=%s coin=%s shot=%s stats=%d pause=%d idle=%d",
            c.page_size, c.mixed_english, c.auto_commit_english,
            c.chinese_punct, c.learning, c.commit_after_four,
            c.commit_first_at_four, c.commit_unique_four, c.phrase_hint,
            c.exact_char_freq_rank,
            c.enter_english, c.shift_english, c.font_size, c.autostart,
            c.quick_actions_enabled, c.quick_actions_count, c.ai_enabled,
            c.ai_api_base, c.ai_model, c.coin_hotkey, c.shot_hotkey,
            c.stats_enabled, c.stats_pause_secs, c.stats_idle_exclude_secs);
    gtk_widget_hide(ui->window);
}

static void on_cancel(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    SettingsUi *ui = user_data;
    ui_from_config(ui); /* 还原显示 */
    gtk_widget_hide(ui->window);
}

/* ---- AI "测试连接":子进程执行 lyyime_ai.py --check,完成后弹窗汇报 ---- */
typedef struct {
    SettingsUi *ui;
    GString *out;
    guint io_id;
    GIOChannel *ch;
} AiCheck;

static void ai_check_drain(AiCheck *ck)
{
    gchar tmp[4096];
    gsize n = 0;
    for (;;) {
        GIOStatus st =
            g_io_channel_read_chars(ck->ch, tmp, sizeof(tmp), &n, NULL);
        if (st != G_IO_STATUS_NORMAL || n == 0)
            break;
        g_string_append_len(ck->out, tmp, (gssize)n);
    }
}

static void ai_check_finish(AiCheck *ck, gboolean ok)
{
    SettingsUi *ui = ck->ui;
    ui->ai_test_busy = 0;
    if (ck->io_id) {
        g_source_remove(ck->io_id);
        ck->io_id = 0;
    }
    ai_check_drain(ck);
    if (ck->ch) {
        g_io_channel_unref(ck->ch);
        ck->ch = NULL;
    }
    gchar *msg = g_strstrip(g_strdup(ck->out->str));
    gchar *body = ok
                      ? g_strdup_printf("AI 连接成功\n\n%s",
                                        msg[0] ? msg : "(无输出)")
                      : g_strdup_printf(
                            "AI 连接失败\n\n%s",
                            msg[0] ? msg
                                   : "(无输出,详见 ~/.local/share/lyyime/"
                                     "logs/xim.log)");
    GtkWidget *dlg = gtk_message_dialog_new(
        GTK_WINDOW(ui->window), GTK_DIALOG_MODAL,
        ok ? GTK_MESSAGE_INFO : GTK_MESSAGE_ERROR, GTK_BUTTONS_OK, "%s",
        body);
    gtk_window_set_title(GTK_WINDOW(dlg), "lyyIme AI 连接测试");
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
    g_free(body);
    g_free(msg);
    g_string_free(ck->out, TRUE);
    g_free(ck);
}

static void on_ai_check_exit(GPid pid, gint status, gpointer user_data)
{
    AiCheck *ck = user_data;
    g_spawn_close_pid(pid);
    ai_check_finish(ck, g_spawn_check_wait_status(status, NULL));
}

static gboolean on_ai_check_output(GIOChannel *ch, GIOCondition cond,
                                   gpointer user_data)
{
    (void)cond;
    AiCheck *ck = user_data;
    gchar tmp[4096];
    gsize n = 0;
    for (;;) {
        GIOStatus st =
            g_io_channel_read_chars(ch, tmp, sizeof(tmp), &n, NULL);
        if (st != G_IO_STATUS_NORMAL || n == 0)
            break;
        g_string_append_len(ck->out, tmp, (gssize)n);
    }
    return G_SOURCE_CONTINUE;
}

static void on_ai_test(GtkWidget *widget, gpointer user_data)
{
    (void)widget;
    SettingsUi *ui = user_data;
    if (ui->ai_test_busy)
        return;
    const char *helper = lyy_ai_find_helper();
    if (!helper) {
        GtkWidget *dlg = gtk_message_dialog_new(
            GTK_WINDOW(ui->window), GTK_DIALOG_MODAL, GTK_MESSAGE_ERROR,
            GTK_BUTTONS_OK,
            "未找到 AI 助手程序 lyyime-ai。\n"
            "请升级 lyyIme 组件(xim/install.sh 或 scripts/install-all.sh)。");
        gtk_dialog_run(GTK_DIALOG(dlg));
        gtk_widget_destroy(dlg);
        return;
    }
    /* 测试读取的是"已保存"的配置(按钮文案已注明先保存);--check 会
     * 打印配置摘要并做一次最小连通请求,失败原因原样展示。 */
    /* 助手须可直接执行(Rust 二进制 lyyime-ai;与 ai_capture.c 同规则) */
    gchar *argv[] = {(gchar *)helper, (gchar *)"--check", NULL};
    GPid pid = 0;
    gint outfd = -1;
    GError *err = NULL;
    AiCheck *ck = g_new0(AiCheck, 1);
    ck->ui = ui;
    ck->out = g_string_new("");
    if (!g_spawn_async_with_pipes(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL,
                                  NULL, &pid, NULL, &outfd, NULL, &err)) {
        GtkWidget *dlg = gtk_message_dialog_new(
            GTK_WINDOW(ui->window), GTK_DIALOG_MODAL, GTK_MESSAGE_ERROR,
            GTK_BUTTONS_OK, "启动测试失败:%s",
            err ? err->message : "未知错误");
        gtk_dialog_run(GTK_DIALOG(dlg));
        gtk_widget_destroy(dlg);
        g_clear_error(&err);
        g_string_free(ck->out, TRUE);
        g_free(ck);
        return;
    }
    ui->ai_test_busy = 1;
    ck->ch = g_io_channel_unix_new(outfd);
    g_io_channel_set_flags(ck->ch, G_IO_FLAG_NONBLOCK, NULL);
    ck->io_id = g_io_add_watch(ck->ch, G_IO_IN | G_IO_HUP | G_IO_ERR,
                               on_ai_check_output, ck);
    g_child_watch_add(pid, on_ai_check_exit, ck);
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
    ui->chk_commit_four = GTK_WIDGET(
        gtk_builder_get_object(builder, "chk_commit_after_four"));
    ui->chk_commit_unique_four = GTK_WIDGET(
        gtk_builder_get_object(builder, "chk_commit_unique_four"));
    ui->chk_commit_first_at_four = GTK_WIDGET(
        gtk_builder_get_object(builder, "chk_commit_first_at_four"));
    ui->chk_phrase_hint =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_phrase_hint"));
    ui->chk_exact_freq_rank =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_exact_freq_rank"));
    ui->combo_enter_en =
        GTK_WIDGET(gtk_builder_get_object(builder, "combo_enter_en"));
    ui->combo_shift_en =
        GTK_WIDGET(gtk_builder_get_object(builder, "combo_shift_en"));
    ui->chk_autostart =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_autostart"));
    ui->chk_quick_actions =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_quick_actions"));
    ui->ent_coin_hotkey =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_coin_hotkey"));
    ui->ent_shot_hotkey =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_shot_hotkey"));
    ui->chk_ai_enabled =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_ai_enabled"));
    ui->ent_ai_base =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_ai_base"));
    ui->ent_ai_key =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_ai_key"));
    ui->ent_ai_model =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_ai_model"));
    ui->ent_ai_prompt =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_ai_prompt"));
    ui->spin_ai_timeout =
        GTK_WIDGET(gtk_builder_get_object(builder, "spin_ai_timeout"));
    ui->ent_cq_label =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_cq_label"));
    ui->ent_cq_url =
        GTK_WIDGET(gtk_builder_get_object(builder, "ent_cq_url"));
    ui->btn_ai_test =
        GTK_WIDGET(gtk_builder_get_object(builder, "btn_ai_test"));
    ui->chk_stats_enabled =
        GTK_WIDGET(gtk_builder_get_object(builder, "chk_stats_enabled"));
    ui->spin_stats_pause =
        GTK_WIDGET(gtk_builder_get_object(builder, "spin_stats_pause"));
    ui->spin_stats_idle =
        GTK_WIDGET(gtk_builder_get_object(builder, "spin_stats_idle"));

    if (!ui->window || !ui->spin_page || !ui->spin_font || !ui->chk_mixed ||
        !ui->chk_auto || !ui->chk_punct || !ui->chk_learn ||
        !ui->chk_commit_four || !ui->chk_commit_unique_four ||
        !ui->chk_commit_first_at_four ||
        !ui->chk_phrase_hint || !ui->chk_exact_freq_rank ||
        !ui->combo_enter_en || !ui->combo_shift_en ||
        !ui->chk_autostart ||
        !ui->chk_quick_actions || !ui->chk_ai_enabled ||
        !ui->ent_ai_base || !ui->ent_ai_key || !ui->ent_ai_model ||
        !ui->ent_ai_prompt || !ui->spin_ai_timeout || !ui->btn_ai_test ||
        !ui->ent_coin_hotkey || !ui->chk_stats_enabled ||
        !ui->spin_stats_pause || !ui->spin_stats_idle ||
        !ui->ent_cq_label || !ui->ent_cq_url) {
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
    g_signal_connect(ui->btn_ai_test, "clicked", G_CALLBACK(on_ai_test), ui);

    /* 字体大一号:主题默认字号 +1(反馈:设置窗口与相关文字过小)。
     * override_font 在 GTK3.16 标记弃用但功能完好,与文件头 StatusIcon 同理。 */
    {
        GtkSettings *gs = gtk_settings_get_default();
        gchar *fname = NULL;
        g_object_get(gs, "gtk-font-name", &fname, NULL);
        PangoFontDescription *fd = pango_font_description_from_string(
            fname && fname[0] ? fname : "Sans 10");
        int sz = pango_font_description_get_size(fd);
        if (sz <= 0) {
            pango_font_description_free(fd);
            fd = pango_font_description_from_string("Sans 11");
            sz = pango_font_description_get_size(fd);
        }
        if (pango_font_description_get_size_is_absolute(fd))
            pango_font_description_set_absolute_size(fd, sz + PANGO_SCALE);
        else
            pango_font_description_set_size(fd, sz + PANGO_SCALE);
        gtk_widget_override_font(ui->window, fd); /* 级联到全部子控件 */
        pango_font_description_free(fd);
        g_free(fname);
    }

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
