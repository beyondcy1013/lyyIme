/*
 * core FFI:dlopen liblyyime_core.so,对接 docs/ARCHITECTURE.md §3 C ABI(全符号)。
 *
 * - 路径解析顺序:环境变量 LYYIME_CORE_LIB(集成期主控注入真库用)→
 *   ~/.local/lib/lyyime/ → /usr/local/lib/lyyime/ → /usr/local/lib/;
 * - 任一符号缺失视为加载失败,日志给出"人话"修复指引;
 * - 加载失败不退出:lyyime-xim 进入降级直通模式(红线:core 异常不影响应用收键)。
 * 线程约定:§3 规定全部函数线程不安全、宿主保证单线程调用 —— 本进程所有
 * FFI 调用都发生在 GLib 主循环线程内。
 */
#ifndef LYY_CORE_FFI_H_
#define LYY_CORE_FFI_H_

#include <stdint.h>

/* §3 key_id 枚举(python 侧同样常量) */
enum {
    LKEY_CHAR = 0,   /* 小写字母 a–z,宿主负责小写化(chr=码点) */
    LKEY_DIGIT = 1,  /* '0'..'9'(chr=数字字符;0 = 选第 10 个候选) */
    LKEY_SPACE = 2,
    LKEY_ENTER = 3,
    LKEY_BACKSPACE = 4,
    LKEY_ESC = 5,
    LKEY_PAGEUP = 6,
    LKEY_PAGEDOWN = 7,
    LKEY_PUNCT = 8,  /* 标点原字符(chr=半角字符) */
    LKEY_SHIFTPRESS = 9,
    LKEY_OTHER = 10,
    /* 造词(合同 §12):热键与方向键 */
    LKEY_COIN = 11,  /* 造词热键(默认 Ctrl+=,coin_hotkey 可配置) */
    LKEY_LEFT = 12,  /* 方向键 ←:造词少选一字(非造词模式同 OTHER) */
    LKEY_RIGHT = 13, /* 方向键 →:造词多选一字 */
    LKEY_UP = 14,    /* 方向键 ↑:造词多选一字 */
    LKEY_DOWN = 15,  /* 方向键 ↓:造词少选一字 */
};

typedef struct CoreFfi {
    int loaded; /* 1 = 全符号就绪 */
    char lib_path[1024];
    /* §14 快速功能键符号组:1 = 五个符号齐备(旧 core 库可能缺失,
     * 缺失仅禁用该功能,不影响其余符号加载与打字主链路) */
    int qa_ok;
    /* 英文上屏去向符号组(§6,2026-09-24):1 = 两符号齐备;旧 core 库
     * 缺失时保持 core 各自默认(回车临时 / Shift 切英文),不整体降级 */
    int en_mode_ok;
    /* 精确单字按词频排位符号(§5,2026-09-24):1 = 符号在;旧 core 库
     * 缺失时沿用 core 默认(开),不整体降级 */
    int freq_rank_ok;
    /* 1 = 支持"四码首选上屏"可选符号(lyyime_set_commit_first_at_four) */
    int first_four_ok;
    /* §15 候选右键菜单符号组(2026-09-29):1 = 两符号齐备;
     * 旧 core 库缺失时右键行静默无菜单,打字/点选主链路照常 */
    int cand_ops_ok;
    /* §3 原型,逐字对应 */
    void *(*lyyime_new)(const char *data_dir);                       /* NULL=失败 */
    void (*lyyime_free)(void *eng);
    void (*lyyime_reset)(void *eng);
    int (*lyyime_mode)(void *eng);                    /* 0=中文 1=英文 */
    int (*lyyime_toggle_mode)(void *eng);             /* 返回新 mode */
    int (*lyyime_set_commit_after_four)(void *eng, int enabled); /* 返回生效值 */
    int (*lyyime_set_commit_unique_four)(void *eng, int enabled); /* 返回生效值 */
    /* 四码首选上屏(§6,可选符号):0 = 关,非 0 = 开(满四码且首选是
     * 五笔命中时直接上屏首选,有重码也上屏第一个) */
    int (*lyyime_set_commit_first_at_four)(void *eng, int enabled); /* 返回生效值 */
    int (*lyyime_set_phrase_hint)(void *eng, int enabled);        /* 返回生效值 */
    int64_t (*lyyime_process_key)(void *eng, int key_id, uint32_t chr,
                                  char *buf, int64_t buf_cap);
    int (*lyyime_cand)(void *eng, int i, char *buf, int cap);
    int (*lyyime_cand_comment)(void *eng, int i, char *buf, int cap);
    /* 英文上屏去向(§6,可选符号组):0 = temp 临时,非 0 = en 切英文模式 */
    int (*lyyime_set_enter_english)(void *eng, int en_mode); /* 返回生效 0/1 */
    int (*lyyime_set_shift_english)(void *eng, int en_mode); /* 返回生效 0/1 */
    /* 精确单字按词频排位(§5,可选符号):0 = 关(恒居首位),非 0 = 开 */
    int (*lyyime_set_exact_char_freq_rank)(void *eng, int enabled); /* 返回生效 0/1 */
    /* §14 快速功能键(可选符号组) */
    int (*lyyime_set_quick_actions_enabled)(void *eng, int enabled); /* 返回生效值 */
    void (*lyyime_clear_quick_actions)(void *eng);
    int (*lyyime_add_quick_action)(void *eng, const char *trigger,
                                   const char *label, const char *command);
    int (*lyyime_action_command)(void *eng, int i, char *buf, int cap);
    int64_t (*lyyime_select_candidate)(void *eng, int idx, char *buf,
                                       int64_t buf_cap);
    /* §15 候选右键操作(可选符号组):
     * cand_pinned → -1 行不支持菜单 / 0 未固定 / 1 已固定;
     * cand_op → 同 process_key 的效果流 JSON 两段式协议,op∈{1,2,3} */
    int (*lyyime_cand_pinned)(void *eng, int idx);
    int64_t (*lyyime_cand_op)(void *eng, int idx, int op, char *buf,
                              int64_t buf_cap);
} CoreFfi;

/*
 * 加载并解析全部 §3 符号。成功返回 0;
 * 失败返回 -1 并由实现方写日志(含 LYYIME_CORE_LIB 用法与安装指引)。
 * §14 快速功能键为可选符号组(qa_ok),缺失不视为加载失败。
 */
int lyy_core_ffi_load(CoreFfi *ffi);

/* 喂键便捷封装:自动处理"两段式返回 -needed"协议;返回的 JSON 写入调用方 buf */
int lyy_core_process_key_json(const CoreFfi *ffi, void *eng, int key_id,
                              uint32_t chr, char *out, int out_cap);

/* 点选候选便捷封装(§14 鼠标点选;语义同 process_key_json):返回 0 成功 */
int lyy_core_select_candidate_json(const CoreFfi *ffi, void *eng, int idx,
                                   char *out, int out_cap);

/* 候选右键操作便捷封装(§15;语义同 process_key_json):返回 0 成功;
 * 要求 cand_ops_ok,否则返回 -1(宿主据此禁用菜单) */
int lyy_core_cand_op_json(const CoreFfi *ffi, void *eng, int idx, int op,
                          char *out, int out_cap);

/* 取第 i 个候选文本/注释;-needed 自动重试;失败返回 NULL */
const char *lyy_core_cand_text(const CoreFfi *ffi, void *eng, int i,
                               char *buf, int cap);

#endif /* LYY_CORE_FFI_H_ */
