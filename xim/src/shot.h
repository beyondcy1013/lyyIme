/* 截屏助手拉起(合同 §13):截屏热键与托盘菜单共用的子进程入口。
 * 本进程只负责拉起 lyyime-shot(框选截屏 GTK 程序),不做任何 X 截图;
 * 助手独立进程崩溃/缺失均不影响输入法主流程。 */
#ifndef LYY_SHOT_H_
#define LYY_SHOT_H_

#include "common.h"

/* 拉起 lyyime-shot;解析顺序 $LYYIME_SHOT → /usr/local/bin → 引擎(exe)
 * 同级目录 → PATH。全部缺失时给候选条"人话"提示并写日志。 */
void lyy_spawn_shot(App *app);

#endif /* LYY_SHOT_H_ */
