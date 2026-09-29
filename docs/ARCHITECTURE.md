# lyyIme 架构与接口合同(v1)

> 本文件是**实现合同**。所有模块以此为准;改动需主控批准并同步全文档。
> 目标:五笔/拼音/英文同字母空间混打,行为对齐搜狗/万能五笔的常用习惯。

## 1. 总体结构

```
                    ┌────────────────────────────┐
   字母/按键        │        lyyime-core         │   效果流(Effect)
 ┌──────────┐       │  码表加载/匹配/排序/学习     │ ┌──────────────────────┐
 │ Mode A   │ ibus  │  (Rust, headless, 纯逻辑)  │ │ Commit/Preedit/      │
 │ python   ├──────►│                            ├─┤ Candidates/Pass/...  │
 │ ctypes→FFI│      │  FFI: liblyyime_core.so    │ └──────────┬───────────┘
 └──────────┘       └──────────▲───────────────────┘            │
 ┌──────────┐                  │                                ▼
 │ Mode B   │ 直接 Rust 调用    │                    ibus 候选窗 / GTK3 自绘候选窗
 │ lyyime-app├─────────────────┘                  (宿主负责渲染与按键路由)
 │ (X11+GTK3)│
 └────┬─────┘
      │ 工具菜单
      ▼
 lyyime-doctor (lib+bin):诊断/修复 Linux 中文输入法环境
```

- **core 不感知 X11/ibus**:输入是抽象逻辑键 `LKey`,输出是效果 `Effect` 列表。
- 两个宿主(Mode A python、Mode B Rust GUI)只做三件事:按键采集与路由、候选窗渲染、上屏执行。
- 数据流:`dicts/(原始) → dicttool → data/runtime/(TSV) → core 启动时加载(内存索引)+ ~/.local/share/lyyime/user.tsv(用户词,write-behind)`。

## 2. 抽象键与效果(核心 API,Rust)

```rust
// crates/lyyime-core/src/lib.rs
pub struct Engine;                       // Send(宿主可能跨线程)
impl Engine {
    pub fn new(data_dir: &Path) -> Result<Self, Error>;
    pub fn set_config(&mut self, cfg: Config);
    pub fn config(&self) -> &Config;
    pub fn mode(&self) -> Mode;          // Chinese | English
    pub fn toggle_mode(&mut self) -> Mode;
    pub fn reset(&mut self);             // 清缓冲(焦点切换时宿主调用)
    pub fn process_key(&mut self, key: LKey) -> Vec<Effect>;
    pub fn select_candidate(&mut self, idx: usize) -> Vec<Effect>;
    pub fn flush_page(&self) -> &[Candidate];   // 当前页候选
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LKey {
    Char(char),          // 字母 a–z/A–Z:小写普通组词;大写=Shift+字母 原样传入(全大写敲入走大写候选通道,2026-09-28,§6)
    Digit(u8),           // '0'..'9' → 0..9(0 = 选第 10 个候选)
    Space, Enter, Backspace, Esc, PageUp, PageDown,
    Punct(char),         // 标点原字符(半角)
    ShiftPress,          // Shift 按下(宿主用于单击检测,见 §6)
    Coin,                // 造词热键(默认 Ctrl+=,coin_hotkey 可配置,§12)
    ArrowLeft, ArrowRight, ArrowUp, ArrowDown,
                         // 方向键:造词模式增减选字,非造词模式同 Other
    Other,               // 其余:core 恒回 Effect::Pass
}

#[derive(Clone, Debug)]
pub enum Effect {
    Commit(String),               // 上屏文本(UTF-8),宿主负责注入;core 已清缓冲
    Preedit(Option<String>),      // 预编辑串更新(None=清除)
    Candidates(Arc<Vec<Candidate>>), // 当前候选页更新(含总页数信息)
    Pass,                         // 宿主原样放行该键(英文态字母、未识别键)
    Consumed,                     // 吞掉但不产生可见效果
    Notice(String),               // 辅助区临时提示(造词结果等,宿主数秒后清除)
    Hint(String),                 // 词组效率提示(候选条展示,无定时,下一次输入清除)
    ModeChanged(Mode),            // 宿主更新 UI 指示
    Action(usize),                // 快速功能键命中(§14):宿主执行功能,不上屏文本
}

#[derive(Clone, Debug)]
pub struct Candidate { pub text: String, pub comment: String, pub score: f32, pub kind: CandKind }
pub enum CandKind { Wubi, Pinyin, English, User, Action(u8) }  // Action(§14)=quick_actions 下标
```

约定:
- `process_key` 返回的 `Effect` 列表顺序即宿主处理顺序;典型:`[Preedit, Candidates]`、`[Commit]`、`[Commit, Preedit(None)]`。
- `Pass` 意味着宿主必须把原键交给应用(ibus: 返回 false;app: XTest 回放)。

## 3. FFI 合同(C ABI,供 python ctypes 与测试)

```c
/* liblyyime_core.so —— 全部函数线程不安全,宿主保证单线程调用 */
void* lyyime_new(const char* data_dir);            /* NULL=失败 */
void  lyyime_free(void* eng);
void  lyyime_reset(void* eng);
int   lyyime_mode(void* eng);                      /* 0=中文 1=英文 */
int   lyyime_toggle_mode(void* eng);               /* 返回新 mode */
/* 喂键:key_id 见下表, chr 为 Char/Punct 的码点(其余填 0)。
   返回 effects JSON 写入 buf 所需字节数(含\0);若 buf_cap 不够,不写入并返回 -needed。
   effects JSON: [{"t":"commit","s":"你好"},{"t":"preedit","s":"nihao"},{"t":"cands","n":5,"page":0,"pages":3},
                  {"t":"pass"},{"t":"consumed"},{"t":"notice","s":"已造词:你好(wqvb)"},
                  {"t":"hint","s":"词组提示:「你好」可用 wqvb 打出"},{"t":"mode","m":1},
                  {"t":"action","i":0}]   /* §14 快速功能键命中,i=quick_actions 下标 */
   cands 的具体候选另取:lyyime_cand(eng, i, buf, cap) 返回候选文本,-needed 表示不足;
   lyyime_cand_comment 同理。 */
int64_t lyyime_process_key(void* eng, int key_id, uint32_t chr, char* buf, int64_t buf_cap);
int   lyyime_cand(void* eng, int i, char* buf, int cap);
int   lyyime_cand_comment(void* eng, int i, char* buf, int cap);
/* §14 快速功能键(可选符号组:旧库缺任一符号时宿主只禁用该功能,不整体降级) */
int   lyyime_set_quick_actions_enabled(void* eng, int enabled);   /* 返回生效 0/1 */
void  lyyime_clear_quick_actions(void* eng);
int   lyyime_add_quick_action(void* eng, const char* trigger, const char* label, const char* command);  /* 0 成功 -1 非法/超上限 */
int   lyyime_action_command(void* eng, int i, char* buf, int cap); /* 第 i 条 command,-needed 同上 */
int64_t lyyime_select_candidate(void* eng, int idx, char* buf, int64_t buf_cap); /* 点选候选(§14),两段式纪律同 process_key */
/* §15 候选右键操作(可选符号组:旧库缺失时宿主禁用右键菜单,不影响输入) */
int   lyyime_cand_pinned(void* eng, int idx);      /* 当前候选是否已固定:0/1;非用户可编辑候选=-1 */
int64_t lyyime_cand_op(void* eng, int op, int idx, char* buf, int64_t buf_cap);
                                                 /* op:0=固定/取消固定 1=删除词组 2=反查英文;
                                                    返回当前页 effects JSON(同 process_key 两段式纪律) */
```

`LKEY_CHAR` 的 `chr` 接受小写与大写码点:小写为普通组词,大写为 Shift+字母 原样传入(core 内部小写化组词并以 buf_raw 镜像敲入原形,全大写敲入走大写候选通道,2026-09-28,§6)。

