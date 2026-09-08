/*
 * 主窗口(lyyIme 输入法门面/控制中心)
 *
 * 托盘菜单与 `lyyime-xim --mainwin`(含 SIGUSR2 唤起)进入;从主窗口可达:
 *   - 输入设置…(打开设置对话框)
 *   - 直输模式…(拉起 lyyime-float 悬浮独立输入窗)
 *   - 工具箱(截屏/修复输入法/输入法管理/重载词库/日志,与托盘共用 tools.c)
 * 顶部状态行随中英切换/启停/引擎态即时刷新(update_mode_ui 钩子)。
 * 关闭=隐藏(保单实例与状态),退出走托盘菜单。
 */
#ifndef LYY_MAINWIN_H_
#define LYY_MAINWIN_H_

#include <gtk/gtk.h>

typedef struct MainWin {
    GtkWidget *window;
    GtkWidget *lbl_status; /* 状态行:版本 · 启用/停用 · 中/EN · 引擎态 */
    int built;             /* init 成功标记 */
} MainWin;

void lyy_mainwin_init(MainWin *mw, const char *icon_dir);
void lyy_mainwin_show(MainWin *mw);
void lyy_mainwin_refresh(MainWin *mw); /* 模式/状态变化时刷新(未初始化安全) */

#endif /* LYY_MAINWIN_H_ */
