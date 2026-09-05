/*
 * 设置对话框(GtkBuilder 加载 xim/res/settings.ui)
 *
 * 项目:候选数 / 混合英文 / 自动直通 / 中文标点 / 学习开关 / 字体大小 / 开机自启
 * 保存:写 ~/.config/lyyime/config.toml(保留未知行与注释)→ 开机自启落
 *       ~/.config/autostart → core 引擎重建(配置即时生效)+ 候选窗字体即时生效。
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
    int built; /* .ui 加载成功标记;失败时 show 给出降级提示 */
    char ui_dir[1200];
} SettingsUi;

void lyy_settings_init(SettingsUi *ui, const char *ui_dir);
void lyy_settings_show(SettingsUi *ui);

#endif /* LYY_SETTINGS_H_ */
