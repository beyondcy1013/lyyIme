#define GDK_DISABLE_DEPRECATION_WARNINGS /* GtkStatusIcon 为任务书指定方案(XEmbed),
                                             GTK3.14 起标记弃用但功能完好,此处有意使用 */
#include "settings.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "ai_capture.h"
#include "common.h"
#include "keysym_map.h"
#include "skin.h"

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
    gtk_toggle_button_set_active(
        GTK_TOGGLE_BUTTON(ui->chk_next_word_prediction),
        c->next_word_prediction);
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
    /* 菜单触发页(动态控件;目录行勾选=禁止文字触发) */
    if (ui->chk_mt_enabled)
        gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->chk_mt_enabled),
                                     c->menu_trigger_enabled);
    if (ui->combo_mt_key) {
        int k = c->menu_trigger_key;
        if (k < 1 || k > 12)
            k = 7;
        gtk_combo_box_set_active(GTK_COMBO_BOX(ui->combo_mt_key), k - 1);
    }
    for (int i = 0; i < ui->mt_count; i++)
        gtk_toggle_button_set_active(
            GTK_TOGGLE_BUTTON(ui->mt_rows[i]),
            lyy_config_csv_contains(c->menu_trigger_disabled, ui->mt_ids[i]));
    /* 皮肤页:选中配置对应卡片(未知/空 id 经注册表归一 system);
     * 只复位草稿,不动真实候选窗 */
    if (ui->skin_buttons[0]) {
        const LyySkin *cur = lyy_skin_find(c->skin);
        int n = lyy_skin_count();
        int cap = (int)(sizeof(ui->skin_buttons) / sizeof(ui->skin_buttons[0]));
        if (n > cap)
            n = cap;
        for (int i = 0; i < n; i++)
            if (ui->skin_buttons[i] && lyy_skin_at(i) == cur)
                gtk_toggle_button_set_active(
                    GTK_TOGGLE_BUTTON(ui->skin_buttons[i]), TRUE);
    }
}

/* 逗号列表尾部追加 token(容量不足静默跳过;行数受目录上限约束) */
static void csv_append(char *buf, size_t cap, const char *tok)
{
    size_t len = strlen(buf);
    size_t need = strlen(tok) + (len ? 1 : 0);
    if (len + need + 1 > cap)
        return;
    if (len)
        strcat(buf, ",");
    strcat(buf, tok);
}

/* UI → LyyConfig;返回 0=可保存,-1=黑名单合并不下(安全开关不得
 * 静默保存失败,调用方必须中止保存并提示用户) */
