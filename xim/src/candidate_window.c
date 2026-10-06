#include "candidate_window.h"

#include <stdio.h>
#include <string.h>
#include <xcb/xproto.h>

#include "skin.h"

/* candidate.css 缺失/损坏时的内置布局兜底(颜色归皮肤层,这里只保骨架) */
static const char LYY_LAYOUT_FALLBACK[] =
    ".lyy-outer { background: transparent; }\n"
    ".lyy-frame { border: 1px solid; border-radius: 8px; padding: 6px 10px; }\n"
    ".lyy-header { margin-bottom: 2px; }\n"
    ".lyy-row { padding: 1px 4px; border-radius: 4px; }\n"
    ".lyy-first .lyy-num,.lyy-first .lyy-word,.lyy-first .lyy-comment {"
    " font-weight: bold; }\n";

/* gtk_css_provider_load_from_data 恒返回 TRUE;解析错误经 parsing-error
 * 信号上报,先装临时 provider 计数,保证坏 CSS 不污染生效样式 */
static void on_css_error(GtkCssProvider *p, GtkCssSection *section,
                         const GError *error, gpointer user_data)
{
    (void)p;
    (void)section;
    int *n = user_data;
    (*n)++;
    g_warning("lyyime 候选窗 CSS 解析错误:%s",
              error ? error->message : "(未知)");
}

static int css_error_count(const char *css)
{
    GtkCssProvider *probe = gtk_css_provider_new();
    int errs = 0;
    g_signal_connect(probe, "parsing-error", G_CALLBACK(on_css_error), &errs);
    gtk_css_provider_load_from_data(probe, css, -1, NULL);
    g_object_unref(probe);
    return errs;
}

/* 样式装配:candidate.css(布局基准,可用户自定义)+ 皮肤层 CSS
 * (颜色/圆角/字号,skin.c 生成,排在其后同优先级覆盖)。
 * provider 复用同一对象、只挂候选窗子树(lyy_skin_apply_tree,APPLICATION
 * 优先级),不走 screen 级注入——设置窗皮肤预览与本窗互不透染。
 * 自定义布局损坏时回退内置骨架;皮肤 CSS 为程序生成不参与降级;
 * 仍失败则保留上一份有效 CSS(不空窗)。 */
static void apply_css(CandidateWindow *cw)
{
    cw->last_dark = lyy_skin_system_is_dark();
    const LyySkin *skin = lyy_skin_find(cw->skin);

    gchar *layout = NULL;
    if (cw->css_dir[0]) {
        char path[1200];
        snprintf(path, sizeof(path), "%s/candidate.css", cw->css_dir);
        gsize len = 0;
        if (!g_file_get_contents(path, &layout, &len, NULL))
            layout = NULL;
    }

    gchar *skin_css = lyy_skin_css(skin->id, cw->last_dark, cw->font_size);
    gchar *css = g_strdup_printf("%s\n%s\n",
                                 layout ? layout : LYY_LAYOUT_FALLBACK,
                                 skin_css);
    int errs = css_error_count(css);
    if (errs && layout) {
        g_warning("lyyime 候选窗:%s/candidate.css 解析失败(%d 处),"
                  "回退内置布局", cw->css_dir, errs);
        g_free(css);
        css = g_strdup_printf("%s\n%s\n", LYY_LAYOUT_FALLBACK, skin_css);
        errs = css_error_count(css);
    }
    if (!errs) {
        if (!cw->css_provider) {
            cw->css_provider = gtk_css_provider_new();
            gtk_css_provider_load_from_data(cw->css_provider, css, -1,
                                            NULL);
            lyy_skin_apply_tree(cw->win, cw->css_provider);
        } else {
            gtk_css_provider_load_from_data(cw->css_provider, css, -1,
                                            NULL);
        }
    } else {
        g_warning("lyyime 候选窗:皮肤 CSS 异常(%d 处),保留上一份有效样式",
                  errs);
    }
    g_free(css);
    g_free(skin_css);
    g_free(layout);
}

/* 主题切换在 X11/GTK3 没有统一信号;低频复查让「跟随系统」皮肤在运行中
 * 跟随明暗切换。显式皮肤不做定时重解析(固定调色板,与系统明暗无关);
 * system 仅在明暗实际变化时才重建 CSS,避免无谓解析。 */
static gboolean on_theme_timer(gpointer user_data)
{
    CandidateWindow *cw = user_data;
    if (strcmp(cw->skin, "system") != 0)
        return G_SOURCE_CONTINUE;
    gboolean dark = lyy_skin_system_is_dark();
    if (dark != cw->last_dark)
        apply_css(cw);
    return G_SOURCE_CONTINUE;
}

