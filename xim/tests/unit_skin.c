/*
 * 皮肤注册表 + 候选窗皮肤 CSS 单元测试(需要 X 显示,经 xvfb-run 运行)
 * 覆盖:注册表枚举/查找回退、全部皮肤 CSS 双向明暗可解析、候选窗实际
 *       解析色值/字号(font 10/14/28)、双预览子树独立不透染、非法 id 回退
 *       system、字号切换不换肤、system 定时跟随系统明暗、显式皮肤稳定。
 * 运行:make skin-test(xvfb-run -a 自动配显示;真 GTK 上下文与窗口)
 */
#include "candidate_window.h"
#include "skin.h"

#include <math.h>
#include <stdio.h>
#include <string.h>
#include <xcb/xcb.h>

static int g_failed = 0;

#define CHECK(cond, msg)                                                   \
    do {                                                                   \
        if (cond)                                                          \
            printf("  通过: %s\n", msg);                                   \
        else {                                                             \
            printf("  失败: %s(行 %d)\n", msg, __LINE__);                 \
            g_failed++;                                                    \
        }                                                                  \
    } while (0)

/* "#rrggbb" → GdkRGBA */
static GdkRGBA hex_rgba(const char *hex)
{
    GdkRGBA c = { 0, 0, 0, 1 };
    unsigned r = 0, g = 0, b = 0;
    if (hex && hex[0] == '#' &&
        sscanf(hex + 1, "%02x%02x%02x", &r, &g, &b) == 3) {
        c.red = r / 255.0;
        c.green = g / 255.0;
        c.blue = b / 255.0;
    }
    return c;
}

static int rgba_eq(GdkRGBA a, GdkRGBA b)
{
    return fabs(a.red - b.red) < 0.02 && fabs(a.green - b.green) < 0.02 &&
           fabs(a.blue - b.blue) < 0.02;
}

static GdkRGBA color_of(GtkWidget *w)
{
    GdkRGBA c = { 0, 0, 0, 0 };
    gtk_style_context_get_color(gtk_widget_get_style_context(w),
                                GTK_STATE_FLAG_NORMAL, &c);
    return c;
}

/* background-color 是 boxed GdkRGBA:gtk_style_context_get 出参是
 * GdkRGBA**(复制体,用 gdk_rgba_free 释放),不是直接写结构体 */
static GdkRGBA bg_of(GtkWidget *w)
{
    GdkRGBA result = { 0, 0, 0, 0 };
    GdkRGBA *value = NULL;
    gtk_style_context_get(gtk_widget_get_style_context(w),
                          GTK_STATE_FLAG_NORMAL, "background-color", &value,
                          NULL);
    if (value) {
        result = *value;
        gdk_rgba_free(value);
    }
    return result;
}

/* 解析字号(px);font-size:Npx 存为绝对单位,主题默认是磅(pt)需按
 * 屏幕 dpi 换算;-1 = 未取到 */
static int font_px(GtkWidget *w)
{
    PangoFontDescription *fd = NULL;
    gtk_style_context_get(gtk_widget_get_style_context(w),
                          GTK_STATE_FLAG_NORMAL, "font", &fd, NULL);
    if (!fd)
        return -1;
    double size = (double)pango_font_description_get_size(fd) / PANGO_SCALE;
    int px;
    if (pango_font_description_get_size_is_absolute(fd)) {
        px = (int)(size + 0.5);
    } else {
        double dpi = gdk_screen_get_resolution(gtk_widget_get_screen(w));
        if (dpi <= 0)
            dpi = 96;
        px = (int)(size * dpi / 72.0 + 0.5);
    }
    pango_font_description_free(fd);
    return px;
}

/* load_from_data 恒返回 TRUE,解析错误经 parsing-error 信号上报 */
static void on_parse_error(GtkCssProvider *p, GtkCssSection *s,
                           const GError *e, gpointer data)
{
    (void)p;
    (void)s;
    (void)e;
    ++*(int *)data;
}

static int css_parse_ok(const char *css)
{
    int errs = 0;
    GtkCssProvider *p = gtk_css_provider_new();
    g_signal_connect(p, "parsing-error", G_CALLBACK(on_parse_error), &errs);
    gtk_css_provider_load_from_data(p, css, -1, NULL);
    g_object_unref(p);
    return errs == 0;
}

/* 跑 GTK 主循环 ms 毫秒(让样式生效、定时器到点) */
static void pump(int ms)
{
    gint64 end = g_get_monotonic_time() + (gint64)ms * 1000;
    while (g_get_monotonic_time() < end) {
        while (g_main_context_iteration(NULL, FALSE))
            ;
        g_usleep(20000);
    }
}

