/*
 * 候选窗皮肤注册表 + CSS 生成器(Mode B 自绘候选窗专用)
 *
 * - 单一注册表供设置画廊与真实候选窗共用:同一份调色板/CSS 模板,
 *   预览所见即所得;
 * - lyy_skin_css 生成皮肤层 CSS(颜色/圆角/字号),候选窗把它排在
 *   candidate.css 布局基准之后同优先级覆盖(文件只管布局,颜色归皮肤);
 * - lyy_skin_apply_tree 把 provider 逐 widget 挂到指定子树的 style
 *   context(APPLICATION 优先级),绝不走 screen 级注入——设置窗预览
 *   之间、预览与候选窗之间互不透染;
 * - 「跟随系统」特殊项:浅色为默认注册表调色板,dark=1 时解析到暗色
 *   覆盖表;显式皮肤忽略 dark。
 */
#ifndef LYY_SKIN_H_
#define LYY_SKIN_H_

#include <gtk/gtk.h>

typedef struct {
    const char *id;          /* 稳定 id(config.toml skin 值) */
    const char *name;        /* 中文展示名 */
    const char *category;    /* 分类标签(经典/商务/可爱…) */
    const char *description; /* 一句话描述 */
    const char *bg;          /* 底板起始色 #rrggbb */
    const char *bg_end;      /* 渐变结束色(135° 渐变) */
    const char *fg;          /* 前景(输入串/候选词) */
    const char *muted;       /* 弱色(序号/注释/翻页) */
    const char *border;      /* 描边色 */
    const char *accent;      /* 首选行底色 */
    const char *selected_fg; /* 首选行内文字色 */
    int radius;              /* 外框圆角 px */
} LyySkin;

/* 注册表条目数(含 system) */
int lyy_skin_count(void);

/* 取下标;越界返回 system(下标 0) */
const LyySkin *lyy_skin_at(int index);

/* 按 id 查找;NULL/空串/未知 id 均返回 system */
const LyySkin *lyy_skin_find(const char *id);

/* GTK 当前有效主题是否深色(prefer-dark 建议值 + 探测窗前景亮度兜底;
 * 语义即原 candidate_window.c theme_is_dark,迁此供皮肤/定时器共用) */
gboolean lyy_skin_system_is_dark(void);

/* 生成皮肤层 CSS(g_strdup_printf 系,g_free 释放);
 * id 经注册表归一(未知按 system);font_size 钳制 10..28;
 * dark 仅对 system 生效(显式皮肤忽略) */
char *lyy_skin_css(const char *id, gboolean dark, int font_size);

/* 把 provider 挂到 root 整棵 widget 子树的每个 style context
 * (APPLICATION 优先级);screen 级一律不用 */
void lyy_skin_apply_tree(GtkWidget *root, GtkCssProvider *provider);

#endif /* LYY_SKIN_H_ */