static int config_from_ui(SettingsUi *ui, LyyConfig *c)
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
    c->next_word_prediction = gtk_toggle_button_get_active(
        GTK_TOGGLE_BUTTON(ui->chk_next_word_prediction));
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
    /* 菜单触发页:总开关/确认键/目录黑名单(勾选=禁止文字触发) */
    if (ui->chk_mt_enabled)
        c->menu_trigger_enabled = gtk_toggle_button_get_active(
            GTK_TOGGLE_BUTTON(ui->chk_mt_enabled));
    if (ui->combo_mt_key) {
        int k = gtk_combo_box_get_active(GTK_COMBO_BOX(ui->combo_mt_key)) + 1;
        c->menu_trigger_key = (k >= 1 && k <= 12) ? k : 7;
    }
    if (ui->mt_count > 0) {
        /* 结果 = 勾选 id(目录序) + 原串中不在目录内的 token(未知 id
         * 前向兼容,原序保留);mt_count==0(旧 core)时原配置不动 */
        char checked[2048] = "", known[2048] = "";
        char base[LYY_CFG_STR_CMD];
        snprintf(base, sizeof(base), "%s", c->menu_trigger_disabled);
        for (int i = 0; i < ui->mt_count; i++) {
            csv_append(known, sizeof(known), ui->mt_ids[i]);
            if (gtk_toggle_button_get_active(
                    GTK_TOGGLE_BUTTON(ui->mt_rows[i])))
                csv_append(checked, sizeof(checked), ui->mt_ids[i]);
        }
        char merged[LYY_CFG_STR_CMD];
        if (lyy_config_merge_menu_disabled(merged, sizeof(merged), base,
                                           known, checked) != 0) {
            /* 黑名单合并不下=安全开关丢失风险:整体中止保存 */
            lyy_log(&lyy_app()->log,
                    "ERROR 菜单触发黑名单超出配置容量,保存已中止");
            return -1;
        }
        snprintf(c->menu_trigger_disabled,
                 sizeof(c->menu_trigger_disabled), "%s", merged);
    }
    /* 皮肤页:取勾选卡片对应的注册表 id(防御性归一在写回时再做一次) */
    if (ui->skin_buttons[0]) {
        int n = lyy_skin_count();
        int cap = (int)(sizeof(ui->skin_buttons) / sizeof(ui->skin_buttons[0]));
        if (n > cap)
            n = cap;
        for (int i = 0; i < n; i++) {
            if (ui->skin_buttons[i] &&
                gtk_toggle_button_get_active(
                    GTK_TOGGLE_BUTTON(ui->skin_buttons[i]))) {
                snprintf(c->skin, sizeof(c->skin), "%s", lyy_skin_at(i)->id);
                break;
            }
        }
    }
    return 0;
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
    /* 保留键:Ctrl+. 固定为中文态中英文标点切换,不允许再分配给
     * 造词/截屏——保存拒绝但输入框保持可编辑(不还原、不静默改写)。 */
    if (!strcmp(coin, "ctrl+period") || !strcmp(shot, "ctrl+period")) {
        hotkey_msg_dialog(ui, GTK_MESSAGE_ERROR,
                          "Ctrl+. 已用于切换中英文标点,"
                          "请选择其他快捷键。");
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
    if (config_from_ui(ui, &c) != 0) {
        /* 菜单触发黑名单合并不下:禁用开关可能失效,绝不静默保存 */
        hotkey_msg_dialog(ui, GTK_MESSAGE_ERROR,
                          "菜单触发黑名单超出配置容量,保存已取消。\n"
                          "请减少勾选项或精简 config.toml 中 "
                          "menu_trigger_disabled 的自定义条目。");
        return;
    }
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
    lyy_candwin_set_skin(&app->candwin, c.skin); /* 皮肤即时生效 */
    /* §15 自定义查询(候选右键菜单第 4 项)即时生效 */
    lyy_candwin_set_query(&app->candwin, c.custom_query_label,
                          c.custom_query_url);
    lyy_log(&app->log,
            "设置已保存并生效:page_size=%d mixed=%d auto=%d punct=%d learn=%d four=%d first4=%d unique4=%d hint=%d pred=%d freq_rank=%d enter_en=%s shift_en=%s font=%d autostart=%d qa=%d(%d条) ai=%d base=%s model=%s coin=%s shot=%s stats=%d pause=%d idle=%d mt=%d mtkey=F%d mtdis=[%s]",
            c.page_size, c.mixed_english, c.auto_commit_english,
            c.chinese_punct, c.learning, c.commit_after_four,
            c.commit_first_at_four, c.commit_unique_four, c.phrase_hint,
            c.next_word_prediction, c.exact_char_freq_rank,
            c.enter_english, c.shift_english, c.font_size, c.autostart,
            c.quick_actions_enabled, c.quick_actions_count, c.ai_enabled,
            c.ai_api_base, c.ai_model, c.coin_hotkey, c.shot_hotkey,
            c.stats_enabled, c.stats_pause_secs, c.stats_idle_exclude_secs,
            c.menu_trigger_enabled, c.menu_trigger_key,
            c.menu_trigger_disabled);
    lyy_log(&app->log, "设置已保存:skin=%s", c.skin);
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

/* ---- 菜单触发页(2026-09-30):目录勾选行运行时由 core 可选符号组
 * lyyime_menu_trigger_* 生成(单一目录源,不写死 label/id);旧库缺符号
 * 时页内降级为说明文字。勾选语义=「禁止文字触发」,只影响上屏文字匹配,
 * 与快捷键页的快速功能键(§14 字母缓冲触发)互不相关。 */
static void build_menu_tab(SettingsUi *ui)
{
    App *app = lyy_app();
    if (!ui->notebook)
        return;
    GtkWidget *outer = gtk_box_new(GTK_ORIENTATION_VERTICAL, 10);
    gtk_container_set_border_width(GTK_CONTAINER(outer), 10);

    GtkWidget *desc = gtk_label_new(
        "输入的中文短语正常上屏;若命中下方菜单功能名,候选条提示\n"
        "「匹配了菜单功能,按 Fn 进入该功能」,按下确认键才执行。");
    gtk_label_set_xalign(GTK_LABEL(desc), 0.0);
    gtk_box_pack_start(GTK_BOX(outer), desc, FALSE, FALSE, 0);

    ui->chk_mt_enabled = gtk_check_button_new_with_label(
        "启用菜单触发(上屏文字命中菜单功能名后按功能键进入)");
    gtk_box_pack_start(GTK_BOX(outer), ui->chk_mt_enabled, FALSE, FALSE, 0);

    GtkWidget *krow = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    gtk_box_pack_start(GTK_BOX(krow), gtk_label_new("确认键:"), FALSE, FALSE, 0);
    ui->combo_mt_key = gtk_combo_box_text_new();
    for (int i = 1; i <= 12; i++) {
        char t[8];
        snprintf(t, sizeof(t), "F%d", i);
        gtk_combo_box_text_append_text(GTK_COMBO_BOX_TEXT(ui->combo_mt_key),
                                       t);
    }
    gtk_combo_box_set_active(GTK_COMBO_BOX(ui->combo_mt_key), 6); /* 默认 F7 */
    gtk_box_pack_start(GTK_BOX(krow), ui->combo_mt_key, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(outer), krow, FALSE, FALSE, 0);

    GtkWidget *tip = gtk_label_new(
        "文字先正常上屏,匹配后按确认键执行;继续输入或切换窗口即取消\n"
        "下方勾选 = 禁止该功能被文字触发(不影响「快捷键」页的快速功能键)");
    gtk_label_set_xalign(GTK_LABEL(tip), 0.0);
    gtk_box_pack_start(GTK_BOX(outer), tip, FALSE, FALSE, 0);

    GtkWidget *sw = gtk_scrolled_window_new(NULL, NULL);
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(sw), GTK_POLICY_NEVER,
                                   GTK_POLICY_AUTOMATIC);
    gtk_widget_set_size_request(sw, -1, 220);
    GtkWidget *rows = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);

    if (app->core.mt_ok) {
        int n = app->core.lyyime_menu_trigger_count();
        int cap = (int)(sizeof(ui->mt_rows) / sizeof(ui->mt_rows[0]));
        if (n > cap)
            n = cap;
        for (int i = 0; i < n; i++) {
            char id[64], label[128];
            app->core.lyyime_menu_trigger_id(i, id, (int)sizeof(id));
            app->core.lyyime_menu_trigger_label(i, label, (int)sizeof(label));
            snprintf(ui->mt_ids[i], sizeof(ui->mt_ids[0]), "%s", id);
            /* 行标签只显示中文功能名(技术 id 放 tooltip,不干扰用户) */
            ui->mt_rows[i] = gtk_check_button_new_with_label(
                label[0] ? label : "?");
            char tip[96];
            snprintf(tip, sizeof(tip), "文字触发 id:%s", id);
            gtk_widget_set_tooltip_text(ui->mt_rows[i], tip);
            gtk_box_pack_start(GTK_BOX(rows), ui->mt_rows[i], FALSE, FALSE, 0);
        }
        ui->mt_count = n;
    } else {
        GtkWidget *warn = gtk_label_new(
            "当前 core 库缺少菜单触发符号组(lyyime_menu_trigger_*),\n"
            "该特性不可用;请更新 liblyyime_core.so 后重启 lyyime-xim。");
        gtk_label_set_xalign(GTK_LABEL(warn), 0.0);
        gtk_box_pack_start(GTK_BOX(rows), warn, FALSE, FALSE, 0);
    }
    gtk_container_add(GTK_CONTAINER(sw), rows);
    gtk_box_pack_start(GTK_BOX(outer), sw, TRUE, TRUE, 0);

    GtkWidget *tab = gtk_label_new("菜单触发");
    gtk_notebook_append_page(GTK_NOTEBOOK(ui->notebook), outer, tab);
    gtk_widget_show_all(tab);
    gtk_widget_show_all(outer);
}