/* ---- 迷你预览树:与候选窗同一份类名(.lyy-frame/.lyy-header/.lyy-row 等),
 * 供"互不透染"断言用;返回行内关键控件供取色 */
typedef struct {
    GtkWidget *frame;
    GtkWidget *num0;   /* 首行序号(.lyy-first 内) */
    GtkWidget *word1;  /* 次行候选词 */
    GtkWidget *num1;
    GtkCssProvider *provider;
} Preview;

static Preview *preview_new(const char *skin_id, gboolean dark, int font)
{
    Preview *pv = g_new0(Preview, 1);
    pv->frame = gtk_box_new(GTK_ORIENTATION_VERTICAL, 2);
    gtk_style_context_add_class(gtk_widget_get_style_context(pv->frame),
                                "lyy-frame");
    GtkWidget *header = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
    gtk_style_context_add_class(gtk_widget_get_style_context(header),
                                "lyy-header");
    GtkWidget *preedit = gtk_label_new("nihao");
    gtk_style_context_add_class(gtk_widget_get_style_context(preedit),
                                "lyy-preedit");
    GtkWidget *page = gtk_label_new("1/3");
    gtk_style_context_add_class(gtk_widget_get_style_context(page),
                                "lyy-page");
    gtk_box_pack_start(GTK_BOX(header), preedit, FALSE, FALSE, 0);
    gtk_box_pack_end(GTK_BOX(header), page, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(pv->frame), header, FALSE, FALSE, 0);
    const char *cells[2][3] = { { "1.", "你好", "nihao" },
                                { "2.", "你们", "nimen" } };
    for (int i = 0; i < 2; i++) {
        GtkWidget *row = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
        gtk_style_context_add_class(gtk_widget_get_style_context(row),
                                    "lyy-row");
        if (i == 0)
            gtk_style_context_add_class(gtk_widget_get_style_context(row),
                                        "lyy-first");
        GtkWidget *num = gtk_label_new(cells[i][0]);
        gtk_style_context_add_class(gtk_widget_get_style_context(num),
                                    "lyy-num");
        GtkWidget *word = gtk_label_new(cells[i][1]);
        gtk_style_context_add_class(gtk_widget_get_style_context(word),
                                    "lyy-word");
        GtkWidget *com = gtk_label_new(cells[i][2]);
        gtk_style_context_add_class(gtk_widget_get_style_context(com),
                                    "lyy-comment");
        gtk_box_pack_start(GTK_BOX(row), num, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(row), word, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(row), com, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(pv->frame), row, FALSE, FALSE, 0);
        if (i == 0)
            pv->num0 = num;
        else {
            pv->num1 = num;
            pv->word1 = word;
        }
    }
    pv->provider = gtk_css_provider_new();
    char *css = lyy_skin_css(skin_id, dark, font);
    gtk_css_provider_load_from_data(pv->provider, css, -1, NULL);
    g_free(css);
    lyy_skin_apply_tree(pv->frame, pv->provider);
    return pv;
}

static void preview_free(Preview *pv)
{
    g_object_unref(pv->provider);
    g_free(pv);
}

