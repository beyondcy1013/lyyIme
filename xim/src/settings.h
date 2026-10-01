/*
 * 设置窗口(GtkBuilder 加载 xim/res/settings.ui,多标签页 GtkNotebook)
 *
 * 页签:常规(候选数/字体/标点/学习/自启)、输入(混输/直通/四码/词组提示/
 *       精确单字词频排位/回车与Shift英文上屏去向)、快捷键(造词/截屏/快速功能键)、
 *       AI 助手(OpenAI 兼容接口:
 *       地址/密钥/模型/系统提示/超时 + 测试连接)、
 *       输入统计(全局 stats_*:总开关/停顿秒数/最长停顿;显示端=悬浮窗)、
 *       菜单触发(代码动态追加:总开关/确认键/目录黑名单勾选)、
 *       皮肤(代码动态追加:注册表驱动画廊,仅作用于 lyyIme 自绘候选窗)。
 * 保存:确定=保存全部页,写 ~/.config/lyyime/config.toml(保留未知行与注释)
 *       → 开机自启落 ~/.config/autostart → core 引擎重建(配置即时生效)
 *       + 候选窗字体即时生效;AI 配置由 ai_capture 每次按键实时读取,
 *       保存即生效,无需重启。取消=还原全部页显示。
 */
#ifndef LYY_SETTINGS_H_
#define LYY_SETTINGS_H_

#include <gtk/gtk.h>

typedef struct SettingsUi {
    GtkWidget *window;
    GtkWidget *spin_page;
    GtkWidget *spin_font;
    GtkWidget *chk_mixed;
    GtkWidget *chk_auto;
    GtkWidget *chk_punct;
    GtkWidget *chk_learn;
    GtkWidget *chk_commit_four;
    GtkWidget *chk_commit_unique_four;
    GtkWidget *chk_commit_first_at_four; /* 四码首选上屏(五笔首选且候选条只此一条直接上屏) */
    GtkWidget *chk_phrase_hint; /* 词组效率提示(上屏后提示更省键词组) */
    GtkWidget *chk_next_word_prediction; /* 上屏后联想(提示接下来可输入的词句) */
    GtkWidget *chk_exact_freq_rank; /* 精确单字按词频排位(低频字让位高频词组) */
    /* 英文上屏去向(§6,输入页;0=临时 temp,1=切英文模式 en) */
    GtkWidget *combo_enter_en;  /* 回车上屏英文原串后(默认临时) */
    GtkWidget *combo_shift_en;  /* Shift 上屏英文原串后(默认切英文) */
    GtkWidget *chk_autostart;
    GtkWidget *chk_quick_actions; /* 快速功能键总开关(合同 §14) */
    GtkWidget *ent_coin_hotkey; /* 造词快捷键(coin_hotkey,合同 §12) */
    GtkWidget *ent_shot_hotkey; /* 截屏快捷键(shot_hotkey,合同 §13) */
    /* AI 助手([ai] 段) */
    GtkWidget *chk_ai_enabled;
    GtkWidget *ent_ai_base;
    GtkWidget *ent_ai_key;
    GtkWidget *ent_ai_model;
    GtkWidget *ent_ai_prompt;
    GtkWidget *spin_ai_timeout;
    GtkWidget *btn_ai_test;
    /* 输入统计(config.toml 顶层 stats_*;全局数据,显示端=悬浮窗状态行) */
    GtkWidget *chk_stats_enabled;
    GtkWidget *spin_stats_pause;
    GtkWidget *spin_stats_idle;
    /* 自定义查询(§15 候选右键菜单第 4 项;config.toml custom_query_*) */
    GtkWidget *ent_cq_label; /* 菜单显示名(空=「自定义查询」) */
    GtkWidget *ent_cq_url;   /* 网址模板,{q}=查询词(空=菜单不显示) */
    /* 菜单触发页(2026-09-30,运行时由 core 目录动态生成,不在 settings.ui) */
    GtkWidget *notebook;       /* 页签容器(子页切换/追加菜单触发页) */
    GtkWidget *chk_mt_enabled; /* 总开关 */
    GtkWidget *combo_mt_key;   /* 确认键 F1..F12(默认 F7) */
    GtkWidget *mt_rows[24];    /* 目录勾选行(勾选=禁止文字触发) */
    char mt_ids[24][64];       /* 与 mt_rows 平行的目录稳定 id */
    int mt_count;              /* 目录行数(0=core 无此符号组) */
    /* 皮肤页(页 6,运行时由 skin.c 注册表生成,不在 settings.ui):
     * 每卡 = 分组单选(皮肤名) + 分类/描述 + 同模板 .lyy-frame 预览子树,
     * provider 仅挂预览子树,不透染真实候选窗与邻卡 */
    GtkWidget *skin_buttons[9];
    GtkWidget *skin_previews[9];
    GtkCssProvider *skin_providers[9];
    guint ai_test_busy; /* 测试连接子进程在跑标记(防重复点击) */
    int built;          /* .ui 加载成功标记;失败时 show 给出降级提示 */
    char ui_dir[1200];
} SettingsUi;

void lyy_settings_init(SettingsUi *ui, const char *ui_dir);
void lyy_settings_show(SettingsUi *ui);

/* 显示设置窗并切到指定页(page<0 = 不切页,等同 lyy_settings_show);
 * 页号即 GtkNotebook 0 基序号(菜单触发页=5,皮肤页=6)。 */
void lyy_settings_show_page(SettingsUi *ui, int page);

#endif /* LYY_SETTINGS_H_ */
