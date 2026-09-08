/* 工具动作实现:从 tray.c 抽出供主窗口/托盘菜单共用(模块合同见 tools.h) */
#include "tools.h"

#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "shot.h"

#define DOCTOR_BIN "lyyime-doctor"

/* ---- 通用:GtkTextView 滚动对话框(承载长输出,AGENTS 边界完善红线) ---- */
static void show_text_dialog(GtkWindow *parent, const char *title,
                             const char *text)
{
    GtkWidget *dlg = gtk_dialog_new_with_buttons(
        title, parent, GTK_DIALOG_MODAL | GTK_DIALOG_DESTROY_WITH_PARENT,
        "_关闭", GTK_RESPONSE_CLOSE, NULL);
    gtk_window_set_default_size(GTK_WINDOW(dlg), 680, 440);

    GtkWidget *sw = gtk_scrolled_window_new(NULL, NULL);
    gtk_scrolled_window_set_policy(GTK_SCROLLED_WINDOW(sw), GTK_POLICY_AUTOMATIC,
                                   GTK_POLICY_AUTOMATIC);
    GtkWidget *view = gtk_text_view_new();
    gtk_text_view_set_editable(GTK_TEXT_VIEW(view), FALSE);
    gtk_text_view_set_wrap_mode(GTK_TEXT_VIEW(view), GTK_WRAP_WORD_CHAR);
    gtk_text_view_set_monospace(GTK_TEXT_VIEW(view), TRUE);
    GtkTextBuffer *buf = gtk_text_view_get_buffer(GTK_TEXT_VIEW(view));
    gtk_text_buffer_set_text(buf, text ? text : "", -1);
    gtk_container_set_border_width(GTK_CONTAINER(sw), 8);
    gtk_box_pack_start(GTK_BOX(gtk_dialog_get_content_area(GTK_DIALOG(dlg))),
                       sw, TRUE, TRUE, 0);
    gtk_widget_show_all(dlg);
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
}

/* ---- 通用:执行命令并捕获输出;返回 0 成功。missing=1 时给出安装提示 ---- */
static int run_command_capture(const char *cmd, gchar **out, int *missing)
{
    GError *err = NULL;
    gchar *stdout_s = NULL, *stderr_s = NULL;
    gint status = 0;
    *missing = 0;
    if (!g_spawn_command_line_sync(cmd, &stdout_s, &stderr_s, &status, &err)) {
        if (err && err->code == G_SPAWN_ERROR_NOENT)
            *missing = 1;
        g_clear_error(&err);
        g_free(stdout_s);
        g_free(stderr_s);
        return -1;
    }
    GString *g = g_string_new("");
    if (stdout_s && stdout_s[0])
        g_string_append(g, stdout_s);
    if (stderr_s && stderr_s[0]) {
        g_string_append(g, "\n[stderr]\n");
        g_string_append(g, stderr_s);
    }
    if (!(status == 0))
        g_string_append_printf(g, "\n[退出码 %d]", status);
    *out = g_string_free(g, FALSE);
    g_free(stdout_s);
    g_free(stderr_s);
    return 0;
}

static void doctor_missing_dialog(GtkWindow *parent)
{
    GtkWidget *dlg = gtk_message_dialog_new(
        parent, GTK_DIALOG_MODAL, GTK_MESSAGE_INFO, GTK_BUTTONS_OK,
        "未找到 %s(lyyIme 诊断/管理工具)。\n\n"
        "安装方式(任选其一):\n"
        "  1. 源码构建:scripts/build.sh(产物 crates/lyyime-doctor)\n"
        "  2. 拷贝到 PATH:cp lyyime-doctor /usr/local/bin/\n\n"
        "安装后即可在此使用“修复输入法 / 输入法管理”。",
        DOCTOR_BIN);
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
}

/* ---- 直输模式窗口:助手解析顺序 env → 安装位 → exe 同级 → PATH 兜底
 * (与 shot.c lyy_spawn_shot 同思路;E2E 可经 $LYYIME_FLOAT 注入桩程序) ---- */
static int exe_sibling(const char *name, char *out, size_t cap)
{
    char buf[1024];
    ssize_t n = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
    if (n <= 0)
        return -1;
    buf[n] = '\0';
    char *slash = strrchr(buf, '/');
    if (!slash)
        return -1;
    size_t dirlen = (size_t)(slash - buf) + 1; /* 含目录斜杠 */
    size_t rest = strlen(name);
    if (dirlen + rest + 1 > cap)
        return -1; /* 截断风险:直接放弃该候选路径 */
    memcpy(out, buf, dirlen);
    memcpy(out + dirlen, name, rest + 1);
    return 0;
}

