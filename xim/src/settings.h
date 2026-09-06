/*
 * 设置对话框(GtkBuilder 加载 xim/res/settings.ui)
 *
 * 项目:候选数 / 混合英文 / 自动直通 / 中文标点 / 学习开关 / 字体大小 / 开机自启
 *       / AI 助手(OpenAI 兼容接口:地址/密钥/模型/系统提示/超时 + 测试连接)
 * 保存:写 ~/.config/lyyime/config.toml(保留未知行与注释)→ 开机自启落
 *       ~/.config/autostart → core 引擎重建(配置即时生效)+ 候选窗字体即时生效;
 *       AI 配置由 ai_capture 每次按键实时读取,保存即生效,无需重启。
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
    GtkWidget *chk_autostart;
    GtkWidget *ent_coin_hotkey; /* 造词快捷键(coin_hotkey,合同 §12) */
    /* AI 助手([ai] 段) */
    GtkWidget *chk_ai_enabled;
    GtkWidget *ent_ai_base;
    GtkWidget *ent_ai_key;
    GtkWidget *ent_ai_model;
    GtkWidget *ent_ai_prompt;
    GtkWidget *spin_ai_timeout;
    GtkWidget *btn_ai_test;
    guint ai_test_busy; /* 测试连接子进程在跑标记(防重复点击) */
    int built;          /* .ui 加载成功标记;失败时 show 给出降级提示 */
    char ui_dir[1200];
} SettingsUi;

void lyy_settings_init(SettingsUi *ui, const char *ui_dir);
void lyy_settings_show(SettingsUi *ui);

#endif /* LYY_SETTINGS_H_ */