/* ---- 皮肤页(页 6):注册表驱动画廊 -----------------------------------
 * 预览与真实候选窗共用同一份 CSS 生成器(skin.c):卡内 .lyy-frame 预览
 * 子树各挂独立 provider(APPLICATION 优先级,只挂本子树),不透染真实
 * 候选窗与邻卡;选择只改草稿,确定保存后才写盘并对候选窗即时生效。
 * 作用域仅 lyyIme 自绘候选窗,不影响 IBus 系统面板。 */

/* 迷你候选窗预览:与真实窗口同一份类名,所见即所得(不可交互) */
static GtkWidget *skin_preview_build(void)
{
    GtkWidget *frame = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_style_context_add_class(gtk_widget_get_style_context(frame),
                                "lyy-frame");

    GtkWidget *header = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    gtk_style_context_add_class(gtk_widget_get_style_context(header),
                                "lyy-header");
    GtkWidget *preedit = gtk_label_new("nihao");
    gtk_style_context_add_class(gtk_widget_get_style_context(preedit),
                                "lyy-preedit");
    gtk_widget_set_halign(preedit, GTK_ALIGN_START);
    GtkWidget *page = gtk_label_new("1/3");
    gtk_style_context_add_class(gtk_widget_get_style_context(page),
                                "lyy-page");
    gtk_box_pack_start(GTK_BOX(header), preedit, FALSE, FALSE, 0);
    gtk_box_pack_end(GTK_BOX(header), page, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(frame), header, FALSE, FALSE, 0);

    const char *cells[2][3] = { { "1.", "你好", "nihao" },
                                { "2.", "你们", "nimen" } };
    for (int i = 0; i < 2; i++) {
        GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gtk_style_context_add_class(gtk_widget_get_style_context(row),
                                    "lyy-row");
        if (i == 0)
            gtk_style_context_add_class(gtk_widget_get_style_context(row),
                                        "lyy-first");
        const char *cls[3] = { "lyy-num", "lyy-word", "lyy-comment" };
        for (int j = 0; j < 3; j++) {
            GtkWidget *cell = gtk_label_new(cells[i][j]);
            gtk_style_context_add_class(gtk_widget_get_style_context(cell),
                                        cls[j]);
            gtk_widget_set_halign(cell, GTK_ALIGN_START);
            gtk_box_pack_start(GTK_BOX(row), cell, FALSE, FALSE, 0);
        }
        gtk_box_pack_start(GTK_BOX(frame), row, FALSE, FALSE, 0);
    }
    return frame;
}

