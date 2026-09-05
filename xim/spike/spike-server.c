/*
 * lyyIme Mode B 探路石(spike)——最小 XIM server
 *
 * 目的:验证 xcb-imdkit(vendor)在本机 Xvfb 上能服务 GTK3 内建 xim immodule
 * 客户端:XIM 连接、forward event 接收、commit string 上屏,三条链路全通。
 *
 * 借鉴出处:fcitx/xcb-imdkit 官方示例 test/test_server.c(master@44f5c8219bca,
 *   即本仓库 xim/vendor/xcb-imdkit/test/test_server.c)的 xcb_im_create /
 *   xcb_im_open_im / xcb_im_filter_event / xcb_im_commit_string 用法;
 *   root-window 输入样式(XIMPreeditNothing|XIMStatusNothing)的选型依据见
 *   docs/RESEARCH.md §2.2(uim-xim / gcin / yong 同款路线)。
 *
 * 行为约定(供 run.sh 断言):
 *   - 收到任何 XIM_FORWARD_EVENT 都打一行日志(keysym/keycode);
 *   - 收到字串键 't' 时,向客户端 commit 固定文本 "你好尖兵";
 *   - 其余按键原样 xcb_im_forward_event 回放给客户端。
 *
 * 本 spike 不注册 trigger 键(nKeys=0):XIM 走"静态事件流",客户端把所有按键
 * 全部转发给 server,链路验证最直接;trigger 键(两个 Shift,动态事件流)是
 * 正式版 xim/src/xim_server.c 的职责。
 */
#include "encoding.h"
#include "imdkit.h"
#include "ximproto.h"

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <xcb/xcb.h>
#include <xcb/xcb_aux.h>
#include <xcb/xcb_keysyms.h>
#include <xcb/xproto.h>

/* 上屏固定文本(任务书约定) */
#define SPIKE_COMMIT_TEXT "你好尖兵"

static FILE *g_log = NULL;             /* 日志文件(默认 stderr) */
static xcb_key_symbols_t *g_keysyms = NULL;

static void slog(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    vfprintf(g_log, fmt, ap);
    va_end(ap);
    fputc('\n', g_log);
    fflush(g_log);
}

/* 收到 client→server 请求的统一回调(除 forward event 外仅打日志) */
static void im_callback(xcb_im_t *im, xcb_im_client_t *client,
                        xcb_im_input_context_t *ic,
                        const xcb_im_packet_header_fr_t *hdr, void *frame,
                        void *arg, void *user_data)
{
    (void)client;
    (void)frame;
    (void)user_data;

    switch (hdr->major_opcode) {
    case XCB_XIM_CONNECT:
        slog("client connected (opcode=%u minor=%u)", hdr->major_opcode,
             hdr->minor_opcode);
        break;
    case XCB_XIM_OPEN:
        slog("client XIM open (locale negotiated)");
        break;
    case XCB_XIM_CREATE_IC:
        slog("client created input context");
        break;
    case XCB_XIM_DISCONNECT:
        slog("client disconnected");
        break;
    case XCB_XIM_FORWARD_EVENT: {
        xcb_key_press_event_t *event = arg;
        xcb_keysym_t sym = xcb_key_symbols_get_keysym(g_keysyms,
                                                      event->detail, 0);
        slog("forward event: keycode=%u keysym=0x%lx state=0x%x %s",
             event->detail, (unsigned long)sym, event->state,
             event->response_type == XCB_KEY_PRESS ? "press" : "release");
        if (sym == 't') {
            /* 用 UTF-8 → COMPOUND_TEXT 转码后 commit(官方示例同款流程) */
            size_t len = 0;
            char *ct = xcb_utf8_to_compound_text(SPIKE_COMMIT_TEXT,
                                                 strlen(SPIKE_COMMIT_TEXT),
                                                 &len);
            if (ct) {
                xcb_im_commit_string(im, ic, XCB_XIM_LOOKUP_CHARS, ct,
                                     (uint32_t)len, 0);
                slog("committed \"%s\" (%zu bytes compound text)",
                     SPIKE_COMMIT_TEXT, len);
                free(ct);
            } else {
                slog("ERROR: xcb_utf8_to_compound_text failed");
            }
        } else {
            /* Pass 类键:协议级原样回放 */
            xcb_im_forward_event(im, ic, event);
        }
        break;
    }
    default:
        slog("opcode=%u minor=%u", hdr->major_opcode, hdr->minor_opcode);
        break;
    }
}

/* 只提供 root-window 样式:preedit 与候选都画在 IM 自己的窗口(docs/RESEARCH.md §2.2) */
static uint32_t style_array[] = {
    XCB_IM_PreeditNothing | XCB_IM_StatusNothing,
};

static char *encoding_array[] = {
    "COMPOUND_TEXT",
};

static xcb_im_encodings_t encodings = { 1, encoding_array };
static xcb_im_styles_t styles = { 1, style_array };

int main(int argc, char *argv[])
{
    /* argv[1]=日志路径(可选);argv[2]=server 名(默认 lyyime,便于隔离实验) */
    g_log = stderr;
    const char *server_name = "lyyime";
    if (argc > 1 && strcmp(argv[1], "-") != 0) {
        g_log = fopen(argv[1], "a");
        if (!g_log) {
            perror("fopen log");
            return 1;
        }
    }
    if (argc > 2)
        server_name = argv[2];

    xcb_compound_text_init(); /* commit 转码前置初始化(官方示例要求) */

    int screen_no = 0;
    xcb_connection_t *conn = xcb_connect(NULL, &screen_no);
    if (xcb_connection_has_error(conn)) {
        slog("ERROR: xcb_connect failed (%d)", xcb_connection_has_error(conn));
        return 1;
    }
    xcb_screen_t *screen = xcb_aux_get_screen(conn, screen_no);
    xcb_key_symbols_t *keysyms = xcb_key_symbols_alloc(conn);
    g_keysyms = keysyms;
    if (!screen || !keysyms) {
        slog("ERROR: screen/keysyms unavailable");
        return 1;
    }

    /* server 窗口:XIM 协议用它承载 selection 所有权 */
    xcb_window_t w = xcb_generate_id(conn);
    xcb_create_window(conn, XCB_COPY_FROM_PARENT, w, screen->root, 0, 0, 1, 1,
                      1, XCB_WINDOW_CLASS_INPUT_OUTPUT, screen->root_visual, 0,
                      NULL);

    /* 不注册 trigger 键:静态事件流,客户端全量转发(见文件头说明) */
    xcb_im_trigger_keys_t keys = { 0, NULL };

    xcb_im_t *im = xcb_im_create(conn, screen_no, w, server_name,
                                 XCB_IM_ALL_LOCALES, &styles, &keys, &keys,
                                 &encodings, 0, im_callback, keysyms);
    if (!im) {
        slog("ERROR: xcb_im_create failed");
        return 1;
    }
    if (!xcb_im_open_im(im)) {
        slog("ERROR: xcb_im_open_im failed(同名 XIM server 已在运行?)");
        return 1;
    }
    xcb_flush(conn);
    slog("XIM server ready: name=%s window=0x%x display-screen=%d",
         server_name, w, screen_no);

    xcb_generic_event_t *event;
    while ((event = xcb_wait_for_event(conn))) {
        xcb_im_filter_event(im, event);
        free(event);
    }

    slog("connection closed, exiting");
    xcb_im_close_im(im);
    xcb_im_destroy(im);
    xcb_key_symbols_free(keysyms);
    xcb_disconnect(conn);
    return 0;
}
