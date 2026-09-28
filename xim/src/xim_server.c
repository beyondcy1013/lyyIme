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
#include "shot.h"
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

/* ai_capture 用:AI 回复提交到当前焦点输入上下文(无焦点返回 -1) */
int lyy_xim_commit_utf8(App *app, const char *utf8)
{
    if (!app->xim.focused_ic)
        return -1;
    do_commit(app->xim.im, app->xim.focused_ic, utf8);
    xcb_flush(app->xim.conn);
    return 0;
}

static void hide_preedit(App *app); /* 定义在效果流消费段(此处前置声明) */

/* ---- notice 辅助提示(合同 §12:造词结果等),4 秒自动清除 ---- */
static gboolean notice_timeout(gpointer user_data)
{
    App *app = user_data;
    app->notice_timer_id = 0;
    hide_preedit(app);
    return G_SOURCE_REMOVE;
}

void lyy_show_notice(App *app, const char *text)
{
    if (app->notice_timer_id) {
        g_source_remove(app->notice_timer_id);
        app->notice_timer_id = 0;
    }
    lyy_candwin_begin_rows(&app->candwin);
    lyy_candwin_set_preedit(&app->candwin, text);
    lyy_candwin_set_page(&app->candwin, 0, 0);
    lyy_candwin_commit_layout(&app->candwin);
    app->notice_timer_id = g_timeout_add_seconds(4, notice_timeout, app);
}

/* ---- hint 词组效率提示(合同 §6):与 notice 同候选条通道,但**不带定时**
 * ——保留到下一次输入产生新效果流时由后续 preedit/cands/清除效果替换或隐藏。*/
static void show_hint(App *app, const char *text)
{
    /* 未到的 notice 清除定时会让提示提前消失,先作废 */
    if (app->notice_timer_id) {
        g_source_remove(app->notice_timer_id);
        app->notice_timer_id = 0;
    }
    lyy_candwin_begin_rows(&app->candwin);
    lyy_candwin_set_preedit(&app->candwin, text);
    lyy_candwin_set_page(&app->candwin, 0, 0);
    lyy_candwin_commit_layout(&app->candwin);
}

/* ---- 快速功能键执行(合同 §14):@settings/@shot/@help 宿主内置,其余按
 * shell 命令执行(sh -c,与 Mode A service.run_quick_action 同合同) ---- */
void lyy_run_quick_action(App *app, int index)
{
    if (!app->core.qa_ok || !app->engine)
        return;
    char cmd[1024];
    int n = app->core.lyyime_action_command(app->engine, index, cmd,
                                            (int)sizeof(cmd));
    if (n <= 1) {
        lyy_log(&app->log, "WARN 快速功能键下标越界:%d", index);
        return;
    }
    if (!strcmp(cmd, "@settings")) {
        lyy_log(&app->log, "快速功能键命中:打开配置(@settings)");
        lyy_request_show_settings(app);
        return;
    }
    if (!strcmp(cmd, "@shot")) {
        /* 工具提示带配置的热键;助手缺失时 lyy_spawn_shot 的安装指引
         * 会覆盖本提示(候选条文本替换) */
        lyy_log(&app->log, "快速功能键命中:截图(@shot),热键 %s",
                app->config.shot_hotkey);
        char msg[320];
        snprintf(msg, sizeof(msg), "已拉起截屏(热键 %.200s)",
                 app->config.shot_hotkey);
        lyy_show_notice(app, msg);
        lyy_spawn_shot(app);
        return;
    }
    if (!strcmp(cmd, "@help")) {
        lyy_log(&app->log, "快速功能键命中:帮助(@help)");
        lyy_show_notice(app,
                        "帮助:Shift单击=中英切换  1-9选词  -/=翻页  "
                        "Ctrl+=造词  Ctrl+Alt+A截屏  /AI+提示词=AI  "
                        "peizhi/shezhi=设置 jietu=截图 bangzhu=帮助");
        return;
    }
    lyy_log(&app->log, "快速功能键命中[%d]:执行 %s", index, cmd);
    char *argv[] = { "sh", "-c", cmd, NULL };
    if (!g_spawn_async(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL, NULL,
                       NULL, NULL)) {
        lyy_log(&app->log, "WARN 快速功能命令执行失败:%s", cmd);
    }
}

