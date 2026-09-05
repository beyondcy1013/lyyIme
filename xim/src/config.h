/*
 * 配置读写:~/.config/lyyime/config.toml(与 doctor/app/ibus 共用,见
 * docs/ARCHITECTURE.md §4)。本模块只实现"平面键值 TOML 子集":
 *   - 仅支持顶层 `key = value`(int / bool),不含字符串值与嵌套表;
 *   - **保留未知行、[section] 行、整行注释与被管理行的行尾注释**;
 *   - 写回时已知键原位更新,未出现的键追加到文件尾。
 * 子集约定借鉴 TOML v1.0.0 规范(https://toml.io);只解析自己管理的键,
 * 其余行原样字节保留,保证与并行开发的 lyyime-core/doctor 互不踩踏。
 */
#ifndef LYY_CONFIG_H_
#define LYY_CONFIG_H_

typedef struct {
    int page_size;           /* 候选数(1..9),对齐 core Config */
    int mixed_english;       /* 中英混合(无中文候选时给英文词) */
    int auto_commit_english; /* 高置信英文词标点/空格自动直通 */
    int chinese_punct;       /* 中文态使用中文标点 */
    int learning;            /* 用户词学习开关 */
    int commit_after_four;   /* 满足四码后,继续输入字母先顶屏当前选中 */
    int font_size;           /* 候选窗字体大小(10..28) */
    int autostart;           /* 开机自启(写 ~/.config/autostart) */
} LyyConfig;

void lyy_config_defaults(LyyConfig *c);

/* 返回 0=读到 1=文件不存在已用默认 -1=读失败已用默认 */
int lyy_config_load(const char *path, LyyConfig *out);

/* 保存;返回 0 成功。保留未知行与注释(见文件头) */
int lyy_config_save(const char *path, const LyyConfig *c);

/* 按配置落/删开机自启项 ~/.config/autostart/lyyime-xim.desktop */
int lyy_config_apply_autostart(int enable);

#endif /* LYY_CONFIG_H_ */
