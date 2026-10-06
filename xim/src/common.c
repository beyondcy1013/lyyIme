#include "common.h"
#include "keysym_map.h"

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
    app->core.lyyime_set_commit_unique_four(app->engine, app->config.commit_unique_four);
    /* 四码首选上屏(§6,可选符号):旧 core 库缺符号时沿用 core 默认(开)。 */
    if (app->core.first_four_ok) {
        app->core.lyyime_set_commit_first_at_four(app->engine,
                                                  app->config.commit_first_at_four);
    } else {
        lyy_log(&app->log, "INFO core 库无四码首选上屏符号,"
                           "commit_first_at_four 不生效(请更新 liblyyime_core.so)");
    }
    app->core.lyyime_set_phrase_hint(app->engine, app->config.phrase_hint);
    if (app->core.pinyin_ok) {
        app->core.lyyime_set_pinyin_only(app->engine, app->config.pinyin_only);
    } else if (app->config.pinyin_only) {
        lyy_log(&app->log, "INFO core 库无输入方案符号,pinyin_only 不生效"
                           "(请更新 liblyyime_core.so)");
    }
    if (app->core.learn_ok) {
        app->core.lyyime_set_learning(app->engine, app->config.learning);
    } else if (!app->config.learning) {
        lyy_log(&app->log, "INFO core 库无学习开关符号,learning 不生效"
                           "(请更新 liblyyime_core.so)");
    }
    /* 上屏后联想(可选符号):旧 core 库缺符号时无法配置联想,
     * 保留原输入行为,不整体降级。 */
    if (app->core.pred_ok) {
        app->core.lyyime_set_next_word_prediction(
            app->engine, app->config.next_word_prediction);
    } else {
        lyy_log(&app->log, "INFO core 库无上屏后联想符号,"
                           "next_word_prediction 不生效(请更新 liblyyime_core.so)");
    }
    /* 中文标点默认(可选符号组):旧 core 库缺符号时无法配置,
     * 沿用 core 默认(中文标点开),保留原输入行为,不整体降级。 */
    if (app->core.punct_ok) {
        app->core.lyyime_set_chinese_punctuation(app->engine,
                                                 app->config.chinese_punct);
        app->punct_runtime = app->config.chinese_punct;
    } else {
        lyy_log(&app->log, "INFO core 库无中英文标点切换符号,"
                           "chinese_punct 不生效且 Ctrl+. 不拦截"
                           "(请更新 liblyyime_core.so)");
    }
    /* 精确单字按词频排位(§5,可选符号):旧 core 库缺符号时沿用 core 默认。 */
    if (app->core.freq_rank_ok) {
        app->core.lyyime_set_exact_char_freq_rank(app->engine,
                                                  app->config.exact_char_freq_rank);
    } else {
        lyy_log(&app->log, "INFO core 库无精确单字频率排位符号(§5),"
                           "exact_char_freq_rank 不生效(请更新 liblyyime_core.so)");
    }
    /* 英文上屏去向(§6,可选符号组):回车/Shift 上屏英文原串后的模式去向;
     * 旧 core 库缺符号时沿用 core 各自默认,不降级。 */
    if (app->core.en_mode_ok) {
        app->core.lyyime_set_enter_english(
            app->engine, !strcmp(app->config.enter_english, "en"));
        app->core.lyyime_set_shift_english(
            app->engine, !strcmp(app->config.shift_english, "en"));
    } else {
        lyy_log(&app->log, "INFO core 库无英文上屏去向符号(§6),"
                           "enter_english/shift_english 不生效(请更新 liblyyime_core.so)");
    }
    /* 快速功能键(合同 §14):总开关 + 触发词表注入(可选符号组,旧库跳过) */
    if (app->core.qa_ok) {
        app->core.lyyime_set_quick_actions_enabled(
            app->engine, app->config.quick_actions_enabled);
        app->core.lyyime_clear_quick_actions(app->engine);
        for (int i = 0; i < app->config.quick_actions_count; i++) {
            LyyQuickAction *a = &app->config.quick_actions[i];
            if (app->core.lyyime_add_quick_action(app->engine, a->trigger,
                                                  a->label,
                                                  a->command) != 0)
                lyy_log(&app->log, "WARN 快速功能键注入失败(触发词 %s)",
                        a->trigger);
        }
        lyy_log(&app->log, "快速功能键已注入:%d 条(开关=%d)",
                app->config.quick_actions_count,
                app->config.quick_actions_enabled);
    } else {
        lyy_log(&app->log, "INFO core 库无快速功能键符号(§14),该功能不可用"
                           "(请更新 liblyyime_core.so 后重启)");
    }
    lyy_log(&app->log, "core 引擎已创建:data_dir=%s", app->data_dir);
    lyy_menu_trigger_apply(app); /* 菜单触发器:与引擎解耦,引擎就绪后配置 */
    return 0;
}

/* 菜单触发器:创建(首次)并按当前配置重配;configure 内部即复位
 * 尾串与待执行(配置重载=硬边界)。mt_ok=0(旧 core 库)时为空操作。 */
void lyy_menu_trigger_apply(App *app)
{
    if (!app->core.mt_ok)
        return;
    if (!app->menu_trigger) {
        app->menu_trigger = app->core.lyyime_menu_trigger_new();
        if (!app->menu_trigger) {
            lyy_log(&app->log, "WARN 菜单触发器创建失败(内存?)");
            return;
        }
    }
    if (app->core.lyyime_menu_trigger_configure(
            app->menu_trigger, app->config.menu_trigger_enabled,
            app->config.menu_trigger_key,
            app->config.menu_trigger_disabled) != 0) {
        lyy_log(&app->log, "WARN 菜单触发配置失败(disabled=\"%s\")",
                app->config.menu_trigger_disabled);
        return;
    }
    /* 截屏提示须显示「实际生效」的快捷键:与 lyy_app_reload_hotkey 同一
     * 解析与回退(配置合法用配置,非法回退 ctrl+alt+a),避开设置页
     * 「先重载引擎(触发本函数)后重载热键」的顺序差;旧库符号为 NULL 即跳。 */
    if (app->core.lyyime_menu_trigger_set_shot_hotkey) {
        uint32_t mods = 0, sym = 0;
        const char *spec =
            lyy_hotkey_parse(app->config.shot_hotkey, &mods, &sym)
                ? app->config.shot_hotkey : "ctrl+alt+a";
        app->core.lyyime_menu_trigger_set_shot_hotkey(app->menu_trigger, spec);
    }
    lyy_log(&app->log, "菜单触发:enabled=%d key=F%d disabled=[%s]",
            app->config.menu_trigger_enabled, app->config.menu_trigger_key,
            app->config.menu_trigger_disabled);
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

void lyy_request_show_settings_page(App *app, int page)
{
    app->settings_page = page;
    app->settings_requested = 1;
}

/* 让二次启动"唤起"已存在实例:SIGUSR1 → 设置窗,SIGUSR2 → 主窗口 */
static void on_signal(int sig)
{
    App *app = lyy_app();
    if (sig == SIGUSR1)
        app->settings_requested = 1; /* volatile 语义字段,仅置位 */
    else if (sig == SIGUSR2)
        app->mainwin_requested = 1;
    else
        app->quit_requested = 1;
}

void lyy_install_signal_handlers(void)
{
    signal(SIGUSR1, on_signal);
    signal(SIGUSR2, on_signal);
    signal(SIGTERM, on_signal);
    signal(SIGINT, on_signal);
}
