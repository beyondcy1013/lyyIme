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
    Char(char),          // 小写字母 a–z(宿主负责小写化)
    Digit(u8),           // '1'..'9' → 1..9
    Space, Enter, Backspace, Esc, PageUp, PageDown,
    Punct(char),         // 标点原字符(半角)
    ShiftPress,          // Shift 按下(宿主用于单击检测,见 §6)
    Other,               // 其余:core 恒回 Effect::Pass
}

#[derive(Clone, Debug)]
pub enum Effect {
    Commit(String),               // 上屏文本(UTF-8),宿主负责注入;core 已清缓冲
    Preedit(Option<String>),      // 预编辑串更新(None=清除)
    Candidates(Arc<Vec<Candidate>>), // 当前候选页更新(含总页数信息)
    Pass,                         // 宿主原样放行该键(英文态字母、未识别键)
    Consumed,                     // 吞掉但不产生可见效果
    ModeChanged(Mode),            // 宿主更新 UI 指示
}

#[derive(Clone, Debug)]
pub struct Candidate { pub text: String, pub comment: String, pub score: f32, pub kind: CandKind }
pub enum CandKind { Wubi, Pinyin, English, User }
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
                  {"t":"pass"},{"t":"consumed"},{"t":"mode","m":1}]
   cands 的具体候选另取:lyyime_cand(eng, i, buf, cap) 返回候选文本,-needed 表示不足;
   lyyime_cand_comment 同理。 */
