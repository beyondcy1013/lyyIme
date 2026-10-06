/*
 * 全局截屏快捷键(合同 §13):经 XFCE xfconf 把 shot_hotkey 登记为
 * xfce4-keyboard-shortcuts 频道的 /commands/custom/<accelerator> 绑定,
 * 命令固定 /usr/local/bin/lyyime-shot。
 *
 * 语义要点:
 *   - 绑定由 xfsettingsd 全局抓取,对所有输入法/无输入法环境生效,
 *     lyyime-xim 停止后仍然可用(不再依赖输入法内按键转发);
 *   - 仅支持 XFCE:XDG_CURRENT_DESKTOP 显式为其它桌面时直接报错,
 *     不做任何写操作;未设置时照常尝试(由会话总线探测兜底);
 *   - 事务化:apply 先做全量快照并冲突检查,再依次 Set/Reset;
 *     中途任一步 DBus 失败自动逆序回滚;调用方也可在保存失败时
 *     显式 rollback(只还原本次变更的属性)。
 *   - rollback 不释放事务对象,无论回滚成败都必须随后调用
 *     commit 释放快照/连接/事务;
 *   - commit 只释放内存与连接引用,不会撤销已写入的绑定。
 */
#ifndef LYY_GLOBAL_HOTKEY_H_
#define LYY_GLOBAL_HOTKEY_H_

#include <gio/gio.h>
#include <glib.h>

typedef struct LyyGlobalHotkeyChange LyyGlobalHotkeyChange;

/* 把 spec(如 "ctrl+alt+a")登记为全局截屏绑定。
 * 成功返回事务对象(须由 commit 释放,或 rollback+commit 回滚);
 * 失败返回 NULL 并填充 error——失败路径会尽力逆序回滚本次已发生的
 * 变更;若回滚本身也失败,error 中会附带回滚失败信息(不保证绝对不
 * 留残余,但 error 一定如实报告)。 */
LyyGlobalHotkeyChange *lyy_global_shot_apply(const char *spec,
                                             GError **error);

/* 逆序回滚本次事务做过的全部变更;不释放 change(须再调 commit)。 */
gboolean lyy_global_shot_rollback(LyyGlobalHotkeyChange *change,
                                  GError **error);

/* 结束事务并释放资源(绑定保持已写入状态)。 */
void lyy_global_shot_commit(LyyGlobalHotkeyChange *change);

/* 移除全部自有全局绑定(/commands/custom/ 下命令恰为 lyyime-shot
 * 安装路径的直接子叶属性),内部同样事务化;他人的绑定一律不动。 */
gboolean lyy_global_shot_remove(GError **error);

/* 供测试与内部使用:把 shot_hotkey 写法规范化为 GTK accelerator 名
 * (lyy_hotkey_parse → gtk_accelerator_name,如 "<Primary><Alt>a");
 * 写法非法返回 NULL + error。 */
gchar *lyy_global_shot_accelerator(const char *spec, GError **error);

#endif /* LYY_GLOBAL_HOTKEY_H_ */
