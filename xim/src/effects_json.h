/*
 * §3 JSON 效果流迷你解析器(固定 schema,非通用 JSON 库)
 *
 * 合同形态(docs/ARCHITECTURE.md §3):
 *   [{"t":"commit","s":"你好"},{"t":"preedit","s":"nihao"},
 *    {"t":"cands","n":5,"page":0,"pages":3},
 *    {"t":"pass"},{"t":"consumed"},{"t":"mode","m":1}]
 * 集成补充(主控通报,2026-09-05):preedit 清空输出 {"t":"preedit"}(无 "s")
 *   —— 解析为空串,宿主语义=清除预编辑。
 *
 * 实现边界(有意为之,固定 schema 不需要更多):
 *   - 字段顺序任意;对象成员只认 t/s/n/page/pages/m/i;
 *   - 字符串仅支持 \" \\ \/ \b \f \n \r \t 转义,不支持 \uXXXX(我们的 core
 *     JSON 序列化按 UTF-8 原样输出,见合同示例"你好");
 *   - "t" 取值必须是合同效果类型之一(commit/preedit/cands/pass/consumed/
 *     notice/hint/mode/action),否则整体判错(宿主降级直通,便于暴露违约)。
 * 纯 C 无依赖,可 headless 单测(xim/tests/unit_effects_json.c)。
 */
#ifndef LYY_EFFECTS_JSON_H_
#define LYY_EFFECTS_JSON_H_

/* 单条效果文本上限(commit/preedit 词长,UTF-8) */
#define LYY_EFF_STR_MAX 512

typedef enum {
    LYY_EFF_COMMIT = 0,   /* {"t":"commit","s":...} 上屏文本 */
    LYY_EFF_PREEDIT,      /* {"t":"preedit","s":...} 或 {"t":"preedit"}=清除 */
    LYY_EFF_CANDS,        /* {"t":"cands","n":..,"page":..,"pages":..} */
    LYY_EFF_PASS,         /* {"t":"pass"} 宿主原样放行该键 */
    LYY_EFF_CONSUMED,     /* {"t":"consumed"} 吞掉无可见效果 */
    LYY_EFF_NOTICE,       /* {"t":"notice","s":...} 辅助区提示(造词结果等) */
    LYY_EFF_HINT,         /* {"t":"hint","s":...} 词组效率提示(候选条,下一次输入清除) */
    LYY_EFF_MODE,         /* {"t":"mode","m":0|1} 中英指示 */
    LYY_EFF_ACTION,       /* {"t":"action","i":..} 快速功能键命中(§14;i=配置下标) */
} LyyEffKind;

typedef struct {
    int kind;                          /* LyyEffKind */
    char s[LYY_EFF_STR_MAX];           /* commit/preedit 文本(UTF-8) */
    int n, page, pages;                /* cands */
    int m;                             /* mode: 0=中文 1=英文 */
    int i;                             /* action: quick_actions 下标(§14) */
} LyyEffect;

/*
 * 解析效果流。成功返回 0,*count = 效果条数;
 * 失败返回 -1(schema 违约/坏 JSON);超出 max_out 返回 -2。
 */
int lyy_effects_parse(const char *json, LyyEffect *out, int max_out, int *count);

/* 调试用:效果类型名(中文) */
const char *lyy_effect_kind_name(int kind);

#endif /* LYY_EFFECTS_JSON_H_ */
