/*
 * /AI 触发会话(Mode B;与 Mode A ibus 引擎 lyyime.py 的 AI 状态机行为一致,
 * 交互合同见 docs/ARCHITECTURE.md §11)
 *
 * 中文态输入 `/AI`(`a|A` `i|I` 大小写均可)+ 提示词,回车调用配置好的
 * OpenAI 兼容大模型并把回复上屏:
 *   - 触发三键被吞下;打歪(如 /x)时补发已吞按键后按普通路径处理,原行为不变;
 *   - 采集态字母仍进 core 组词(可写中文提示词),组词上屏结果进入提示词;
 *   - 空格/数字/标点无候选时作为原字符进提示词;回车=发送;退格删字符,
 *     删穿 /AI 前缀=取消;Esc 先清组词、空缓冲再按取消;
 *   - AI 调用由 python3 执行共享助手脚本 ibus-engine/engine/lyyime_ai.py
 *     (C 端不做 HTTP,遵守"不引第三方"红线),异步 spawn + 管道回读,
 *     完成后经 XIM commit 到当前焦点 IC;焦点已切走则丢弃并记日志。
 */
#ifndef LYY_AI_CAPTURE_H_
#define LYY_AI_CAPTURE_H_

#include <glib.h>
#include <xcb/xproto.h>

#include "imdkit.h" /* xcb_im_input_context_t(vendor,-I vendor/xcb-imdkit/src) */

typedef struct _App App;

/* 提示词字节上限(UTF-8;超限丢弃并提示一次) */
#define LYY_AI_PROMPT_MAX 6000

typedef enum {
    LYY_AI_IDLE = 0,    /* 未触发 */
    LYY_AI_SLASH,       /* 已吞 '/' */
    LYY_AI_SLASH_A,     /* 已吞 '/A' */
    LYY_AI_CAPTURE,     /* 提示词采集中 */
} LyyAiState;

typedef struct AiCapture {
    LyyAiState state;
    GString *prompt;                  /* 已录提示词 */
    char core_preedit[512];           /* core 组词码镜像(触发门控+展示) */
    int prompt_full_notified;
    guint notice_timer;               /* 提示自动清除 */
    guint kill_timer;                 /* 请求超时兜底 */
    guint child_watch;                /* g_child_watch_add 源 id */
    GPid busy_pid;                    /* 进行中的助手子进程;0=空闲 */
    guint out_watch, err_watch;       /* stdout/stderr GIO watch id */
    GIOChannel *out_ch, *err_ch;
    GString *out_buf, *err_buf;
    xcb_key_press_event_t pend_ev[2]; /* 已吞按键(打歪时补发) */
    int pend_n;
} AiCapture;

/* xim_server.c 提供:喂 core 一个键并按采集态语义应用效果流
 * (commit→提示词、pass→原字符进提示词不回放),返回 1=已消费 0=pass -1=异常 */
int lyy_ai_feed_core(App *app, xcb_key_press_event_t *ev, uint32_t sym,
                     int key, uint32_t chr);

/* xim_server.c 提供:把 UTF-8 文本 commit 到当前焦点 IC(无焦点返回 -1) */
int lyy_xim_commit_utf8(App *app, const char *utf8);

/* 解析 AI 助手脚本路径(env LYYIME_AI_HELPER → 安装位 → 源码树);未找到返回 NULL */
const char *lyy_ai_find_helper(void);

void lyy_ai_init(AiCapture *ai);
void lyy_ai_clear(AiCapture *ai); /* 进程退出时释放资源 */

/* 功能门控:托盘启用 + core 就绪(调用方保证)+ [ai] 配齐 + 中文态 */
int lyy_ai_enabled(const App *app);
/* 是否处于采集态(apply_effects 语义切换用) */
int lyy_ai_capturing(const App *app);

/* 按键入口(handle_key_event 在 Shift 处理后调用;仅 press)。
 * 返回 0=与本模块无关(走普通路径) 1=已消费 2=已处理但须放行该键 */
int lyy_ai_take(App *app, xcb_key_press_event_t *ev, uint32_t keysym);

/* 采集态效果流接入(apply_effects 调用) */
void lyy_ai_on_commit(App *app, const char *text);   /* 组词上屏→提示词 */
void lyy_ai_on_pass_key(App *app, uint32_t keysym);  /* core 放行→原字符进提示词 */
void lyy_ai_mirror_preedit(App *app, const char *text); /* 总是镜像组词码 */
void lyy_ai_show_preedit(App *app); /* 采集态预编辑刷新(commit/preedit 后) */

/* 焦点切换/IC 销毁/触发态切换:放弃会话并清展示(不打扰进行中的请求) */
void lyy_ai_reset(App *app);

#endif /* LYY_AI_CAPTURE_H_ */
