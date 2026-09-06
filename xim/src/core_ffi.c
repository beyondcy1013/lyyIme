#include "core_ffi.h"

#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void *resolve(void *handle, const char *name, const char **missing)
{
    void *sym = dlsym(handle, name);
    if (!sym && !*missing)
        *missing = name;
    return sym;
}

int lyy_core_ffi_load(CoreFfi *ffi)
{
    memset(ffi, 0, sizeof(*ffi));

    /* 搜索路径:LYYIME_CORE_LIB 优先(集成期主控注入真库),再常规安装位 */
    const char *env = getenv("LYYIME_CORE_LIB");
    const char *home = getenv("HOME");
    char cands[4][1024];
    int ncand = 0;
    if (env && env[0]) {
        /* 同 HOME:手工截断,避免超长路径 -Wformat-truncation 告警 */
        char eb[1024];
        size_t el = strlen(env);
        if (el >= sizeof(eb))
            el = sizeof(eb) - 1;
        memcpy(eb, env, el);
        eb[el] = '\0';
        snprintf(cands[ncand++], sizeof(cands[0]), "%s", eb);
    }
    if (home && home[0]) {
        /* HOME 手工截断拼接,避免超长路径(>1024)在 -Wformat-truncation 下告警 */
        char hb[512];
        size_t hl = strlen(home);
        if (hl >= sizeof(hb))
            hl = sizeof(hb) - 1;
        memcpy(hb, home, hl);
        hb[hl] = '\0';
        snprintf(cands[ncand++], sizeof(cands[0]),
                 "%s/.local/lib/lyyime/liblyyime_core.so", hb);
    }
    snprintf(cands[ncand++], sizeof(cands[0]), "/usr/local/lib/lyyime/liblyyime_core.so");
    snprintf(cands[ncand++], sizeof(cands[0]), "/usr/local/lib/liblyyime_core.so");

    void *handle = NULL;
    for (int i = 0; i < ncand; i++) {
        handle = dlopen(cands[i], RTLD_NOW | RTLD_LOCAL);
        if (handle) {
            /* 记录实际加载路径(手工拷贝,路径长度受数组钳制) */
            size_t cl = strlen(cands[i]);
            if (cl >= sizeof(ffi->lib_path))
                cl = sizeof(ffi->lib_path) - 1;
            memcpy(ffi->lib_path, cands[i], cl);
            ffi->lib_path[cl] = '\0';
            break;
        }
    }
    if (!handle)
        return -1;

    const char *missing = NULL;
    ffi->lyyime_new = resolve(handle, "lyyime_new", &missing);
    ffi->lyyime_free = resolve(handle, "lyyime_free", &missing);
    ffi->lyyime_reset = resolve(handle, "lyyime_reset", &missing);
    ffi->lyyime_mode = resolve(handle, "lyyime_mode", &missing);
    ffi->lyyime_toggle_mode = resolve(handle, "lyyime_toggle_mode", &missing);
    ffi->lyyime_set_commit_after_four =
        resolve(handle, "lyyime_set_commit_after_four", &missing);
    ffi->lyyime_set_commit_unique_four =
        resolve(handle, "lyyime_set_commit_unique_four", &missing);
    ffi->lyyime_set_phrase_hint =
        resolve(handle, "lyyime_set_phrase_hint", &missing);
    ffi->lyyime_process_key = resolve(handle, "lyyime_process_key", &missing);
    ffi->lyyime_cand = resolve(handle, "lyyime_cand", &missing);
    ffi->lyyime_cand_comment = resolve(handle, "lyyime_cand_comment", &missing);

    if (missing) {
        fprintf(stderr, "liblyyime_core.so 符号缺失:%s(库:%s)\n", missing,
                ffi->lib_path);
        dlclose(handle);
        return -1;
    }
    ffi->loaded = 1;

    /* §14 快速功能键为可选符号组(2026-09 新增):旧 core 库缺任一符号时
     * 只禁用该功能(qa_ok=0),不整体降级 —— 打字主链路照常。 */
    ffi->lyyime_set_quick_actions_enabled =
        dlsym(handle, "lyyime_set_quick_actions_enabled");
    ffi->lyyime_clear_quick_actions =
        dlsym(handle, "lyyime_clear_quick_actions");
    ffi->lyyime_add_quick_action = dlsym(handle, "lyyime_add_quick_action");
    ffi->lyyime_action_command = dlsym(handle, "lyyime_action_command");
    ffi->lyyime_select_candidate = dlsym(handle, "lyyime_select_candidate");
    ffi->qa_ok = ffi->lyyime_set_quick_actions_enabled &&
                 ffi->lyyime_clear_quick_actions &&
                 ffi->lyyime_add_quick_action && ffi->lyyime_action_command &&
                 ffi->lyyime_select_candidate;
    return 0;
}

int lyy_core_process_key_json(const CoreFfi *ffi, void *eng, int key_id,
                              uint32_t chr, char *out, int out_cap)
{
    if (!ffi->loaded || !eng || !out || out_cap <= 0)
        return -1;
    /* §3 返回值协议:正数=所需字节数(含\0);不足时返回 -needed 且不写入 */
    int64_t r = ffi->lyyime_process_key(eng, key_id, chr, out, out_cap);
    if (r >= 0)
        return 0;
    int64_t need = -r;
    if (need <= 0 || (int64_t)out_cap < need)
        return -1;
    r = ffi->lyyime_process_key(eng, key_id, chr, out, out_cap);
    return (r >= 0) ? 0 : -1;
}

/* §14 点选候选:语义同 process_key_json(两段式重试纪律) */
int lyy_core_select_candidate_json(const CoreFfi *ffi, void *eng, int idx,
                                   char *out, int out_cap)
{
    if (!ffi->loaded || !ffi->qa_ok || !eng || !out || out_cap <= 0)
        return -1;
    int64_t r = ffi->lyyime_select_candidate(eng, idx, out, out_cap);
    if (r >= 0)
        return 0;
    int64_t need = -r;
    if (need <= 0 || (int64_t)out_cap < need)
        return -1;
    r = ffi->lyyime_select_candidate(eng, idx, out, out_cap);
    return (r >= 0) ? 0 : -1;
}

/* §3:lyyime_cand/lyyime_cand_comment 返回候选文本,-needed 表示不足 */
static const char *cand_get(int (*fn)(void *, int, char *, int), void *eng,
                            int i, char *buf, int cap)
{
    int64_t r = fn(eng, i, buf, cap);
    if (r >= 0)
        return buf;
    int64_t need = -r;
    if (need <= 0 || cap < need)
        return NULL;
    return (fn(eng, i, buf, cap) >= 0) ? buf : NULL;
}

const char *lyy_core_cand_text(const CoreFfi *ffi, void *eng, int i,
                               char *buf, int cap)
{
    if (!ffi->loaded || !eng || !buf || cap <= 0)
        return NULL;
    return cand_get(ffi->lyyime_cand, eng, i, buf, cap);
}