/* ---- 造词热键解析(启动/设置保存后调用;失败回退默认 Ctrl+=) ---- */
void lyy_app_reload_hotkey(App *app)
{
    uint32_t mods = 0, sym = 0;
    if (lyy_hotkey_parse(app->config.coin_hotkey, &mods, &sym)) {
        app->hotkey_coin.mods = mods;
        app->hotkey_coin.sym = sym;
        app->hotkey_coin.ok = 1;
        lyy_log(&app->log, "造词热键:%s(mods=0x%x keysym=0x%x)",
                app->config.coin_hotkey, mods, sym);
    } else {
        /* 解析失败回退默认,保证造词功能始终可用 */
        if (lyy_hotkey_parse("ctrl+equal", &mods, &sym)) {
            app->hotkey_coin.mods = mods;
            app->hotkey_coin.sym = sym;
            app->hotkey_coin.ok = 1;
        }
        lyy_log(&app->log,
                "WARN 造词热键配置不合法(%s),回退默认 ctrl+equal",
                app->config.coin_hotkey);
    }

    /* 截屏热键(合同 §13):解析失败同样回退默认 */
    if (lyy_hotkey_parse(app->config.shot_hotkey, &mods, &sym)) {
        app->hotkey_shot.mods = mods;
        app->hotkey_shot.sym = sym;
        app->hotkey_shot.ok = 1;
        lyy_log(&app->log, "截屏热键:%s(mods=0x%x keysym=0x%x)",
                app->config.shot_hotkey, mods, sym);
    } else {
        if (lyy_hotkey_parse("ctrl+alt+a", &mods, &sym)) {
            app->hotkey_shot.mods = mods;
            app->hotkey_shot.sym = sym;
            app->hotkey_shot.ok = 1;
        }
        lyy_log(&app->log,
                "WARN 截屏热键配置不合法(%s),回退默认 ctrl+alt+a",
                app->config.shot_hotkey);
    }
}

/* ai_capture 用:喂 core 一个键并按当前语义应用效果流
 * 返回 1=已消费 0=core 放行(pass) -1=core 异常(已标记降级) */
static void apply_effects(App *app, xcb_im_input_context_t *ic,
                          xcb_key_press_event_t *ev, uint32_t sym,
                          const char *json, int *had_content_out,
                          int *pass_out);