int main(void)
{
    if (!gtk_init_check(NULL, NULL)) {
        fprintf(stderr, "无 X 显示:本测试需 xvfb-run 运行\n");
        return 2;
    }
    printf("== skin 单测 ==\n");

    /* 1. 注册表:system 居首,越界/空/未知一律回退 system */
    CHECK(lyy_skin_count() == 9, "注册表共 9 款皮肤");
    const LyySkin *sys = lyy_skin_at(0);
    CHECK(sys && strcmp(sys->id, "system") == 0, "system 居注册表首位");
    CHECK(lyy_skin_at(-1) == sys && lyy_skin_at(99) == sys &&
              lyy_skin_at(lyy_skin_count()) == sys,
          "越界下标回退 system");
    CHECK(lyy_skin_find(NULL) == sys && lyy_skin_find("") == sys &&
              lyy_skin_find("no-such") == sys,
          "NULL/空/未知 id 回退 system");
    CHECK(lyy_skin_find("sakura") != sys &&
              strcmp(lyy_skin_find("sakura")->id, "sakura") == 0,
          "sakura 命中注册表");

    /* 2. 全部皮肤 × 明暗 × 边界字号:生成 CSS 必须可被 GTK 解析 */
    {
        int all_ok = 1, n = lyy_skin_count();
        for (int i = 0; i < n; i++) {
            const LyySkin *s = lyy_skin_at(i);
            for (int dark = 0; dark <= 1; dark++) {
                char *css = lyy_skin_css(s->id, dark, 14);
                if (!css || !css_parse_ok(css) ||
                    !strstr(css, ".lyy-frame") || !strstr(css, ".lyy-first"))
                    all_ok = 0;
                g_free(css);
            }
        }
        CHECK(all_ok, "全部皮肤明/暗 CSS 均可解析且含 .lyy-frame/.lyy-first");
    }
    /* 字号钳制:越界值收敛到 10/28 */
    {
        char *lo = lyy_skin_css("sakura", FALSE, 3);
        char *hi = lyy_skin_css("sakura", FALSE, 99);
        CHECK(lo && strstr(lo, "font-size: 10px") && hi &&
                  strstr(hi, "font-size: 28px"),
              "lyy_skin_css 字号钳制 10..28");
        g_free(lo);
        g_free(hi);
    }
    /* system 暗色解析色表:暗底/亮字 */
    {
        char *dark_css = lyy_skin_css("system", TRUE, 14);
        char *lite_css = lyy_skin_css("system", FALSE, 14);
        CHECK(dark_css && strstr(dark_css, "#202020") &&
                  strstr(dark_css, "#f2f2f2") && lite_css &&
                  strstr(lite_css, "#fafafa") && strstr(lite_css, "#242424"),
              "system 皮肤随 dark 参数切换明暗色表");
        g_free(dark_css);
        g_free(lite_css);
    }

    /* 3. 真实候选窗:逐皮肤/字号核对解析颜色与字号 */
    int scr_no = 0;
    xcb_connection_t *conn = xcb_connect(NULL, &scr_no);
    CHECK(conn && !xcb_connection_has_error(conn), "xcb 连接成功");
    if (!conn || xcb_connection_has_error(conn))
        return 2;
    const xcb_setup_t *setup = xcb_get_setup(conn);
    xcb_screen_iterator_t it = xcb_setup_roots_iterator(setup);
    xcb_window_t root = it.data->root;

    CandidateWindow cw;
    lyy_candwin_init(&cw, conn, root, NULL, 14); /* css_dir NULL → 内置布局 */
    CHECK(strcmp(cw.skin, "system") == 0, "候选窗默认皮肤 system");
    gtk_widget_show_all(cw.win);
    pump(300);

    const int fonts[] = { 10, 14, 28 };
    int colors_ok = 1;
    int n = lyy_skin_count();
    for (int i = 1; i < n; i++) { /* system 明暗另测,这里遍历显式皮肤 */
        const LyySkin *s = lyy_skin_at(i);
        for (size_t f = 0; f < sizeof(fonts) / sizeof(fonts[0]); f++) {
            lyy_candwin_set_skin(&cw, s->id);
            lyy_candwin_set_font_size(&cw, fonts[f]);
            lyy_candwin_begin_rows(&cw);
            lyy_candwin_add_row(&cw, 0, "你好", "nihao");
            lyy_candwin_add_row(&cw, 1, "你们", "nimen");
            pump(120);
            GdkRGBA n0 = color_of(cw.num[0]), w0 = color_of(cw.word[0]),
                    c0 = color_of(cw.comment[0]), w1 = color_of(cw.word[1]),
                    n1 = color_of(cw.num[1]), fb = bg_of(cw.frame),
                    rb = bg_of(cw.rows[0]);
            int fp_w = font_px(cw.word[1]), fp_n = font_px(cw.num[1]);
            if (!rgba_eq(n0, hex_rgba(s->selected_fg)) ||
                !rgba_eq(w0, hex_rgba(s->selected_fg)) ||
                !rgba_eq(c0, hex_rgba(s->selected_fg)) ||
                !rgba_eq(w1, hex_rgba(s->fg)) ||
                !rgba_eq(n1, hex_rgba(s->muted)) ||
                !rgba_eq(fb, hex_rgba(s->bg)) ||
                !rgba_eq(rb, hex_rgba(s->accent)) ||
                fp_w != fonts[f] || fp_n != fonts[f] - 2) {
                colors_ok = 0;
                printf("    [失] skin=%s font=%d\n"
                       "      首选行字色 n0=(%.3f,%.3f,%.3f) "
                       "w0=(%.3f,%.3f,%.3f) c0=(%.3f,%.3f,%.3f) want %s\n"
                       "      次行字色 w1=(%.3f,%.3f,%.3f) want %s;"
                       " 序号 n1=(%.3f,%.3f,%.3f) want %s\n"
                       "      底板 bg=(%.3f,%.3f,%.3f) want %s;"
                       " 首选行底 bg=(%.3f,%.3f,%.3f) want %s\n"
                       "      字号 word=%d(要 %d) num=%d(要 %d)\n",
                       s->id, fonts[f], n0.red, n0.green, n0.blue, w0.red,
                       w0.green, w0.blue, c0.red, c0.green, c0.blue,
                       s->selected_fg, w1.red, w1.green, w1.blue, s->fg,
                       n1.red, n1.green, n1.blue, s->muted, fb.red, fb.green,
                       fb.blue, s->bg, rb.red, rb.green, rb.blue, s->accent,
                       fp_w, fonts[f], fp_n, fonts[f] - 2);
            }
        }
    }
    CHECK(colors_ok, "8 款皮肤 × 字号 10/14/28 解析色值/字号全部命中");

    /* 4. 非法 set_skin 回退 system;字号切换/重复设置不换肤 */
    lyy_candwin_set_skin(&cw, "totally-bogus");
    CHECK(strcmp(cw.skin, "system") == 0, "非法皮肤 id 回退 system");
    lyy_candwin_set_skin(&cw, "sakura");
    lyy_candwin_set_font_size(&cw, 20);
    CHECK(strcmp(cw.skin, "sakura") == 0 && cw.font_size == 20,
          "改字号不重置皮肤");
    lyy_candwin_set_skin(&cw, "cyber");
    lyy_candwin_set_skin(&cw, "cyber");
    pump(120);
    CHECK(strcmp(cw.skin, "cyber") == 0 &&
              rgba_eq(color_of(cw.word[1]), hex_rgba("#e9f8ff")),
          "重复设置同皮肤无副作用(cyber 色保持)");

    /* 5. 双预览子树不透染:同一窗口内两套 provider,解析色各自独立 */
    {
        GtkWidget *hold = gtk_window_new(GTK_WINDOW_TOPLEVEL);
        GtkWidget *vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
        gtk_container_add(GTK_CONTAINER(hold), vbox);
        Preview *a = preview_new("sakura", FALSE, 14);
        Preview *b = preview_new("cyber", FALSE, 14);
        gtk_box_pack_start(GTK_BOX(vbox), a->frame, FALSE, FALSE, 0);
        gtk_box_pack_start(GTK_BOX(vbox), b->frame, FALSE, FALSE, 0);
        gtk_widget_show_all(hold);
        pump(200);
        CHECK(rgba_eq(color_of(a->num0), hex_rgba("#ffffff")) &&
                  rgba_eq(bg_of(a->frame), hex_rgba("#fff5f8")),
              "预览 A(sakura)色值命中");
        CHECK(rgba_eq(color_of(b->num0), hex_rgba("#102a32")) &&
                  rgba_eq(bg_of(b->frame), hex_rgba("#101d29")),
              "预览 B(cyber)色值命中,未被 A 染色");
        CHECK(!rgba_eq(bg_of(a->frame), bg_of(b->frame)) &&
                  rgba_eq(color_of(cw.word[1]), hex_rgba("#e9f8ff")),
              "预览不透染候选窗,候选窗仍是 cyber");
        gtk_widget_destroy(hold);
        preview_free(a);
        preview_free(b);
    }

    /* 6. system 皮肤定时跟随:翻 gtk-application-prefer-dark-theme,
     * theme_timer(5s)到点后候选窗换色;显式皮肤则不受影响 */
    {
        GtkSettings *gs = gtk_settings_get_default();
        g_object_set(gs, "gtk-application-prefer-dark-theme", FALSE, NULL);
        lyy_candwin_set_skin(&cw, "system");
        pump(300);
        CHECK(rgba_eq(color_of(cw.word[1]), hex_rgba("#242424")),
              "system 亮模式:候选词 #242424");
        g_object_set(gs, "gtk-application-prefer-dark-theme", TRUE, NULL);
        pump(6300); /* theme_timer 周期 5s,留余量等到点 */
        CHECK(rgba_eq(color_of(cw.word[1]), hex_rgba("#f2f2f2")),
              "系统转暗 ≥5s 后候选窗跟随(#f2f2f2)");
        g_object_set(gs, "gtk-application-prefer-dark-theme", FALSE, NULL);
        pump(6300);
        CHECK(rgba_eq(color_of(cw.word[1]), hex_rgba("#242424")),
              "系统转亮 ≥5s 后候选窗跟随(#242424)");

        lyy_candwin_set_skin(&cw, "cyber");
        pump(120);
        g_object_set(gs, "gtk-application-prefer-dark-theme", TRUE, NULL);
        pump(6300);
        CHECK(rgba_eq(color_of(cw.word[1]), hex_rgba("#e9f8ff")) &&
                  strcmp(cw.skin, "cyber") == 0,
              "显式皮肤(cyber)不受系统明暗切换影响");
        g_object_set(gs, "gtk-application-prefer-dark-theme", FALSE, NULL);
    }

    lyy_candwin_hide(&cw);
    gtk_widget_destroy(cw.win);
    xcb_disconnect(conn);

    printf("== 结果:%s(失败 %d 项)==\n", g_failed ? "有失败" : "全部通过",
           g_failed);
    return g_failed ? 1 : 0;
}
