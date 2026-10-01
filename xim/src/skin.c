/*
 * 候选窗皮肤注册表与 CSS 生成(见 skin.h 头注释)。
 * 调色板为静态表:新增皮肤只在表尾追加条目,候选窗与设置画廊自动收录。
 */
#include "skin.h"

#include <string.h>

/* 注册表:首项必须 system(回退锚点);调色板取色即所见,
 * 预览与真实候选窗共用同一份 CSS 模板 */
static const LyySkin g_skins[] = {
    { "system", "跟随系统", "经典", "随桌面自动切换明暗",
      "#fafafa", "#fafafa", "#242424", "#606060", "#b0b0b0", "#2463b4",
      "#ffffff", 8 },
    { "business-navy", "深海蓝金", "商务", "沉静藏蓝与香槟金,专注每一次表达",
      "#162338", "#23334b", "#f2f5fa", "#b7c6db", "#53657d", "#e4c48a",
      "#182438", 8 },
    { "porcelain", "云瓷简白", "商务", "细腻瓷白与雾蓝,干净从容",
      "#ffffff", "#edf3fa", "#20334e", "#52657b", "#bdcddd", "#315a88",
      "#ffffff", 10 },
    { "sakura", "樱花奶糖", "可爱", "奶油粉与莓果红,给日常一点甜",
      "#fff5f8", "#fce4ef", "#5c2943", "#885269", "#e8b5cb", "#ad3f70",
      "#ffffff", 16 },
    { "peach", "蜜桃软糖", "可爱", "柔软杏桃色,温暖又轻盈",
      "#fff8ed", "#ffe9d6", "#633823", "#86583e", "#e7bd9e", "#a94724",
      "#ffffff", 16 },
    { "bamboo", "青岚竹影", "自然", "清透青绿,让灵感自在呼吸",
      "#eff9f4", "#dceee5", "#213f33", "#4c6d5e", "#a6cbbb", "#28664e",
      "#ffffff", 12 },
    { "lavender", "暮紫星河", "梦幻", "低饱和暮紫,收藏夜晚的灵感",
      "#27223e", "#383052", "#f4efff", "#c7bbdf", "#6b5d8b", "#ccb6f5",
      "#302143", 14 },
    { "cyber", "霓虹夜航", "科技", "深空墨色与电光青,清晰利落",
      "#101d29", "#172f3b", "#e9f8ff", "#a0bdcc", "#3e6c80", "#69dfdf",
      "#102a32", 6 },
    { "contrast", "曜石明晰", "高对比", "纯粹黑白与亮黄,轻松辨认候选",
      "#101010", "#101010", "#ffffff", "#e0e0e0", "#eeeeee", "#ffe083",
      "#111111", 4 },
};

/* system 暗色覆盖表(仅 dark=1 且 id=system 时替代浅色字段) */
static const struct {
    const char *bg, *bg_end, *fg, *muted, *border, *accent, *selected_fg;
} g_system_dark = { "#202020", "#202020", "#f2f2f2", "#b8b8b8",
                    "#626262", "#2463b4", "#ffffff" };

int lyy_skin_count(void)
{
    return (int)(sizeof(g_skins) / sizeof(g_skins[0]));
}

const LyySkin *lyy_skin_at(int index)
{
    if (index < 0 || index >= lyy_skin_count())
        return &g_skins[0];
    return &g_skins[index];
}

const LyySkin *lyy_skin_find(const char *id)
{
    if (id && id[0]) {
        for (int i = 0; i < lyy_skin_count(); i++)
            if (!strcmp(id, g_skins[i].id))
                return &g_skins[i];
    }
    return &g_skins[0];
}

/* 检测 GTK 当前有效主题是否为深色。GTK 深色主题的前景色是亮色,因此用
 * 前景色亮度兜底;gtk-application-prefer-dark-theme 只是建议值,部分 XFCE
 * 主题/设置不导出或不变更,不能只依赖它。(自 candidate_window.c 迁来) */
gboolean lyy_skin_system_is_dark(void)
{
    GtkSettings *settings = gtk_settings_get_default();
    gboolean preferred = FALSE;
    if (settings)
        g_object_get(settings,
                     "gtk-application-prefer-dark-theme", &preferred,
                     NULL);

    GtkWidget *probe = gtk_window_new(GTK_WINDOW_POPUP);
    if (!probe)
        return preferred; /* GTK 未初始化等异常:仅凭建议值 */
    GtkStyleContext *ctx = gtk_widget_get_style_context(probe);
    GdkRGBA color;
    gtk_style_context_get_color(ctx, GTK_STATE_FLAG_NORMAL, &color);
    gtk_widget_destroy(probe);
    double luminance = 0.2126 * color.red + 0.7152 * color.green +
                       0.0722 * color.blue;
    return preferred || luminance > 0.55;
}

/* 皮肤层 CSS 模板:颜色/圆角/字号全在此;candidate.css 只作布局基准
 * (同名选择器同优先级,本层排在其后覆盖)。GTK3 CSS 支持
 * linear-gradient/alpha()/box-shadow,均为已验证子集 */
char *lyy_skin_css(const char *id, gboolean dark, int font_size)
{
    const LyySkin *s = lyy_skin_find(id);
    const char *bg = s->bg, *bg_end = s->bg_end, *fg = s->fg,
               *muted = s->muted, *border = s->border,
               *accent = s->accent, *selfg = s->selected_fg;
    if (s == &g_skins[0] && dark) {
        bg = g_system_dark.bg;
        bg_end = g_system_dark.bg_end;
        fg = g_system_dark.fg;
        muted = g_system_dark.muted;
        border = g_system_dark.border;
        accent = g_system_dark.accent;
        selfg = g_system_dark.selected_fg;
    }
    if (font_size < 10)
        font_size = 10;
    if (font_size > 28)
        font_size = 28;
    int row_radius = s->radius - 5;
    if (row_radius < 3)
        row_radius = 3;

    return g_strdup_printf(
        ".lyy-outer { background: transparent; }\n"
        ".lyy-frame { background-color: %s;"
        " background-image: linear-gradient(135deg, %s, %s);"
        " color: %s; border: 1px solid %s; border-radius: %dpx;"
        " padding: 8px 12px; box-shadow: 0 3px 10px alpha(%s, 0.20); }\n"
        ".lyy-header { margin-bottom: 5px; }\n"
        ".lyy-row { padding: 3px 6px; border-radius: %dpx; }\n"
        ".lyy-preedit,.lyy-word { color: %s; font-size: %dpx; }\n"
        ".lyy-num,.lyy-comment,.lyy-page { color: %s; font-size: %dpx; }\n"
        ".lyy-first { background-color: %s; background-image: none; }\n"
        ".lyy-first .lyy-num,.lyy-first .lyy-word,.lyy-first .lyy-comment {"
        " color: %s; font-weight: bold; }\n",
        bg, bg, bg_end, fg, border, s->radius, border, row_radius,
        fg, font_size, muted, font_size - 2, accent, selfg);
}

static void apply_tree_cb(GtkWidget *w, gpointer user_data)
{
    GtkStyleContext *ctx = gtk_widget_get_style_context(w);
    gtk_style_context_add_provider(ctx, GTK_STYLE_PROVIDER(user_data),
                                   GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    if (GTK_IS_CONTAINER(w))
        gtk_container_forall(GTK_CONTAINER(w), apply_tree_cb, user_data);
}

void lyy_skin_apply_tree(GtkWidget *root, GtkCssProvider *provider)
{
    if (!root || !provider)
        return;
    apply_tree_cb(root, provider);
}