int lyy_ai_feed_core(App *app, xcb_key_press_event_t *ev, uint32_t sym,
                     int key, uint32_t chr)
{
    char json[4096];
    if (lyy_core_process_key_json(&app->core, app->engine, key, chr, json,
                                  (int)sizeof(json)) != 0) {
        lyy_core_mark_degraded(app, "process_key 调用失败");
        return -1;
    }
    int had = 0, passed = 0;
    apply_effects(app, app->xim.focused_ic, ev, sym, json, &had, &passed);
    return passed ? 0 : 1;
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
                          xcb_key_press_event_t *ev, uint32_t sym,
                          const char *json, int *had_content_out,
                          int *pass_out)
{
    xcb_im_t *im = app->xim.im;
    int had_content = 0;
    int passed = 0;
    /* /AI 采集态:commit 进提示词、pass 进原字符,不再直达应用 */
    int ai_capture = lyy_ai_capturing(app);
    LyyEffect effects[16];
    int count = 0;
    if (lyy_effects_parse(json, effects, 16, &count) != 0) {
        /* 合同违约:降级直通,记录现场 JSON 便于与 core 对齐。
         * 点选路径(§14)无原始键事件可回放,仅记录。 */
        lyy_log(&app->log, "ERROR 效果流解析失败,降级直通: %s", json);
        if (ev)
            xcb_im_forward_event(im, ic, ev);
        return;
    }

    for (int i = 0; i < count; i++) {
        LyyEffect *e = &effects[i];
        switch (e->kind) {
        case LYY_EFF_COMMIT:
            if (e->s[0]) {
                had_content = 1;
                if (ai_capture) {
                    lyy_ai_on_commit(app, e->s);
                    lyy_log(&app->log, "AI 采集←组词上屏: %s", e->s);
                } else {
                    do_commit(im, ic, e->s);
                    lyy_log(&app->log, "commit: %s", e->s);
                }
            }
            break;
        case LYY_EFF_PREEDIT:
            /* 无 "s" 字段=清空预编辑(主控通报形态),set 空串即清除 */
            lyy_ai_mirror_preedit(app, e->s); /* 组词码镜像(触发门控用) */
            if (ai_capture)
                lyy_ai_show_preedit(app);
            else
                lyy_candwin_set_preedit(&app->candwin, e->s);
            if (e->s[0])
                had_content = 1;
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
            if (ai_capture) {
                /* 采集态不回放:字母/数字/标点按原字符归入提示词 */
                lyy_ai_on_pass_key(app, sym);
            } else if (ev) {
                xcb_im_forward_event(im, ic, ev);
            }
            passed = 1;
            break;
        case LYY_EFF_CONSUMED:
            break;
        case LYY_EFF_NOTICE:
            if (e->s[0]) {
                lyy_show_notice(app, e->s);
                lyy_log(&app->log, "notice: %s", e->s);
            }
            break;
        case LYY_EFF_HINT:
            /* /AI 采集态不打扰:候选条预编辑行正展示提示词 */
            if (e->s[0] && !ai_capture) {
                show_hint(app, e->s);
                lyy_log(&app->log, "hint: %s", e->s);
            }
            break;
        case LYY_EFF_MODE:
            /* core 在键处理中切了模式(§6:回车/Shift 上屏英文原串后按
             * enter_english/shift_english 配置转英文):m=1 关 trigger 转
             * 英文直通,m=0 开 trigger 回中文。core 模式已在效果流返回前
             * apply,set_trigger 内部不会再 toggle;托盘/主窗状态随之刷新。 */
            lyy_log(&app->log, "mode 效果:core 切%s(trigger 同步)",
                    e->m == 1 ? "英文" : "中文");
            lyy_xim_set_trigger(app, e->m == 1 ? 0 : 1);
            break;
        case LYY_EFF_ACTION:
            /* §14 快速功能键命中:执行功能,不上屏文本
             * (效果流已含 preedit/cands 清除;采集态同样执行,与 Mode A 一致) */
            lyy_log(&app->log, "action: index=%d", e->i);
            lyy_run_quick_action(app, e->i);
            break;
        default:
            break;
        }
    }
    lyy_candwin_commit_layout(&app->candwin);
    /* 新内容上屏时撤销未到的 notice 清除定时,避免误清组合显示 */
    if (had_content && app->notice_timer_id) {
        g_source_remove(app->notice_timer_id);
        app->notice_timer_id = 0;
    }
    if (had_content_out)
        *had_content_out = had_content;
    if (pass_out)
        *pass_out = passed;
}

/* ---- 候选窗行点击(§14 鼠标点选) ----
 * 经 core lyyime_select_candidate 走与数字选词同一条效果流路径:
 * 功能键候选 → action(执行功能),普通候选 → commit(上屏)。
 * 无原始键事件(ev=NULL):select_candidate 恒不产生 pass,解析失败/越界
 * 仅记录,绝不回放空事件。 */
