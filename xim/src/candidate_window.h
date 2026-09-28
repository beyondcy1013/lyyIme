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

/* 行点击回调(§14 鼠标点选):idx=行下标(0 基),GTK 主线程内调用;
 * 由宿主(xim_server)注入,点选走 core select_candidate(功能键/普通候选同路径) */
typedef void (*LyyCandwinClickFn)(int idx, void *user_data);

/* 候选右键操作码(§15 右键菜单),1-3 与 core CandOp/FFI lyyime_cand_op
 * 对齐;LYY_CAND_OP_QUERY 为宿主侧动作(候选窗内直接处理:网址模板
 * 代入词 → xdg-open,不经 core/IC,菜单 grab 期间亦可执行) */
enum {
    LYY_CAND_OP_PIN = 1,      /* 固定首位/取消固定 */
    LYY_CAND_OP_DELETE = 2,   /* 删除词组(屏蔽+移出造词库) */
    LYY_CAND_OP_EN_LOOKUP = 3, /* 反查英文 */
    LYY_CAND_OP_QUERY = 4     /* 自定义查询(候选窗本地处理,不下发 op_fn) */
};

/* 右键菜单状态查询:idx=页内下标;返回 <0 不支持菜单(功能键/空行),
 * =0 未固定,=1 已固定(菜单项据此显示"固定首位/取消固定") */
typedef int (*LyyCandwinOpStateFn)(int idx, void *user_data);
/* 右键菜单项激活回调:op∈上面枚举;宿主转发 core cand_op 并按效果流刷新 */
typedef void (*LyyCandwinOpFn)(int idx, int op, void *user_data);

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
    guint theme_timer;  /* 低频复查 GTK 明暗主题变化 */
    GtkCssProvider *css_provider; /* 复用同一 provider,避免定时刷新累积 */
    xcb_connection_t *conn;
    xcb_window_t root;
    int font_size;
    char css_dir[1024];    /* candidate.css 所在目录(空=未找到) */
    LyyCandwinClickFn on_click;   /* 行点击回调(§14);NULL=不响应点击 */
    void *click_user_data;        /* 回调入参(宿主传 App*) */
    LyyCandwinOpStateFn op_state_fn; /* 右键菜单状态查询(§15);NULL=不建菜单 */
    LyyCandwinOpFn op_fn;            /* 右键菜单项激活回调 */
    void *op_user_data;              /* 右键回调入参(与 click 共用 App*) */
    gboolean hover;       /* 指针悬停在窗内 → 暂停跟随,行才可点/可右键 */
    gboolean menu_open;   /* 右键菜单打开期间强制冻结(菜单夺走指针) */
    /* §15 自定义查询(菜单第 4 项):url 模板含 {q} 占位符(词百分号编码
     * 代入);url 为空则菜单不显示此项;label 空显示「自定义查询」 */
    char query_label[128];
    char query_url[512];
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

/* 注入右键菜单回调(§15);NULL 组则禁用右键菜单 */
void lyy_candwin_set_op_fns(CandidateWindow *cw, LyyCandwinOpStateFn state_fn,
                            LyyCandwinOpFn op_fn, void *user_data);

/* 注入自定义查询配置(§15 菜单第 4 项):label/url 来自 config.toml
 * custom_query_label/custom_query_url;url 为空 → 菜单不显示该项。
 * 启动与设置保存后各调一次,即时生效。 */
void lyy_candwin_set_query(CandidateWindow *cw, const char *label,
                           const char *url);

/* 空内容则隐藏,否则显示并跟随光标 */
void lyy_candwin_commit_layout(CandidateWindow *cw);

void lyy_candwin_hide(CandidateWindow *cw);
int lyy_candwin_is_visible(const CandidateWindow *cw);

#endif /* LYY_CANDIDATE_WINDOW_H_ */
