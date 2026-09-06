/*
 * 配置读写:~/.config/lyyime/config.toml(与 doctor/ibus 引擎/AI 客户端共用,
 * 见 docs/ARCHITECTURE.md §4/§11)。本模块实现"平面键值 + 单层 [ai] 段"的
 * TOML 子集:
 *   - 顶层 `key = value`(int / bool)+ `[ai]` 段内 `key = value`
 *     (bool / int / 带引号字符串);
 *   - **保留未知行、[section] 行、整行注释与被管理行的行尾注释**;
 *   - 写回时已知键原位更新,未出现的顶层键追加到文件尾;[ai] 段未出现的
 *     键插到 [ai] 头/最后一个 ai 键之后,整段缺失时新建 [ai] 头再追加;
 *   - 行尾注释统一写为 ` # 注释`(历史版本漏写 #,导致文件不是合法 TOML、
 *     python tomllib 解析失败 —— 本版起修正,旧行读取兼容、保存时归一)。
 * 子集约定借鉴 TOML v1.0.0 规范(https://toml.io);只解析自己管理的键,
 * 其余行原样字节保留,保证与并行开发的 lyyime-core/doctor 互不踩踏。
 */
#ifndef LYY_CONFIG_H_
#define LYY_CONFIG_H_

/* 字符串配置容量(字节,含 \0);system_prompt 用 1024 容纳中文长句 */
#define LYY_CFG_STR_BASE 256
#define LYY_CFG_STR_KEY 512
#define LYY_CFG_STR_MODEL 128
#define LYY_CFG_STR_PROMPT 1024

typedef struct {
    int page_size;           /* 候选数(1..9),对齐 core Config */
    int mixed_english;       /* 中英混合(无中文候选时给英文词) */
    int auto_commit_english; /* 高置信英文词标点/空格自动直通 */
    int chinese_punct;       /* 中文态使用中文标点 */
    int learning;            /* 用户词学习开关 */
    int commit_after_four;   /* 满足四码后,继续输入字母先顶屏当前选中 */
    int font_size;           /* 候选窗字体大小(10..28) */
    int autostart;           /* 开机自启(写 ~/.config/autostart) */
    /* AI 助手([ai] 段;触发/调用实现见 ibus-engine/engine/lyyime_ai.py,
     * Mode A 直接 import,Mode B 以子进程调用同一脚本,配置一份两处生效) */
    int ai_enabled;                                  /* 功能总开关 */
    char ai_api_base[LYY_CFG_STR_BASE];              /* OpenAI 兼容 base_url */
    char ai_api_key[LYY_CFG_STR_KEY];                /* API 密钥(本地服务可空) */
    char ai_model[LYY_CFG_STR_MODEL];                /* 模型名 */
    char ai_system_prompt[LYY_CFG_STR_PROMPT];       /* 系统提示(可空) */
    int ai_timeout;                                  /* 请求超时秒数(5..300) */
    /* 造词热键(合同 §12):写法 "ctrl+equal",修饰至少一个;
     * 解析见 keysym_map.c lyy_hotkey_parse(与 Mode A 同规格) */
    char coin_hotkey[LYY_CFG_STR_BASE];
} LyyConfig;

void lyy_config_defaults(LyyConfig *c);

/* 返回 0=读到 1=文件不存在已用默认 -1=读失败已用默认 */
int lyy_config_load(const char *path, LyyConfig *out);

/* 保存;返回 0 成功。保留未知行与注释(见文件头) */
int lyy_config_save(const char *path, const LyyConfig *c);

/* AI 功能是否已配齐可用(启用 + api_base + model;key 本地服务可空) */
int lyy_config_ai_active(const LyyConfig *c);

/* 按配置落/删开机自启项 ~/.config/autostart/lyyime-xim.desktop */
int lyy_config_apply_autostart(int enable);

#endif /* LYY_CONFIG_H_ */