void lyy_candwin_row_clicked(int idx, void *user_data)
{
    App *app = user_data;
    xcb_im_input_context_t *ic =
        app->xim.focused_ic ? app->xim.focused_ic : app->xim.cand_op_ic;
    if (!lyy_core_ready(app) || !ic || !app->core.qa_ok)
        return;
    char json[4096];
    if (lyy_core_select_candidate_json(&app->core, app->engine, idx, json,
                                       (int)sizeof(json)) != 0) {
        lyy_log(&app->log, "WARN 点选候选失败(idx=%d):select_candidate 调用异常",
                idx);
        return;
    }
    int had = 0, passed = 0;
    apply_effects(app, ic, NULL, 0, json, &had, &passed);
}

/* ---- 候选窗右键菜单(§15 候选管理:固定首位/删除词组/反查英文) ----
 * 右键不触发上屏:core cand_op 效果流只含 candidates/preedit/notice。
 * 功能键/空行 core 返回 <0 → 菜单不弹出;旧 core 库缺符号 → 回调不注册。 */
int lyy_candwin_op_state(int idx, void *user_data)
{
    App *app = user_data;
    if (!lyy_core_ready(app) || !app->core.cand_ops_ok)
        return -1;
    int pinned = app->core.lyyime_cand_pinned(app->engine, idx);
    /* 菜单即将弹出:记住当前 IC —— 菜单 grab 期间客户端 focus-out,
       focused_ic 会被清空,activate 时用这份回投效果 */
    if (pinned >= 0)
        app->xim.cand_op_ic = app->xim.focused_ic;
    return pinned;
}

