#include "candidate_window.h"

#include <stdio.h>
#include <string.h>
#include <xcb/xproto.h>

/* CSS 提供者:文件样式 + 动态 font-size 覆盖(设置即时生效) */
static void apply_css(CandidateWindow *cw)
{
    GtkCssProvider *provider = gtk_css_provider_new();
    gchar *builtin =
        (gchar *)".lyy-outer { background: rgba(0,0,0,0); }\n"
                 ".lyy-frame { background: rgba(250,250,250,0.98);"
                 " border: 1px solid #b0b0b0; border-radius: 8px;"
                 " padding: 6px 10px;"
                 " box-shadow: 0 4px 16px rgba(0,0,0,0.35); }\n"
                 ".lyy-header { margin-bottom: 2px; }\n"
                 ".lyy-preedit { color: #242424; font-weight: bold; }\n"
                 ".lyy-page { color: #6e6e6e; margin-left: 10px; }\n"
                 ".lyy-row { padding: 1px 4px; border-radius: 4px; }\n"
                 ".lyy-first { background: #3584e4; }\n"
                 ".lyy-num { color: #8a8a8a; margin-right: 6px; }\n"
                 ".lyy-first .lyy-num { color: #dce8f7; }\n"
                 ".lyy-word { color: #242424; }\n"
                 ".lyy-first .lyy-word { color: #ffffff; font-weight: bold; }\n"
                 ".lyy-comment { color: #707070; margin-left: 10px; }\n"
                 ".lyy-first .lyy-comment { color: #dce8f7; }\n";

    gchar *css;
    gchar *file_css = NULL;
    if (cw->css_dir[0]) {
        char path[1200];
        snprintf(path, sizeof(path), "%s/candidate.css", cw->css_dir);
        /* 读样式文件;失败回退内置样式 */
        gsize len = 0;
        if (g_file_get_contents(path, &file_css, &len, NULL) && file_css) {
            /* 贴边阴影/高亮主题以文件为准(运维可改),兜底类追加在后 */
            css = g_strdup_printf("%s\n", file_css);
        } else {
            css = g_strdup(builtin);
        }
    } else {
        css = g_strdup(builtin);
    }
    gchar *with_font = g_strdup_printf("%s.lyy-preedit { font-size: %dpx; }"
                                       ".lyy-word { font-size: %dpx; }"
                                       ".lyy-num { font-size: %dpx; }"
                                       ".lyy-comment { font-size: %dpx; }"
                                       ".lyy-page { font-size: %dpx; }\n",
                                       css, cw->font_size, cw->font_size,
                                       cw->font_size - 2,
                                       cw->font_size - 2, cw->font_size - 2);
    gtk_css_provider_load_from_data(provider, with_font, -1, NULL);
    gtk_style_context_add_provider_for_screen(
        gdk_screen_get_default(), GTK_STYLE_PROVIDER(provider),
        GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    g_free(with_font);
    g_free(css);
    g_free(file_css);
    g_object_unref(provider);
}

/* 光标跟随:80ms 轮询 xcb_query_pointer(root-window style 下无 spot 通知) */
static gboolean on_pos_timer(gpointer user_data)
{
    CandidateWindow *cw = user_data;
    if (!gtk_widget_get_visible(cw->win))
        return G_SOURCE_REMOVE;

    xcb_query_pointer_cookie_t cookie = xcb_query_pointer(cw->conn, cw->root);
    xcb_query_pointer_reply_t *reply =
        xcb_query_pointer_reply(cw->conn, cookie, NULL);
    if (reply) {
        int wx = reply->root_x + 20;
        int wy = reply->root_y + 30;
        GdkWindow *gwin = gtk_widget_get_window(cw->win);
        GdkDisplay *display = gdk_window_get_display(gwin);
        /* 屏幕尺寸:GdkMonitor API(gdk_screen_get_width 3.22 起弃用) */
        GdkMonitor *mon = gdk_display_get_monitor_at_window(display, gwin);
        GdkRectangle mon_geo;
        gdk_monitor_get_geometry(mon, &mon_geo);
        GtkAllocation alloc;
        gtk_widget_get_allocation(cw->win, &alloc);
        int sw = mon_geo.width;
        int sh = mon_geo.height;
        if (wx + alloc.width > sw - 8)
            wx = sw - alloc.width - 8; /* 贴右边缘防裁切 */
        if (wx < 8)
            wx = 8;
        if (wy + alloc.height > sh - 8)
            wy = reply->root_y - alloc.height - 12; /* 底边时翻到光标上方 */
        if (wy < 8)
            wy = 8;
        gtk_window_move(GTK_WINDOW(cw->win), wx, wy);
        free(reply);
    }
    return G_SOURCE_CONTINUE;
}

static GtkWidget *make_row(CandidateWindow *cw, int i)
{
    GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    gtk_style_context_add_class(gtk_widget_get_style_context(row), "lyy-row");
    cw->num[i] = gtk_label_new(NULL);
    gtk_style_context_add_class(gtk_widget_get_style_context(cw->num[i]),
                                "lyy-num");
    cw->word[i] = gtk_label_new(NULL);
    gtk_style_context_add_class(gtk_widget_get_style_context(cw->word[i]),
                                "lyy-word");
    cw->comment[i] = gtk_label_new(NULL);
    gtk_style_context_add_class(gtk_widget_get_style_context(cw->comment[i]),
                                "lyy-comment");
    gtk_widget_set_halign(cw->comment[i], GTK_ALIGN_START);
    gtk_box_pack_start(GTK_BOX(row), cw->num[i], FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(row), cw->word[i], FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(row), cw->comment[i], FALSE, FALSE, 0);
    return row;
}

void lyy_candwin_init(CandidateWindow *cw, xcb_connection_t *conn,
                      xcb_window_t root, const char *css_dir, int font_size)
{
    memset(cw, 0, sizeof(*cw));
    cw->conn = conn;
    cw->root = root;
    cw->font_size = font_size < 10 || font_size > 28 ? 14 : font_size;
    if (css_dir)
        snprintf(cw->css_dir, sizeof(cw->css_dir), "%s", css_dir);

    cw->win = gtk_window_new(GTK_WINDOW_POPUP); /* override-redirect */
    gtk_window_set_type_hint(GTK_WINDOW(cw->win),
                             GDK_WINDOW_TYPE_HINT_TOOLTIP);
    gtk_window_set_accept_focus(GTK_WINDOW(cw->win), FALSE);
    gtk_window_set_focus_on_map(GTK_WINDOW(cw->win), FALSE);
    gtk_window_set_resizable(GTK_WINDOW(cw->win), FALSE);
    gtk_widget_set_app_paintable(cw->win, TRUE);

    /* 无合成器时 rgba visual 为 NULL,自动回退普通视觉 */
    GdkVisual *visual =
        gdk_screen_get_rgba_visual(gdk_screen_get_default());
    if (visual)
        gtk_widget_set_visual(cw->win, visual);

    GtkWidget *outer = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    gtk_style_context_add_class(gtk_widget_get_style_context(outer),
                                "lyy-outer");
    gtk_container_add(GTK_CONTAINER(cw->win), outer);

    cw->frame = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_style_context_add_class(gtk_widget_get_style_context(cw->frame),
                                "lyy-frame");
    gtk_box_pack_start(GTK_BOX(outer), cw->frame, FALSE, FALSE, 0);

    GtkWidget *header = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    gtk_style_context_add_class(gtk_widget_get_style_context(header),
                                "lyy-header");
    gtk_box_pack_start(GTK_BOX(cw->frame), header, FALSE, FALSE, 0);

    cw->preedit = gtk_label_new(NULL);
    gtk_style_context_add_class(gtk_widget_get_style_context(cw->preedit),
                                "lyy-preedit");
    gtk_widget_set_halign(cw->preedit, GTK_ALIGN_START);
    gtk_box_pack_start(GTK_BOX(header), cw->preedit, FALSE, FALSE, 0);

    cw->page = gtk_label_new(NULL);
    gtk_style_context_add_class(gtk_widget_get_style_context(cw->page),
                                "lyy-page");
    gtk_box_pack_end(GTK_BOX(header), cw->page, FALSE, FALSE, 0);

    for (int i = 0; i < LYY_MAX_ROWS; i++) {
        cw->rows[i] = make_row(cw, i);
        gtk_box_pack_start(GTK_BOX(cw->frame), cw->rows[i], FALSE, FALSE, 0);
    }
    cw->row_count = 0;

    apply_css(cw);
}

void lyy_candwin_set_font_size(CandidateWindow *cw, int font_size)
{
    if (font_size < 10 || font_size > 28)
        return;
    if (font_size == cw->font_size)
        return;
    cw->font_size = font_size;
    apply_css(cw);
}

void lyy_candwin_begin_rows(CandidateWindow *cw)
{
    for (int i = 0; i < LYY_MAX_ROWS; i++)
        gtk_widget_hide(cw->rows[i]);
    cw->row_count = 0;
}

void lyy_candwin_add_row(CandidateWindow *cw, int idx, const char *text,
                         const char *comment)
{
    if (idx < 0 || idx >= LYY_MAX_ROWS)
        return;
    gchar num[16];
    snprintf(num, sizeof(num), "%d.", idx + 1);
    gtk_label_set_text(GTK_LABEL(cw->num[idx]), num);
    gtk_label_set_text(GTK_LABEL(cw->word[idx]), text ? text : "");
    gtk_label_set_text(GTK_LABEL(cw->comment[idx]),
                       comment && comment[0] ? comment : "");
    if (idx == 0)
        gtk_style_context_add_class(gtk_widget_get_style_context(cw->rows[0]),
                                    "lyy-first");
    gtk_widget_show_all(cw->rows[idx]);
    if (idx + 1 > cw->row_count)
        cw->row_count = idx + 1;
}

void lyy_candwin_set_preedit(CandidateWindow *cw, const char *text)
{
    gtk_label_set_text(GTK_LABEL(cw->preedit), text ? text : "");
}

void lyy_candwin_set_page(CandidateWindow *cw, int page, int pages)
{
    gchar buf[64];
    if (pages > 1)
        snprintf(buf, sizeof(buf), "%d/%d 页  -上一页 =下一页", page, pages);
    else
        buf[0] = '\0';
    gtk_label_set_text(GTK_LABEL(cw->page), buf);
}

void lyy_candwin_commit_layout(CandidateWindow *cw)
{
    gboolean has_preedit = gtk_label_get_text(GTK_LABEL(cw->preedit)) &&
                           gtk_label_get_text(GTK_LABEL(cw->preedit))[0];
    if (!has_preedit && cw->row_count == 0) {
        lyy_candwin_hide(cw);
        return;
    }
    if (!gtk_widget_get_visible(cw->win)) {
        gtk_widget_show_all(cw->win);
        on_pos_timer(cw); /* 先定位一次再等轮询 */
        cw->pos_timer = g_timeout_add(80, on_pos_timer, cw);
    }
}

void lyy_candwin_hide(CandidateWindow *cw)
{
    if (cw->pos_timer) {
        g_source_remove(cw->pos_timer);
        cw->pos_timer = 0;
    }
    if (gtk_widget_get_visible(cw->win))
        gtk_widget_hide(cw->win);
}

int lyy_candwin_is_visible(const CandidateWindow *cw)
{
    return gtk_widget_get_visible(cw->win);
}
