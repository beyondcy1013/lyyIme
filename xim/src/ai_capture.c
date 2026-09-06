#include "ai_capture.h"

#include <stdarg.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "common.h"
#include "keysym_map.h"

/* keysymdef.h 功能宏(标准用法,必须先于 include 定义) */
#define XK_MISCELLANY
#define XK_XKB_KEYS
#define XK_LATIN1
#include <X11/keysymdef.h>

/* Ctrl/Alt/Super 组合键属于应用快捷键:不进 AI 会话,出现即放弃 */
#define LYY_AI_BLOCK_MODS \
    (XCB_MOD_MASK_CONTROL | XCB_MOD_MASK_1 | XCB_MOD_MASK_4)

/* ---- 资源目录:助手脚本解析顺序(env → ibus 安装位 → tools 安装位 → 源码树) ---- */
const char *lyy_ai_find_helper(void)
{
    static const char *kCandidates[] = {
        "/usr/local/bin/lyyime-ai",
        "/usr/local/share/lyyime/tools/lyyime-ai",
        LYY_SRC_AI_HELPER,
    };
    static char chosen[1024];

    const char *env = g_getenv("LYYIME_AI_HELPER");
    if (env && env[0] && g_file_test(env, G_FILE_TEST_EXISTS)) {
        snprintf(chosen, sizeof(chosen), "%s", env);
        return chosen;
    }
    for (size_t i = 0; i < sizeof(kCandidates) / sizeof(kCandidates[0]); i++) {
        if (kCandidates[i] && kCandidates[i][0] &&
            g_file_test(kCandidates[i], G_FILE_TEST_EXISTS)) {
            snprintf(chosen, sizeof(chosen), "%s", kCandidates[i]);
            return chosen;
        }
    }
    return NULL;
}

/* ---- 展示与提示 ---- */
static void candwin_show_text(App *app, const char *text)
{
    lyy_candwin_begin_rows(&app->candwin);
    lyy_candwin_set_preedit(&app->candwin, text);
    lyy_candwin_set_page(&app->candwin, 0, 0);
    lyy_candwin_commit_layout(&app->candwin);
}

static void candwin_hide(App *app)
{
    candwin_show_text(app, ""); /* 空内容 → commit_layout 内部隐藏 */
}

static gboolean notice_timeout(gpointer user_data)
{
    App *app = user_data;
    app->ai.notice_timer = 0;
    if (app->ai.state == LYY_AI_IDLE)
        candwin_hide(app);
    return G_SOURCE_REMOVE;
}