/* 全部预览卡按当前草稿字号重建皮肤 CSS(字号 spin 改动时调用;
 * 只刷预览,不动真实候选窗——字体对候选窗的生效仍在保存后) */
static void skin_previews_refresh(SettingsUi *ui)
{
    if (!ui->skin_providers[0])
        return;
    int font =
        (int)gtk_spin_button_get_value(GTK_SPIN_BUTTON(ui->spin_font));
    gboolean dark = lyy_skin_system_is_dark();
    int n = lyy_skin_count();
    int cap = (int)(sizeof(ui->skin_providers) / sizeof(ui->skin_providers[0]));
    if (n > cap)
        n = cap;
    for (int i = 0; i < n; i++) {
        if (!ui->skin_providers[i])
            continue;
        char *css = lyy_skin_css(lyy_skin_at(i)->id, dark, font);
        gtk_css_provider_load_from_data(ui->skin_providers[i], css, -1,
                                        NULL);
        g_free(css);
    }
}

static void on_font_changed(GtkSpinButton *spin, gpointer user_data)
{
    (void)spin;
    skin_previews_refresh(user_data);
}

/* 整张卡片都可点选:EventBox 窗置于子控件之上,命中即激活对应单选 */
static gboolean on_skin_card_press(GtkWidget *w, GdkEventButton *ev,
                                   gpointer user_data)
{
    SettingsUi *ui = user_data;
    int idx = GPOINTER_TO_INT(g_object_get_data(G_OBJECT(w), "lyy-skin-idx"));
    if (ev->button == 1 && idx >= 0 && idx < 9 && ui->skin_buttons[idx]) {
        gtk_toggle_button_set_active(GTK_TOGGLE_BUTTON(ui->skin_buttons[idx]),
                                     TRUE);
        return TRUE;
    }
    return FALSE;
}