void lyy_tools_float_window(App *app)
{
    static const char *kName = "lyyime-float";
    static const char *kInstalled = "/usr/local/bin/lyyime-float";
    char sibling[1024];
    const char *use = NULL;
    char chosen[1024];

    const char *env = g_getenv("LYYIME_FLOAT");
    if (env && env[0] && g_file_test(env, G_FILE_TEST_EXISTS)) {
        snprintf(chosen, sizeof(chosen), "%s", env);
        use = chosen;
    } else if (g_file_test(kInstalled, G_FILE_TEST_EXISTS)) {
        use = kInstalled;
    } else if (exe_sibling(kName, sibling, sizeof(sibling)) == 0 &&
               g_file_test(sibling, G_FILE_TEST_EXISTS)) {
        use = sibling;
    }

    char *argv[2];
    argv[1] = NULL;
    GSpawnFlags flags = G_SPAWN_STDOUT_TO_DEV_NULL | G_SPAWN_STDERR_TO_DEV_NULL;
    if (use) {
        argv[0] = (gchar *)use;
    } else {
        /* 无确切路径:按名字走 PATH(g_spawn 搜索语义);仍失败则提示安装 */
        argv[0] = (gchar *)kName;
        flags |= G_SPAWN_SEARCH_PATH;
    }

    GError *err = NULL;
    if (!g_spawn_async(NULL, argv, NULL, flags, NULL, NULL, NULL, &err)) {
        lyy_log(&app->log, "ERROR 拉起直输模式窗口失败(%s):%s", argv[0],
                err ? err->message : "?");
        GtkWidget *dlg = gtk_message_dialog_new(
            NULL, GTK_DIALOG_MODAL, GTK_MESSAGE_INFO, GTK_BUTTONS_OK,
            "未找到 %s(Mode C 直输悬浮窗:独立窗口打字,直输/粘贴发送)。\n\n"
            "安装方式:scripts/install-all.sh 或 "
            "crates/lyyime-float/install.sh。",
            kName);
        gtk_dialog_run(GTK_DIALOG(dlg));
        gtk_widget_destroy(dlg);
        g_clear_error(&err);
        return;
    }
    /* lyyime-float 自带单实例(二次启动 SIGUSR1 唤起已存在窗口),重复拉起安全 */
    lyy_log(&app->log, "已拉起直输模式窗口:%s", argv[0]);
}

/* ---- 工具:截屏(拉起 lyyime-shot;热键不可达的英文直通态也能用) ---- */
void lyy_tools_screenshot(App *app)
{
    lyy_spawn_shot(app);
}

/* ---- 工具:修复输入法(危险操作,先确认再执行) ---- */
void lyy_tools_fix_ime(App *app)
{
    GtkWidget *confirm = gtk_message_dialog_new(
        NULL, GTK_DIALOG_MODAL, GTK_MESSAGE_QUESTION, GTK_BUTTONS_OK_CANCEL,
        "将执行 \"%s check --fix\":\n"
        "诊断并修复输入法环境(XMODIFIERS/autostart/ibus 注册/缓存),\n"
        "可能修改会话配置文件。是否继续?",
        DOCTOR_BIN);
    if (gtk_dialog_run(GTK_DIALOG(confirm)) != GTK_RESPONSE_OK) {
        gtk_widget_destroy(confirm);
        return;
    }
    gtk_widget_destroy(confirm);

    gchar *out = NULL;
    int missing = 0;
    if (run_command_capture(DOCTOR_BIN " check --fix", &out, &missing) != 0) {
        if (missing)
            doctor_missing_dialog(NULL);
        else
            show_text_dialog(NULL, "修复输入法 — 执行失败",
                             "命令执行失败,请检查 lyyime-doctor 是否可用。");
        return;
    }
    show_text_dialog(NULL, "修复输入法 — 执行结果", out);
    g_free(out);
    (void)app;
}

/* ---- 工具:输入法管理(exec lyyime-doctor ime-list 并解析 JSON) ----
 * doctor 合同(docs/ARCHITECTURE.md §9.1):输出对象含
 *   id/name/kind/installed/active/is_default。
 * 此处做容错解析:能解析则展示清单(变更操作给 CLI 指引),
 * 解析失败则原样展示文本并提示用 CLI。 */
typedef struct {
    char id[128];
    char name[192];
    char kind[32];
    int installed, active, is_default;
} ImeEntry;

/* 在一个 JSON 对象文本内提取 "key":"value" / "key":true|false */
static void json_obj_field(const char *obj, const char *key, char *out,
                           size_t cap)
{
    out[0] = '\0';
    char pat[64];
    snprintf(pat, sizeof(pat), "\"%s\"", key);
    const char *p = strstr(obj, pat);
    if (!p)
        return;
    p = strchr(p + strlen(pat), ':');
    if (!p)
        return;
    p++;
    while (*p == ' ')
        p++;
    if (*p == '"') {
        p++;
        size_t w = 0;
        while (*p && *p != '"' && w + 1 < cap)
            out[w++] = *p++;
        out[w] = '\0';
    } else {
        snprintf(out, cap, "%.*s", (int)strcspn(p, ",}"), p);
    }
}