static void on_win_destroy_conn(GtkWidget *w, gpointer user_data)
{
    (void)w;
    CandidateWindow *cw = user_data;
    if (cw->pointer_conn) {
        xcb_disconnect(cw->pointer_conn);
        cw->pointer_conn = NULL;
    }
}

/* 光标跟随:80ms 轮询 xcb_query_pointer(root-window style 下无 spot 通知)。
 * 悬停/菜单冻结(§15):指针一旦进入候选窗或右键菜单打开,窗口定住——
 * 否则窗永远贴在指针右下 20px,行根本点不中、右键菜单无法交互。 */
static gboolean on_pos_timer(gpointer user_data)
{
    CandidateWindow *cw = user_data;
    if (!gtk_widget_get_visible(cw->win))
        return G_SOURCE_REMOVE;
    if (cw->hover || cw->menu_open)
        return G_SOURCE_CONTINUE;
    if (!cw->pointer_conn)
        return G_SOURCE_CONTINUE;

    xcb_query_pointer_cookie_t cookie =
        xcb_query_pointer(cw->pointer_conn, cw->root);
    xcb_query_pointer_reply_t *reply =
        xcb_query_pointer_reply(cw->pointer_conn, cookie, NULL);
    if (reply) {
        /* 几何悬停兜底:指针已在当前窗口矩形内 → 直接冻结。
         * enter/leave 事件经 GTK 派发,与 80ms 定时存在竞态(指针瞬移进窗、
         * 定时器先跑 → 窗跳走 → 悬停标记立灭);这里同步判定保证窗口
         * 一旦被"够到"就不再逃逸。 */
        GtkAllocation cur;
        gtk_widget_get_allocation(cw->win, &cur);
        gint ox = 0, oy = 0;
        GdkWindow *self = gtk_widget_get_window(cw->win);
        if (self) {
            gdk_window_get_origin(self, &ox, &oy);
            if (reply->root_x >= ox && reply->root_x < ox + cur.width &&
                reply->root_y >= oy && reply->root_y < oy + cur.height) {
                free(reply);
                return G_SOURCE_CONTINUE;
            }
        }
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

/* 指针进出窗口 → 悬停冻结跟随(见 on_pos_timer) */
static gboolean on_win_enter(GtkWidget *w, GdkEventCrossing *ev,
                             gpointer user_data)
{
    (void)w; (void)ev;
    ((CandidateWindow *)user_data)->hover = TRUE;
    return FALSE;
}

static gboolean on_win_leave(GtkWidget *w, GdkEventCrossing *ev,
                             gpointer user_data)
{
    (void)w; (void)ev;
    ((CandidateWindow *)user_data)->hover = FALSE;
    return FALSE;
}

/* ---- 右键菜单(§15 候选管理:固定首位/删除词组/反查英文) ---- */

/* 菜单关闭:解冻跟随 + 销毁菜单(popup 期间 grab 会压住行事件) */
static gboolean menu_destroy_idle(gpointer data)
{
    GtkWidget *menu = data;
    gtk_menu_popdown(GTK_MENU(menu));
    gtk_widget_destroy(menu);
    g_object_unref(menu);
    return G_SOURCE_REMOVE;
}

static void on_menu_done(GtkMenuShell *menu, gpointer user_data)
{
    ((CandidateWindow *)user_data)->menu_open = FALSE;
    if (g_object_get_data(G_OBJECT(menu), "lyy-done"))
        return;
    g_object_set_data(G_OBJECT(menu), "lyy-done", GINT_TO_POINTER(1));
    g_idle_add(menu_destroy_idle, menu);
}

/* 自定义查询(§15 菜单第 4 项,宿主侧动作):网址模板 {q} 占位符替换为
 * 百分号编码的候选词,xdg-open 拉起浏览器;不经 core/op_fn,菜单 grab
 * 引起的 XIM 焦点漂移不影响执行。模板无 {q} 时原样打开。 */
static void open_query_url(CandidateWindow *cw, int idx)
{
    if (idx < 0 || idx >= cw->row_count || !cw->query_url[0])
        return;
    const char *word = gtk_label_get_text(GTK_LABEL(cw->word[idx]));
    if (!word || !*word)
        return;
    /* allow_utf8=FALSE:UTF-8 字节也百分号编码(查询字段值规范;中文词
     * → %EXX%EXX%EXX,与 core wordops::custom_query_url 同合同) */
    char *enc = g_uri_escape_string(word, NULL, FALSE);
    char **parts = g_strsplit(cw->query_url, "{q}", -1);
    char *url = g_strjoinv(enc, parts);
    g_free(enc);
    g_strfreev(parts);
    char *argv[] = { (char *)"xdg-open", url, NULL };
    GError *err = NULL;
    if (!g_spawn_async(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL, NULL,
                       NULL, &err)) {
        g_warning("自定义查询启动失败:%s",
                  err ? err->message : "未知错误");
        g_clear_error(&err);
    }
    g_free(url);
}

/* 菜单项激活:op/idx 挂 item 对象数据(菜单短生命周期,无需堆分配) */
static void on_menu_item_activate(GtkMenuItem *item, gpointer user_data)
{
    CandidateWindow *cw = user_data;
    int idx = GPOINTER_TO_INT(g_object_get_data(G_OBJECT(item), "lyy-idx"));
    int op = GPOINTER_TO_INT(g_object_get_data(G_OBJECT(item), "lyy-op"));
    if (op == LYY_CAND_OP_QUERY) {
        open_query_url(cw, idx); /* 宿主侧动作,不走 core op_fn */
        return;
    }
    if (cw->op_fn)
        cw->op_fn(idx, op, cw->op_user_data);
}

GtkWidget *lyy_candwin_build_menu(CandidateWindow *cw, int idx)
{
    if (idx == -1) {
        if (!cw->general_fn)
            return NULL;
        GtkWidget *menu = gtk_menu_new();
        cw->general_fn(GTK_MENU_SHELL(menu), cw->general_user_data);
        return menu;
    }
    if (idx < 0 || idx >= cw->row_count || !cw->op_state_fn || !cw->op_fn)
        return NULL;
    int pinned = cw->op_state_fn(idx, cw->op_user_data);
    if (pinned < 0)
        return NULL;
    GtkWidget *menu = gtk_menu_new();
    struct { const char *label; int op; } items[] = {
        { pinned ? "取消固定首位" : "固定首位", LYY_CAND_OP_PIN },
        { "删除词组", LYY_CAND_OP_DELETE },
        { "反查英文", LYY_CAND_OP_EN_LOOKUP },
    };
    for (gsize i = 0; i < G_N_ELEMENTS(items); i++) {
        GtkWidget *item = gtk_menu_item_new_with_label(items[i].label);
        g_object_set_data(G_OBJECT(item), "lyy-idx", GINT_TO_POINTER(idx));
        g_object_set_data(G_OBJECT(item), "lyy-op",
                          GINT_TO_POINTER(items[i].op));
        g_signal_connect(item, "activate",
                         G_CALLBACK(on_menu_item_activate), cw);
        gtk_menu_shell_append(GTK_MENU_SHELL(menu), item);
    }
    /* 自定义查询(config.toml custom_query_*;url 空则不显示) */
    if (cw->query_url[0]) {
        const char *label = cw->query_label[0] ? cw->query_label
                                               : "自定义查询";
        GtkWidget *item = gtk_menu_item_new_with_label(label);
        g_object_set_data(G_OBJECT(item), "lyy-idx", GINT_TO_POINTER(idx));
        g_object_set_data(G_OBJECT(item), "lyy-op",
                          GINT_TO_POINTER(LYY_CAND_OP_QUERY));
        g_signal_connect(item, "activate",
                         G_CALLBACK(on_menu_item_activate), cw);
        gtk_menu_shell_append(GTK_MENU_SHELL(menu), item);
    }
    return menu;
}
static void popup_row_menu(CandidateWindow *cw, int idx, GdkEventButton *ev)
{
    GtkWidget *menu = lyy_candwin_build_menu(cw, idx);
    if (!menu)
        return;
    cw->menu_open = TRUE;
    g_object_ref_sink(menu);
    g_signal_connect(menu, "deactivate", G_CALLBACK(on_menu_done), cw);
    g_signal_connect(menu, "selection-done", G_CALLBACK(on_menu_done), cw);
    gtk_widget_show_all(menu);
    gtk_menu_popup_at_pointer(GTK_MENU(menu), (GdkEvent *)ev);
}

static gboolean on_gear_pressed(GtkWidget *w, GdkEventButton *ev,
                                gpointer user_data)
{
    (void)w;
    CandidateWindow *cw = user_data;
    if (ev->button == 1) {
        if (cw->settings_fn)
            cw->settings_fn(cw->settings_user_data);
        return TRUE;
    }
    if (ev->button == 3 && cw->general_fn) {
        popup_row_menu(cw, -1, ev);
        return TRUE;
    }
    return FALSE;
}

/* 行点击(§14 鼠标点选 + §15 右键菜单):左键 → 宿主回调(select_candidate),
 * 右键 → 候选管理菜单(固定首位/删除词组/反查英文),不触发上屏。
 * 事件必须在顶层窗按坐标命中行:no-window 行 widget 收不到按钮事件 ——
 * GTK3 POPUP 窗的隐式指针 grab 使事件只派发到顶层 GdkWindow,
 * 无窗 widget 的 add_events 不产生 X 级事件选择(实测:Xvfb 下行 handler
 * 永不触发,窗口级 handler 正常)。 */
static gboolean on_win_pressed(GtkWidget *win, GdkEventButton *ev,
                               gpointer user_data)
{
    CandidateWindow *cw = user_data;
    {
        int gx = 0, gy = 0;
        if (cw->gear && gtk_widget_get_visible(cw->gear) &&
            gtk_widget_translate_coordinates(cw->gear, win, 0, 0, &gx, &gy)) {
            GtkAllocation ga;
            gtk_widget_get_allocation(cw->gear, &ga);
            if (ev->x >= gx && ev->x < gx + ga.width && ev->y >= gy &&
                ev->y < gy + ga.height) {
                if (ev->button == 1) {
                    if (cw->settings_fn)
                        cw->settings_fn(cw->settings_user_data);
                    return TRUE;
                }
                if (ev->button == 3 && cw->general_fn) {
                    popup_row_menu(cw, -1, ev);
                    return TRUE;
                }
                return TRUE;
            }
        }
    }
    int idx = -1;
    for (int i = 0; i < cw->row_count; i++) {
        int rx = 0, ry = 0;
        if (!gtk_widget_translate_coordinates(cw->rows[i], win, 0, 0, &rx,
                                              &ry))
            continue;
        GtkAllocation a;
        gtk_widget_get_allocation(cw->rows[i], &a);
        if (ev->x >= rx && ev->x < rx + a.width && ev->y >= ry &&
            ev->y < ry + a.height) {
            idx = i;
            break;
        }
    }
    if (idx < 0) {
        if (ev->button == 3 && cw->general_fn) {
            popup_row_menu(cw, -1, ev);
            return TRUE;
        }
        return FALSE;
    }
    if (ev->button == 3) {
        if ((cw->op_state_fn && cw->op_fn) || cw->general_fn)
            popup_row_menu(cw, idx, ev);
        return TRUE; /* 右键永不穿透为点选 */
    }
    if (ev->button == 1 && cw->on_click) {
        cw->on_click(idx, cw->click_user_data);
        return TRUE;
    }
    return FALSE;
}

static GtkWidget *make_row(CandidateWindow *cw, int i)
{
    /* 单元格间距 6px:序号/候选词/注释不粘连,与皮肤预览一致 */
    GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    gtk_widget_set_halign(row, GTK_ALIGN_START);
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
    cw->pointer_conn = xcb_connect(NULL, NULL);
    if (xcb_connection_has_error(cw->pointer_conn)) {
        xcb_disconnect(cw->pointer_conn);
        cw->pointer_conn = NULL;
        g_warning("候选窗独立指针连接失败:跟随定位停用");
    }
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

    cw->gear = gtk_button_new();
    GtkWidget *gear_img = gtk_image_new_from_icon_name(
        "emblem-system-symbolic", GTK_ICON_SIZE_MENU);
    gtk_button_set_image(GTK_BUTTON(cw->gear), gear_img);
    gtk_button_set_relief(GTK_BUTTON(cw->gear), GTK_RELIEF_NONE);
    gtk_widget_set_can_focus(cw->gear, FALSE);
    gtk_widget_set_tooltip_text(cw->gear, "打开设置");
    AtkObject *gear_acc = gtk_widget_get_accessible(cw->gear);
    if (gear_acc)
        atk_object_set_name(gear_acc, "打开设置");
    gtk_box_pack_end(GTK_BOX(header), cw->gear, FALSE, FALSE, 0);
    g_signal_connect(cw->gear, "button-press-event",
                     G_CALLBACK(on_gear_pressed), cw);

    cw->page = gtk_label_new(NULL);
    gtk_style_context_add_class(gtk_widget_get_style_context(cw->page),
                                "lyy-page");
    gtk_box_pack_end(GTK_BOX(header), cw->page, FALSE, FALSE, 0);

    for (int i = 0; i < LYY_MAX_ROWS; i++) {
        cw->rows[i] = make_row(cw, i);
        gtk_box_pack_start(GTK_BOX(cw->frame), cw->rows[i], FALSE, FALSE, 0);
    }
    cw->row_count = 0;

    /* 悬停冻结(§15)+ 行点击(§14/§15):事件掩码必须挂在顶层窗
       (无窗子 widget 的 add_events 不产生 X 级选择,见 on_win_pressed 注释) */
    gtk_widget_add_events(cw->win, GDK_BUTTON_PRESS_MASK |
                                   GDK_ENTER_NOTIFY_MASK |
                                   GDK_LEAVE_NOTIFY_MASK);
    g_signal_connect(cw->win, "button-press-event",
                     G_CALLBACK(on_win_pressed), cw);
    g_signal_connect(cw->win, "enter-notify-event",
                     G_CALLBACK(on_win_enter), cw);
    g_signal_connect(cw->win, "leave-notify-event",
                     G_CALLBACK(on_win_leave), cw);
    g_signal_connect(cw->win, "destroy",
                     G_CALLBACK(on_win_destroy_conn), cw);

    /* 皮肤默认「跟随系统」;宿主(main.c)随后按 config.skin 调 set_skin */
    snprintf(cw->skin, sizeof(cw->skin), "%s", "system");
    apply_css(cw);
    cw->theme_timer = g_timeout_add_seconds(5, on_theme_timer, cw);
}

void lyy_candwin_set_op_fns(CandidateWindow *cw, LyyCandwinOpStateFn state_fn,
                            LyyCandwinOpFn op_fn, void *user_data)
{
    cw->op_state_fn = state_fn;
    cw->op_fn = op_fn;
    cw->op_user_data = user_data;
}

void lyy_candwin_set_general_fn(CandidateWindow *cw, LyyCandwinMenuFn fn,
                                void *user_data)
{
    cw->general_fn = fn;
    cw->general_user_data = user_data;
}

void lyy_candwin_set_settings_fn(CandidateWindow *cw,
                                 LyyCandwinSettingsFn fn, void *user_data)
{
    cw->settings_fn = fn;
    cw->settings_user_data = user_data;
}
void lyy_candwin_set_query(CandidateWindow *cw, const char *label,
                           const char *url)
{
    snprintf(cw->query_label, sizeof(cw->query_label), "%s",
             label ? label : "");
    snprintf(cw->query_url, sizeof(cw->query_url), "%s", url ? url : "");
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

void lyy_candwin_set_skin(CandidateWindow *cw, const char *id)
{
    const LyySkin *s = lyy_skin_find(id); /* 注册表归一,未知/空 → system */
    if (!strcmp(cw->skin, s->id))
        return;
    snprintf(cw->skin, sizeof(cw->skin), "%s", s->id);
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
        /* show_all 递归复活 begin_rows 藏起的空行(rows[row_count..MAX)
         * 会带陈旧文本残影:立即按 row_count 重新隐藏 */
        for (int i = cw->row_count; i < LYY_MAX_ROWS; i++)
            gtk_widget_hide(cw->rows[i]);
        on_pos_timer(cw); /* 先定位一次再等轮询 */
        cw->pos_timer = g_timeout_add(80, on_pos_timer, cw);
        if (!cw->theme_timer)
            cw->theme_timer = g_timeout_add_seconds(5, on_theme_timer, cw);
    }
    /* 自提升到栈顶:override-redirect 窗不随应用焦点重排,
     * 客户端窗口被激活/点击抬高后会把候选窗压到下层(行点击/右键
     * 全部落空);每次候选排版都把自己抬上去。组合期按键必触发
     * commit_layout,故实际效果等价"始终置顶"。 */
    {
        GdkWindow *gw = gtk_widget_get_window(cw->win);
        if (gw)
            gdk_window_raise(gw);
    }
}

void lyy_candwin_hide(CandidateWindow *cw)
{
    cw->hover = FALSE;
    cw->menu_open = FALSE;
    if (cw->pos_timer) {
        g_source_remove(cw->pos_timer);
        cw->pos_timer = 0;
    }
    if (cw->theme_timer) {
        g_source_remove(cw->theme_timer);
        cw->theme_timer = 0;
    }
    if (gtk_widget_get_visible(cw->win))
        gtk_widget_hide(cw->win);
}

int lyy_candwin_is_visible(const CandidateWindow *cw)
{
    return gtk_widget_get_visible(cw->win);
}
