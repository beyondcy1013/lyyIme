/*
 * lyyIme Mode B 公共定义(app 上下文)
 *
 * 模块划分(docs/ARCHITECTURE.md §8 合同):
 *   main.c            入口/单实例/信号/主循环
 *   xim_server.c      xcb-imdkit 封装(trigger 键=两个 Shift,root-window style)
 *   core_ffi.c        dlopen liblyyime_core.so(FFI 合同 §3 全符号)
 *   effects_json.c    §3 JSON 效果流迷你解析器(固定 schema)
 *   keysym_map.c      keysym → LKey 映射(§6 按键行为)
 *   ai_capture.c      /AI 触发会话(采集提示词,子进程调 lyyime_ai.py 上屏)
 *   candidate_window.c GTK3 override-redirect 候选窗
 *   tray.c            托盘与工具菜单
 *   settings.c        设置对话框(GtkBuilder)
 *   config.c          ~/.config/lyyime/config.toml 平面键值 TOML 子集读写
 */
#ifndef LYY_COMMON_H_
#define LYY_COMMON_H_

#include "ai_capture.h"
#include "candidate_window.h"
#include "config.h"
#include "core_ffi.h"
#include "log.h"
#include "settings.h"
#include "tray.h"
#include "xim_server.h"

#include <limits.h>

#define LYY_APP_NAME "lyyime-xim"
#define LYY_APP_VERSION "0.3.0"
#define LYY_XIM_SERVER_NAME "lyyime"

/* 前向声明:xim_server.h 等头文件引用 App 指针,定义在文件尾 */
typedef struct _App App;

/* 全局单例:GLib 回调没有 user_data 传递链时使用(单线程,安全) */
App *lyy_app(void);

/* core 引擎是否可用(启用 + 未降级 + FFI 就绪 + 实例存在) */
int lyy_core_ready(App *app);

/* core FFI 初始化/重建引擎/释放;degraded 时 engine 为 NULL,一切按键直通 */
int lyy_engine_ensure(App *app);      /* 打开(或重建)core 引擎,返回 0/‑1 */
void lyy_engine_reload(App *app);     /* 工具菜单"重载词库"/设置保存后调用 */

/* 崩溃安全:任何 core 异常都只记日志并降级直通,绝不让 XIM server 带病上屏 */
void lyy_core_mark_degraded(App *app, const char *why);

/* 请求显示设置对话框(可从任意 GLib 回调/信号安全路径调用) */
void lyy_request_show_settings(App *app);

/* 安装 SIGUSR1(唤起/设置)/SIGTERM/SIGINT(优雅退出)处理器 */
void lyy_install_signal_handlers(void);

struct _App {
    LyyLog log;                    /* ~/.local/share/lyyime/logs/xim.log */
    LyyConfig config;
    char config_path[PATH_MAX];
    char data_dir[PATH_MAX];       /* ~/.local/share/lyyime(词典/用户词) */
    char log_dir[PATH_MAX];
    CoreFfi core;                  /* dlopen 符号表;loaded=0 时全降级 */
    void *engine;                  /* lyyime_new 句柄;NULL=降级直通 */
    int degraded;                  /* 1=core 缺失/异常,已降级 */
    int enabled;                   /* 托盘"启用/停用"总开关,1=启用 */
    XimServer xim;
    CandidateWindow candwin;
    Tray tray;
    SettingsUi settings;
    AiCapture ai;                  /* /AI 触发会话(见 ai_capture.h) */
    GMainLoop *loop;               /* GLib 主循环 */
    guint shift_timer_id;          /* Shift 单击判定时间窗(280ms) */
    int shift_pending;             /* Shift 已按下待判定(与焦点 IC 同步) */
    guint notice_timer_id;         /* notice 提示自动清除定时器(4s) */
    struct {                       /* 造词热键(合同 §12,coin_hotkey 解析结果) */
        uint32_t mods;             /* XCB_MOD_MASK_* 组合 */
        uint32_t sym;              /* 目标 keysym */
        int ok;                    /* 1=解析成功(否则不拦截) */
    } hotkey_coin;
    int settings_requested;        /* SIGUSR1 唤起 → 主循环里弹设置窗 */
    int quit_requested;            /* SIGTERM/SIGINT → 优雅退出 */
};

#endif /* LYY_COMMON_H_ */
