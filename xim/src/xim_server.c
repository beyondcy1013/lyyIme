#include "xim_server.h"

#include <glib-unix.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <xcb/xcb_aux.h>
#include <xcb/xcb_keysyms.h>
#include <xcb/xproto.h>

/* keysymdef.h 功能宏(标准用法,必须先于 include 定义) */
#define XK_MISCELLANY
#define XK_XKB_KEYS
#include <X11/keysymdef.h>

#include "common.h"
#include "effects_json.h"
#include "encoding.h"
#include "imdkit.h"
#include "keysym_map.h"
#include "ximproto.h"

/* ---- trigger 键:两个 Shift,onKeys 生效;offKeys 留空(见头文件说明) ----
 * modifier_mask 覆盖除 CapsLock/NumLock 外全部修饰位:单击 Shift 时事件里
 * 不应残留其它修饰;Lock(0x02)与 Mod2/NumLock(0x10)被显式忽略
 * (借鉴 fcitx 触发键的"干净修饰"惯例)。 */
#define LYY_CLEAN_MOD_MASK 0xEDu
static xcb_im_ximtriggerkey_fr_t g_on_keys[2] = {
    { XK_Shift_L, 0, LYY_CLEAN_MOD_MASK },
    { XK_Shift_R, 0, LYY_CLEAN_MOD_MASK },
};
static xcb_im_trigger_keys_t g_trigger = { 2, g_on_keys };
static xcb_im_trigger_keys_t g_no_trigger = { 0, NULL };

static uint32_t g_styles[] = { XCB_IM_PreeditNothing | XCB_IM_StatusNothing };
static char *g_encodings[] = { "COMPOUND_TEXT" };
static xcb_im_encodings_t g_encoding_list = { 1, g_encodings };
static xcb_im_styles_t g_style_list = { 1, g_styles };

/* ---- 每个 IC 的会话状态(挂在 xcb_im_input_context 上) ---- */
typedef struct {
    int shift_pending;  /* on 态:Shift 已按未放,等待单击判定 */
    int combo_guard;    /* off→on 后的组合键防护窗 */
    long long guard_ms; /* 防护窗起点(单调毫秒) */
} IcState;