`key_id` 枚举(python 侧同样常量):`LKEY_CHAR=0, LKEY_DIGIT=1, LKEY_SPACE=2, LKEY_ENTER=3, LKEY_BACKSPACE=4, LKEY_ESC=5, LKEY_PAGEUP=6, LKEY_PAGEDOWN=7, LKEY_PUNCT=8, LKEY_SHIFTPRESS=9, LKEY_OTHER=10, LKEY_COIN=11, LKEY_LEFT=12, LKEY_RIGHT=13, LKEY_UP=14, LKEY_DOWN=15`(11–15 见 §12)。

`LKEY_SHIFTPRESS` 表示 **Shift 按下**:core 对有缓冲组合先上屏英文原串,再按 `shift_english`(默认 `en`)于效果流末尾追加 `{"t":"mode","m":1}` 切英文模式;空缓冲回 Consumed,由宿主继续做 Shift 单击判定。回车上屏英文原串同理由 `enter_english`(默认 `temp`)决定是否追加 mode 效果——两个键共用 temp/en 取值,一处配置(XIM 设置窗「输入」页 / config.toml `enter_english`、`shift_english`),均有 FFI 开关 `lyyime_set_enter_english` / `lyyime_set_shift_english`(非 0 = en,返回生效后的 0/1)。四码顶屏默认**开启**(`commit_on_extra_after_four`,2026-09-28 起默认改为开启;XIM 写盘键名 `commit_after_four`,core `ConfigToml` 与 ibus 读取均兼容该别名,规范键优先):恰好四码且合并排序后首选来自五笔通道(Wubi/User)时,再输入字母先上屏首选、该字母开启新组合;首选是拼音/英文/功能键候选或缓冲恰是触发词前缀时不顶屏,继续渐进组词(`niha`+'o' 续拼 nihao、`hell`+'o' 续拼 hello、长触发词不被劫持)。**四码首选上屏默认开启**(`commit_first_at_four = true`):恰好凑满四码、合并排序后首选是五笔命中(按 `wubi_exact` 词条归属判定,学习过的拼音词标 User 不算)且**候选条只此一条**时,core 直接回 Commit(首选),免按空格;**候选条多于一条禁止上屏**(2026-09-28 修订:同码五笔重码与拼音/简拼/英文混排候选都算重码,候选条只要超过一条即不自动上屏)——保留组合交给空格/数字选词,或继续输入由四码顶屏上屏首选;首选是拼音/英文候选时不触发(`niha` 是 nihao 的中间态、"hell" 不该四键上屏英文前缀词,拼音长码与英文单词输入不被打断);缓冲恰等于快速功能键触发词(或为其前缀)时同样不触发,功能候选须经用户确认(§14)。四码唯一上屏默认**开启**(`commit_unique_four = true`):`commit_first_at_four` 关闭后回退为本项判定——恰好四码、有中文命中且合并排序后候选唯一时免空格上屏;多候选不触发,唯一候选是英文词时同样不触发(避免四键即把英文前缀词上屏)。**死码保护**:原组合还有中文命中时,新字母若把缓冲推进完全无候选的死胡同(五笔码 ≤4、拼音/简拼索引均前缀单调,加长后不可能救回中文命中),该字母不进缓冲、回 Consumed,候选状态原样保留——四码未选继续敲击不再把候选清空、进而被空格/标点当英文字母直通;快速功能键触发词的前缀(§14)不受此保护,保证触发词总能敲完。**词组效率提示**默认**开启**(`phrase_hint = true`):每次 commit 含汉字后,core 回看最近 2–6 个上屏汉字,若该后缀是词库五笔词组(含用户造词)且词组编码长度**严格小于**这几个字实敲的字母数(多字同屏按均摊计),在同一效果流末尾追加 `{"t":"hint","s":"词组提示:「词」可用 编码 打出"}`;同码词组打过的不重复提示。宿主把 hint 展示在候选条/辅助区且**不做定时清除**,直到下一次输入产生新效果流时自然替换或隐藏。两项均有 FFI 开关:`lyyime_set_commit_after_four` / `lyyime_set_commit_unique_four` / `lyyime_set_phrase_hint`(非 0 启用,返回生效后的 0/1)。