/* 候选窗预编辑行提示(4 秒自动清;输入可随时覆盖) */
static void ai_notice(App *app, const char *fmt, ...)
{
    char msg[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(msg, sizeof(msg), fmt, ap);
    va_end(ap);
    if (app->ai.notice_timer) {
        g_source_remove(app->ai.notice_timer);
        app->ai.notice_timer = 0;
    }
    candwin_show_text(app, msg);
    app->ai.notice_timer = g_timeout_add_seconds(4, notice_timeout, app);
    lyy_log(&app->log, "AI 提示: %s", msg);
}

/* ---- 采集态展示 ---- */
void lyy_ai_mirror_preedit(App *app, const char *text)
{
    snprintf(app->ai.core_preedit, sizeof(app->ai.core_preedit), "%s",
             text ? text : "");
}

/* 与 Mode A 引擎一致:'/'→'/';'/A'→'/A';采集态→"/AI "+提示词+组词码 */
static const char *display_text(App *app)
{
    static char buf[LYY_AI_PROMPT_MAX + 600];
    switch (app->ai.state) {
    case LYY_AI_SLASH:
        return "/";
    case LYY_AI_SLASH_A:
        return "/A";
    case LYY_AI_CAPTURE:
        snprintf(buf, sizeof(buf), "/AI %s%s", app->ai.prompt->str,
                 app->ai.core_preedit);
        return buf;
    default:
        return "";
    }
}

static void ai_show(App *app)
{
    if (app->ai.notice_timer) {
        g_source_remove(app->ai.notice_timer);
        app->ai.notice_timer = 0;
    }
    candwin_show_text(app, display_text(app));
}

void lyy_ai_show_preedit(App *app)
{
    /* 只更新文本不布局:效果流([preedit][cands])中间调用,末尾由
     * apply_effects 的 commit_layout 统一收口,避免逐键窗口闪烁 */
    if (app->ai.notice_timer) {
        g_source_remove(app->ai.notice_timer);
        app->ai.notice_timer = 0;
    }
    lyy_candwin_set_preedit(&app->candwin, display_text(app));
}

/* ---- 提示词维护 ---- */
static void append_prompt(App *app, const char *s)
{
    AiCapture *ai = &app->ai;
    if (ai->prompt->len + strlen(s) > LYY_AI_PROMPT_MAX) {
        if (!ai->prompt_full_notified) {
            ai->prompt_full_notified = 1;
            ai_notice(app, "AI:提示词已达 %d 字节上限", LYY_AI_PROMPT_MAX);
        }
        return;
    }
    g_string_append(ai->prompt, s);
}

void lyy_ai_on_commit(App *app, const char *text)
{
    append_prompt(app, text);
    /* 效果流中间态:只刷新文本,布局由 apply_effects 末尾统一收口 */
    lyy_candwin_set_preedit(&app->candwin, display_text(app));
}

void lyy_ai_on_pass_key(App *app, uint32_t keysym)
{
    if (keysym >= 0x20 && keysym <= 0x7e) {
        char one[2] = { (char)keysym, '\0' };
        append_prompt(app, one);
        lyy_candwin_set_preedit(&app->candwin, display_text(app));
    }
}

/* ---- 会话生命周期 ---- */
static void remove_request_sources(AiCapture *ai)
{
    if (ai->kill_timer) {
        g_source_remove(ai->kill_timer);
        ai->kill_timer = 0;
    }
    if (ai->child_watch) {
        g_source_remove(ai->child_watch);
        ai->child_watch = 0;
    }
    if (ai->out_watch) {
        g_source_remove(ai->out_watch);
        ai->out_watch = 0;
    }
    if (ai->err_watch) {
        g_source_remove(ai->err_watch);
        ai->err_watch = 0;
    }
    if (ai->out_ch) {
        g_io_channel_unref(ai->out_ch);
        ai->out_ch = NULL;
    }
    if (ai->err_ch) {
        g_io_channel_unref(ai->err_ch);
        ai->err_ch = NULL;
    }
}

static gboolean drain_fd(int fd, GString *buf)
{
    char tmp[4096];
    for (;;) {
        ssize_t n = read(fd, tmp, sizeof(tmp));
        if (n > 0) {
            g_string_append_len(buf, tmp, (gssize)n);
            if (buf->len > 256 * 1024)
                return TRUE; /* 异常超大输出:截断防拖死 */
            continue;
        }
        return TRUE; /* n<=0:EOF 或暂时无数据(子进程已退出,不会阻塞) */
    }
}

static void on_child_exit(GPid pid, gint status, gpointer user_data)
{
    App *app = user_data;
    AiCapture *ai = &app->ai;
    ai->child_watch = 0; /* 本回调即该源,先清 id 再摘其它 watch */
    remove_request_sources(ai); /* 先摘 IO watch,下面同步收尾读 */
    if (ai->out_ch) {
        drain_fd(g_io_channel_unix_get_fd(ai->out_ch), ai->out_buf);
        g_io_channel_unref(ai->out_ch);
        ai->out_ch = NULL;
    }
    if (ai->err_ch) {
        drain_fd(g_io_channel_unix_get_fd(ai->err_ch), ai->err_buf);
        g_io_channel_unref(ai->err_ch);
        ai->err_ch = NULL;
    }
    g_spawn_close_pid(pid);
    ai->busy_pid = 0;

    gboolean ok = g_spawn_check_wait_status(status, NULL);
    lyy_log(&app->log, "AI 助手退出 status=%d ok=%d stdout=%uB stderr=%uB",
            status, ok, (unsigned)ai->out_buf->len,
            (unsigned)ai->err_buf->len);
    if (ok && ai->out_buf->len) {
        if (app->xim.focused_ic) {
            if (lyy_xim_commit_utf8(app, ai->out_buf->str) == 0)
                lyy_log(&app->log, "AI 回复已上屏(%u 字节)",
                        (unsigned)ai->out_buf->len);
            else
                lyy_log(&app->log, "AI 回复上屏失败:无焦点输入上下文");
        } else {
            lyy_log(&app->log, "AI 回复丢弃:焦点已离开(无输入上下文)");
        }
    } else {
        gchar *msg = g_strstrip(g_strdup(ai->err_buf->len ? ai->err_buf->str
                                                          : "调用失败,详见日志"));
        ai_notice(app, "AI 失败:%s", msg);
        g_free(msg);
    }
}

static gboolean on_request_timeout(gpointer user_data)
{
    App *app = user_data;
    AiCapture *ai = &app->ai;
    ai->kill_timer = 0;
    if (ai->busy_pid) {
        lyy_log(&app->log, "AI 请求超时,终止子进程 pid=%d", (int)ai->busy_pid);
        kill(ai->busy_pid, SIGTERM);
        ai_notice(app, "AI:请求超时已取消");
    }
    return G_SOURCE_REMOVE;
}

static gboolean on_pipe_input(GIOChannel *ch, GIOCondition cond,
                              gpointer user_data)
{
    (void)cond;
    GString *buf = user_data;
    gchar tmp[4096];
    gsize readn = 0;
    for (;;) {
        GIOStatus st = g_io_channel_read_chars(ch, tmp, sizeof(tmp), &readn,
                                               NULL);
        if (st == G_IO_STATUS_NORMAL && readn > 0) {
            g_string_append_len(buf, tmp, (gssize)readn);
            if (buf->len > 256 * 1024)
                return G_SOURCE_REMOVE;
            continue;
        }
        break;
    }
    return G_SOURCE_CONTINUE;
}

void lyy_ai_init(AiCapture *ai)
{
    memset(ai, 0, sizeof(*ai));
    ai->state = LYY_AI_IDLE;
    ai->prompt = g_string_new("");
    ai->out_buf = g_string_new("");
    ai->err_buf = g_string_new("");
}

void lyy_ai_clear(AiCapture *ai)
{
    remove_request_sources(ai);
    if (ai->busy_pid) {
        kill(ai->busy_pid, SIGTERM);
        g_spawn_close_pid(ai->busy_pid);
        ai->busy_pid = 0;
    }
    if (ai->prompt)
        g_string_free(ai->prompt, TRUE);
    if (ai->out_buf)
        g_string_free(ai->out_buf, TRUE);
    if (ai->err_buf)
        g_string_free(ai->err_buf, TRUE);
    memset(ai, 0, sizeof(*ai));
}

void lyy_ai_reset(App *app)
{
    AiCapture *ai = &app->ai;
    ai->state = LYY_AI_IDLE;
    ai->pend_n = 0;
    ai->prompt_full_notified = 0;
    g_string_truncate(ai->prompt, 0);
    ai->core_preedit[0] = '\0';
    if (ai->notice_timer) {
        g_source_remove(ai->notice_timer);
        ai->notice_timer = 0;
    }
    /* 进行中的 AI 请求不打断:完成时若无焦点上下文会自动丢弃 */
}

int lyy_ai_capturing(const App *app)
{
    return app->ai.state == LYY_AI_CAPTURE;
}

int lyy_ai_enabled(const App *app)
{
    return app->enabled && !app->degraded && app->core.loaded &&
           app->engine != NULL && lyy_config_ai_active(&app->config) &&
           app->core.lyyime_mode(app->engine) == 0;
}

/* ---- 请求提交 ---- */
static void submit(App *app)
{
    AiCapture *ai = &app->ai;
    gchar *prompt = g_strdup(ai->prompt->str);
    g_strstrip(prompt);
    lyy_ai_reset(app); /* 展示清除;提交内容已拷贝 */

    if (!prompt[0]) {
        ai_notice(app, "AI:提示词为空,已取消");
        g_free(prompt);
        return;
    }
    if (ai->busy_pid) {
        ai_notice(app, "AI:上一个请求仍在进行中,请稍候");
        g_free(prompt);
        return;
    }
    const char *helper = lyy_ai_find_helper();
    if (!helper) {
        lyy_log(&app->log,
                "ERROR 未找到 AI 助手脚本 lyyime_ai.py(已探测 env "
                "LYYIME_AI_HELPER、/usr/local/share/lyyime/{ibus/engine,tools})");
        ai_notice(app, "AI:助手脚本未安装,请升级 lyyIme 组件");
        g_free(prompt);
        return;
    }

    /* 助手须可直接执行(Rust 二进制 lyyime-ai,或带 shebang 的 755 脚本);
     * 项目规则:主程序一律 Rust,Python 仅可测试,故不再写死 python3 前缀 */
    gchar *argv[] = { (gchar *)helper,
                      (gchar *)"--prompt", prompt, NULL };
    gint outfd = -1, errfd = -1;
    GPid pid = 0;
    GError *err = NULL;
    /* DO_NOT_REAP_CHILD:g_child_watch_add 的前置要求,否则拿不到
     * 真实退出状态(wait 结果为 -1,成功也被判失败) */
    if (!g_spawn_async_with_pipes(NULL, argv, NULL,
                                  G_SPAWN_SEARCH_PATH |
                                      G_SPAWN_DO_NOT_REAP_CHILD,
                                  NULL, NULL, &pid, NULL, &outfd, &errfd,
                                  &err)) {
        lyy_log(&app->log, "ERROR AI 助手启动失败:%s",
                err ? err->message : "未知错误");
        ai_notice(app, "AI:启动失败(%s)",
                  err ? err->message : "未知错误");
        g_clear_error(&err);
        g_free(prompt);
        return;
    }
    ai->busy_pid = pid;
    g_string_truncate(ai->out_buf, 0);
    g_string_truncate(ai->err_buf, 0);
    ai->out_ch = g_io_channel_unix_new(outfd);
    g_io_channel_set_flags(ai->out_ch, G_IO_FLAG_NONBLOCK, NULL);
    ai->err_ch = g_io_channel_unix_new(errfd);
    g_io_channel_set_flags(ai->err_ch, G_IO_FLAG_NONBLOCK, NULL);
    ai->out_watch = g_io_add_watch(ai->out_ch, G_IO_IN | G_IO_HUP | G_IO_ERR,
                                   on_pipe_input, ai->out_buf);
    ai->err_watch = g_io_add_watch(ai->err_ch, G_IO_IN | G_IO_HUP | G_IO_ERR,
                                   on_pipe_input, ai->err_buf);
    ai->child_watch = g_child_watch_add(pid, on_child_exit, app);
    ai->kill_timer = g_timeout_add_seconds(
        (guint)(app->config.ai_timeout > 0 ? app->config.ai_timeout + 15 : 75),
        on_request_timeout, app);
    lyy_log(&app->log, "AI 提交:%d 字节提示词 → %s(model=%s)", (int)strlen(prompt),
            helper, app->config.ai_model);
    ai_notice(app, "AI 生成中…(%s)", app->config.ai_model);
    g_free(prompt);
}

/* ---- 打歪补发 ---- */
static void flush_pending(App *app)
{
    AiCapture *ai = &app->ai;
    xcb_im_input_context_t *ic = app->xim.focused_ic;
    for (int i = 0; i < ai->pend_n && ic; i++)
        xcb_im_forward_event(app->xim.im, ic, &ai->pend_ev[i]);
    ai->pend_n = 0;
}

/* ---- 采集态按键 ----
 * 注:core 的 Pass 效果由 apply_effects 统一转成"原字符进提示词"
 * (lyy_ai_on_pass_key),本函数不重复归档。 */
static int capture_key(App *app, xcb_key_press_event_t *ev, uint32_t sym)
{
    AiCapture *ai = &app->ai;

    if (sym == XK_Return || sym == XK_KP_Enter) {
        /* core 有缓冲:按合同"上屏原始字母"→ 原字母归入提示词后发送 */
        lyy_ai_feed_core(app, ev, sym, LKEY_ENTER, 0);
        submit(app);
        return 1;
    }
    if (sym == XK_Escape) {
        if (lyy_ai_feed_core(app, ev, sym, LKEY_ESC, 0) == 0) {
            lyy_ai_reset(app); /* 空组词缓冲 Esc:退出 AI 会话 */
            candwin_hide(app);
        } else {
            ai_show(app);
        }
        return 1;
    }
    if (sym == XK_BackSpace) {
        int consumed = lyy_ai_feed_core(app, ev, sym, LKEY_BACKSPACE, 0);
        if (consumed > 0) {
            ai_show(app);
        } else if (ai->prompt->len) {
            gsize pos = ai->prompt->len;
            while (pos > 0 &&
                   ((unsigned char)ai->prompt->str[pos - 1] & 0xC0) == 0x80)
                pos--; /* 回退整个 UTF-8 字符 */
            if (pos > 0)
                pos--;
            g_string_truncate(ai->prompt, pos);
            ai_show(app);
        } else {
            lyy_ai_reset(app); /* 删穿 /AI 前缀:取消(前缀不回放) */
            candwin_hide(app);
        }
        return 1;
    }
    if (sym == XK_space) {
        /* 有组词:确认首选进提示词;空缓冲:core Pass → 原空格进提示词 */
        lyy_ai_feed_core(app, ev, sym, LKEY_SPACE, 0);
        ai_show(app);
        return 1;
    }

    int key = LKEY_OTHER;
    uint32_t chr = 0;
    lyy_keysym_map(sym, &key, &chr);
    if (key == LKEY_PAGEUP || key == LKEY_PAGEDOWN) {
        lyy_ai_feed_core(app, ev, sym, key, chr); /* 只翻候选页 */
        return 1;
    }
    if (key == LKEY_OTHER) {
        /* 方向键/功能键等:复位组词、放弃会话并原样放行 */
        lyy_ai_feed_core(app, ev, sym, LKEY_OTHER, 0);
        lyy_ai_reset(app);
        candwin_hide(app);
        return 2;
    }
    lyy_ai_feed_core(app, ev, sym, key, chr);
    ai_show(app);
    return 1;
}

int lyy_ai_take(App *app, xcb_key_press_event_t *ev, uint32_t keysym)
{
    AiCapture *ai = &app->ai;
    if (!lyy_ai_enabled(app))
        return 0;

    /* 应用组合键(Ctrl/Alt/Super+键):放弃会话并放行;空闲时不介入 */
    if (ev->state & LYY_AI_BLOCK_MODS) {
        if (ai->state != LYY_AI_IDLE) {
            lyy_ai_reset(app);
            candwin_hide(app);
            lyy_log(&app->log, "AI 会话因组合键放弃");
            return 2;
        }
        return 0;
    }

    char ch = (keysym >= 0x20 && keysym <= 0x7e) ? (char)keysym : '\0';

    switch (ai->state) {
    case LYY_AI_IDLE:
        if (ch == '/' && ai->core_preedit[0] == '\0') {
            ai->pend_ev[0] = *ev;
            ai->pend_n = 1;
            ai->state = LYY_AI_SLASH;
            ai_show(app);
            lyy_log(&app->log, "AI 触发:'/' 已吞,等待 A/I");
            return 1;
        }
        return 0;

    case LYY_AI_SLASH:
        if (ch == 'a' || ch == 'A') {
            ai->pend_ev[1] = *ev;
            ai->pend_n = 2;
            ai->state = LYY_AI_SLASH_A;
            ai_show(app);
            return 1;
        }
        flush_pending(app); /* 打歪:补发 '/',当前键走普通路径 */
        ai->state = LYY_AI_IDLE;
        lyy_log(&app->log, "AI 触发失败(非 A),已补发 '/'");
        return 0;

    case LYY_AI_SLASH_A:
        if (ch == 'i' || ch == 'I') {
            ai->pend_n = 0;
            ai->state = LYY_AI_CAPTURE;
            g_string_truncate(ai->prompt, 0);
            ai->prompt_full_notified = 0;
            ai->core_preedit[0] = '\0';
            ai_show(app);
            lyy_log(&app->log, "AI 触发成功,进入提示词采集");
            return 1;
        }
        flush_pending(app); /* 补发 '/' + 'A',当前键走普通路径 */
        ai->state = LYY_AI_IDLE;
        lyy_log(&app->log, "AI 触发失败(非 I),已补发 '/A'");
        return 0;

    case LYY_AI_CAPTURE:
        return capture_key(app, ev, keysym);

    default:
        ai->state = LYY_AI_IDLE;
        return 0;
    }
}