void lyy_candwin_op(int idx, int op, void *user_data)
{
    App *app = user_data;
    xcb_im_input_context_t *ic =
        app->xim.focused_ic ? app->xim.focused_ic : app->xim.cand_op_ic;
    if (!lyy_core_ready(app) || !ic || !app->core.cand_ops_ok) {
        lyy_log(&app->log,
                "WARN 候选右键操作被跳过(idx=%d,op=%d,ready=%d,ic=%p,ops=%d)",
                idx, op, lyy_core_ready(app), (void *)ic,
                app->core.cand_ops_ok);
        return;
    }
    lyy_log(&app->log, "候选右键操作 idx=%d op=%d", idx, op);
    char json[4096];
    if (lyy_core_cand_op_json(&app->core, app->engine, idx, op, json,
                              (int)sizeof(json)) != 0) {
        lyy_log(&app->log, "WARN 候选右键操作失败(idx=%d,op=%d)", idx, op);
        return;
    }
    int had = 0, passed = 0;
    apply_effects(app, ic, NULL, 0, json, &had, &passed);
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
    /* 单击确认:通过 XIM trigger 协议切英文(应用此后直接收键) */
    xcb_im_preedit_end(app->xim.im, app->xim.focused_ic);
    xcb_flush(app->xim.conn);
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
        if (!is_press) {
            /* 单击确认主路径:本服务端已请求转发 KeyRelease,按下挂起后
             * release 必然随后到达,即视为一次完整单击 → 切英文。
             * (280ms 时间窗仅为不转发 release 的客户端兜底,见 timeout。) */
            if (st->shift_pending) {
                if (app->shift_timer_id) {
                    g_source_remove(app->shift_timer_id);
                    app->shift_timer_id = 0;
                }
                st->shift_pending = 0;
                app->shift_pending = 0;
                xcb_im_preedit_end(app->xim.im, app->xim.focused_ic);
                xcb_flush(app->xim.conn);
                lyy_xim_set_trigger(app, 0);
                lyy_log(&app->log,
                        "Shift 单击(release 确认)→ 英文直通(trigger off)");
            }
            xcb_im_forward_event(xs->im, ic, ev);
            return;
        }
        /* 组合守护:off→on 后立刻收到 Shift 按下,视为一次完整单击开新组合。 */
        if (st->combo_guard && now_ms() - st->guard_ms <= LYY_COMBO_GUARD_MS) {
            st->combo_guard = 0;
        }
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

    /* ---- 截屏热键(合同 §13;默认 Ctrl+Alt+A,shot_hotkey 可配置) ----
     * 纯工具组合键:直接拉起 lyyime-shot 子进程并吞键,不经 core、不进组词。
     * 仅 trigger on(中文接管)可见;英文直通态按键不经本服务,托盘菜单兜底。 */
    if (is_press && app->hotkey_shot.ok &&
        lyy_hotkey_match(ev->state & LYY_CLEAN_MOD_MASK, (uint32_t)sym,
                         app->hotkey_shot.mods, app->hotkey_shot.sym)) {
        lyy_ai_reset(app);
        lyy_log(&app->log, "截屏热键命中 keysym=0x%lx", (unsigned long)sym);
        lyy_spawn_shot(app);
        return;
    }

    /* ---- 造词热键(合同 §12;默认 Ctrl+=,coin_hotkey 可配置) ----
     * 位置在 AI 触发之前:与 Mode A 一致,组合键先放弃 AI 会话再进造词。 */
    int coin_key = 0;
    if (is_press && app->hotkey_coin.ok &&
        lyy_hotkey_match(ev->state & LYY_CLEAN_MOD_MASK, (uint32_t)sym,
                         app->hotkey_coin.mods, app->hotkey_coin.sym)) {
        coin_key = 1;
        lyy_ai_reset(app);
        lyy_log(&app->log, "造词热键命中 keysym=0x%lx", (unsigned long)sym);
    }

    /* ---- /AI 触发会话(中文态 + [ai] 配置齐备才介入;ai_capture.h) ----
     * 位置在 Shift+字母直通之前:采集态须把大写字母也一并收进提示词。 */
    if (!coin_key && is_press) {
        int ai_rc = lyy_ai_take(app, ev, (uint32_t)sym);
        if (ai_rc == 1)
            return;
        if (ai_rc == 2) {
            xcb_im_forward_event(xs->im, ic, ev);
            return;
        }
    }

    /* CapsLock 大写态(合同 §6):字母键一律原样直通英文,不进组词缓冲 ——
     * 无 Shift 输出大写字母;Shift+字母(键值列 1)由应用按 Caps+Shift
     * 翻译输出小写字母。直通前送 core LKEY_OTHER 复位可能残留的组词缓冲
     * (空缓冲 core 恒回 Pass,由 apply_effects 原样回放一次,不重复转发)。
     * 数字/标点等非字母键不受 CapsLock 影响,继续走常态。 */
    if (is_press && (ev->state & XCB_MOD_MASK_LOCK) &&
        (((sym >= (uint32_t)'A') && (sym <= (uint32_t)'Z')) ||
         ((sym >= (uint32_t)'a') && (sym <= (uint32_t)'z')))) {
        lyy_log(&app->log, "CapsLock 大写态字母直通 keysym=0x%lx",
                (unsigned long)sym);
        if (lyy_ai_feed_core(app, ev, (uint32_t)sym, LKEY_OTHER, 0) == -1) {
            lyy_core_mark_degraded(app, "process_key 调用失败");
            xcb_im_forward_event(xs->im, ic, ev);
        }
        return;
    }

    /* 中文态 Shift+字母(2026-09-28 需求):不再原样直通,大写键值进组词
     * 缓冲 —— 全大写敲入走大写候选通道(候选 1=原样大写、2=首字母大写、
     * 3=全小写、4+=中文翻译),单一大写与混合大小写由 core 按既有通道处理。
     * 与 Mode A(ibus)行为对齐:大写键值原样传给 core,继续走下方常态路径。 */

    if (!is_press) {
        /* 普通键 release 一律回放(应用需要配对事件) */
        xcb_im_forward_event(xs->im, ic, ev);
        return;
    }

    if (coin_key) {
        key = LKEY_COIN;
        chr = 0;
    } else {
        lyy_keysym_map(sym, &key, &chr);
        if (is_press && lyy_keysym_is_shift(base_sym)) {
            key = LKEY_SHIFTPRESS;
            chr = 0;
        }
    }
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
    int had_content = 0;
    apply_effects(app, ic, ev, (uint32_t)sym, json, &had_content, NULL);

    /* Shift:core 已按合同处理。有缓冲时 had_content=1,原串已上屏且
     * (shift_english=en 时)mode 效果已在 apply_effects 内关 trigger 转英文,
     * 本次 Shift 到此消费完毕;只有空缓冲 Shift 才挂起单击判定并允许切英文。 */
    if (key == LKEY_SHIFTPRESS && !had_content && !st->shift_pending) {
        st->shift_pending = 1;
        app->shift_pending = 1;
        if (app->shift_timer_id)
            g_source_remove(app->shift_timer_id);
        app->shift_timer_id =
            g_timeout_add(LYY_SHIFT_CLICK_MS, shift_click_timeout, app);
        lyy_log(&app->log, "Shift 单击判定:按下挂起,等待时间窗确认");
    }
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
        lyy_ai_reset(app);
        if (app->xim.focused_ic == ic)
            app->xim.focused_ic = NULL;
        hide_preedit(app);
        break;
    case XCB_XIM_CREATE_IC:
        ic_state(ic); /* 预建状态 */
        lyy_log(&app->log, "创建输入上下文");
        break;
    case XCB_XIM_DESTROY_IC:
        lyy_ai_reset(app);
        if (app->xim.focused_ic == ic)
            app->xim.focused_ic = NULL;
        hide_preedit(app);
        break;
    case XCB_XIM_SET_IC_FOCUS: {
        /* §15 菜单会话恢复:右键菜单期间被假焦的同一 IC 回来,
           不重置引擎(候选右键操作的效果/缓冲需要保留) */
        gboolean resume_menu =
            app->xim.cand_op_ic && app->xim.cand_op_ic == ic;
        app->xim.cand_op_ic = NULL;
        app->xim.focused_ic = ic;
        cancel_shift_pending(app);
        lyy_ai_reset(app); /* 新焦点:AI 会话不跨上下文延续 */
        if (lyy_core_ready(app)) {
            IcState *st = ic_state(ic);
            if (st) {
                st->shift_pending = 0;
                st->combo_guard = 0;
            }
            if (!resume_menu) {
                /* 新焦点:进入中文态(trigger on)并复位 core 缓冲(§6 reset) */
                app->core.lyyime_reset(app->engine);
                if (app->core.lyyime_mode(app->engine) != 0)
                    app->core.lyyime_toggle_mode(app->engine);
                xcb_im_preedit_start(im, ic);
                lyy_log(&app->log, "输入上下文获得焦点 → trigger on(中文态)");
            }
        }
        lyy_app_update_mode_ui(app);
        break;
    }
    case XCB_XIM_UNSET_IC_FOCUS:
        cancel_shift_pending(app);
        lyy_ai_reset(app);
        if (app->xim.focused_ic == ic) {
            app->xim.focused_ic = NULL;
            /* §15 候选右键菜单的 GTK grab 会让客户端发 UNSET focus
               (假焦):保留引擎缓冲与候选窗,记下 IC 供菜单 activate 回投 */
            if (app->candwin.menu_open)
                app->xim.cand_op_ic = ic;
        }
        if (!app->candwin.menu_open) {
            app->xim.cand_op_ic = NULL; /* 真失焦:菜单会话标记失效 */
            if (lyy_core_ready(app))
                app->core.lyyime_reset(app->engine);
            hide_preedit(app);
        }
        break;
    case XCB_XIM_TRIGGER_NOTIFY: {
        xcb_im_trigger_notify_fr_t *nf = frame;
        IcState *st = ic_state(ic);
        lyy_ai_reset(app); /* 中英切换:AI 会话随之放弃 */
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
    lyy_ai_reset(app); /* 模式切换:AI 会话随之放弃 */
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
    lyy_ai_reset(app);
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
    lyy_mainwin_refresh(&app->mainwin); /* 主窗口状态行同步(未建时安全) */
}