static long long now_ms(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void ic_state_free(void *p)
{
    free(p);
}

/* 惰性建 IC 状态;返回 NULL 表示内存不足(调用方须直通放行) */
static IcState *ic_state(xcb_im_input_context_t *ic)
{
    IcState *st = xcb_im_input_context_get_data(ic);
    if (!st) {
        st = calloc(1, sizeof(*st));
        if (!st)
            return NULL;
        xcb_im_input_context_set_data(ic, st, ic_state_free);
    }
    return st;
}

/* ---- commit:UTF-8 → COMPOUND_TEXT(库官方转码路径,spike 已验证) ---- */
static void do_commit(xcb_im_t *im, xcb_im_input_context_t *ic,
                      const char *utf8)
{
    size_t len = strlen(utf8);
    size_t ct_len = 0;
    char *ct = xcb_utf8_to_compound_text(utf8, len, &ct_len);
    if (!ct) {
        /* 极端情况(转码失败):按原字节提交,交客户端容错 */
        xcb_im_commit_string(im, ic, XCB_XIM_LOOKUP_CHARS, utf8,
                             (uint32_t)len, 0);
        return;
    }
    xcb_im_commit_string(im, ic, XCB_XIM_LOOKUP_CHARS, ct, (uint32_t)ct_len, 0);
    free(ct);
}

/* ---- 效果流消费:JSON → commit/候选窗/回放 ---- */
static void hide_preedit(App *app)
{
    lyy_candwin_begin_rows(&app->candwin);
    lyy_candwin_set_preedit(&app->candwin, "");
    lyy_candwin_set_page(&app->candwin, 0, 0);
    lyy_candwin_commit_layout(&app->candwin); /* 空内容 → 隐藏 */
}

static void apply_effects(App *app, xcb_im_input_context_t *ic,
                          xcb_key_press_event_t *ev, const char *json)
{
    xcb_im_t *im = app->xim.im;
    LyyEffect effects[16];
    int count = 0;
    if (lyy_effects_parse(json, effects, 16, &count) != 0) {
        /* 合同违约:降级直通,记录现场 JSON 便于与 core 对齐 */
        lyy_log(&app->log, "ERROR 效果流解析失败,降级直通: %s", json);
        xcb_im_forward_event(im, ic, ev);
        return;
    }

    for (int i = 0; i < count; i++) {
        LyyEffect *e = &effects[i];
        switch (e->kind) {
        case LYY_EFF_COMMIT:
            if (e->s[0]) {
                do_commit(im, ic, e->s);
                lyy_log(&app->log, "commit: %s", e->s);
            }
            break;
        case LYY_EFF_PREEDIT:
            /* 无 "s" 字段=清空预编辑(主控通报形态),set 空串即清除 */
            lyy_candwin_set_preedit(&app->candwin, e->s);
            break;
        case LYY_EFF_CANDS: {
            lyy_candwin_begin_rows(&app->candwin);
            int n = e->n;
            if (n > LYY_MAX_ROWS)
                n = LYY_MAX_ROWS;
            char buf[LYY_EFF_STR_MAX], cbuf[LYY_EFF_STR_MAX];
            for (int k = 0; k < n; k++) {
                const char *text = lyy_core_cand_text(&app->core, app->engine,
                                                      k, buf, sizeof(buf));
                const char *comment = NULL;
                if (app->core.lyyime_cand_comment(app->engine, k, cbuf,
                                                  (int)sizeof(cbuf)) >= 0)
                    comment = cbuf;
                lyy_candwin_add_row(&app->candwin, k, text ? text : "?",
                                    comment ? comment : "");
            }
            lyy_candwin_set_page(&app->candwin, e->page + 1, e->pages);
            break;
        }
        case LYY_EFF_PASS:
            xcb_im_forward_event(im, ic, ev);
            break;
        case LYY_EFF_CONSUMED:
            break;
        case LYY_EFF_MODE:
            lyy_app_update_mode_ui(app);
            break;
        default:
            break;
        }
    }
    lyy_candwin_commit_layout(&app->candwin);
}

/* ---- forward event 主处理(§6 按键行为 + Shift 单击/组合判定) ---- */

/* ---- Shift 触发键(两个 Shift,单击切换) ----
 * 实证约束(docs/RESEARCH.md §2 实证精神):Xlib XIM 客户端在 trigger on 时
 * 只转发 KeyPress、不转发 KeyRelease(Xvfb 实测,SET_EVENT_MASK 含 RELEASE
 * 亦然)。因此"按下后无其它键即释放"的单击判定改为 280ms 时间窗:
 *   - Shift 按下 → 挂起;280ms 内有其它键 → 取消(Shift 组合);
 *   - 时间窗到期仍无其它键 → 判定单击 → trigger off 切英文;
 *   - 若客户端(如 Xlib 以外的 XIM 实现)转发了 Shift release,则立即确认。
 * 中文态下 Shift+字母:原样直通大写字母(对齐主流输入法,不进组词缓冲)。 */
#define LYY_SHIFT_CLICK_MS 280

static gboolean shift_click_timeout(gpointer user_data)
{
    App *app = user_data;
    xcb_im_input_context_t *ic = app->xim.focused_ic;
    if (!ic || !app->shift_pending)
        return G_SOURCE_REMOVE;
    IcState *st = ic_state(ic);
    app->shift_timer_id = 0;
    if (!st || !st->shift_pending)
        return G_SOURCE_REMOVE;
    st->shift_pending = 0;
    app->shift_pending = 0;
    /* 单击确认:切回英文(trigger off,应用此后直接收键) */
    lyy_xim_set_trigger(app, 0);
    lyy_log(&app->log, "Shift 单击(时间窗确认)→ 英文直通(trigger off)");
    return G_SOURCE_REMOVE;
}

static void handle_key_event(App *app, xcb_im_input_context_t *ic,
                             xcb_key_press_event_t *ev)
{
    XimServer *xs = &app->xim;
    xcb_keysym_t sym;
    uint32_t chr = 0;
    int key = LKEY_OTHER;
    int is_press = (ev->response_type == XCB_KEY_PRESS);
    IcState *st = ic_state(ic);

    /* 停用/降级/内存不足:一切原样回放(崩溃安全语义,绝不吞键) */
    if (!lyy_core_ready(app) || !st) {
        xcb_im_forward_event(xs->im, ic, ev);
        return;
    }

    /* Shift 列选择:带 Shift 时取上档 keysym(Shift+1 → '!') */
    int col = (ev->state & XCB_MOD_MASK_SHIFT) ? 1 : 0;
    sym = xcb_key_symbols_get_keysym(xs->keysyms, ev->detail, col);
    /* 修饰键判定固定用基名列:Shift 键自身的 release 带 ShiftMask,
     * col=1 的 keysym 无意义,会导致 Shift 识别失败 */
    xcb_keysym_t base_sym = xcb_key_symbols_get_keysym(xs->keysyms,
                                                       ev->detail, 0);

    /* ---- Shift 触发键 ---- */
    if (lyy_keysym_is_shift(base_sym)) {
        if (is_press) {
            if (!st->shift_pending) {
                st->shift_pending = 1;
                app->shift_pending = 1;
                if (app->shift_timer_id)
                    g_source_remove(app->shift_timer_id);
                app->shift_timer_id =
                    g_timeout_add(LYY_SHIFT_CLICK_MS, shift_click_timeout, app);
                lyy_log(&app->log, "Shift 按下,挂起等待单击判定");
            }
        } else if (st->shift_pending) {
            /* 客户端转发了 release:立即确认单击(快于时间窗) */
            if (app->shift_timer_id) {
                g_source_remove(app->shift_timer_id);
                app->shift_timer_id = 0;
            }
            st->shift_pending = 0;
            app->shift_pending = 0;
            lyy_xim_set_trigger(app, 0);
            lyy_log(&app->log, "Shift 单击(release 确认)→ 英文直通(trigger off)");
        } else {
            xcb_im_forward_event(xs->im, ic, ev);
        }
        return;
    }

    /* ---- Shift 组合/后续键:取消挂起的单击判定 ---- */
    if (st->shift_pending) {
        st->shift_pending = 0;
        app->shift_pending = 0;
        if (app->shift_timer_id) {
            g_source_remove(app->shift_timer_id);
            app->shift_timer_id = 0;
        }
        lyy_log(&app->log, "Shift 单击判定取消(出现其它键,判定为组合)");
    }

    /* ---- 组合键防护:off→on 后立刻来了带 Shift 的键 → 判定组合,回退 ---- */
    if (st->combo_guard) {
        if (now_ms() - st->guard_ms > LYY_COMBO_GUARD_MS) {
            st->combo_guard = 0;
        } else if (is_press && (ev->state & XCB_MOD_MASK_SHIFT)) {
            st->combo_guard = 0;
            lyy_xim_set_trigger(app, 0);
            lyy_log(&app->log,
                    "Shift 组合键防护:回退英文,原样放行 keysym=0x%lx",
                    (unsigned long)sym);
            xcb_im_forward_event(xs->im, ic, ev);
            return;
        }
    }

    /* 中文态 Shift+字母:大写字母无组词语义,原样直通(主流输入法行为) */
    if (is_press && (ev->state & XCB_MOD_MASK_SHIFT) &&
        (((sym >= (uint32_t)'A') && (sym <= (uint32_t)'Z')) ||
         ((sym >= (uint32_t)'a') && (sym <= (uint32_t)'z')))) {
        lyy_log(&app->log, "Shift+字母原样直通 keysym=0x%lx",
                (unsigned long)sym);
        xcb_im_forward_event(xs->im, ic, ev);
        return;
    }

    if (!is_press) {
        /* 普通键 release 一律回放(应用需要配对事件) */
        xcb_im_forward_event(xs->im, ic, ev);
        return;
    }

    lyy_keysym_map(sym, &key, &chr);
    lyy_log(&app->log, "forward keysym=0x%lx → LKey=%d chr=%u",
            (unsigned long)sym, key, chr);

    char json[4096];
    if (lyy_core_process_key_json(&app->core, app->engine, key, chr, json,
                                  (int)sizeof(json)) != 0) {
        /* core 异常:标记降级并放行该键 */
        lyy_core_mark_degraded(app, "process_key 调用失败");
        xcb_im_forward_event(xs->im, ic, ev);
        return;
    }
    apply_effects(app, ic, ev, json);
}

/* 取消 Shift 单击挂起(焦点切换/IC 销毁/进入新触发时调用) */
static void cancel_shift_pending(App *app)
{
    if (app->shift_timer_id) {
        g_source_remove(app->shift_timer_id);
        app->shift_timer_id = 0;
    }
    app->shift_pending = 0;
}

/* ---- xcb_im 回调分发 ---- */
static void im_callback(xcb_im_t *im, xcb_im_client_t *client,
                        xcb_im_input_context_t *ic,
                        const xcb_im_packet_header_fr_t *hdr, void *frame,
                        void *arg, void *user_data)
{
    App *app = user_data;
    (void)client;
    (void)im;

    switch (hdr->major_opcode) {
    case XCB_XIM_CONNECT:
        lyy_log(&app->log, "XIM client 已连接");
        break;
    case XCB_XIM_DISCONNECT:
        lyy_log(&app->log, "XIM client 断开");
        if (app->xim.focused_ic == ic)
            app->xim.focused_ic = NULL;
        hide_preedit(app);
        break;
    case XCB_XIM_CREATE_IC:
        ic_state(ic); /* 预建状态 */
        lyy_log(&app->log, "创建输入上下文");
        break;
    case XCB_XIM_DESTROY_IC:
        if (app->xim.focused_ic == ic)
            app->xim.focused_ic = NULL;
        hide_preedit(app);
        break;
    case XCB_XIM_SET_IC_FOCUS: {
        app->xim.focused_ic = ic;
        cancel_shift_pending(app);
        if (lyy_core_ready(app)) {
            IcState *st = ic_state(ic);
            if (st) {
                st->shift_pending = 0;
                st->combo_guard = 0;
            }
            /* 新焦点:进入中文态(trigger on)并复位 core 缓冲(§6 reset) */
            app->core.lyyime_reset(app->engine);
            if (app->core.lyyime_mode(app->engine) != 0)
                app->core.lyyime_toggle_mode(app->engine);
            xcb_im_preedit_start(im, ic);
            lyy_log(&app->log, "输入上下文获得焦点 → trigger on(中文态)");
        }
        lyy_app_update_mode_ui(app);
        break;
    }
    case XCB_XIM_UNSET_IC_FOCUS:
        cancel_shift_pending(app);
        if (app->xim.focused_ic == ic)
            app->xim.focused_ic = NULL;
        if (lyy_core_ready(app))
            app->core.lyyime_reset(app->engine);
        hide_preedit(app);
        break;
    case XCB_XIM_TRIGGER_NOTIFY: {
        xcb_im_trigger_notify_fr_t *nf = frame;
        IcState *st = ic_state(ic);
        if (nf->flag == 0) {
            /* on:off(英文)态收到 Shift 按下 → 中文态 + 组合键防护窗 */
            if (lyy_core_ready(app)) {
                if (app->core.lyyime_mode(app->engine) != 0)
                    app->core.lyyime_toggle_mode(app->engine);
                xcb_im_preedit_start(im, ic);
                if (st) {
                    st->combo_guard = 1;
                    st->guard_ms = now_ms();
                    st->shift_pending = 0;
                }
                lyy_app_update_mode_ui(app);
                lyy_log(&app->log, "trigger on(Shift 按下,中文态+防护窗)");
            } else {
                /* 停用/降级:立刻退回直通,不让客户端停在转发态 */
                xcb_im_preedit_end(im, ic);
            }
        } else {
            /* off:客户端请求(本实现 off 列表为空,正常不会走到) */
            xcb_im_preedit_end(im, ic);
            lyy_app_update_mode_ui(app);
            lyy_log(&app->log, "trigger off(客户端请求)");
        }
        break;
    }
    case XCB_XIM_FORWARD_EVENT:
        if (arg)
            handle_key_event(app, ic, (xcb_key_press_event_t *)arg);
        break;
    default:
        break;
    }
}

/* ---- GLib 事件源:把 xcb fd 融入主循环(单线程模型) ---- */
static gboolean on_xcb_ready(gint fd, GIOCondition cond, gpointer data)
{
    App *app = data;
    (void)fd;
    xcb_connection_t *conn = app->xim.conn;

    if (xcb_connection_has_error(conn)) {
        lyy_log(&app->log, "ERROR X 连接已断开,退出");
        app->quit_requested = 1;
        return G_SOURCE_REMOVE;
    }
    xcb_generic_event_t *ev;
    while ((ev = xcb_poll_for_event(conn)) != NULL) {
        xcb_im_filter_event(app->xim.im, ev);
        free(ev);
    }
    if (xcb_connection_has_error(conn)) {
        lyy_log(&app->log, "ERROR X 连接已断开,退出");
        app->quit_requested = 1;
        return G_SOURCE_REMOVE;
    }
    xcb_flush(conn);
    return (cond & (G_IO_ERR | G_IO_HUP)) ? G_SOURCE_REMOVE : G_SOURCE_CONTINUE;
}

int lyy_xim_init(XimServer *xs, App *app)
{
    memset(xs, 0, sizeof(*xs));
    xs->conn = xcb_connect(NULL, &xs->screen_no);
    if (xcb_connection_has_error(xs->conn)) {
        lyy_log(&app->log, "ERROR 无法连接 X 显示(检查 DISPLAY)");
        return -1;
    }
    xcb_screen_t *screen = xcb_aux_get_screen(xs->conn, xs->screen_no);

    xcb_compound_text_init();
    xs->keysyms = xcb_key_symbols_alloc(xs->conn);

    xs->server_win = xcb_generate_id(xs->conn);
    xcb_create_window(xs->conn, XCB_COPY_FROM_PARENT, xs->server_win,
                      screen->root, 0, 0, 1, 1, 1,
                      XCB_WINDOW_CLASS_INPUT_OUTPUT, screen->root_visual, 0,
                      NULL);

    xs->im = xcb_im_create(xs->conn, xs->screen_no, xs->server_win,
                           LYY_XIM_SERVER_NAME, XCB_IM_ALL_LOCALES,
                           &g_style_list, &g_trigger, &g_no_trigger,
                           &g_encoding_list,
                           /* 转发掩码必须含 KeyRelease:Shift 单击检测需要
                              press/release 配对(默认仅 KeyPress,release 会
                              直接送达应用,server 永远看不到) */
                           XCB_EVENT_MASK_KEY_PRESS | XCB_EVENT_MASK_KEY_RELEASE,
                           im_callback, app);
    if (!xs->im || !xcb_im_open_im(xs->im)) {
        lyy_log(&app->log,
                "ERROR XIM server(%s)启动失败:同名 server 已在运行?"
                "(pgrep lyyime-xim;XIM server 名全机唯一)",
                LYY_XIM_SERVER_NAME);
        return -1;
    }
    xcb_flush(xs->conn);
    lyy_log(&app->log,
            "XIM server ready: name=%s(两个 Shift 触发,root-window 样式)",
            LYY_XIM_SERVER_NAME);

    xs->xcb_source_id = g_unix_fd_add(xcb_get_file_descriptor(xs->conn),
                                      G_IO_IN | G_IO_ERR | G_IO_HUP,
                                      on_xcb_ready, app);
    return 0;
}

void lyy_xim_shutdown(XimServer *xs)
{
    if (xs->xcb_source_id) {
        g_source_remove(xs->xcb_source_id);
        xs->xcb_source_id = 0;
    }
    if (xs->im) {
        xcb_im_close_im(xs->im);
        xcb_im_destroy(xs->im);
        xs->im = NULL;
    }
    if (xs->keysyms)
        xcb_key_symbols_free(xs->keysyms);
    if (xs->conn) {
        xcb_flush(xs->conn);
        xcb_disconnect(xs->conn);
        xs->conn = NULL;
    }
}

/* 模式切换(托盘/Shift 单击共用):to_chinese=1 → 中文(trigger on) */
void lyy_xim_set_trigger(App *app, int to_chinese)
{
    xcb_im_input_context_t *ic = app->xim.focused_ic;
    /* 同步 core 模式(降级时只改协议态) */
    if (app->core.loaded && !app->degraded && app->engine) {
        int want = to_chinese ? 0 : 1;
        if (app->core.lyyime_mode(app->engine) != want)
            app->core.lyyime_toggle_mode(app->engine);
    }
    if (ic) {
        if (to_chinese)
            xcb_im_preedit_start(app->xim.im, ic);
        else
            xcb_im_preedit_end(app->xim.im, ic);
    }
    hide_preedit(app);
    lyy_app_update_mode_ui(app);
}

/* 托盘"启用/停用":停用后一切按键直通,焦点恢复时不再自动转中文 */
void lyy_xim_set_enabled(App *app, int enabled)
{
    if (app->enabled == enabled)
        return;
    app->enabled = enabled;
    if (enabled) {
        /* 恢复即回中文态(与获得焦点语义一致) */
        lyy_xim_set_trigger(app, 1);
        lyy_log(&app->log, "输入法已启用 → 中文态");
    } else {
        if (app->xim.focused_ic)
            xcb_im_preedit_end(app->xim.im, app->xim.focused_ic);
        hide_preedit(app);
        lyy_app_update_mode_ui(app);
        lyy_log(&app->log, "输入法已停用 → 全部按键直通");
    }
}

void lyy_app_update_mode_ui(App *app)
{
    int english = 1;
    if (app->core.loaded && !app->degraded && app->engine)
        english = app->core.lyyime_mode(app->engine) == 1;
    lyy_tray_set_mode(&app->tray, english ? LYY_MODE_EN : LYY_MODE_ZH,
                      app->enabled && !app->degraded);
}