static int parse_ime_list(const char *text, ImeEntry *entries, int max)
{
    int n = 0;
    const char *p = text;
    while (p && (p = strchr(p, '{')) != NULL) {
        const char *end = strchr(p, '}');
        if (!end)
            break;
        char obj[1024];
        size_t len = (size_t)(end - p + 1);
        if (len >= sizeof(obj))
            len = sizeof(obj) - 1;
        memcpy(obj, p, len);
        obj[len] = '\0';

        char id[128];
        json_obj_field(obj, "id", id, sizeof(id));
        if (id[0] && n < max) {
            ImeEntry *e = &entries[n++];
            snprintf(e->id, sizeof(e->id), "%s", id);
            json_obj_field(obj, "name", e->name, sizeof(e->name));
            json_obj_field(obj, "kind", e->kind, sizeof(e->kind));
            char flag[16];
            json_obj_field(obj, "installed", flag, sizeof(flag));
            e->installed = strstr(flag, "true") != NULL;
            json_obj_field(obj, "active", flag, sizeof(flag));
            e->active = strstr(flag, "true") != NULL;
            json_obj_field(obj, "is_default", flag, sizeof(flag));
            e->is_default = strstr(flag, "true") != NULL;
        }
        p = end + 1;
    }
    return n;
}

void lyy_tools_manage_ime(App *app)
{
    gchar *out = NULL;
    int missing = 0;
    /* 合同未保证 --json 旗标;先试 --json,失败退化为原始输出展示 */
    gchar *cmd = g_strdup_printf("%s ime-list --json", DOCTOR_BIN);
    int rc = run_command_capture(cmd, &out, &missing);
    g_free(cmd);
    if (rc != 0) {
        if (missing)
            doctor_missing_dialog(NULL);
        else
            show_text_dialog(NULL, "输入法管理", "ime-list 执行失败。");
        return;
    }
    if (!out || !out[0]) {
        g_free(out);
        out = NULL;
        cmd = g_strdup_printf(DOCTOR_BIN " ime-list");
        rc = run_command_capture(cmd, &out, &missing);
        g_free(cmd);
        if (rc != 0) {
            if (missing)
                doctor_missing_dialog(NULL);
            else
                show_text_dialog(NULL, "输入法管理", "ime-list 执行失败。");
            return;
        }
    }

    ImeEntry entries[32];
    int n = parse_ime_list(out ? out : "", entries, 32);
    if (n <= 0) {
        GString *g = g_string_new(
            "未能解析 ime-list JSON(可用 CLI 直接操作:\n"
            "lyyime-doctor ime-add/ime-remove/ime-default <id> "
            "[--dry-run|--yes])。\n\n--- 原始输出 ---\n");
        g_string_append(g, out ? out : "");
        show_text_dialog(NULL, "输入法管理", g->str);
        g_string_free(g, TRUE);
        g_free(out);
        return;
    }

    /* 组装展示文本 */
    GString *g = g_string_new("本机输入法(lyyime-doctor ime-list):\n\n");
    for (int i = 0; i < n; i++) {
        g_string_append_printf(g, "  %-24s %-10s %-6s %s%s%s\n",
                               entries[i].id, entries[i].kind,
                               entries[i].installed ? "已装" : "未装",
                               entries[i].name,
                               entries[i].active ? "  [使用中]" : "",
                               entries[i].is_default ? "  [默认]" : "");
    }
    g_string_append(g, "\n操作需二次确认;保护规则由 doctor 保证(绝不误删 "
                       "lyyime 自身或最后一个可用输入法)。\n"
                       "变更类操作 CLI:lyyime-doctor ime-add / ime-remove / "
                       "ime-default <id> --yes。");
    show_text_dialog(NULL, "输入法管理", g->str);
    g_string_free(g, TRUE);
    g_free(out);
    (void)app;
}

/* ---- 工具:重载词库 / 打开日志 ---- */
void lyy_tools_reload_dict(App *app)
{
    lyy_engine_reload(app);
    GtkWidget *dlg = gtk_message_dialog_new(
        NULL, GTK_DIALOG_MODAL, GTK_MESSAGE_INFO, GTK_BUTTONS_OK,
        "词库与配置已重载(引擎重建)。详见日志:\n%s", app->log.path);
    gtk_dialog_run(GTK_DIALOG(dlg));
    gtk_widget_destroy(dlg);
}

void lyy_tools_open_log(App *app)
{
    gchar *content = NULL;
    if (!g_file_get_contents(app->log.path, &content, NULL, NULL))
        content = g_strdup("(日志为空或不可读)");
    /* 只展示尾部 8000 字符,避免超长卡顿 */
    size_t len = strlen(content);
    const char *tail = (len > 8000) ? content + (len - 8000) : content;
    show_text_dialog(NULL, "xim.log(尾部)", tail);
    g_free(content);
}