**chr 传值约定(v1.1 实现期确认)**:`chr` 携带 Char/Punct/**Digit** 的码点——Digit 传 `'0'..'9'`(ASCII 0x30..0x39),core 按 `chr-'0'` 取值,0 = 选第 10 个候选;其余 key_id 填 0。**有状态纪律**:`lyyime_process_key` 必须先生成完整 effects JSON、确认写入容量足够后才落内部状态变更,保证宿主因 `-needed` 扩容重试时同一键不会二次生效。

## 4. 数据文件格式(data/runtime/,UTF-8 TSV,dicttool 产物)

| 文件 | 列 | 说明 |
|---|---|---|
| `wubi.tsv` | `code\tword\tfreq` | 五笔码→词;含单字全码/简码与词组;按 code 升序、freq 降序;海峰源库「词尾 `.`」隐藏词条(生僻字标记)在 convert 阶段过滤,不入表 |
| `pinyin_char.tsv` | `pinyin\tchar\tfreq` | 全拼(无声调)→单字 |
| `pinyin_phrase.tsv` | `word\tpinyin\tfreq` | 词组、无声调全拼(音节空格分隔)、词频 |
| `english.tsv` | `word\tfreq` | 小写英文词+词频,≥1万行 |
| `suggestion.tsv` | `word\tfreq` | 通用词频(排序兜底) |
| `char_tier.tsv` | `char\ttier` | GB2312 单字分档(1=一级常用 3755 字,2=二级次常用 3008 字);`dicttool tier` 从内嵌表生成,缺失时引擎按纯语料排序降级 |
| `meta.json` | `{"version":1,"source":...,"rows":...,"built_at":...}` | 校验信息 |

用户数据:`~/.local/share/lyyime/user.tsv`,格式 `word\tfreq_extra\tlast_used_epoch`;core 定期(每 64 次 commit/退出时)批量落盘。
用户造词:`~/.local/share/lyyime/user_words.tsv`,格式 `word\tcode\tcount`(与 user.tsv 同目录,随 user_dict 覆盖迁移);启动时整表并入五笔索引,造词即时落盘(临时文件 + rename 原子替换),见 §12。
候选固定:`~/.local/share/lyyime/pinned.tsv`,格式 `code\tword`(右键「固定首位」产物,§15);引擎启动整表读入,候选重算时把 `code` 精确命中的词置顶、注释追加「固」。
候选屏蔽:`~/.local/share/lyyime/blocked.tsv`,每行一个词(右键「删除词组」产物,§15);被屏蔽词在任何编码下均不出候选,再造该词自动解屏蔽。
中英反查:`data/runtime/zh_en.tsv`,格式 `word\ten1\ten2…`(`dicttool zhen` 从 ECDICT/StarDict 生成,§15);缺失时反查菜单项给「没有英文反查结果」提示,不影响其余功能。
英译中翻译:`data/runtime/en_trans.tsv`,格式 `词\t译1\t译2…`(`dicttool entrans` 从 ECDICT/StarDict 生成、义项按行清洗去重,内置常用缩略语人工校对表义项恒排最前,如 WHO→世界卫生组织;词必须小写、≤6 条);全大写输入候选(§6)的中文翻译来源,缺失时降级为仅大小写变体,不影响其余功能。

输入统计:`~/.local/share/lyyime/stats/YYYY-MM-DD.tsv`(本地日期,按天分文件),每行 `epoch_ms\tchars`——一次上屏一条(chars=非空白字符数)。Mode A(`EngineLogic::dispatch` 的 Commit 效果)与 Mode C(悬浮窗 commit 成功后)各自进程内追加同一目录(单行 O_APPEND,不交错)。查询走 core `stats` 模块(`today_summary`,纯函数、时间由调用方传入):字数 = 当天 chars 之和;速度 = 字数 ÷ 活跃时长,相邻上屏间隔 ≤ 排除阈值才计入时长(超时空隙——思考/离开——不算)。显示端为 lyyime-float:输入停顿 `stats_pause_secs`(默认 10)秒后在状态行显示「今日已输入 N 字 · 约 M 字/分」,重新输入即还原;统计设置为**全局配置**,真源在 config.toml 顶层 `stats_enabled` / `stats_pause_secs` / `stats_idle_exclude_secs`,由设置窗口「输入统计」页统一管理(悬浮窗菜单不再有设置入口),悬浮窗对 config.toml 做文件监视、改动即时生效;悬浮窗旧 config.json 的同名键仅作迁移回退(toml 一个统计键都没有时才读)。统计为旁路功能,任何文件错误静默,绝不影响输入主链路。

Mode C 不抢焦点模式(float config.json `keep_target_focus`,默认关,菜单「光标留在目标窗口(不抢焦点)」切换):悬浮窗 `accept_focus/focus_on_map=false`(WM_HINTS input=False,点击/映射均不夺焦,目标文本框光标全程保持),键盘经 GdkSeat 抓取(`gdk_seat_grab` KEYBOARD,owner_events=true——本进程菜单/对话框照常收键)送达输入框;窗口映射未完成时抓取按 100ms 重试。上屏/直通退格的 XTest 注入前必须解抓→注入→重抓(抓取在,注入会被自己的 grab 拦回悬浮窗)。该模式下悬浮窗可见期间全局键盘由其接管(含 Alt+Tab),隐藏(—)即让出。E2E:`crates/lyyime-float/e2e_no_focus.sh`(断言全程活动窗口=目标、目标内容恰为连续上屏文本)。

Mode C 文本框撤销/重做:GTK3 的 Entry/TextView 无内建 undo,`src/undo.rs` 自实现状态栈(每次变更加一个状态,上限 200,回填去重,撤销后输入丢弃重做分支),覆盖编码框/打字板/短语编辑器(Ctrl+Z 撤销、Ctrl+Y/Ctrl+Shift+Z 重做)。编码框历史按词重开(上屏/Esc 即 reset);空缓冲且无可撤销时 Ctrl+Z/Ctrl+Y 直通目标窗口,撤销/重做的是目标应用里刚上屏的文本(与空缓冲退格直通同一合同)。

配置:`~/.config/lyyime/config.toml`(doctor/app/ibus 共用;字段见 core `Config` 默认值,注释中文)。快捷键类:`coin_hotkey`(§12)、`shot_hotkey`(§13),写法均为「修饰(ctrl/alt/super/shift,至少一个)+键名」。英文上屏去向:`enter_english` / `shift_english`(§6,取值 `temp` 临时 / `en` 切英文模式,默认分别为 temp / en;XIM 设置窗「输入」页两行下拉,XIM 宿主经 `lyyime_set_enter_english` / `lyyime_set_shift_english` 下发,取值缺失/未知按各键默认)。排序类:`exact_char_freq_rank`(§5,默认开;XIM 经 `lyyime_set_exact_char_freq_rank` 下发,关闭恢复精确单字恒居首位旧行为)。

## 5. 匹配与排序算法(v1)

1. 缓冲区 = 连续小写字母(≤12),Backspace 删尾。
2. **五笔通道**:查 wubi.tsv,`code == buf` 完全命中优先;`code 是 buf 前缀` 给渐进候选;取每码频次前 9。
3. **拼音通道**:对 buf 做音节切分(DP,音节表取自 pinyin_char 去重),允许末音节不完整;查 pinyin_phrase(词组)与 pinyin_char(单字);简拼(每音节首字母,≥2 键)低权重参与。
   **注释反查五笔**:候选的 comment 编码提示统一为五笔——拼音命中的词经 wubi.tsv 的词→码反查索引(`wubi_rev`,同词多码取最长全码)显示其五笔编码,便于拼音打字时学习五笔;词不在五笔表时保留拼音注释兜底。五笔候选注释仍为其命中编码,英文候选仍为 `en`。
4. **英文通道**(修订版细则见第 5 条末"英文通道修订"):无中文命中 → 前缀候选;buffer 是完整高频英文词 → 即使有中文命中也入选;直通上屏按"无中文命中 或 top-500 词"门控。
5. **合并排序(v1.1,真实词库集成后修订——层级主导词典序)**:排序键为**词典序 (tier, specificity, freq_norm)**——层间不可跨越,层内先按 specificity(词组音节数放大、完整片段>更短片段、完全同码英文词 +0.5,用于压制伪切分;wubi_exact 层内 spec:**`exact_char_freq_rank` 开(默认)时单字与词组同档 1.0,由 freq_norm(词频)定次序——"四码/简码符合的单字"不再无条件置前(2026-09-27 修正:原"单字恒居词组前"让不常用单字压住高频词组);表外生僻字仍取 0.5 沉在词组之后。** 开关关闭时恢复旧分档:单字按 GB2312 分档 一级 3.0 > 二级 2.5 > 词组 1.0 > 表外生僻 0.5——码表 freq 对大量字是默认值、拼音语料对生僻字是填充值(`pinyin_char.tsv` 里 牏/汆 等共享 585000),都回答不了"是不是常用字",分档表(`char_tier.tsv`)是旧分档的依据;繁体(歟/與/種)、扩展区生僻字沉到词组之后但仍可翻页选出;无分档表时退化为 单字 2.0 > 词组 1.0),再按 `freq_norm ∈ [0,10)`。单字 freq_norm 用真实语料频次(pinyin_char.tsv 全量、同字取最大)归一,语料未覆盖按码表频 norm ×0.1,生僻档与无语料时按码表频;词组恒按码表频。**精确单字频率档位(同开关)**:有分档表时,语料常用字(归一频率 f ≥ 0.5)保持 wubi_exact 层,低频/生僻单字(f < 0.5,表外生僻取 0.15、表内未覆盖取 0.5)按频率在 `[wubi_prefix−间隔, 60]` 线性降档(`rank::exact_char_tier`),让位更高频的前缀词组与高频词。实现详见 `crates/lyyime-core/src/rank.rs`(模块文档含层级表)。设计动机:各源频率尺度差 4 个数量级以上,加性权重会被大频值跨层碾压。
   | 层 | tier_base | 说明 |
   |---|---|---|
   | wubi_exact | 60 | code==buffer(简码奖励并入此层);`exact_char_freq_rank` 开启时低频单字按语料频率降档至 [40−间隔, 60) |
   | english_no_cn | 55 | 无任何中文命中时的英文候选 |
   | pinyin_full | 50 | 完整音节切分全命中(词组/单字),同等切分数优先覆盖更多完整音节 |
   | wubi_prefix | 40 | wubi 前缀渐进 |
   | pinyin_partial | 35 | 仅末音节不完整 |
   | english_with_cn | 30 | buffer 是完整高频英文词但存在中文命中 |
   | pinyin_abbrev | 20 | 简拼 |
   `freq_norm = log10(1+freq) / log10(1+maxf_of_file) × 10`(每文件独立归一,加载时缓存 max)。设计动机:各源频率尺度差 4 个数量级以上(拼音单字 1e9 vs 词组 1e3),加性权重会被大频值跨层碾压;层级化后同层内同源尺度自然一致。用户词加成为层内 freq 乘子(×1.5)。输出前 page_size 条。
   英文通道修订:完整英文词 ∈ english.tsv 前 mixed_auto_commit_top_n(默认 500)时即使有中文命中也入选(english_with_cn);∈ 前 en_freq_top_n(2000)且无中文命中时进 english_no_cn。**中文态下英文词只作为候选展示;Space 是确认键,始终顶屏当前选中;单个英文词用回车临时上屏(默认),整段英文输入用 Shift 上屏并切英文态(§6)。**
6. **学习**:commit 候选词 → freq_extra += 1,重排时乘 user 权重。

## 6. 按键行为规范(宿主必须一致实现)

| 输入 | 行为 |
|---|---|
| a–z | 缓冲,更新 preedit/候选;有中文命中时若该字母把缓冲推进完全无候选的死胡同,则吞键保留候选(死码保护,§3;功能键触发词前缀例外) |
| CapsLock 大写态 + 字母 | **原样直通英文,不进组词缓冲**:无 Shift 输出大写字母;Shift+字母由应用按 Caps+Shift 翻译输出小写字母。直通前宿主送 core `Other` 复位可能残留的缓冲(CapsLock 键本身经"其它键"路径清缓冲);数字/标点等非字母键不受 CapsLock 影响,行为同常态 |
| Shift+字母(中文态,2026-09-28) | 大写字母进组词缓冲(core 内部小写化组词,`buf_raw` 镜像敲入原形,preedit 显示敲入的大小写)。**全大写敲入**(缓冲全部字符为大写)时候选固定为 原样大写 → 首字母大写 → 全小写 → 中文翻译(en_trans.tsv,候选 4 起、常用在前,候选 5、6 后为其它常用翻译),不混入五笔/拼音候选;无翻译词条时仅三个大小写变体,单字母去重。混合大小写一经出现即回退小写普通通道。Space/Enter/标点收尾与 Shift 上屏均保留敲入大小写(空格顶屏=原样大写);全大写四码不触发四码首选/唯一上屏;数字/点选按位选择不变 |
| 四码首选上屏(可配置) | 恰好输入第 4 个字母、首选是五笔命中且**候选条只此一条**:core 直接 Commit(首选),缓冲与候选一并清空;**候选条多于一条不上屏**(同码重码或混排候选都算),保留组合等选词或继续输入顶屏;首选为拼音/英文或缓冲命中功能键触发词(前缀)不触发。默认开启(`commit_first_at_four`) |
| 四码唯一上屏(可配置) | `commit_first_at_four` 关闭后回退判定:恰好四码、有中文命中且合并候选唯一时 core 直接 Commit(该候选);多候选或唯一候选为英文词不触发。默认开启(`commit_unique_four`),两者全关后第 4 键保持组合(§3) |
| 四码顶屏(可配置) | 缓冲恰四码且首选是五笔命中(Wubi/User)时再输入字母:先 Commit(当前首选)、该字母开启新组合;首选为拼音/英文/功能键候选或缓冲是触发词前缀时不顶屏,继续渐进组词。默认开启(`commit_on_extra_after_four`,别名 `commit_after_four`) |
| 1–9 | 有候选:选第 N 个上屏;无候选:Pass(数字原样)。选中快速功能键候选(§14)时回 `Action(i)` 而非 Commit |
| 0 | 有候选且当前页 ≥ 10 条:选第 10 个上屏(`page_size` 默认 10,数字键只到 9,0 补足第 10 个);否则 Consumed(无候选时经"无候选放行"路径 Pass 的是 1–9;0 无候选即吞) |
| Space | **任何时候都确认当前选中项**:有候选顶屏首选(首选为快速功能键候选时同样回 `Action(i)`,§14);有缓冲无候选 Commit(原字母);无缓冲 Pass(空格) |
| Enter | 有缓冲:Commit(原字母)——单个英文词的输入方式,去向按 `enter_english`(默认 `temp` 临时:保持中文模式;`en` 长久:追加 `ModeChanged(English)` 切英文态);无缓冲:Pass |
| Backspace | 有缓冲删尾;空:Pass |
| Esc | 清缓冲(Consumed);空:Pass |
| `-`/`=` | 有候选翻页(Consumed);否则 Pass |
| 标点 | 中文态空缓冲→Commit(对应中文标点);有缓冲→Commit(首选)+Commit(中文标点);英文态 Pass |
| 上屏后词组提示(可配置) | 含汉字的 Commit 之后,若最近 2–6 个上屏字有更省键的五笔词组(编码长 < 实敲字母数),效果流末尾追加 `Hint(「词」可用 编码 打出)`;宿主候选条展示、**无定时**,下一次输入的新效果流自然替换/清除(§3;`phrase_hint` 默认开) |
| Shift 按下 | 有缓冲:**Commit(原字母)**(英文原串),去向按 `shift_english`(默认 `en` 长久:追加 `ModeChanged(English)` 进入英文模式,宿主同步中英指示/XIM trigger;`temp` 临时:仅上屏,保持中文,恢复旧版行为);空缓冲:Consumed 并进入单击检测 |
| Shift 单击(空缓冲) | toggle_mode + ModeChanged(单击=按下后未产生其它键即释放,且无其它修饰) |
| 其它键 | Pass(有缓冲时先 reset) |
| 造词热键(Ctrl+=) | 进入造词模式(§12);组合中先按普通流程上屏再进入;英文态直通 |
| 方向键 ←→↑↓ | 造词模式:→/↑ 多选一字、←/↓ 少选一字;非造词模式同"其它键" |

中文标点映射(可配置):`, . ? ! ; :` → `， 。 ？ ！ ； ：`,`( ) { }` → `（ ） ｛ ｝`,`[ ]` → `【】`,单/双引号开合交替;默认集在 core `punct.rs`(映射值一律 Unicode 转义书写,2026-09-06 修复映射表曾被写成半角恒等的缺陷)。

## 7. Mode A:ibus python 引擎

- 位置(安装后):`/usr/local/share/lyyime/ibus/engine/lyyime.py` + component XML `/usr/local/share/ibus/component/lyyime.xml`(engine name `lyyime`,symbol `伍`)。
- `lyyime.py`:标准 `IBus.Engine` 子类;`do_process_key_event` 把 keysym/state 映射为 `LKey`(在 ctypes 边界小写化字母、区分 Shift 释放序列)→ `lyyime_process_key` → 依 JSON 效果流调用 `commit_text / update_preedit_text / update_lookup_table / page_up|down`。
- 数字键选词、`-`/`=` 翻页由 core 返回 Candidates 后的 lookup table 承载;`Pass` 的键返回 `False` 让 ibus 放行。
- 托盘属性菜单:中英切换 / 截屏(§13)/ 设置(拉起 `lyyime-app --settings`)/ 工具与修复(拉起 `lyyime-doctor --gui`)。
- 崩溃隔离:engine 异常时退化为英文直通并打日志 `~/.local/share/lyyime/logs/ibus.log`。


**引擎状态发布(悬浮窗联动)**:引擎进程把中/英模式与启用态发布到 `/tmp/lyyime-engine-state.json`
(`{"source":"ibus","mode":"cn|en","enabled":bool,"ts":ms}`;变更即写 + 10s 心跳刷 ts),
悬浮窗状态行前缀「内置:中/EN/停用」每秒读取联动;超过 30s 无新鲜心跳显示「未运行」。
## 8. Mode B:lyyime-xim 独立外挂(X11,XIM server 路线)

> 架构决策依据见 RESEARCH.md §2(本机实证:被动 grab 回放不可靠,XIM 是正统路线)。
> 二进制 `lyyime-xim`(C 语言:IMdkit + GTK3 + dlopen(liblyyime_core.so) C ABI;IMdkit vendor 进仓库)。

- **接入**:应用设置 `XMODIFIERS=@im=lyyime`(doctor 的 Mode B profile 负责写入并重启会话应用);GTK3 内建 xim immodule / Xlib 应用原生接入。覆盖 GTK3+Xlib+XIM 类终端;Qt5 走 Mode A(互补全覆盖)。
- **按键流**(root-window style):trigger on 后,XIM forward event → 映射 LKey → core.process_key → 效果流:Preedit/Candidates 画进自绘候选窗;Commit → `IMCommitString`(任意 Unicode);Pass 类键 → `IMForwardEvent`(协议级原样回放,零风险)。
- **Shift 单击切换** = 空缓冲时以 XIM trigger off/on 切换中英:off 后应用直接收键(英文态),再 on 恢复中文态;组合中 Shift 已用于上屏英文原串,该次按键被消费且 release/超时不得再次切换模式。上屏英文原串的 mode 效果(shift_english=en,§6)在 apply_effects 内即时 `set_trigger(0)` 关 trigger 转英文,与单击路径同机制。
- **候选窗**:GTK3 override-redirect、无边框、accept_focus(false),跟随光标(root style 下用 XQueryPointer);序号高亮首选、编码提示、翻页指示,样式对齐主流输入法。
- **托盘**:Gtk.StatusIcon(XEmbed,兼容 xfce4-panel):状态(中/EN)+ 左键单击切换中英 + 右键菜单:主窗口、启用/停用、模式、设置、工具(直输模式/截屏(§13)/ 修复输入法 / 输入法管理(exec `lyyime-doctor` CLI 并解析 JSON,危险操作 GTK 确认对话框)、重载词库、日志)、退出。
- **主窗口**:GTK3 纯代码构建(`mainwin.c`,标题 "lyyIme 输入法"):状态行(版本/启用/中 EN/引擎态,随 `update_mode_ui` 即时刷新)+ 入口(输入设置…=打开设置窗;直输模式…=拉起 lyyime-float,Mode C 悬浮独立输入)+ 工具箱(与托盘工具共用 `tools.c` 动作)。唤起路径:托盘菜单、`lyyime-xim --mainwin`(单实例二次启动发 SIGUSR2;SIGUSR1 仍弹设置窗)、`--mainwin` 首次启动直弹。关闭=隐藏保活,退出走托盘。
- **设置窗**:GTK3(GtkBuilder .ui),读写 ~/.config/lyyime/config.toml,保存即 set_config 生效;含 Mode B 专属项(XMODIFIERS 一键切换到 lyyime/恢复 ibus)。
- **探路石前置**:先交付"最小 XIM server + GTK3 Entry 连通"spike(Xvfb 实证),通过后才铺全量;失败则升级为自带 GTK immodule 方案并回报主控。
- 设置窗:GTK3,读写 config.toml,保存即生效(core set_config)。

## 9. lyyime-doctor(工具菜单核心)

`check` 项(输出 JSON+文本,共 10 项):env(XMODIFIERS/GTK_IM_MODULE/QT_IM_MODULE 是否指向运行中的框架);gui-env(采样桌面会话进程 `/proc/<pid>/environ` 的真实三件套,含跨进程一致性——诊断以 GUI 进程环境为准,不看调用方 shell);session-bus(IBus 是否注册在桌面会话真实使用的总线上:busctl 地面真值,daemon 进程环境兜底——根治「daemon 活着但总线接错」的假活故障);daemon(ibus-daemon 存活);engine-register(lyyime 引擎组件 XML 是否就位/ibus 缓存);autostart 项;GTK2/3 与 Qt 的 immodule 文件存在性;`~/.local/share/lyyime` 数据完整性;最近日志错误;locale 编码。
`fix`(幂等,可 `--dry-run`):写会话环境(~/.xprofile + ~/.config/lyyime/env.sh,managed block;Mode B 用 modeb-env)、`ibus write-cache` + 重启 ibus-daemon(**按会话真实环境注入 DBUS_SESSION_BUS_ADDRESS/DISPLAY 后拉起**,取自 GUI 进程 environ,根治接错总线)、重建 autostart、重装/重转词库、清 GTK immodule 缓存。GUI 由 lyyime-app 工具菜单嵌入(列表+一键修复)。
`probe`(端到端验证):自建 `lyyime-probe` 窗口 + `xdotool windowfocus` 显式聚焦(绝不碰用户焦点窗口)+ 注入字母 + 缓冲断言;通过=环境→总线→daemon→引擎→上屏全通;对「字母直通」(IM 未接管→指向 gui-env/session-bus)与「全角字母」(引擎全角模式,Shift+Space)自动判因。

### 9.1 输入法管理(工具菜单 · 增删其它输入法)

doctor lib 额外提供一组管理 API(`ImeManager`,CLI 子命令同名),lyyime-app 工具菜单的"输入法管理"对话框基于它实现:

- `ime-list`:枚举本机输入法 —— ① ibus 引擎:解析 `/usr/share/ibus/component/*.xml` 与 `~/.local/share/ibus/component/*.xml` 得到 engine 名/语言/图标;② 框架与包:rpm -qa 探测 `ibus-*`、`fcitx5*`、`ibus-rime` 等;③ lyyime 自身(Mode A/Mode B)。输出 {id, name, kind: ibus-engine|fcitx5|lyyime|other, installed, active, is_default, package}。
- `ime-add <id>`:按内置清单(`catalog.rs`:id→dnf 包名,如 pinyin→ibus-libpinyin、wubi-haifeng→ibus-table-chinese-wubi-haifeng86、rime→ibus-rime、fcitx5→fcitx5+fcitx5-chinese-addons+fcitx5-table、wubi-jidian→ibus-table-chinese-wubi-jidian86 等)执行 `dnf install -y <pkgs>`,随后注册到 ibus 预载列表(见 ime-default)并提示重启 ibus。仅 root 可装包;非 root 时给出 sudo 提示并失败。
- `ime-remove <id>`:卸载对应包(`dnf remove`)或注销引擎(从预载列表移除 + 删用户级组件 XML)。**保护规则**:拒绝移除 lyyime 双模式中仍被设为默认的那个、拒绝移除会话里唯一可用的中文输入法,除非显式 `--force`。
- `ime-default <id>`:把引擎设为默认(ibus:gsettings schema **`org.freedesktop.ibus.general`**(openEuler ibus 1.5.29 实测确认;旧文档 `desktop.ibus.general` 作为兼容探测),目标 engine 放到 preload-engines 与 engines-order 首位;schema 缺失时降级 `dconf write /desktop/ibus/general/preload-engines`;fcitx5:写其 profile 第一项)。先探测 gsettings/dconf 可用性,不可用则降级给出手动指引并 warn。(依据 RESEARCH.md §1.6;包名按 openEuler 实际,如 `ibus-table-chinese-wubi-haifeng`)
- 安全:所有增删默认 `--dry-run` 打印将执行的命令,实际执行需 `--yes`(GUI 端为确认对话框);包操作前后各跑一次 `ime-list` 差量作为结果证据;全程写日志到 `~/.local/share/lyyime/logs/ime-manager.log`。
- 测试:catalog 与命令构造用纯函数 + tempdir/stub 验证(不真装包);真机演练只对"list"与"dry-run"做 e2e 断言。

## 10. 测试矩阵

| 层 | 手段 | 位置 |
|---|---|---|
| core 逻辑 | cargo test(纯函数,数据用 fixtures) | crates/lyyime-core/tests |
| FFI | ctypes 冒烟脚本(py 喂键序列断言 JSON) | crates/lyyime-core/ffi-test.py |
| dicttool | verify 子命令 + 固定断言 | crates/lyyime-dicttool |
| ibus 引擎 | 不经 daemon 直接实例化喢单元键 | tests/unit_ibus_engine.py |
| Mode B 全链路 | Xvfb :99 + xdotool key/send,断言 gedit/简单 GTK 文本域内容 | tests/e2e/run.sh |
| 截屏助手 | Xvfb :95:--auto 精确裁剪 / --full / 框选拖拽 / 单击与 Esc 取消 / 双击整屏 | tests/e2e/shot_e2e.sh |
| doctor | 破坏环境→check 发现→fix 恢复 | tests/e2e/doctor_test.sh |

## 11. AI 助手(/AI 触发调用自定义大模型)

中文态输入 `/AI`(大小写均可)+ 提示词,回车调用 **OpenAI Chat Completions
兼容**服务(DeepSeek/千问/智谱/SiliconFlow/Ollama/LM Studio 等),回复直接
上屏。未配置 `[ai]` 或 `enabled=false` 时功能完全不介入按键(行为与历史
版本逐键一致)。

### 11.1 交互合同(Mode A / Mode B 宿主必须一致实现)

| 输入 | 行为 |
|---|---|
| `/` `A|a` `I|i`(中文态、core 空缓冲) | 依次吞下,进入采集态;预编辑显示 `/AI ` |
| 触发中途打歪(如 `/x`) | 补发已吞按键后按普通路径处理(Mode A 补发为文本上屏 `/`;Mode B 协议级原样补发事件) |
| 采集态字母 | 照常进 core 组词;**组词上屏结果进入提示词**(可写中文提示词) |
| 采集态空格/数字/标点 | 有候选/映射:确认结果进提示词;否则原字符进提示词 |
| Backspace | 有组词删组词;否则删提示词尾字符;删穿 `/AI` 前缀=取消(前缀不回放) |
| Esc | 先清组词;空缓冲再按=取消会话 |
| Enter | 发送:core 有缓冲先按合同"上屏原始字母"并入提示词;空提示词取消并提示 |
| 方向键/功能键/Ctrl/Alt/Super 组合、焦点切换 | 放弃会话(组合键/不可归档键原样放行) |

回复上屏 = 一次普通 Commit;失败经候选窗/辅助区给"人话"提示。提示词上限
2000 字(Mode B 6000 字节),请求超时 `timeout`(5..300 秒)。

### 11.2 分层与实现

- **共享客户端** `ibus-engine/engine/lyyime_ai.py`(仅标准库):读取
  `config.toml` 的 `[ai]` 段(tomllib,损坏时逐行兜底——兼容历史 C 端写入的
  无 `#` 注释行)、`chat(prompt, cfg)` 调用 `/chat/completions`、CLI
  (`--prompt`/`--check`)。接口形态借鉴 OpenAI API Reference(platform
  .openai.com/docs/api-reference/chat)。
- **Mode A**:`lyyime.py` 的 `EngineLogic` 触发状态机(`_ai_take`)在按键
  映射前拦截;`on_ai_submit` 由 `LyyimeEngine` 在 daemon 线程调用
  `lyyime_ai.chat`,结果经 `GLib.idle_add` 回主循环 commit(单线程 FFI 纪律)。
- **Mode B**:`ai_capture.c` 状态机(挂在 xcb_im 回调内,Shift 处理之后、
  Shift+字母直通之前);HTTP 由 `python3 lyyime_ai.py --prompt ...`
  **子进程**完成(C 端零网络依赖),`g_spawn_async_with_pipes +
  G_SPAWN_DO_NOT_REAP_CHILD` + 管道回读,完成后 commit 到当前焦点 IC;
  脚本解析顺序:`$LYYIME_AI_HELPER` → `/usr/local/share/lyyime/ibus/engine/`
  → `/usr/local/share/lyyime/tools/` → 源码树。
- **配置**:`~/.config/lyyime/config.toml` 的 `[ai]` 段:`enabled`(默认
  false)、`api_base`、`api_key`(本地服务可空)、`model`、`system_prompt`
  (可选)、`timeout`。两模式读同一份;Mode B 设置窗(xim/res/settings.ui)
  提供 AI 页与"测试连接"(`--check`),保存即生效(Mode A 每次焦点进入、
  Mode B 每键实时读取)。

### 11.3 测试

单测:`tests/unit_lyyime_ai.py`(本地 mock HTTP,零外网)、
`tests/unit_ibus_engine.py::AiTriggerTestCase`(逻辑层桩)。E2E:
`tests/e2e/run.sh` 起 `tests/e2e/mock_ai_server.py`,双模式各断言一条
`/AI hi` → 回复上屏 → mock 收到正确 path/鉴权/提示词的全链路。

## 12. 造词模式(自定义快捷键,Ctrl+= 默认)

上屏过的汉字即时组成五笔词组入库(自造词),交互习惯借鉴极点五笔/万能
五笔的 Ctrl+= 造词;词组取码规则为五笔86 标准词组编码(与海峰86 码表一致)。

### 12.1 交互合同(Mode A / Mode B 宿主必须一致实现)

| 输入 | 行为 |
|---|---|
| 造词热键(中文态) | 进入造词模式:选取最近上屏的连续汉字(初始=最近一次 commit 的汉字串,仅 1 字时自动带上前一字凑二字);预编辑显示 `造词:<选区>`,候选窗单条展示选区+自动编码注释;组合中按下则先按普通流程上屏再进入;无历史时提示「还没有可造词的上屏汉字」;英文态直通 |
| `→` / `↑` | 多选一个字(向前扩);已达历史上限(≤32 字)吞键 |
| `←` / `↓` / Backspace | 少选一个字;二字下限吞键 |
| Enter / Space | 确认造词:按取码规则自动编码,写入 `user_words.tsv` 并即时并入词库,辅助区提示「已造词:xxx(yyyy),可直接用该编码打出」;**不向应用输出任何文本** |
| Esc | 取消造词模式 |
| 字母/数字/标点/翻页/Shift | 退出造词模式并按普通路径处理本键(继续正常输入);焦点切换 reset 一并取消 |

### 12.2 取码规则(五笔86 词组编码)

按 `wubi.tsv` 反查索引(wubi_rev,同词多码取最长全码)取单字全码后:

| 词长 | 取码 |
|---|---|
| 1 字 | 全码(等同学习加成) |
| 2 字 | 第 1、2 字各取前 2 码(你好 = wqiy+ vbg → wqvb) |
| 3 字 | 第 1、2 字各第 1 码 + 第 3 字前 2 码 |
| 4 字 | 每字第 1 码 |
| ≥5 字 | 第 1、2、3 字与末字各第 1 码 |

任一参与取码的字不在五笔码表 → 造词失败,提示「『x』不在五笔码表」,不入库。

### 12.3 配置与实现

- **配置**:`config.toml` 顶层 `coin_hotkey = "ctrl+equal"`(三端共用;
  写法=修饰(`ctrl/alt/super/shift`,**至少一个**,纯键热键与打字冲突一律拒绝)
  + 键名(单字符字面量/字母数字/f1–f24/常用名/0x keysym)。两宿主各自解析,
  规格一致:Mode A `lyyime.py parse_hotkey`,Mode B `keysym_map.c
  lyy_hotkey_parse`(均有单测);Mode B 设置窗提供输入框,保存即生效。
- **core**:`LKey::Coin` + `LKey::Arrow*` 进入/驱动造词状态机(Engine 持有
  最近 64 字上屏 CJK 历史);`Effect::Notice` 承载提示;两段式 plan/apply
  纪律不变(可写性预检在 plan 阶段,落盘在 apply 阶段)。
- **存储**:`user_words.tsv`(§4);造词同时计入学习加成(×1.5),排序词频
  取当前码表最大词频,保证同码首位。
- **宿主渲染**:预编辑 `造词:<选区>` + 单候选(注释=编码预览);Notice 走
  各自辅助区(Mode A auxiliary text 4 秒,Mode B 候选窗预编辑行 4 秒)。
- **测试**:core `tests/coin_test.rs`(15 例)+ FFI `ffi_造词_*`;
  Mode A `tests/unit_ibus_engine.py::CoinHotkeyTestCase`(桩库演示串
  "你好好吗");Mode B `xim/tests/unit_hotkey.c` + `unit_config.c` §13 +
  `xim_e2e.sh` 场景 D(ctrl+equal→Right→Return 全链路)。


## 13. 截屏助手(lyyime-shot,自定义快捷键,Ctrl+Alt+A 默认)

对标搜狗/QQ 截屏:热键拉起框选截屏,拖拽选区,图片存图片目录并复制
剪贴板,桌面通知给路径。实现为独立 Rust/GTK3 程序 `crates/lyyime-shot`
(`lyyime-shot`),输入法宿主只负责热键拦截与拉起 —— 截屏进程崩溃/缺失
均不影响输入法主流程。截图机制:`gdk_pixbuf_get_from_window(root)`
(X11 XGetImage);覆盖窗展示**先截好的**整屏快照(冻结画面)后框选,
避免自截(借鉴 GNOME Screenshot / flameshot 流程)。

### 13.1 交互合同(Mode A / Mode B 宿主必须一致实现)

| 输入 | 行为 |
|---|---|
| 截屏热键(默认 Ctrl+Alt+A,`shot_hotkey` 可配置) | 命中即吞键并拉起 `lyyime-shot`(不进组词缓冲、不经 core、放弃 /AI 会话);英文态:Mode A 中 ibus 激活期间按键必经引擎,中英态同效;Mode B 英文直通态(trigger off)按键不经 XIM 服务,该态下用托盘「截屏」菜单兜底 |
| 框选窗内 拖拽 | 选区(冻结画面上,选区外压暗、虚线边框、尺寸角标);抬起确认 |
| 框选窗内 Esc / 单击(无拖拽) | 取消(无产物,退出码 0) |
| 框选窗内 Enter / Space | 确认当前选区;无选区 = 整屏 |
| 双击 | 整屏(单击取消有 280ms 消歧定时) |
| 确认后 | PNG 存 `$LYYIME_SHOT_DIR` → `--dir` → `~/图片` → `~/Pictures` → `~`,文件名 `lyyIme_YYYYMMDD_HHMMSS.png`(同秒递增 `_1`);复制 CLIPBOARD;notify-send 通知;stdout 输出 `已保存:<路径>` |

### 13.2 配置与实现

- **配置**:`config.toml` 顶层 `shot_hotkey = "ctrl+alt+a"`(三端共用;
  写法同 §12 `coin_hotkey`,core 仅集中 schema 不消费)。
- **Mode A**:`keysym.rs parse_hotkey`(与 Mode B C 版同规格,单测各自
  覆盖)→ `logic.rs on_press` 在 BLOCKING 组合放行之前精确匹配(修饰位只比
  ctrl/alt/super/shift 四位,CapsLock/NumLock 不影响)→ `Host::on_shot` →
  `service.rs spawn_shot` 异步拉起,缺失时辅助区给安装指引;托盘菜单属性树
  (`root_property`)含「截屏」等全部入口(property_activate 同名分支)。
- **Mode B**:`shot.c lyy_spawn_shot`(helper 解析:$LYYIME_SHOT →
  /usr/local/bin → exe 同级 → PATH,与 AI helper 同思路;E2E 经
  `$LYYIME_SHOT` 注入桩)→ `xim_server.c handle_key_event` 在造词热键之前
  拦截;托盘工具菜单「截屏」;设置窗快捷键输入框保存即生效。
- **测试**:shot crate 单测(时间戳/目录链/参数解析)+ Mode A logic 单测
  (命中/吞键/中英态/配置非法)+ `xim/tests/unit_config.c` §14 +
  `shot_e2e.sh`(7 场景)+ `xim_e2e.sh` 场景 F + `run.sh` PASS A4(双模式
  热键→拉起桩全链路)。

### 13.3 快捷键冲突自动升级(造词/截屏通用,2026-09-06 新增)

快捷键与其它 lyyime 快捷键占用同一 `修饰+键` 组合时自动避让("自动滑向
下一级"),升级阶梯 = **原组合 → +Alt → +Alt+Shift**(已含的修饰自动跳过;
如 ctrl+equal → ctrl+alt+equal → ctrl+alt+shift+equal)。

- **判定**:规范化后比较 —— 别名归一(control≡ctrl、"="≡equal、
  mod4/win≡super)+ 修饰定序(ctrl+alt+super+shift)+ 键名标准形;
  写法非法的热键不参与冲突(宿主按各自合同回退默认)。
- **设置窗保存**(Mode B `settings.c on_ok`):写法非法 → 报错并还原该
  输入框,不保存;两键冲突时**刚改动的一侧**避让(两侧同改/都未改则截屏
  让位),最终值**直接回写输入框**(所见即所得)并弹窗说明;三级全占用 →
  报错还原该侧,请人工修改。
- **配置加载**(Mode A `read_core_config` / Mode B `main.c` 启动):冲突
  即自愈,**截屏热键让位**(工具键让位打字键)并写日志;设置窗打开时
  显示的即自愈后的生效值。
- **规格单源**:`lyyime-core src/hotkey.rs`(parse_hotkey / canon_hotkey /
  escalate_hotkey / resolve_config_hotkeys;Mode A `keysym.rs` 直接复用),
  Mode B C 镜像 `keysym_map.c lyy_hotkey_canon / lyy_hotkey_escalate` +
  `config.c lyy_config_resolve_hotkey_conflicts`,两端单测各自覆盖
  (core `hotkey` 用例、`unit_hotkey.c` §7/§8、`unit_config.c` §15)。

## 14. 快速功能键(触发词候选,数字/点选执行)

对标"输入特定词弹出功能入口"的效率玩法:输入缓冲与配置的触发词**完全相等**
时,候选条追加"功能候选"(注释固定为「功能键」),数字键/鼠标点选/空格确认
后宿主执行对应功能 —— **不上屏任何文本、不学习、不入造词历史**。

### 14.1 交互合同(Mode A / Mode B 宿主必须一致实现)

| 环节 | 行为 |
|---|---|
| 触发 | 缓冲 == 触发词(整串,非前缀)时候选追加功能候选;紧跟首选之后(无词库命中时置顶),多条触发词按配置顺序;普通候选永不功能键顶替,四码首选上屏/四码唯一上屏/四码顶屏对功能键候选不触发(缓冲等于触发词或其前缀时四码不自动上屏,功能必须经用户确认) |
| 展示 | 候选文本 = `label`,注释 = 「功能键」;总开关 `quick_actions_enabled` 关闭则整表不生效 |
| 执行 | 数字 1–9 / 鼠标点击候选行 / Space 确认首选 → core 回 `Effect::Action(i)`(i = 配置下标) + 清除效果流;宿主执行 `command`,不上屏文本 |
| 标点/Enter 收尾 | 功能键候选不作首选文本:退回原始字母直通(不会把 label 当文字打出) |
| command 语义 | `@settings` = 打开设置窗;`@shot` = 拉起截屏助手并提示「已拉起截屏(热键 …)」,热键取 `shot_hotkey` 配置(助手缺失时安装指引覆盖提示);`@help` = 辅助区帮助提示(两宿主同文案);其余按 `sh -c` 执行(异步,不阻塞按键流) |
| 候选注释 | 普通 = 「功能键」;`@shot` = 「功能键 热键:<shot_hotkey>」——候选阶段即可看到快捷方式(跟随配置) |

### 14.2 配置与实现

- **配置**(`config.toml`,三端共用;键名两端同名,吸取 §8 历史陷阱教训):
  顶层 `quick_actions_enabled = true` + `[[quick_actions]]` 数组表
  (`trigger` = 1–12 个小写字母 / `label` = 候选文本 / `command`)。缺省表 =
  peizhi(打开配置)/ shezhi(设置)/ jietu(截图,@shot)/ bangzhu(帮助),
  与 core `Config::default` 一致;
  条目上限 8(`QUICK_ACTIONS_MAX`/`LYY_QA_MAX`);非法触发词的条目剔除,
  全部非法回退默认表。core 侧非法配置判"配置损坏"(人话错误)。
- **core**:`types.rs` `CandKind::Action(u8)` + `Effect::Action(usize)`;
  `engine.rs` `insert_quick_actions`(重算后追加)与 `plan_select`
  (数字/空格/点选三路共用的选中分发);`config.rs` `QuickAction` 全链路
  (字段/校验/example_toml 回环);FFI 见 §3 可选符号组
  (`lyyime_set_quick_actions_enabled / clear / add / action_command /
  select_candidate`)。
- **Mode A**:`main.rs read_core_config` 增读 `quick_actions_enabled` 与
  `[[quick_actions]]`(非法条目跳过)→ `logic.rs` `Host::on_action` 回调 +
  `select_candidate`(点选)→ `service.rs` `run_quick_action`(执行)与
  `candidate_clicked`(ibus D-Bus 点选入口);托盘设置入口复用 `launch_setup`。
- **Mode B**:`config.c` `[[quick_actions]]` 块解析/原位重写(首个块位置
  重写、缺失追加文件尾;数组表头兼作顶层键插入点,保证合法 TOML)→
  `common.c lyy_engine_ensure` 注入(core FFI `qa_ok` 可选符号组,旧库缺失
  只禁用该功能)→ `xim_server.c` `LYY_EFF_ACTION` 分支 `lyy_run_quick_action`
  + `lyy_candwin_row_clicked`(候选窗行点击 → `lyyime_select_candidate`,
  与数字选词同一条效果流路径)→ 设置窗「快速功能键」复选框
  (`settings.ui chk_quick_actions`),保存即生效(引擎重建重注入)。
- **测试**:core `engine_test` 快速功能键 8 例(触发/置顶/不顶替/数字/点选/
  多条/开关/四码不直上/标点收尾)+ `ffi_test` 3 例(action JSON/点选 JSON/
  注入与开关)+ `config.rs` 回环 2 例;Mode A `logic.rs` 2 例;
  `xim/tests/unit_effects_json.c` §19(action 解析)+ `unit_config.c` §16
  (默认表/解析/剔除/保存回读)+ 桩库 §14 符号;`tests/e2e/run.sh` 双模式
  触发词→命令执行断言(marker 文件)。

## 15. 候选右键菜单(固定首位 / 删除词组 / 反查英文)

候选条上的词是"用户词汇资产"的可视部分:右键任何一行弹出操作菜单,
三个动作全部走 core 统一语义,持久化在用户数据目录,重启后依然生效。

### 15.1 交互合同(三端宿主必须一致实现)

| 环节 | 行为 |
|---|---|
| 唤起 | 鼠标右键(button=3)点击候选行;行下标 = 当前页内可见序(与数字选词同序) |
| 菜单项 | ①「固定首位」/「取消固定首位」(已固定的词标签取反)②「删除词组」③「反查英文」④「自定义查询」(**已配置才显示**,见 §15.5);功能键候选(§14 `CandKind::Action`)与页脚等非词项不弹菜单 |
| 固定首位 | 词+当前缓冲码记入 `pinned.tsv`;该码的候选重算中此词恒居第一、注释追加「固」;再点同一词「取消固定」回落正常排序 |
| 删除词组 | 词记入 `blocked.tsv`,任何编码下立即消失;用户词同删 `user_words.tsv`;**再造该词(§12 造词成功)自动解除屏蔽**——删除只针对当前词条不封锁词本身 |
| 反查英文 | 当前词查 `zh_en.tsv`;命中则候选区替换为英文释义页(普通候选,数字/点选正常上屏英文);不命中回 notice「没有英文反查结果」,候选原样保留 |
| 菜单后效果 | 操作完成后宿主拿到新的 effects JSON 刷新候选窗(固定/删除→过滤后的原候选页;反查→释义页) |

### 15.2 各端实现要点

- **core**(`wordops.rs` + `engine.rs`):`PinTable/BlockList/ZhEn` 三张持久表
  (临时文件+rename 原子替换,与 user_words 同目录);`Engine::cand_pinned /
  cand_op / plan_cand_op` 统一三动作;`recompute` 先过滤 blocked 再按 pinned
  置顶(仅缓冲恰等于所记 code 才置顶);`Effect::Candidates` 驱动宿主刷新。
  FFI 见 §3 `cand_pinned`/`cand_op` 可选符号组。
- **Mode A(ibus)**:`EngineLogic.cand_menu` 状态 + `cand_menu_open/exec/restore`;
  ibus 面板无弹出菜单 API,右键把候选区**整页替换为操作行**
  ([1]固定首位 [2]删除词组 [3]反查英文),数字 1–3 或再次点选执行、
  Esc/其他键还原;`service.rs candidate_clicked` 按 `button==3` 分派。
- **Mode B(xim)**:候选窗原每 80ms 跟随 `pointer+20,+30`,指针进不了窗口
  ——`candidate_window.c` 加悬停冻结(enter 停跟随、leave 恢复、hide 复位),
  `button-press-event` button=3 弹 `GtkMenu` 三项;回调经
  `xim_server.c → core_ffi.c` 的 `cand_pinned_ok/cand_op` 可选符号组进 core,
  效果 JSON 走与数字选词同一条 apply_effects 路径。
- **Mode C(float)**:候选按钮挂 `button-press-event`,button=3 弹 GTK 菜单;
  `ui.rs` 复用 core `wordops`(同一 user 目录文件,与引擎互见),
  `refresh_cands` 先滤 blocked 再按 pinned 置顶;反查直接把释义塞进候选行。

### 15.3 数据生成

`zh_en.tsv` 由 `lyyime-dicttool zhen <stardict.db> <out_dir>` 生成:ECDict
`translation` 字段抽取中文词段(剥词性/标点/释义括号)、跨词条去重、
按词频倒序截上限(默认 30 万行),与 `convert/fetch` 同一 `meta.json` 维护;
`dicttool verify` 视其为可选文件(缺失放行,存在则校验格式与排序),
`dicttool query --kind zhen <word>` 可查。

### 15.5 自定义查询(菜单第 4 项,宿主侧动作)

config.toml 顶层两键(统一设置窗「常规」页管理,保存即生效):

```toml
custom_query_label = "查词典"                            # 菜单显示名,空=「自定义查询」
custom_query_url = "https://www.baidu.com/s?wd={q}"      # {q}=候选词(百分号编码代入);空=菜单不显示此项
```

- **合同**:模板中全部 `{q}` 出现处替换为 RFC 3986 unreserved 规则百分号
  编码后的词;模板无 `{q}` 则原样打开。核心替换/编码逻辑在
  `lyyime_core::wordops::custom_query_url`(Rust 端共用;xim C 侧同语义)。
- **执行**:三端统一 `xdg-open <url>` 异步拉起浏览器,不阻塞按键流、
  不产生上屏、不动引擎状态(与 §14 快速功能键"宿主执行命令"同型,
  故不走 core `cand_op`/FFI)。
- **Mode B**:菜单第 4 项在 `candidate_window.c` 内部直接处理
  (`open_query_url`),不经 `op_fn`/IC —— 菜单 grab 期间照样可用;
  `lyy_candwin_set_query` 启动注入 + 设置保存再注入。
- **Mode C**:`config::load_custom_query()` 每次弹菜单时读 config.toml。
- **Mode A**:操作行第 4 行(已配置才出现);`Action::OpenUrl` 经
  service `open_query_url` 拉起;`focus_in` 热读(同 ai_cfg 纪律)。

### 15.6 测试

- core `tests/candops_test.rs`:置顶带「固」标记/取消回落/pinned.tsv 持久化
  重载、删除即隐/blocked.tsv 落盘/重启仍在、删除后再造词自动解屏蔽、
  反查命中出释义页并可选中上屏、无反查 notice、功能键候选拒绝操作;
  `ffi_test.rs` 增 `cand_pinned`/`cand_op` JSON 与两段式缓冲语义。
- dicttool `zhen.rs` 单测:词段抽取/词性标点剥离/去重排序/上限截断;
  `verify.rs` zh_en 存在与缺失两路。
- Mode A `logic.rs` 单测:操作行展示/数字执行/Esc 还原/功能行不弹菜单。
- Mode B `xim_e2e.sh` 场景 G:Xvfb 真鼠标——悬停冻结后右键弹 GTK 菜单,
  指针点击依次验证反查(hello 上屏)/删除(首行变 你号)/固定(拟好置顶)
  /自定义查询(桩 xdg-open 断言 {q} 代入网址)。
