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
    LKEY_DIGIT = 1,  /* '1'..'9'(chr=数字字符) */
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
    /* §3 原型,逐字对应 */
    void *(*lyyime_new)(const char *data_dir);                       /* NULL=失败 */
    void (*lyyime_free)(void *eng);
    void (*lyyime_reset)(void *eng);
    int (*lyyime_mode)(void *eng);                    /* 0=中文 1=英文 */
    int (*lyyime_toggle_mode)(void *eng);             /* 返回新 mode */
    int (*lyyime_set_commit_after_four)(void *eng, int enabled); /* 返回生效值 */
    int64_t (*lyyime_process_key)(void *eng, int key_id, uint32_t chr,
                                  char *buf, int64_t buf_cap);
    int (*lyyime_cand)(void *eng, int i, char *buf, int cap);
    int (*lyyime_cand_comment)(void *eng, int i, char *buf, int cap);
} CoreFfi;

/*
 * 加载并解析全部 §3 符号。成功返回 0;
 * 失败返回 -1 并由实现方写日志(含 LYYIME_CORE_LIB 用法与安装指引)。
 */
int lyy_core_ffi_load(CoreFfi *ffi);

/* 喂键便捷封装:自动处理"两段式返回 -needed"协议;返回的 JSON 写入调用方 buf */
int lyy_core_process_key_json(const CoreFfi *ffi, void *eng, int key_id,
                              uint32_t chr, char *out, int out_cap);

/* 取第 i 个候选文本/注释;-needed 自动重试;失败返回 NULL */
const char *lyy_core_cand_text(const CoreFfi *ffi, void *eng, int i,
                               char *buf, int cap);

#endif /* LYY_CORE_FFI_H_ */
