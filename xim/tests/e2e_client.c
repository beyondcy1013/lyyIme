/*
 * E2E 客户端:标准 GTK3 Entry 接入 lyyime-xim(XIM)
 *
 * 用法:e2e_client <缓冲文件> [自动退出秒数]
 *   - 每次 Entry 缓冲变化,把当前文本原子写入缓冲文件(tmp+rename),
 *     供 xim_e2e.sh 轮询断言;
 *   - 关闭/超时时向 stdout 打 "ENTRY_BUFFER=<文本>"(与 spike 客户端同约定);
 *   - 窗口标题固定 lyyime-e2e-client,供 xdotool 定位。
 */
#include <gtk/gtk.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static GtkWidget *g_entry = NULL;
static char g_file[1024];
static int g_dumped = 0;

static void write_buffer_file(const char *text)
{
    if (!g_file[0])
        return;
    char tmp[1200];
    snprintf(tmp, sizeof(tmp), "%s.tmp", g_file);
    FILE *fp = fopen(tmp, "w");
    if (!fp)
        return;
    fputs(text, fp);
    fclose(fp);
    if (rename(tmp, g_file) != 0)
        perror("rename");
}

static void dump_stdout(const char *tag)
{
    if (g_dumped)
        return;
    const char *text = gtk_entry_get_text(GTK_ENTRY(g_entry));
    printf("%s=%s\n", tag, text);
    fflush(stdout);
}

static void on_changed(GtkEditable *editable, gpointer user_data)
{
    (void)editable;
    (void)user_data;
    const char *text = gtk_entry_get_text(GTK_ENTRY(g_entry));
    write_buffer_file(text);
    dump_stdout("ENTRY_CHANGED");
}

static void final_dump(void)
{
    if (g_dumped)
        return;
    const char *text = gtk_entry_get_text(GTK_ENTRY(g_entry));
    write_buffer_file(text);
    printf("ENTRY_BUFFER=%s\n", text);
    fflush(stdout);
    g_dumped = 1;
}

static gboolean on_timeout(gpointer user_data)
{
    int *remain = user_data;
    if (*remain > 0) {
        (*remain)--;
        return G_SOURCE_CONTINUE;
    }
    final_dump();
    gtk_main_quit();
    return G_SOURCE_REMOVE;
}

static void trace_event(GdkEvent *event, gpointer data)
{
    (void)data;
    if (event->type == GDK_KEY_PRESS || event->type == GDK_KEY_RELEASE)
        fprintf(stderr,
                "CLIENT_KEY type=%d keyval=%u window=%p focus=%d\n",
                event->type, event->key.keyval, (void *)event->key.window,
                gtk_widget_has_focus(g_entry));
    else if (event->type == GDK_FOCUS_CHANGE)
        fprintf(stderr, "CLIENT_FOCUS in=%d window=%p\n",
                event->focus_change.in,
                (void *)event->focus_change.window);
    gtk_main_do_event(event);
}

int main(int argc, char *argv[])
{
    if (argc > 1)
        snprintf(g_file, sizeof(g_file), "%s", argv[1]);
    int remain = 20;
    if (argc > 2)
        remain = atoi(argv[2]);

    signal(SIGTERM, SIG_DFL); /* e2e 兜底 kill 直接结束 */

    gtk_init(&argc, &argv);
    gdk_event_handler_set(trace_event, NULL, NULL);

    GtkWidget *win = gtk_window_new(GTK_WINDOW_TOPLEVEL);
    gtk_window_set_title(GTK_WINDOW(win), "lyyime-e2e-client");
    gtk_window_set_default_size(GTK_WINDOW(win), 520, 120);
    g_signal_connect(win, "destroy", G_CALLBACK(gtk_main_quit), NULL);

    GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 6);
    gtk_container_set_border_width(GTK_CONTAINER(box), 12);
    gtk_container_add(GTK_CONTAINER(win), box);

    GtkWidget *label = gtk_label_new("lyyIme E2E 客户端(XIM)");
    gtk_box_pack_start(GTK_BOX(box), label, FALSE, FALSE, 0);

    g_entry = gtk_entry_new();
    g_signal_connect(g_entry, "changed", G_CALLBACK(on_changed), NULL);
    gtk_box_pack_start(GTK_BOX(box), g_entry, FALSE, FALSE, 0);

    gtk_widget_show_all(win);
    gtk_entry_grab_focus_without_selecting(GTK_ENTRY(g_entry));
    gtk_widget_realize(win);
    gdk_window_focus(gtk_widget_get_window(win), GDK_CURRENT_TIME);

    g_timeout_add_seconds(1, on_timeout, &remain);
    gtk_main();
    return 0;
}