static void build_skin_tab(SettingsUi *ui)
{
    if (!ui->notebook)
        return;
    GtkWidget *outer = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    gtk_container_set_border_width(GTK_CONTAINER(outer), 12);

    GtkWidget *title = gtk_label_new(NULL);
    gtk_label_set_markup(GTK_LABEL(title),
                         "<b>让每一次输入,都有自己的风格</b>");
    gtk_box_pack_start(GTK_BOX(outer), title, FALSE, FALSE, 0);

    GtkWidget *note = gtk_label_new(
        "点击预览,确定后生效 · 仅用于 lyyIme 自绘候选窗");
    gtk_label_set_xalign(GTK_LABEL(note), 0.0);
    gtk_widget_set_opacity(note, 0.7);
    gtk_box_pack_start(GTK_BOX(outer), note, FALSE, FALSE, 0);

    /* 双列卡片区:纵向滚动。窗口 resizable=FALSE,自然尺寸取各页最大——
     * 现有最高页把窗口钉在 825x544(menu_trigger_e2e 实测断言);本页
     * 自然高须不超过它:边框/标题/说明/间距约 92px,滚动区上限取 340,
     * 保证窗口几何不变;最小 300 防极端矮屏裁到不可用 */
    GtkWidget *sw = gtk_scrolled_window_new(NULL, NULL);
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(sw),
                                   GTK_POLICY_NEVER, GTK_POLICY_AUTOMATIC);
    gtk_scrolled_window_set_min_content_height(GTK_SCROLLED_WINDOW(sw), 296);
    gtk_scrolled_window_set_max_content_height(GTK_SCROLLED_WINDOW(sw), 332);

    GtkWidget *grid = gtk_grid_new();
    gtk_grid_set_row_spacing(GTK_GRID(grid), 12);
    gtk_grid_set_column_spacing(GTK_GRID(grid), 12);
    gtk_grid_set_column_homogeneous(GTK_GRID(grid), TRUE);

    gboolean dark = lyy_skin_system_is_dark();
    int font =
        (int)gtk_spin_button_get_value(GTK_SPIN_BUTTON(ui->spin_font));
    int n = lyy_skin_count();
    int cap = (int)(sizeof(ui->skin_buttons) / sizeof(ui->skin_buttons[0]));
    if (n > cap)
        n = cap;
    GtkWidget *leader = NULL;
    for (int i = 0; i < n; i++) {
        const LyySkin *s = lyy_skin_at(i);

        GtkWidget *card = gtk_event_box_new();
        gtk_event_box_set_visible_window(GTK_EVENT_BOX(card), FALSE);
        gtk_event_box_set_above_child(GTK_EVENT_BOX(card), TRUE);
        gtk_widget_add_events(card, GDK_BUTTON_PRESS_MASK);
        g_object_set_data(G_OBJECT(card), "lyy-skin-idx", GINT_TO_POINTER(i));
        g_signal_connect(card, "button-press-event",
                         G_CALLBACK(on_skin_card_press), ui);

        GtkWidget *vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
        gtk_container_add(GTK_CONTAINER(card), vbox);

        /* 头部:单选(皮肤名)+ 右侧分类标签。
         * 单选带数字助记符「(_N) 名字」:显示为 (N) 名字、N 带下划线,
         * Alt+N 窗口级直达该卡 —— 无需滚动/坐标即可选中,键盘与
         * 自动化(E2E)都走这条确定性路径 */
        GtkWidget *head = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        gchar *rlabel =
            g_strdup_printf("(_%d) %s", i + 1, s->name);
        GtkWidget *radio;
        if (i == 0) {
            radio = gtk_radio_button_new_with_mnemonic(NULL, rlabel);
            leader = radio;
        } else {
            radio = gtk_radio_button_new_with_mnemonic_from_widget(
                GTK_RADIO_BUTTON(leader), rlabel);
        }
        g_free(rlabel);
        ui->skin_buttons[i] = radio;
        gtk_box_pack_start(GTK_BOX(head), radio, FALSE, FALSE, 0);
        GtkWidget *cat = gtk_label_new(NULL);
        gchar *mark = g_markup_printf_escaped("<small>%s</small>",
                                              s->category);
        gtk_label_set_markup(GTK_LABEL(cat), mark);
        g_free(mark);
        gtk_widget_set_opacity(cat, 0.6);
        gtk_box_pack_end(GTK_BOX(head), cat, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(vbox), head, FALSE, FALSE, 0);

        GtkWidget *desc = gtk_label_new(s->description);
        gtk_label_set_xalign(GTK_LABEL(desc), 0.0);
        gtk_label_set_line_wrap(GTK_LABEL(desc), TRUE);
        gtk_label_set_lines(GTK_LABEL(desc), 2);
        gtk_label_set_max_width_chars(GTK_LABEL(desc), 26);
        gtk_widget_set_opacity(desc, 0.7);
        gtk_box_pack_start(GTK_BOX(vbox), desc, FALSE, FALSE, 0);

        GtkWidget *pv = skin_preview_build();
        ui->skin_previews[i] = pv;
        ui->skin_providers[i] = gtk_css_provider_new();
        char *css = lyy_skin_css(s->id, dark, font);
        gtk_css_provider_load_from_data(ui->skin_providers[i], css, -1,
                                        NULL);
        g_free(css);
        lyy_skin_apply_tree(pv, ui->skin_providers[i]); /* 只挂本预览子树 */
        gtk_box_pack_start(GTK_BOX(vbox), pv, FALSE, FALSE, 0);

        gtk_grid_attach(GTK_GRID(grid), card, i % 2, i / 2, 1, 1);
    }
    gtk_container_add(GTK_CONTAINER(sw), grid);
    gtk_box_pack_start(GTK_BOX(outer), sw, TRUE, TRUE, 0);

    GtkWidget *tab = gtk_label_new("皮肤");
    gtk_notebook_append_page(GTK_NOTEBOOK(ui->notebook), outer, tab);
    gtk_widget_show_all(tab);
    gtk_widget_show_all(outer);
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
    ui->chk_next_word_prediction = GTK_WIDGET(
        gtk_builder_get_object(builder, "chk_next_word_prediction"));
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
        !ui->chk_phrase_hint || !ui->chk_next_word_prediction ||
        !ui->chk_exact_freq_rank ||
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

    /* 菜单触发页/皮肤页:运行时生成(不依赖 settings.ui 静态定义);
     * 页签变多后允许标签栏滚动,防止窗口被页签撑宽 */
    ui->notebook = GTK_WIDGET(gtk_builder_get_object(builder, "notebook"));
    build_menu_tab(ui);
    build_skin_tab(ui);
    if (ui->notebook)
        gtk_notebook_set_scrollable(GTK_NOTEBOOK(ui->notebook), TRUE);
    /* 字号草稿联动:皮肤页预览跟随 spin_font 即时缩放(不动真实候选窗) */
    g_signal_connect(ui->spin_font, "value-changed",
                     G_CALLBACK(on_font_changed), ui);

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

    /* 自身 XIM server 与本窗同进程同主循环:设置窗内 GtkEntry(含
     * SpinButton 子类)若沿用 xim 模块会在 realize 时同步 XOpenIM
     * 打自己的服务,主循环自锁(2026-09-30 gdb 栈实证:
     * present→realize→im-xim→_XimOpenIM→XIfEvent→poll)。本进程内
     * 控件一律 gtk-im-context-simple;仅本进程生效,拉起的工具与
     * 外部客户端的中文输入不受影响(中文仍可粘贴进这些输入框)。 */
    {
        GSList *objs = gtk_builder_get_objects(builder);
        for (GSList *l = objs; l; l = l->next)
            if (GTK_IS_ENTRY(l->data))
                g_object_set(l->data, "im-module",
                             "gtk-im-context-simple", NULL);
        g_slist_free(objs); /* 释放链表;控件引用仍归 builder/父容器 */
    }

    ui->built = 1;
    ui_from_config(ui);
    g_object_unref(builder); /* gtk_builder 保活控件引用,g_object_unref 安全 */
}

void lyy_settings_show(SettingsUi *ui)
{
    /* 打开自身设置窗是硬边界:托盘/右键/CLI/第二实例 --settings-page
     * 都经此缝;无 WM 环境客户端不发焦点 UNSET,残留待执行会越窗
     * 误执行(2026-09-30 E2E F 实证)——与弹窗成败无关,先复位。 */
    lyy_mt_reset(lyy_app());
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
    lyy_log(&lyy_app()->log, "settings show: 设置窗已呈现");
}

void lyy_settings_show_page(SettingsUi *ui, int page)
{
    lyy_settings_show(ui);
    if (ui->built && ui->notebook && page >= 0)
        gtk_notebook_set_current_page(GTK_NOTEBOOK(ui->notebook), page);
}
