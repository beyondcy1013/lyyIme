/*
 * lyyIme Mode B 探路石(spike)——GTK3 XIM 客户端
 *
 * 目的:以"标准 GTK3 应用"身份接入自建 XIM server,验证 GTK 内建 xim
 * immodule 在本机可用(架构合同 docs/ARCHITECTURE.md §8 的接入前提)。
 *
 * 接入方式(运行环境变量,由 run.sh 注入):
 *   XMODIFIERS=@im=lyyime   指定 XIM server 名(Xlib XOpenIM 依据)
 *   GTK_IM_MODULE=xim       强制 GTK 使用内建 xim immodule
 *   LANG/LC_ALL=zh_CN.utf8  COMPOUND_TEXT ↔ UTF-8 转码依赖 locale
 *
 * 行为约定(供 run.sh 断言):
 *   - 窗口内放一个 GtkEntry;
 *   - 每次 Entry 缓冲变化,向 stdout 打一行 "ENTRY_CHANGED:<文本>";
 *   - 窗口关闭(自定超时/WM 关闭)时打一行 "ENTRY_BUFFER=<文本>" 并退出;
 *   - 收到 SIGTERM 也尽量先落一行缓冲再退出(保证断言不依赖时序)。
 *
 * 借鉴出处:GTK3 官方文档 GtkEntry/GtkImMulticontext 用法;
 *   XIM 客户端环境变量约定见 GTK xim immodule(gtk3-immodule-xim)与
 *   Xlib XOpenIM 手册页。
 */
#include <gtk/gtk.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static GtkWidget *g_entry = NULL;
static int g_buf_dumped = 0;

/* 把 Entry 当前缓冲打成一行,前缀固定,供脚本 grep 断言 */
static void dump_buffer(const char *tag)
{
    if (g_buf_dumped)
        return;
    const char *text = gtk_entry_get_text(GTK_ENTRY(g_entry));
    printf("%s=%s\n", tag, text);
    fflush(stdout);
    if (strcmp(tag, "ENTRY_BUFFER") == 0)
        g_buf_dumped = 1;
}

static void on_changed(GtkEditable *editable, gpointer user_data)
{
    (void)editable;
    (void)user_data;
    dump_buffer("ENTRY_CHANGED");
}

static void on_destroy(void)
{
    /* 关闭时把 Entry 缓冲打到 stdout(任务书约定) */
    dump_buffer("ENTRY_BUFFER");
    gtk_main_quit();
}

static gboolean on_delete_event(GtkWidget *widget, GdkEvent *event,
                                gpointer user_data)
{
    (void)widget;
    (void)event;
    (void)user_data;
    dump_buffer("ENTRY_BUFFER");
    return FALSE; /* 继续默认销毁流程 → on_destroy */
}

/* 自定超时自动退出,避免 E2E 依赖外部 kill 时序 */
static gboolean on_timeout(gpointer user_data)
{
    int *alive = user_data;
    if (*alive > 0) {
        (*alive)--;
        return G_SOURCE_CONTINUE;
    }
    dump_buffer("ENTRY_BUFFER");
    gtk_main_quit();
    return G_SOURCE_REMOVE;
}

static void on_sigterm(int sig)
{
    (void)sig;
    /* 信号处理器里不能安全调用 GTK,只置位;由主循环超时兜底输出。
     * 这里直接 _exit 前用 async-signal-safe 的 write 落一条标记,
     * 缓冲内容以 ENTRY_CHANGED 流水为准。 */
    static const char msg[] = "SIGTERM received\n";
    ssize_t rc = write(2, msg, sizeof(msg) - 1);
    (void)rc;
    _exit(0);
}

int main(int argc, char *argv[])
{
    /* argv[1]=自动退出秒数(默认 8;run.sh 传短值加速) */
    int remain = 8;
    if (argc > 1)
        remain = atoi(argv[1]);

    signal(SIGTERM, on_sigterm);

    gtk_init(&argc, &argv);

    GtkWidget *win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(win), "lyyime-spike-client");
    gtk_window_set_default_size(GTK_WINDOW(win), 480, 120);
    g_signal_connect(win, "destroy", G_CALLBACK(on_destroy), NULL);
    g_signal_connect(win, "delete-event", G_CALLBACK(on_delete_event), NULL);

    GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
    gtk_container_set_border_width(GTK_CONTAINER(box), 12);
    gtk_container_add(GTK_CONTAINER(win), box);

    GtkWidget *label = gtk_label_new("lyyIme spike 客户端(XIM):请输入文字");
    gtk_box_pack_start(GTK_BOX(box), label, FALSE, FALSE, 0);

    g_entry = gtk_entry_new();
    g_signal_connect(g_entry, "changed", G_CALLBACK(on_changed), NULL);
    gtk_box_pack_start(GTK_BOX(box), g_entry, FALSE, FALSE, 0);

    gtk_widget_show_all(win);
    gtk_entry_grab_focus_without_selecting(GTK_ENTRY(g_entry));

    /* 让 Entry 真正持有键盘焦点:等窗口映射后再要一次焦点 */
    gtk_widget_realize(win);
    gdk_window_focus(gtk_widget_get_window(win), GDK_CURRENT_TIME);

    g_timeout_add_seconds(1, on_timeout, &remain);
    gtk_main();
    return 0;
}
