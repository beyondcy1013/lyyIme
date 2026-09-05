/*
 * 候选窗:GTK3 override-redirect 无边框窗口(架构合同 §8)
 *
 * - GTK_WINDOW_POPUP 自带 override-redirect;accept_focus(FALSE) 保证永不抢
 *   应用焦点;type_hint 设为 POPUP_MENU/TOOLTIP 让 WM(若有)正确对待;
 * - root-window style 下客户端不报光标位置,跟随光标用 xcb_query_pointer
 *   轮询(docs/RESEARCH.md §2.3:XQueryPointer 是常见 IME 做法;此处用纯 xcb
 *   同语义接口,避免引入 Xlib 链接);
 * - 渲染:输入串 + 带序号候选 + 编码注释 + 翻页指示;首选高亮、贴边阴影,
 *   样式走 GtkCssProvider(xim/res/candidate.css),对齐主流输入法习惯
 *   (借鉴搜狗/万能五笔候选窗布局,AGENTS.MD "UI 精致"红线)。
 */
#ifndef LYY_CANDIDATE_WINDOW_H_
#define LYY_CANDIDATE_WINDOW_H_

#include <gtk/gtk.h>
#include <xcb/xcb.h>

#define LYY_MAX_ROWS 9

typedef struct CandidateWindow {
    GtkWidget *win;
    GtkWidget *frame;      /* .lyy-frame 圆角+阴影容器 */
    GtkWidget *preedit;    /* 输入串(编码) */
    GtkWidget *page;       /* 翻页指示 1/3 */
    GtkWidget *rows[LYY_MAX_ROWS];    /* 每行 GtkBox */
    GtkWidget *num[LYY_MAX_ROWS];     /* 序号 */
    GtkWidget *word[LYY_MAX_ROWS];    /* 候选词 */
    GtkWidget *comment[LYY_MAX_ROWS]; /* 注释 */
    int row_count;
    guint pos_timer;
    xcb_connection_t *conn;
    xcb_window_t root;
    int font_size;
    char css_dir[1024];    /* candidate.css 所在目录(空=未找到) */
} CandidateWindow;

/* 初始化并构建窗口(隐藏状态);css_dir 给出 res 目录,找不到样式用内置兜底 */
void lyy_candwin_init(CandidateWindow *cw, xcb_connection_t *conn,
                      xcb_window_t root, const char *css_dir, int font_size);

/* 重新应用字体大小(设置保存后调用,即时生效) */
void lyy_candwin_set_font_size(CandidateWindow *cw, int font_size);

/* 清空行并逐行填充;first=0 表示高亮第 0 行 */
void lyy_candwin_begin_rows(CandidateWindow *cw);
void lyy_candwin_add_row(CandidateWindow *cw, int idx, const char *text,
                         const char *comment);
void lyy_candwin_set_preedit(CandidateWindow *cw, const char *text);
void lyy_candwin_set_page(CandidateWindow *cw, int page, int pages); /* 1 基 */

/* 空内容则隐藏,否则显示并跟随光标 */
void lyy_candwin_commit_layout(CandidateWindow *cw);

void lyy_candwin_hide(CandidateWindow *cw);
int lyy_candwin_is_visible(const CandidateWindow *cw);

#endif /* LYY_CANDIDATE_WINDOW_H_ */
