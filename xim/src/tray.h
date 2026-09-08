/*
 * 托盘:Gtk.StatusIcon(XEmbed,兼容 xfce4-panel,任务书指定方案)
 * 中/EN 图标;左键单击切换中英;右键菜单:主窗口、启用/停用、切换中英、
 * 设置、工具(直输模式/截屏(§13)/修复输入法/输入法管理/重载词库/日志)、
 * 退出。
 * 工具动作实现统一在 tools.c(主窗口「工具箱」共用);菜单 exec
 * `lyyime-doctor` CLI;缺失时弹安装提示;长输出放 GtkTextView 滚动对话框;
 * 危险操作先 GtkMessageDialog 确认再执行。
 */
#ifndef LYY_TRAY_H_
#define LYY_TRAY_H_

#include <gtk/gtk.h>

/* 模式指示(托盘图标 文 中/EN) */
typedef enum {
    LYY_MODE_ZH = 0,
    LYY_MODE_EN = 1,
} LyyTrayMode;

typedef struct Tray {
    GtkStatusIcon *icon;
    char icon_dir[1024];
    int mode;   /* LyyTrayMode */
    int active; /* 启用且未降级 */
} Tray;

void lyy_tray_init(Tray *tray, const char *icon_dir);
void lyy_tray_set_mode(Tray *tray, int mode, int active);

#endif /* LYY_TRAY_H_ */
