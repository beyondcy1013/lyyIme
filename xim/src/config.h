/*
 * 配置读写:~/.config/lyyime/config.toml(与 doctor/ibus 引擎/AI 客户端共用,
 * 见 docs/ARCHITECTURE.md §4/§11/§14)。本模块实现"平面键值 + 单层 [ai] 段 +
 * [[quick_actions]] 数组表"的 TOML 子集:
 *   - 顶层 `key = value`(int / bool)+ `[ai]` 段内 `key = value`
 *     (bool / int / 带引号字符串)+ `[[quick_actions]]` 数组表(trigger /
 *     label / command 带引号字符串);
 *   - **保留未知行、[section] 行、整行注释与被管理行的行尾注释**;
 *   - 写回时已知键原位更新,未出现的顶层键追加到文件尾;[ai] 段未出现的
 *     键插到 [ai] 头/最后一个 ai 键之后,整段缺失时新建 [ai] 头再追加;
 *     [[quick_actions]] 块原位重写(首个块位置),文件里没有则追加到文件尾;
 *   - 行尾注释统一写为 ` # 注释`(历史版本漏写 #,导致文件不是合法 TOML、
 *     python tomllib 解析失败 —— 本版起修正,旧行读取兼容、保存时归一)。
 * 子集约定借鉴 TOML v1.0.0 规范(https://toml.io);只解析自己管理的键,
 * 其余行原样字节保留,保证与并行开发的 lyyime-core/doctor 互不踩踏。
 */
#ifndef LYY_CONFIG_H_
#define LYY_CONFIG_H_

#include <stddef.h>

/* 字符串配置容量(字节,含 \0);system_prompt 用 1024 容纳中文长句 */
#define LYY_CFG_STR_BASE 256
#define LYY_CFG_STR_KEY 512
#define LYY_CFG_STR_MODEL 128
#define LYY_CFG_STR_PROMPT 1024
#define LYY_CFG_STR_TRIGGER 64
#define LYY_CFG_STR_LABEL 128
#define LYY_CFG_STR_CMD 512
#define LYY_CFG_STR_ENMODE 16

/* 快速功能键条目上限(合同 §14;对齐 core QUICK_ACTIONS_MAX) */
#define LYY_QA_MAX 8

/* 快速功能键(合同 §14):[[quick_actions]] 数组表;触发词整串命中时候选条
 * 追加功能候选,数字/点选后宿主执行 command(@settings/@help 内置或 shell) */
typedef struct {
    char trigger[LYY_CFG_STR_TRIGGER]; /* 小写字母 1–12 个 */
    char label[LYY_CFG_STR_LABEL];     /* 候选展示文本 */
    char command[LYY_CFG_STR_CMD];     /* @settings/@help 或 shell 命令 */
} LyyQuickAction;

typedef struct {
    int page_size;           /* 候选数(1..10;数字键 1-9/0,0=第 10 个),对齐 core Config */
    int mixed_english;       /* 中英混合(无中文候选时给英文词) */
    int auto_commit_english; /* 高置信英文词标点/空格自动直通 */
    int chinese_punct;       /* 中文态使用中文标点 */
    int learning;            /* 用户词学习开关 */
    int commit_after_four;   /* 满足四码后,继续输入字母先顶屏当前选中 */
    int commit_first_at_four;/* 恰好四码且首选是五笔命中时,免空格直接上屏首选(有重码也上屏第一个) */
    int commit_unique_four;  /* 恰好四码且候选唯一时,免空格直接上屏(first_at_four 开时被覆盖) */
    int phrase_hint;         /* 词组效率提示(上屏后提示更省键的词组与编码) */
    int exact_char_freq_rank; /* 精确单字按词频排位(默认开;关=恒居首位旧行为) */
    /* 英文上屏去向(§6,与 core Config::enter_english/shift_english 同名同值):
     * 回车/Shift 上屏英文原串后 "temp"=临时(仅上屏,保持中文模式)、
     * "en"=长久(上屏并切入英文模式)。取值经 lyy_en_mode_canon 归一。 */
    char enter_english[LYY_CFG_STR_ENMODE]; /* 默认 temp(单个英文词输入) */
    char shift_english[LYY_CFG_STR_ENMODE]; /* 默认 en(上屏即转英文) */
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
    /* 截屏快捷键(合同 §13):按下拉起 lyyime-shot 框选截屏(存图片目录 +
     * 剪贴板);写法同造词热键,默认 ctrl+alt+a */
    char shot_hotkey[LYY_CFG_STR_BASE];
    /* 快速功能键(合同 §14):总开关 + 触发词表;
     * 文件缺 [[quick_actions]] 块时使用内置默认表(peizhi/bangzhu),
     * 与 core Config::default 保持一致 */
    int quick_actions_enabled;               /* 功能总开关(默认开) */
    LyyQuickAction quick_actions[LYY_QA_MAX]; /* 条目表 */
    int quick_actions_count;                 /* 0..LYY_QA_MAX */
    /* 输入统计(全局数据,各输入模式共用;显示端=悬浮窗状态行)。
     * config.toml 顶层 stats_* 键,悬浮窗经文件监视即时生效 */
    int stats_enabled;           /* 停顿显示今日统计总开关(默认开) */
    int stats_pause_secs;        /* 停顿多少秒后显示(3..300,默认 10) */
    int stats_idle_exclude_secs; /* 计入速度的最长停顿(5..600,默认 30) */
    /* §15 候选右键·自定义查询(菜单第 4 项;宿主侧 xdg-open 打开,
     * 不动引擎状态):url 模板中 {q} 占位符替换为百分号编码的候选词,
     * url 为空 = 菜单不显示此项;label 为空 = 显示「自定义查询」 */
    char custom_query_label[LYY_CFG_STR_LABEL];
    char custom_query_url[LYY_CFG_STR_CMD];
} LyyConfig;

void lyy_config_defaults(LyyConfig *c);

/* 返回 0=读到 1=文件不存在已用默认 -1=读失败已用默认 */
int lyy_config_load(const char *path, LyyConfig *out);

/* 热键冲突自动升级(合同 §13;加载后/保存前调用,规格同 core hotkey.rs):
 * 造词与截屏快捷键占用同一组合时,截屏热键按 原组合→+Alt→+Alt+Shift
 * 逐级让位(工具键让位打字键)并就地改写 *c,人话说明写入 note。
 * 返回 1=已自动升级 0=无冲突(或任一写法非法,不参与冲突) -1=冲突且
 * 阶梯用尽(配置未改,请人工修改)。 */
int lyy_config_resolve_hotkey_conflicts(LyyConfig *c, char *note, size_t cap);

/* 保存;返回 0 成功。保留未知行与注释(见文件头) */
int lyy_config_save(const char *path, const LyyConfig *c);

/* AI 功能是否已配齐可用(启用 + api_base + model;key 本地服务可空) */
int lyy_config_ai_active(const LyyConfig *c);

/* 英文上屏去向取值归一(§6):temp/temporary → "temp",en/english/persist
 * → "en",空/未知 → def(宽恕手写拼写,与 core EnCommit::from_toml 同义) */
const char *lyy_en_mode_canon(const char *v, const char *def);

/* 按配置落/删开机自启项 ~/.config/autostart/lyyime-xim.desktop */
int lyy_config_apply_autostart(int enable);

#endif /* LYY_CONFIG_H_ */