int64_t lyyime_process_key(void* eng, int key_id, uint32_t chr, char* buf, int64_t buf_cap);
int   lyyime_cand(void* eng, int i, char* buf, int cap);
int   lyyime_cand_comment(void* eng, int i, char* buf, int cap);
```

`key_id` 枚举(python 侧同样常量):`LKEY_CHAR=0, LKEY_DIGIT=1, LKEY_SPACE=2, LKEY_ENTER=3, LKEY_BACKSPACE=4, LKEY_ESC=5, LKEY_PAGEUP=6, LKEY_PAGEDOWN=7, LKEY_PUNCT=8, LKEY_SHIFTPRESS=9, LKEY_OTHER=10`。

`LKEY_SHIFTPRESS` 表示 **Shift 按下**:core 对有缓冲组合先上屏英文原串;空缓冲回 Consumed,由宿主继续做 Shift 单击判定。四码顶屏为可选项:配置 `commit_on_extra_after_four = true` 时,恰好四码且已有候选,再输入字母先上屏当前选中,该字母开启新组合;默认关闭,保持前缀渐进组词。

**chr 传值约定(v1.1 实现期确认)**:`chr` 携带 Char/Punct/**Digit** 的码点——Digit 传 `'1'..'9'`(ASCII 0x31..0x39),core 按 `chr-'0'` 取值;其余 key_id 填 0。**有状态纪律**:`lyyime_process_key` 必须先生成完整 effects JSON、确认写入容量足够后才落内部状态变更,保证宿主因 `-needed` 扩容重试时同一键不会二次生效。

## 4. 数据文件格式(data/runtime/,UTF-8 TSV,dicttool 产物)

| 文件 | 列 | 说明 |
|---|---|---|
| `wubi.tsv` | `code\tword\tfreq` | 五笔码→词;含单字全码/简码与词组;按 code 升序、freq 降序 |
| `pinyin_char.tsv` | `pinyin\tchar\tfreq` | 全拼(无声调)→单字 |
| `pinyin_phrase.tsv` | `word\tpinyin\tfreq` | 词组、无声调全拼(音节空格分隔)、词频 |
| `english.tsv` | `word\tfreq` | 小写英文词+词频,≥1万行 |
| `suggestion.tsv` | `word\tfreq` | 通用词频(排序兜底) |
| `meta.json` | `{"version":1,"source":...,"rows":...,"built_at":...}` | 校验信息 |

用户数据:`~/.local/share/lyyime/user.tsv`,格式 `word\tfreq_extra\tlast_used_epoch`;core 定期(每 64 次 commit/退出时)批量落盘。

配置:`~/.config/lyyime/config.toml`(doctor/app/ibus 共用;字段见 core `Config` 默认值,注释中文)。

## 5. 匹配与排序算法(v1)

1. 缓冲区 = 连续小写字母(≤12),Backspace 删尾。
2. **五笔通道**:查 wubi.tsv,`code == buf` 完全命中优先;`code 是 buf 前缀` 给渐进候选;取每码频次前 9。
3. **拼音通道**:对 buf 做音节切分(DP,音节表取自 pinyin_char 去重),允许末音节不完整;查 pinyin_phrase(词组)与 pinyin_char(单字);简拼(每音节首字母,≥2 键)低权重参与。
4. **英文通道**(修订版细则见第 5 条末"英文通道修订"):无中文命中 → 前缀候选;buffer 是完整高频英文词 → 即使有中文命中也入选;直通上屏按"无中文命中 或 top-500 词"门控。
5. **合并排序(v1.1,真实词库集成后修订——层级主导词典序)**:排序键为**词典序 (tier, specificity, freq_norm)**——层间不可跨越,层内先按 specificity(词组音节数放大、完整片段>更短片段、完全同码英文词 +0.5,用于压制伪切分),再按 `freq_norm ∈ [0,10)`。实现详见 `crates/lyyime-core/src/rank.rs`(模块文档含层级表)。设计动机:各源频率尺度差 4 个数量级以上,加性权重会被大频值跨层碾压。
   | 层 | tier_base | 说明 |
   |---|---|---|
   | wubi_exact | 60 | code==buffer(简码奖励并入此层) |
   | english_no_cn | 55 | 无任何中文命中时的英文候选 |
   | pinyin_full | 50 | 完整音节切分全命中(词组/单字),同等切分数优先覆盖更多完整音节 |
   | wubi_prefix | 40 | wubi 前缀渐进 |
   | pinyin_partial | 35 | 仅末音节不完整 |
   | english_with_cn | 30 | buffer 是完整高频英文词但存在中文命中 |
   | pinyin_abbrev | 20 | 简拼 |
   `freq_norm = log10(1+freq) / log10(1+maxf_of_file) × 10`(每文件独立归一,加载时缓存 max)。设计动机:各源频率尺度差 4 个数量级以上(拼音单字 1e9 vs 词组 1e3),加性权重会被大频值跨层碾压;层级化后同层内同源尺度自然一致。用户词加成为层内 freq 乘子(×1.5)。输出前 page_size 条。
   英文通道修订:完整英文词 ∈ english.tsv 前 mixed_auto_commit_top_n(默认 500)时即使有中文命中也入选(english_with_cn);∈ 前 en_freq_top_n(2000)且无中文命中时进 english_no_cn。**中文态下英文词只作为候选展示;Space 是确认键,始终顶屏当前选中,英文输出先 Shift 单击切换到英文态。**
6. **学习**:commit 候选词 → freq_extra += 1,重排时乘 user 权重。

## 6. 按键行为规范(宿主必须一致实现)

| 输入 | 行为 |
|---|---|
| a–z | 缓冲,更新 preedit/候选 |
| 1–9 | 有候选:选第 N 个上屏;无候选:Pass(数字原样) |
| Space | **任何时候都确认当前选中项**:有候选顶屏首选;有缓冲无候选 Commit(原字母);无缓冲 Pass(空格) |
| Enter | 有缓冲:Commit(原字母);无缓冲:Pass |
| Backspace | 有缓冲删尾;空:Pass |
| Esc | 清缓冲(Consumed);空:Pass |
| `-`/`=` | 有候选翻页(Consumed);否则 Pass |
| 标点 | 中文态空缓冲→Commit(对应中文标点);有缓冲→Commit(首选)+Commit(中文标点);英文态 Pass |
| Shift 按下 | 有缓冲:**Commit(原字母)**(英文原串);空缓冲:Consumed 并进入单击检测 |
| Shift 单击(空缓冲) | toggle_mode + ModeChanged(单击=按下后未产生其它键即释放,且无其它修饰) |
| 其它键 | Pass(有缓冲时先 reset) |

中文标点映射(可配置):`,.?!;:'"()[]{}` → `,.?!;:''""()【】{}` 等,默认集在 core `punct.rs`。

## 7. Mode A:ibus python 引擎

