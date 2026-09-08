/*
 * 工具动作(主窗口「工具箱」与托盘菜单共用)
 *
 * 项目:截屏(lyyime-shot)/ 修复输入法(lyyime-doctor check --fix)/
 *       输入法管理(lyyime-doctor ime-list)/ 重载词库 / 打开日志 /
 *       直输模式窗口(lyyime-float,Mode C 悬浮独立输入)
 * 约定:长输出放 GtkTextView 滚动对话框(AGENTS 边界完善红线);危险操作
 *       先 GtkMessageDialog 确认再执行;助手缺失给安装指引,不静默失败。
 */
#ifndef LYY_TOOLS_H_
#define LYY_TOOLS_H_

#include "common.h"

void lyy_tools_screenshot(App *app);   /* 截屏(拉起 lyyime-shot) */
void lyy_tools_fix_ime(App *app);      /* 修复输入法(doctor check --fix,先确认) */
void lyy_tools_manage_ime(App *app);   /* 输入法管理(doctor ime-list) */
void lyy_tools_reload_dict(App *app);  /* 重载词库(引擎重建) */
void lyy_tools_open_log(App *app);     /* 打开日志(尾部展示) */
void lyy_tools_float_window(App *app); /* 直输模式窗口(拉起 lyyime-float) */

#endif /* LYY_TOOLS_H_ */
