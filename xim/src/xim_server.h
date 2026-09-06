/*
 * XIM server 封装(基于 vendor xcb-imdkit,fcitx5 XIM 前端同款库)
 *
 * - 输入样式:root-window style(PreeditNothing|StatusNothing),见 RESEARCH §2.2;
 * - trigger 键 = 两个 Shift(左/右),干净修饰掩码(忽略 CapsLock/NumLock,
 *   借鉴 fcitx 触发键处理惯例);
 *   · off(英文)态:客户端把匹配 on 键的 Shift 按下转为 XIM_TRIGGER_NOTIFY(on)
 *     → 本端 preedit_start(开始转发)+ 组合键防护窗(见下);
 *   · on(中文)态:off 键列表为空,所有按键(含 Shift 本身)全部转发到本端,
 *     Shift 按下→无其它键→释放 即"单击",此时本端主动 preedit_end 切回英文
 *     (协议级,应用此后直接收键);
 *   · 组合键防护:off→on 切换后 400ms 内若到达带 ShiftMask 的按键(如 Shift+A),
 *     判定为组合而非单击,立即回退英文态并把该键原样放行(键事件自带
 *     ShiftMask,应用看到的仍是正确的 Shift+字母)。
 * - 崩溃安全:core FFI 异常/降级/停用时,一切按键原样 xcb_im_forward_event
 *   回放(协议级原样,零风险),绝不吞键。
 */
#ifndef LYY_XIM_SERVER_H_
#define LYY_XIM_SERVER_H_

#include "imdkit.h" /* vendor 头(-I vendor/xcb-imdkit/src) */
#include <glib.h>
#include <xcb/xcb_keysyms.h>

/* 前向声明,打破 common.h ↔ xim_server.h 循环包含 */
typedef struct _App App;

/* 与文件头说明一致的组合键防护窗(ms) */
#define LYY_COMBO_GUARD_MS 400

typedef struct XimServer {
    xcb_connection_t *conn;
    int screen_no;
    xcb_window_t server_win;
    xcb_key_symbols_t *keysyms;
    xcb_im_t *im;
    xcb_im_input_context_t *focused_ic;
    guint xcb_source_id;
} XimServer;

/* 连接 X、创建 server window 并 xcb_im_open_im;失败返回 -1(已写日志) */
int lyy_xim_init(XimServer *xs, App *app);
void lyy_xim_shutdown(XimServer *xs);

/* 模式切换(托盘/单击共用):to_chinese=1 切中文,0 切英文 */
void lyy_xim_set_trigger(App *app, int to_chinese);

/* 托盘"启用/停用":停用后一切按键直通,焦点恢复时不再自动转中文 */
void lyy_xim_set_enabled(App *app, int enabled);

/* 应用模式指示刷新(托盘图标 文 中/EN) */
void lyy_app_update_mode_ui(App *app);

/* 造词热键(合同 §12):从 app->config.coin_hotkey 重新解析
 * (启动与设置保存后调用;解析失败回退默认 Ctrl+= 并写日志) */
void lyy_app_reload_hotkey(App *app);

/* 辅助区临时提示(notice 效果:造词结果等),约 4 秒后自动清除 */
void lyy_show_notice(App *app, const char *text);

#endif /* LYY_XIM_SERVER_H_ */