- 位置(安装后):`/usr/local/share/lyyime/ibus/engine/lyyime.py` + component XML `/usr/local/share/ibus/component/lyyime.xml`(engine name `lyyime`,symbol `伍`)。
- `lyyime.py`:标准 `IBus.Engine` 子类;`do_process_key_event` 把 keysym/state 映射为 `LKey`(在 ctypes 边界小写化字母、区分 Shift 释放序列)→ `lyyime_process_key` → 依 JSON 效果流调用 `commit_text / update_preedit_text / update_lookup_table / page_up|down`。
- 数字键选词、`-`/`=` 翻页由 core 返回 Candidates 后的 lookup table 承载;`Pass` 的键返回 `False` 让 ibus 放行。
- 托盘属性菜单:开关中英 / 设置(拉起 `lyyime-app --settings`)/ 工具与修复(拉起 `lyyime-doctor --gui`)。
- 崩溃隔离:engine 异常时退化为英文直通并打日志 `~/.local/share/lyyime/logs/ibus.log`。

## 8. Mode B:lyyime-xim 独立外挂(X11,XIM server 路线)

> 架构决策依据见 RESEARCH.md §2(本机实证:被动 grab 回放不可靠,XIM 是正统路线)。
> 二进制 `lyyime-xim`(C 语言:IMdkit + GTK3 + dlopen(liblyyime_core.so) C ABI;IMdkit vendor 进仓库)。

- **接入**:应用设置 `XMODIFIERS=@im=lyyime`(doctor 的 Mode B profile 负责写入并重启会话应用);GTK3 内建 xim immodule / Xlib 应用原生接入。覆盖 GTK3+Xlib+XIM 类终端;Qt5 走 Mode A(互补全覆盖)。
- **按键流**(root-window style):trigger on 后,XIM forward event → 映射 LKey → core.process_key → 效果流:Preedit/Candidates 画进自绘候选窗;Commit → `IMCommitString`(任意 Unicode);Pass 类键 → `IMForwardEvent`(协议级原样回放,零风险)。
- **Shift 单击切换** = XIM trigger off/on:off 后应用直接收键(英文态),再 on 恢复中文态;由 XIM 协议原生保证,无任何 hack。
- **候选窗**:GTK3 override-redirect、无边框、accept_focus(false),跟随光标(root style 下用 XQueryPointer);序号高亮首选、编码提示、翻页指示,样式对齐主流输入法。
- **托盘**:Gtk.StatusIcon(XEmbed,兼容 xfce4-panel):状态(中/EN)+ 右键菜单:启用/停用、模式、设置、工具(修复输入法 / 输入法管理(增删其它输入法、设默认,exec `lyyime-doctor` CLI 并解析 JSON,危险操作 GTK 确认对话框)、重载词库、日志)、退出。
- **设置窗**:GTK3(GtkBuilder .ui),读写 ~/.config/lyyime/config.toml,保存即 set_config 生效;含 Mode B 专属项(XMODIFIERS 一键切换到 lyyime/恢复 ibus)。
- **探路石前置**:先交付"最小 XIM server + GTK3 Entry 连通"spike(Xvfb 实证),通过后才铺全量;失败则升级为自带 GTK immodule 方案并回报主控。
- 设置窗:GTK3,读写 config.toml,保存即生效(core set_config)。

## 9. lyyime-doctor(工具菜单核心)

`check` 项(输出 JSON+文本):XMODIFIERS/GTK_IM_MODULE/QT_IM_MODULE 是否指向运行中的框架;ibus-daemon 存活;lyyime 引擎组件 XML 是否就位(ibuscache);autostart 项;GTK2/3 与 Qt 的 immodule 文件存在性;字体缓存;`~/.local/share/lyyime` 数据完整性;最近日志错误。
`fix`(幂等,可 `--dry-run`):写会话环境(Xfce: ~/.xprofile + xfconf)、`ibus write-cache` + 重启 ibus-daemon、重建 autostart、重装/重转词库、清 GTK immodule 缓存。GUI 由 lyyime-app 工具菜单嵌入(列表+一键修复)。

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
| doctor | 破坏环境→check 发现→fix 恢复 | tests/e2e/doctor_test.sh |
