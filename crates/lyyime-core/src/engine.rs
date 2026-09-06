//! 输入引擎:按键行为状态机 + 候选生成与排序(合同 §5/§6,§3 v1.1 重试纪律)。
//!
//! 宿主只做三件事:把真实按键翻译成 [`LKey`](一般需先小写化字母)、
//! 按 [`Effect`] 顺序渲染/上屏、焦点切换时调 [`Engine::reset`]。
//!
//! 行为速查(细则见 ARCHITECTURE.md §6):
//! - a–z:进缓冲,更新 preedit/候选(≤12 字母;英文态直通);
//! - 1–9:选当前页第 N 个候选;无候选放行数字;越界吞掉;
//! - Space:有候选顶屏首选;有缓冲无候选直通原字母;空缓冲放行;
//! - Enter:有缓冲上屏原始字母;空缓冲放行;
//! - Backspace:删尾;Esc:清缓冲;`-`/`=`(PageUp/PageDown):翻页;
//! - 标点:中文态空缓冲出中文标点;有缓冲先上屏首选再补中文标点;英文态放行;
//! - 四码唯一上屏(可配置):恰好凑满四码且候选唯一时,免空格直接上屏;
//! - 词组提示(可配置):上屏后最近几字有更省键的五笔词组时,效果流在
//!   清除类效果之后追加 [`Effect::Hint`](候选条展示,下一次输入才清除);
//! - 快速功能键(可配置,合同 §14):缓冲与触发词完全相等时候选条追加
//!   功能候选(`CandKind::Action`,紧跟首选之后);数字/鼠标/空格选中产生
//!   [`Effect::Action`](宿主执行功能,不上屏文本、不学习),普通候选
//!   不会被功能键顶替首选位置;
//! - ShiftPress:有缓冲上屏英文原串;空缓冲吞键,宿主判定单击后调
//!   [`Engine::toggle_mode`];
//! - 其它键:Pass(有缓冲先清缓冲)。
//!
//! ## 两段式按键处理(合同 §3 v1.1 重试纪律)
//!
//! `process_key` = [`Engine::plan_key`](纯读取,产出效果流与下一状态 Plan)
//! + [`Engine::apply_plan`](落内部状态)。FFI 层先把效果流 JSON 写入宿主缓冲,
//! **确认写得下才 apply**;返回 -needed 时内部状态保持不变,宿主扩容重试同一键
//! 不会二次生效。Rust 宿主走 `process_key` 时两步合并,行为不变。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::dict::{suggestion_of, DictIndex};
use crate::learner::Learner;
use crate::pinyin;
use crate::punct::{to_chinese, QuoteState};
use crate::rank;
use crate::types::{CandKind, Candidate, Effect, LKey, Mode};
use crate::user_words::UserWords;

/// 缓冲区长度上限(合同 §5.1:连续小写字母,≤12)。
pub(crate) const MAX_BUF: usize = 12;
/// 单次排序候选池上限(合并后截断,页数不至于失控)。
const MAX_CANDS: usize = 50;
/// 完整切分枚举条数上限(防极端组合爆炸)。
const SEGS_CAP: usize = 64;
/// 造词历史容量:保留最近上屏的 CJK 字符数(合同 §12)。
const RECENT_CAP: usize = 64;
/// 造词一次选取的长度上限(词组编码取首末字,过长无意义)。
const MAX_COIN_LEN: usize = 32;
/// 词组提示回看的后缀长度上限(五笔词组常见 2–4 字,≥5 字收益低)。
const PHRASE_HINT_MAX: usize = 6;
/// 实耗键数定点系数:多字同屏时按键数均摊到字(×16 定点,u16 足够)。
const COST_FIXED: u32 = 16;

/// 排序前的原始候选(内部结构)。
///
/// 排序按 (tier, spec, norm) 词典序——层间不可跨越(修订版 §5);
/// `score = tier + norm` 仅在 finalize 时算出,作为对外展示值。
struct RawCand {
    text: String,
    comment: String,
    tier: f32,
    /// 同层排序因子(完整音节偏好):匹配越"具体"(音节多/片段长)越靠前。
    spec: f32,
    /// 同文件 log10 归一 ×10;用户词在 finalize 时 ×1.5(允许层内溢出)。
    norm: f32,
    /// 展示值 = tier + norm(finalize 时填写;排序用词典序,不用该值)。
    score: f32,
    kind: CandKind,
    sug: u64,
}

/// 向候选池插入一条候选:同词保留 (tier, spec, norm) 词典序更大者。
fn insert_cand(
    pool: &mut HashMap<String, RawCand>,
    text: String,
    comment: String,
    tier: f32,
    spec: f32,
    norm: f32,
    kind: CandKind,
    sug: u64,
) {
    if let Some(e) = pool.get_mut(&text) {
        if (tier, spec, norm) > (e.tier, e.spec, e.norm) {
            e.tier = tier;
            e.spec = spec;
            e.norm = norm;
            e.comment = comment;
            e.kind = kind;
        }
        if sug > e.sug {
            e.sug = sug;
        }
        return;
    }
    pool.insert(
        text.clone(),
        RawCand {
            text,
            comment,
            tier,
            spec,
            norm,
            score: 0.0,
            kind,
            sug,
        },
    );
}

/// 一次按键的计划:按键生效后的全部可变状态。
///
/// 由 [`Engine::plan_key`] 纯读取产出,经 [`Engine::apply_plan`] 落地;
/// FFI 层借此实现"JSON 写得下才生效"的有状态重试纪律。
pub(crate) struct Plan {
    pub(crate) buf: String,
    pub(crate) cands: Vec<Candidate>,
    pub(crate) page: usize,
    pub(crate) cn_hit: bool,
    pub(crate) quotes: QuoteState,
    /// 需要学习的上屏词(来自候选顶屏/选词/英文直通;原始字母直通不学习)。
    learned: Option<String>,
    /// 造词:最近上屏 CJK 历史(合同 §12;仅 commit 含汉字时增长)。
    pub(crate) recent: Vec<char>,
    /// 与 `recent` 一一对应的实耗键数(定点 ×[`COST_FIXED`];多字同屏均摊,
    /// 供词组效率提示比较"词组编码 < 实敲键数")。
    pub(crate) recent_cost: Vec<u16>,
    /// 造词:最近一次 commit 贡献的连续汉字数(Ctrl+= 的初始选长)。
    pub(crate) last_run: usize,
    /// 造词模式当前选取的尾部字长;None = 不在造词模式。
    pub(crate) coin: Option<usize>,
    /// 待入库的用户造词(word, code);apply 时写入造词库并落盘。
    pub(crate) user_word: Option<(String, String)>,
}

impl Plan {
    fn unchanged(eng: &Engine) -> Self {
        Self {
            buf: eng.buf.clone(),
            cands: eng.cands.clone(),
            page: eng.page,
            cn_hit: eng.cn_hit,
            quotes: eng.quotes,
            learned: None,
            recent: eng.recent.clone(),
            recent_cost: eng.recent_cost.clone(),
            last_run: eng.last_run,
            coin: eng.coin,
            user_word: None,
        }
    }
}

/// lyyIme 输入引擎(Send:宿主可能跨线程持有,但所有 API 非线程安全,需自行加锁)。
pub struct Engine {
    dict: DictIndex,
    cfg: Config,
    mode: Mode,
    buf: String,
    /// 本次缓冲的全量排序候选(当前页为其中一片)。
    cands: Vec<Candidate>,
    page: usize,
    /// 当前缓冲是否存在中文通道命中(决定英文通道与 auto_commit 门控)。
    cn_hit: bool,
    learner: Learner,
    quotes: QuoteState,
    /// 最近上屏 CJK 历史(造词原料,合同 §12;焦点切换不清,跨窗口即失效)。
    recent: Vec<char>,
    /// 与 recent 一一对应的实耗键数(词组效率提示用,见 [`Plan::recent_cost`])。
    recent_cost: Vec<u16>,
    /// 最近一次 commit 贡献的连续汉字数(造词初始选长)。
    last_run: usize,
    /// 造词模式:Some(选长) = 进行中。
    coin: Option<usize>,
    /// 用户造词库(独立于 learner 的显式词,合同 §12)。
    user_words: UserWords,
}

impl Engine {
    /// 创建引擎并从 `data_dir` 加载词库。
    ///
    /// 目录不存在/为空时不报错:得到一个"空引擎"(所有查询无候选,
    /// 但按键状态机、标点、直通行为照常),可用 [`Engine::is_loaded`] 判断。
    pub fn new(data_dir: &Path) -> Result<Self, crate::Error> {
        let mut dict = DictIndex::load(data_dir);
        let cfg = Config::default();
        let path = cfg
            .user_dict
            .clone()
            .unwrap_or_else(Config::default_user_dict_path);
        let mut learner = Learner::new(cfg.learning, path);
        if cfg.learning {
            learner.load();
        }
        // 造词库与用户词典同目录(user_words.tsv),启动即并入五笔索引。
        let mut user_words = UserWords::new();
        user_words.load(&mut dict, &user_words_path(&cfg));
        Ok(Self {
            dict,
            cfg,
            mode: Mode::Chinese,
            buf: String::new(),
            cands: Vec::new(),
            page: 0,
            cn_hit: false,
            learner,
            quotes: QuoteState::new(),
            recent: Vec::new(),
            recent_cost: Vec::new(),
            last_run: 0,
            coin: None,
            user_words,
        })
    }

    /// 覆盖配置(页大小/混合开关/学习开关/用户词典路径等)。
    ///
    /// 注意:若 `cfg.mode` 与当前模式不同,视同一次模式切换——清空组合缓冲
    /// (config 的语义是"启动默认模式");切换用户词典路径会清空旧数据并重新装载。
    pub fn set_config(&mut self, cfg: Config) {
        let path = cfg
            .user_dict
            .clone()
            .unwrap_or_else(Config::default_user_dict_path);
        self.learner.set_path(path);
        self.learner.set_enabled(cfg.learning);
        // 造词库随用户词典目录走:目录变化才重装载(造的词跟着数据目录迁移)。
        self.user_words.set_path_and_load(&mut self.dict, user_words_path(&cfg));
        if cfg.mode != self.mode {
            self.mode = cfg.mode;
            self.clear_buf();
        }
        self.cfg = cfg;
    }

    /// 当前配置快照。
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// 词库是否加载到了数据(任一通道非空)。
    pub fn is_loaded(&self) -> bool {
        !self.dict.is_empty()
    }

    /// 当前模式。
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// 切换中/英模式,返回新模式;同时清空组合缓冲,避免旧缓冲在新模式下残留。
    ///
    /// "Shift 单击"的按下/释放序列判定由宿主完成,确认是单击后再调用本方法。
    pub fn toggle_mode(&mut self) -> Mode {
        self.mode = match self.mode {
            Mode::Chinese => Mode::English,
            Mode::English => Mode::Chinese,
        };
        self.clear_buf();
        self.mode
    }

    /// 清空组合缓冲与候选(焦点切换时宿主调用)。
    pub fn reset(&mut self) {
        self.clear_buf();
    }

    /// 当前组合缓冲(原始字母,无缓冲为空串)。
    pub fn buffer(&self) -> &str {
        &self.buf
    }

    /// 当前页页码(0 起)。
    pub fn page(&self) -> usize {
        self.page
    }

    /// 总页数(无候选为 0)。
    pub fn page_count(&self) -> usize {
        Self::pages_for(self.cands.len(), self.page_size())
    }

    /// 给定候选数与页大小求总页数(FFI 规划态与引擎态共用)。
    pub(crate) fn pages_for(len: usize, page_size: usize) -> usize {
        let ps = page_size.max(1);
        if len == 0 {
            0
        } else {
            (len + ps - 1) / ps
        }
    }

    /// 当前页候选(排序后切片)。
    pub fn flush_page(&self) -> &[Candidate] {
        self.page_slice()
    }

    /// 处理一个抽象键,返回按宿主处理顺序排列的效果流。
    ///
    /// 内部为 plan + apply 两段;Rust 宿主调用即生效。
    pub fn process_key(&mut self, key: LKey) -> Vec<Effect> {
        let (effects, plan) = self.plan_key(key);
        self.apply_plan(plan);
        effects
    }

    /// 直接选中当前页第 `idx`(0 起)个候选上屏;越界吞掉。
    pub fn select_candidate(&mut self, idx: usize) -> Vec<Effect> {
        let (effects, plan) = self.plan_select_candidate(idx);
        self.apply_plan(plan);
        effects
    }

    /// 点选候选的规划态版本(FFI 两段式重试纪律,合同 §3)。
    pub(crate) fn plan_select_candidate(&self, idx: usize) -> (Vec<Effect>, Plan) {
        // 造词模式的单条候选仅用于展示选区与编码预览,不提供点选上屏。
        if self.coin.is_some() {
            return (vec![Effect::Consumed], Plan::unchanged(self));
        }
        let mut plan = Plan::unchanged(self);
        let effects = match plan_page_slice(self, &plan).get(idx).cloned() {
            None => vec![Effect::Consumed],
            Some(c) => plan_select(self, &mut plan, &c, true),
        };
        (effects, plan)
    }

    /// 把内存累计的用户词立即落盘(write-behind 之外的显式 flush,合同 §4"退出时")。
    pub fn flush_user_dict(&mut self) -> Result<(), crate::Error> {
        self.learner.save()
    }

    // ------------------------------------------------------------------
    // 按键计划(纯读取)与落地
    // ------------------------------------------------------------------

    /// 规划一次按键:不修改任何内部状态,返回效果流与生效后的状态。
    ///
    /// FFI 层据此实现重试纪律:效果流 JSON 写不进宿主缓冲(-needed)时不 apply,
    /// 内部状态保持按键前,宿主扩容重试同一键只生效一次。
    pub(crate) fn plan_key(&self, key: LKey) -> (Vec<Effect>, Plan) {
        // 英文态:除 ShiftPress(由宿主判定单击)外一律直通。
        if self.mode == Mode::English {
            return match key {
                LKey::ShiftPress => (vec![Effect::Consumed], Plan::unchanged(self)),
                _ => (vec![Effect::Pass], Plan::unchanged(self)),
            };
        }
        let mut p = Plan::unchanged(self);
        let effects = match key {
            LKey::Coin => self.plan_coin_start(&mut p),
            _ if self.coin.is_some() => self.plan_coin_key(&mut p, key),
            LKey::Char(c) => self.plan_char(&mut p, c),
            LKey::Digit(n) => self.plan_digit(&mut p, n),
            LKey::Space => self.plan_space(&mut p),
            LKey::Enter => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    // 合同 §6:Enter 上屏原始字母。
                    let raw = p.buf.clone();
                    plan_commit(self, &mut p, &raw, false)
                }
            }
            LKey::Backspace => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    p.buf.pop();
                    p.page = 0;
                    self.recompute_into(&mut p);
                    composition_effects(self, &p)
                }
            }
            LKey::Esc => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    plan_clear(&mut p);
                    vec![Effect::Preedit(None), empty_cands(), Effect::Consumed]
                }
            }
            LKey::PageUp | LKey::PageDown => self.plan_page(&mut p, key == LKey::PageDown),
            LKey::Punct(c) => self.plan_punct(&mut p, c),
            LKey::ShiftPress => {
                // Shift 按下时的用户规则:有缓冲立即上屏英文原串;
                // 空缓冲由宿主判定 Shift 单击后切换中英模式。
                if p.buf.is_empty() {
                    vec![Effect::Consumed]
                } else {
                    let raw = p.buf.clone();
                    plan_commit(self, &mut p, &raw, false)
                }
            }
            LKey::Other => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    plan_clear(&mut p);
                    vec![Effect::Preedit(None), empty_cands(), Effect::Pass]
                }
            }
            // 非造词模式的方向键:无组词语义,行为同 Other(有缓冲先清缓冲)。
            LKey::ArrowLeft | LKey::ArrowRight | LKey::ArrowUp | LKey::ArrowDown => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    plan_clear(&mut p);
                    vec![Effect::Preedit(None), empty_cands(), Effect::Pass]
                }
            }
        };
        (effects, p)
    }

    /// 把计划落为引擎内部状态(FFI 在 JSON 确认写入后调用)。
    pub(crate) fn apply_plan(&mut self, plan: Plan) {
        self.buf = plan.buf;
        self.cands = plan.cands;
        self.page = plan.page;
        self.cn_hit = plan.cn_hit;
        self.quotes = plan.quotes;
        self.recent = plan.recent;
        self.recent_cost = plan.recent_cost;
        self.last_run = plan.last_run;
        self.coin = plan.coin;
        if let Some((word, code)) = plan.user_word {
            // 内存索引即时生效;落盘失败不回滚(plan 阶段已做可写预检,
            // 此处属罕见竞态),保 dirty 让下次造词重试。
            if let Err(e) = self.user_words.add(&mut self.dict, &word, &code) {
                eprintln!("lyyime-core: {e}");
            }
        }
        if let Some(word) = plan.learned {
            self.learner.record(&word);
        }
    }

    fn plan_char(&self, p: &mut Plan, c: char) -> Vec<Effect> {
        // 宿主负责小写化;大写/非字母防御性直通。
        if !c.is_ascii_lowercase() {
            return vec![Effect::Pass];
        }
        if p.buf.chars().count() >= MAX_BUF {
            // 缓冲已满:吞掉,维持现有组合(合同 §5.1 ≤12)。
            return vec![Effect::Consumed];
        }
        // 可选顶屏:恰好四码且已有候选时,再来的字母先确认当前选中,
        // 该字母开启新组合。关闭后保持前缀渐进组词。
        // 功能键候选不作顶屏确认(避免拼到一半误触发功能),继续缓冲。
        if self.cfg.commit_on_extra_after_four && p.buf.chars().count() == 4 {
            if let Some(top) = plan_page_slice(self, p).first() {
                if !matches!(top.kind, CandKind::Action(_)) {
                    let text = top.text.clone();
                    let mut effects = plan_commit(self, p, &text, true);
                    plan_clear(p);
                    p.buf.push(c);
                    p.page = 0;
                    self.recompute_into(p);
                    effects.extend(composition_effects(self, p));
                    return effects;
                }
            }
        }
        p.buf.push(c);
        p.page = 0;
        self.recompute_into(p);
        // 四码唯一上屏(可配置,借鉴搜狗/QQ 五笔的"四码唯一自动上屏"):
        // 恰好凑满四码且合并排序后的候选唯一时,免空格直接上屏。
        // 多候选(拼音/英文通道有共存候选)不触发,避免劫持混打渐进输入;
        // 唯一性按完整候选列表判定,与空格顶屏的选择一致(免按空格而已)。
        // 唯一候选是功能键时同样不触发:功能须经用户确认(数字/点选/空格)。
        if self.cfg.commit_unique_four
            && p.buf.chars().count() == 4
            && p.cands.len() == 1
            && !matches!(p.cands[0].kind, CandKind::Action(_))
        {
            let text = p.cands[0].text.clone();
            return plan_commit(self, p, &text, true);
        }
        composition_effects(self, p)
    }

    fn plan_digit(&self, p: &mut Plan, n: u8) -> Vec<Effect> {
        let page = plan_page_slice(self, p);
        if page.is_empty() {
            // 无候选:数字原样放行(合同 §6)。
            return vec![Effect::Pass];
        }
        if n == 0 {
            return vec![Effect::Consumed];
        }
        let idx = (n - 1) as usize;
        if idx >= page.len() {
            // 越界:吞掉,避免数字被注入到正在组合的文本流中。
            return vec![Effect::Consumed];
        }
        let cand = page[idx].clone();
        plan_select(self, p, &cand, true)
    }

    fn plan_space(&self, p: &mut Plan) -> Vec<Effect> {
        if p.buf.is_empty() {
            return vec![Effect::Pass];
        }
        // Space 是确认键:只要有候选,一律上屏当前选中(首选),不做英文抢占。
        // 英文输入走 Shift 单击切换后的英文直通态;无候选时仍保留原字母兜底。
        // 选中首选是功能键时同样产生 Action 效果(空格=确认首选)。
        if let Some(top) = plan_page_slice(self, p).first().cloned() {
            plan_select(self, p, &top, true)
        } else {
            // 有缓冲无候选:直通原始字母(中英混合,合同 §6)。
            let raw = p.buf.clone();
            plan_commit(self, p, &raw, false)
        }
    }

    fn plan_page(&self, p: &mut Plan, down: bool) -> Vec<Effect> {
        let pages = Self::pages_for(p.cands.len(), self.page_size());
        if p.cands.is_empty() || pages <= 1 {
            return vec![Effect::Pass];
        }
        if down {
            p.page = (p.page + 1).min(pages - 1);
        } else {
            p.page = p.page.saturating_sub(1);
        }
        vec![
            Effect::Candidates(Arc::new(plan_page_slice(self, p).to_vec())),
            Effect::Consumed,
        ]
    }

    fn plan_punct(&self, p: &mut Plan, c: char) -> Vec<Effect> {
        let mut quotes = p.quotes;
        let punct = to_chinese(c, &mut quotes);
        if punct.is_some() {
            p.quotes = quotes;
        }
        let Some(punct) = punct else {
            // 未映射标点:有缓冲先提交组合再放行,空缓冲直接放行。
            return self.plan_commit_then(p, Effect::Pass);
        };
        if !self.cfg.cn_punct {
            // 用户关闭中文标点:一律直通(有缓冲同样先提交组合)。
            return self.plan_commit_then(p, Effect::Pass);
        }
        if p.buf.is_empty() {
            // 中文态空缓冲:输出对应中文标点(合同 §6)。
            return vec![Effect::Commit(punct.to_string())];
        }
        // 有缓冲:Commit(首选)+ Commit(中文标点)。
        let text = self.finish_text(p);
        let learned = self.learn_worthy(p);
        let mut effects = plan_commit(self, p, &text, learned);
        effects.insert(1, Effect::Commit(punct.to_string()));
        effects
    }

    // ------------------------------------------------------------------
    // 造词模式(合同 §12:自定义快捷键 Ctrl+= 默认,方向键增减选字)
    //
    // 交互:中文态空缓冲按造词热键 → 选取最近上屏的连续汉字(初始为最近
    // 一次 commit 的汉字串);→/↑ 多选一字,←/↓/退格 少选一字;Enter/Space
    // 按五笔86 词组取码规则自动编码并存入 user_words.tsv;Esc 取消。
    // 交互习惯借鉴极点五笔/万能五笔的自造词(Ctrl+= 造词)。
    // ------------------------------------------------------------------

    /// 计划态选区文本:最近 `n` 个上屏汉字。
    fn plan_coin_text(&self, p: &Plan, n: usize) -> String {
        let start = p.recent.len().saturating_sub(n);
        p.recent[start..].iter().collect()
    }

    /// 造词模式的效果流:预编辑展示选区 + 单条候选(注释=自动编码预览)。
    /// 候选同时写入计划状态(宿主经 lyyime_cand 逐条获取)。
    fn coin_effects(&self, p: &mut Plan) -> Vec<Effect> {
        let text = self.plan_coin_text(p, p.coin.unwrap_or(0));
        let comment = self
            .dict
            .wubi_word_code(&text)
            .unwrap_or_else(|| "缺码".to_string());
        let cand = Candidate {
            text: text.clone(),
            comment,
            score: 0.0,
            kind: CandKind::User,
        };
        p.cands = vec![cand.clone()];
        p.page = 0;
        vec![
            Effect::Preedit(Some(format!("造词:{text}"))),
            Effect::Candidates(Arc::new(vec![cand])),
        ]
    }

    /// 退出造词模式:清模式标记与候选页(缓冲本就为空)。
    fn plan_coin_exit(p: &mut Plan) {
        p.coin = None;
        plan_clear(p);
    }

    /// 进入造词模式;组合中按下则先按普通流程上屏(历史随之入账)。
    fn plan_coin_start(&self, p: &mut Plan) -> Vec<Effect> {
        if !p.buf.is_empty() {
            let learned = self.learn_worthy(p);
            let text = self.finish_text(p);
            let mut effects = plan_commit(self, p, &text, learned);
            effects.extend(self.plan_coin_start(p));
            return effects;
        }
        // 全程读计划态(p.recent/p.last_run):组合中提交的历史已在本键计划里。
        if p.recent.is_empty() {
            return vec![
                Effect::Notice("造词:还没有可造词的上屏汉字,请先输入中文".to_string()),
                Effect::Consumed,
            ];
        }
        let cap = p.recent.len().min(MAX_COIN_LEN);
        let init = p.last_run.clamp(1, cap);
        // 最近一次只上屏了单字时自动带上前一字,凑成二字词起步。
        p.coin = Some(if init < 2 && cap >= 2 { 2 } else { init });
        self.coin_effects(p)
    }

    /// 造词模式中的按键:方向键增减选字,Enter/Space 存词,Esc 取消;
    /// 其余键退出造词模式并按普通路径处理(用户继续正常输入)。
    fn plan_coin_key(&self, p: &mut Plan, key: LKey) -> Vec<Effect> {
        let cap = p.recent.len().min(MAX_COIN_LEN);
        let min_sel = if cap >= 2 { 2 } else { 1 };
        match key {
            LKey::ArrowRight | LKey::ArrowUp => {
                let n = p.coin.unwrap_or(0);
                if n >= cap {
                    vec![Effect::Consumed]
                } else {
                    p.coin = Some(n + 1);
                    self.coin_effects(p)
                }
            }
            LKey::ArrowLeft | LKey::ArrowDown | LKey::Backspace => {
                let n = p.coin.unwrap_or(0);
                if n <= min_sel {
                    vec![Effect::Consumed]
                } else {
                    p.coin = Some(n - 1);
                    self.coin_effects(p)
                }
            }
            LKey::Enter | LKey::Space => self.plan_coin_commit(p),
            LKey::Esc => {
                Self::plan_coin_exit(p);
                vec![Effect::Preedit(None), empty_cands(), Effect::Consumed]
            }
            LKey::Char(c) => {
                Self::plan_coin_exit(p);
                self.plan_char(p, c)
            }
            LKey::Digit(n) => {
                Self::plan_coin_exit(p);
                self.plan_digit(p, n)
            }
            LKey::Punct(c) => {
                Self::plan_coin_exit(p);
                self.plan_punct(p, c)
            }
            LKey::PageUp | LKey::PageDown => {
                Self::plan_coin_exit(p);
                self.plan_page(p, key == LKey::PageDown)
            }
            // Shift:空缓冲吞键,交宿主做单击判定(与普通路径一致)。
            LKey::ShiftPress => {
                Self::plan_coin_exit(p);
                vec![Effect::Consumed]
            }
            _ => {
                Self::plan_coin_exit(p);
                vec![Effect::Preedit(None), empty_cands(), Effect::Pass]
            }
        }
    }

    /// 确认造词:推算词组编码 → 写入用户造词库(apply 落盘)。
    fn plan_coin_commit(&self, p: &mut Plan) -> Vec<Effect> {
        let sel = self.plan_coin_text(p, p.coin.unwrap_or(0));
        Self::plan_coin_exit(p);
        let clear = vec![Effect::Preedit(None), empty_cands()];
        if sel.is_empty() {
            return vec![Effect::Consumed];
        }
        if let Some(missing) = sel.chars().find(|c| !self.dict.has_wubi_code(*c)) {
            let mut fx = vec![Effect::Notice(format!(
                "造词失败:「{missing}」不在五笔码表,无法自动编码"
            ))];
            fx.extend(clear);
            fx.push(Effect::Consumed);
            return fx;
        }
        let Some(code) = self.dict.wubi_word_code(&sel) else {
            let mut fx = vec![Effect::Notice("造词失败:编码推算异常".to_string())];
            fx.extend(clear);
            fx.push(Effect::Consumed);
            return fx;
        };
        // 可写性预检(幂等、重试安全):失败给人话提示,不做半截应用。
        if !self.user_words.prepare() {
            let mut fx = vec![Effect::Notice(format!(
                "造词失败:无法写入 {},请检查权限",
                self.user_words.path_display()
            ))];
            fx.extend(clear);
            fx.push(Effect::Consumed);
            return fx;
        }
        p.user_word = Some((sel.clone(), code.clone()));
        // 同时计入学习加成,重排时 ×1.5 置顶。
        p.learned = Some(sel.clone());
        vec![
            Effect::Notice(format!("已造词:{sel}({code}),可直接用该编码打出")),
            Effect::Preedit(None),
            empty_cands(),
        ]
    }


    // ------------------------------------------------------------------
    // 提交与状态维护(作用于 Plan)
    // ------------------------------------------------------------------

    /// 先提交当前组合(首选/原字母),再追加 `tail` 效果。
    /// 提交的是候选/直通词才计学习;直通原始字母不污染用户词典。
    fn plan_commit_then(&self, p: &mut Plan, tail: Effect) -> Vec<Effect> {
        if p.buf.is_empty() {
            return vec![tail];
        }
        let learned = self.learn_worthy(p);
        let text = self.finish_text(p);
        let mut effects = plan_commit(self, p, &text, learned);
        effects.push(tail);
        effects
    }

    /// 本次组合结束是否值得学习(来自候选顶屏)。
    fn learn_worthy(&self, p: &Plan) -> bool {
        !plan_page_slice(self, p).is_empty()
    }

    /// 词组效率提示(借鉴万能五笔的高效词提示):最近上屏的连续汉字若
    /// 恰好是词库中的五笔词组、且词组编码比刚才实敲的字母数更省,给出
    /// 「词 + 编码」提示效果(宿主展示于候选条,直到下一次输入才清除)。
    ///
    /// 回看长度 2..min(recent, 6),取最长命中(节省最多、信息量最大);
    /// 只提示"现在就能用该编码打出"的词(精确码桶内确有该词,含用户造词),
    /// 拼音独有词组不冒充五笔编码。实耗键数按字均摊(多字同屏均计入),
    /// 因此刚用同码词组打过的字不会再次提示(编码长 = 实耗,不严格更省)。
    fn plan_phrase_hint(&self, p: &Plan) -> Option<Effect> {
        if !self.cfg.phrase_hint || p.recent.len() < 2 {
            return None;
        }
        let n = p.recent.len();
        for len in (2..=n.min(PHRASE_HINT_MAX)).rev() {
            let word: String = p.recent[n - len..].iter().collect();
            let Some(code) = self.dict.wubi_rev.get(&word) else {
                continue;
            };
            let typed: u32 = p.recent_cost[n - len..]
                .iter()
                .map(|&c| u32::from(c))
                .sum();
            let typeable = self
                .dict
                .wubi_exact
                .get(code)
                .is_some_and(|idxs| idxs.iter().any(|&i| self.dict.wubi[i as usize].word == word));
            if typeable && u32::from(code.len() as u16) * COST_FIXED < typed {
                return Some(Effect::Hint(format!("词组提示:「{word}」可用 {code} 打出")));
            }
        }
        None
    }

    /// 组合结束时应上屏的文本:页内首选 > 原始字母。
    /// 功能键候选不能当文本上屏:退回原始字母(标点等收尾场景不误触发功能)。
    fn finish_text(&self, p: &Plan) -> String {
        match plan_page_slice(self, p).first() {
            Some(top) if !matches!(top.kind, CandKind::Action(_)) => top.text.clone(),
            _ => p.buf.clone(),
        }
    }

    pub(crate) fn page_size(&self) -> usize {
        self.cfg.page_size.max(1)
    }

    fn page_slice(&self) -> &[Candidate] {
        let ps = self.page_size();
        let start = self.page * ps;
        if start >= self.cands.len() {
            return &[];
        }
        &self.cands[start..(start + ps).min(self.cands.len())]
    }

    fn clear_buf(&mut self) {
        self.buf.clear();
        self.cands.clear();
        self.page = 0;
        self.cn_hit = false;
        // 造词模式不跨焦点延续(预编辑已随 reset 消失,继续吞键会卡输入)。
        self.coin = None;
    }

    // ------------------------------------------------------------------
    // 候选生成与排序(修订 §5:层级主导的词典序)
    // ------------------------------------------------------------------

    /// 依据计划缓冲重算候选:wubi → pinyin → english(修订 §5.C 门控)。
    /// 只读引擎配置/词库/学习数据,结果写入 plan(纯函数语义,配合两段式处理)。
    fn recompute_into(&self, p: &mut Plan) {
        let buf = p.buf.clone();
        let mut pool: HashMap<String, RawCand> = HashMap::new();
        let mut cn_hits = 0usize;
        if !buf.is_empty() {
            cn_hits += self.wubi_candidates(&buf, &mut pool);
            cn_hits += self.pinyin_candidates(&buf, &mut pool);
            // 英文通道(修订 §5.C):
            // - 无中文命中 → 前缀候选(不限名次)进 english_no_cn 层;
            // - 有中文命中 → 仅当缓冲本身是 mixed_auto_commit_top_n 内的完整英文词,
            //   以 english_with_cn 层加入单个候选。
            if self.cfg.mixed_en {
                if cn_hits == 0 {
                    self.english_candidates(&buf, &mut pool);
                } else {
                    self.english_with_cn_candidate(&buf, &mut pool);
                }
            }
        }
        p.cn_hit = cn_hits > 0;

        let mut list: Vec<RawCand> = pool.into_values().collect();
        // 用户词加成(合同 §5.6):层内 freq 乘子 ×1.5,允许层内溢出;来源标记为 User。
        // 层间不可跨越由 (tier, …) 词典序保证。
        for c in &mut list {
            if self.learner.get(&c.text).is_some() {
                c.norm *= rank::USER_BOOST;
                c.kind = CandKind::User;
            }
            c.score = c.tier + c.norm;
        }
        // 层级主导的词典序:tier → 完整音节偏好 → 同文件归一频率 → 兜底词频 → 短词 → 词序。
        list.sort_by(|a, b| {
            b.tier
                .partial_cmp(&a.tier)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.spec
                        .partial_cmp(&a.spec)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    b.norm
                        .partial_cmp(&a.norm)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| b.sug.cmp(&a.sug))
                .then_with(|| a.text.chars().count().cmp(&b.text.chars().count()))
                .then_with(|| a.text.cmp(&b.text))
        });
        list.truncate(MAX_CANDS);
        p.cands = list
            .into_iter()
            .map(|rc| Candidate {
                text: rc.text,
                comment: rc.comment,
                score: rc.score,
                kind: rc.kind,
            })
            .collect();
        self.insert_quick_actions(p, &buf);
        p.page = 0;
    }

    /// 快速功能键(合同 §14):缓冲与触发词完全相等时,把功能候选紧跟首选
    /// 之后追加(无任何候选时置顶)——普通候选不被顶替,数字键/鼠标点选
    /// 可达;多条触发词按配置顺序依次排布。不参与排序/截断(重算即插入)。
    fn insert_quick_actions(&self, p: &mut Plan, buf: &str) {
        if !self.cfg.quick_actions_enabled || buf.is_empty() {
            return;
        }
        let mut off = 0usize;
        for (i, a) in self.cfg.quick_actions.iter().enumerate() {
            if a.trigger != buf {
                continue;
            }
            let at = if p.cands.is_empty() { 0 } else { 1 + off };
            p.cands.insert(
                at.min(p.cands.len()),
                Candidate {
                    text: a.label.clone(),
                    comment: "功能键".to_string(),
                    score: 0.0,
                    kind: CandKind::Action(i as u8),
                },
            );
            off += 1;
        }
    }

    /// 五笔通道(§5.2):完全同码(code == buffer,简码奖励并入该层)与前缀渐进分两路查询。
    ///
    /// 精确命中直接查 exact 索引,不受前缀桶截断影响——真实词库中 `a`/`wq` 这类
    /// 高频前缀桶按频次取前 32 时可能把精确码挤出,必须独立保证"精确 > 前缀"。
    fn wubi_candidates(&self, buf: &str, pool: &mut HashMap<String, RawCand>) -> usize {
        let dict = &self.dict;
        let mut hits = 0;
        if let Some(idxs) = dict.wubi_exact.get(buf) {
            for &i in idxs {
                let e = &dict.wubi[i as usize];
                insert_cand(
                    pool,
                    e.word.clone(),
                    e.code.clone(),
                    rank::TIER_WUBI_EXACT,
                    1.0,
                    rank::norm10(e.freq, dict.wubi_max),
                    CandKind::Wubi,
                    suggestion_of(dict, &e.word),
                );
                hits += 1;
            }
        }
        if let Some(idxs) = dict.wubi_prefix.get(buf) {
            for &i in idxs {
                let e = &dict.wubi[i as usize];
                if e.code == buf {
                    continue; // 精确命中已在上面以更高层插入
                }
                insert_cand(
                    pool,
                    e.word.clone(),
                    e.code.clone(),
                    rank::TIER_WUBI_PREFIX,
                    1.0,
                    rank::norm10(e.freq, dict.wubi_max),
                    CandKind::Wubi,
                    suggestion_of(dict, &e.word),
                );
                hits += 1;
            }
        }
        hits
    }

    /// 拼音通道(§5.3):完整切分(pinyin_full)> 末音节不完整(pinyin_partial)> 简拼(pinyin_abbrev)。
    ///
    /// spec(完整音节偏好):词组按音节数放大;末音节单字记 1;
    /// 不完整片段越接近真实音节越具体(frag_len 计入),同层内先排更完整的匹配。
    ///
    /// 注释统一为五笔码(反查):即使输入拼音,候选后的编码也始终显示该词的
    /// 五笔编码;词不在五笔表时才保留拼音注释兜底。
    fn pinyin_candidates(&self, buf: &str, pool: &mut HashMap<String, RawCand>) -> usize {
        if self.dict.syllables.is_empty() {
            return 0;
        }
        let dict = &self.dict;
        let mut hits = 0usize;

        // ① 完整切分(含 xian = xian / xi'an 歧义):词组精确命中 + 末音节单字。
        for seg in pinyin::segmentations(buf, &dict.syllables, SEGS_CAP) {
            let joined = pinyin::seg_joined(buf, &seg);
            if let Some(idxs) = dict.py_exact.get(&joined) {
                for &i in idxs {
                    let p = &dict.phrases[i as usize];
                    insert_cand(
                        pool,
                        p.word.clone(),
                        self.wubi_comment(&p.word, joined.clone()),
                        rank::TIER_PINYIN_FULL,
                        (p.sylls.len() * 3) as f32,
                        rank::norm10(p.freq, dict.py_phrase_max),
                        CandKind::Pinyin,
                        suggestion_of(dict, &p.word),
                    );
                    hits += 1;
                }
            }
            let last = pinyin::seg_last(buf, &seg);
            if let Some(list) = dict.py_chars.get(last) {
                for (ch, freq) in list {
                    insert_cand(
                        pool,
                        ch.to_string(),
                        self.wubi_comment(&ch.to_string(), last.to_string()),
                        rank::TIER_PINYIN_FULL,
                        1.0,
                        rank::norm10(*freq, dict.py_char_max),
                        CandKind::Pinyin,
                        suggestion_of(dict, &ch.to_string()),
                    );
                    hits += 1;
                }
            }
        }

        // ② 末音节不完整:buf[..k] 完整切分 + buf[k..] 为音节前缀(pinyin_partial 层)。
        for k in pinyin::prefix_ends(buf, &dict.syllables) {
            if k >= buf.len() {
                continue; // 完整切分已在 ① 覆盖
            }
            let frag = &buf[k..];
            if !dict.syllable_prefixes.contains(frag) {
                continue;
            }
            let frag_len = frag.chars().count() as f32;
            for seg in pinyin::segmentations(&buf[..k], &dict.syllables, SEGS_CAP / 4) {
                let joined = pinyin::seg_joined(&buf[..k], &seg);
                let Some(idxs) = dict.py_boundary.get(&joined) else {
                    continue;
                };
                for &i in idxs {
                    let p = &dict.phrases[i as usize];
                    // 词组必须还有下一个音节来消化这个不完整片段。
                    if p.sylls.len() > seg.len() && p.sylls[seg.len()].starts_with(frag) {
                        let spec = seg.len() as f32 * 3.0 + 1.0 + frag_len / 10.0;
                        insert_cand(
                            pool,
                            p.word.clone(),
                            self.wubi_comment(&p.word, format!("{joined} {frag}")),
                            rank::TIER_PINYIN_PARTIAL,
                            spec,
                            rank::norm10(p.freq, dict.py_phrase_max),
                            CandKind::Pinyin,
                            suggestion_of(dict, &p.word),
                        );
                        hits += 1;
                    }
                }
            }
            // 不完整音节的单字:片段恰好是完整音节者更具体。
            for syll in &dict.syllables {
                if syll.starts_with(frag) {
                    let spec = if syll == frag {
                        2.0 + frag_len / 10.0
                    } else {
                        1.0 + frag_len / 10.0
                    };
                    if let Some(list) = dict.py_chars.get(syll) {
                        for (ch, freq) in list {
                            insert_cand(
                                pool,
                                ch.to_string(),
                                self.wubi_comment(&ch.to_string(), syll.clone()),
                                rank::TIER_PINYIN_PARTIAL,
                                spec,
                                rank::norm10(*freq, dict.py_char_max),
                                CandKind::Pinyin,
                                suggestion_of(dict, &ch.to_string()),
                            );
                            hits += 1;
                        }
                    }
                }
            }
        }

        // ③ 整串本身是不完整的首音节(如 "n" → ni/nian 的字)。
        if dict.syllable_prefixes.contains(buf) {
            let frag_len = buf.chars().count() as f32;
            for syll in &dict.syllables {
                if syll.starts_with(buf) {
                    let spec = if syll == buf {
                        2.0 + frag_len / 10.0
                    } else {
                        1.0 + frag_len / 10.0
                    };
                    if let Some(list) = dict.py_chars.get(syll) {
                        for (ch, freq) in list {
                            insert_cand(
                                pool,
                                ch.to_string(),
                                self.wubi_comment(&ch.to_string(), syll.clone()),
                                rank::TIER_PINYIN_PARTIAL,
                                spec,
                                rank::norm10(*freq, dict.py_char_max),
                                CandKind::Pinyin,
                                suggestion_of(dict, &ch.to_string()),
                            );
                            hits += 1;
                        }
                    }
                }
            }
        }

        // ④ 简拼(合同 §5.3:每音节首字母,≥2 键,pinyin_abbrev 低层)。
        if buf.chars().count() >= 2 {
            if let Some(idxs) = dict.jian_exact.get(buf) {
                for &i in idxs {
                    let p = &dict.phrases[i as usize];
                    insert_cand(
                        pool,
                        p.word.clone(),
                        self.wubi_comment(&p.word, p.jian.clone()),
                        rank::TIER_PINYIN_ABBREV,
                        1.0,
                        rank::norm10(p.freq, dict.py_phrase_max),
                        CandKind::Pinyin,
                        suggestion_of(dict, &p.word),
                    );
                    hits += 1;
                }
            }
            // 渐进简拼:词组简拼以缓冲开头(如 "ni" → 你好吗"nhm")。
            if let Some(idxs) = dict.jian_boundary.get(buf) {
                for &i in idxs {
                    let p = &dict.phrases[i as usize];
                    insert_cand(
                        pool,
                        p.word.clone(),
                        self.wubi_comment(&p.word, format!("{}?", p.jian)),
                        rank::TIER_PINYIN_ABBREV,
                        1.0,
                        rank::norm10(p.freq, dict.py_phrase_max),
                        CandKind::Pinyin,
                        suggestion_of(dict, &p.word),
                    );
                    hits += 1;
                }
            }
        }
        hits
    }

    /// 反查五笔注释:候选注释统一为五笔编码——拼音命中的词显示其五笔码
    /// (拼音打字也能看到五笔编码,便于学习反查);词不在五笔表时保留
    /// 原通道注释(拼音)兜底。
    fn wubi_comment(&self, word: &str, fallback: String) -> String {
        match self.dict.wubi_rev.get(word) {
            Some(code) => code.clone(),
            None => fallback,
        }
    }

    /// 英文通道(修订 §5.C):无中文命中时的前缀候选。
    ///
    /// 候选不限名次(否则长尾词在真实 20k 词表下永不出现,如 hello 列第 ~2400 名)。
    fn english_candidates(&self, buf: &str, pool: &mut HashMap<String, RawCand>) -> usize {
        let Some(idxs) = self.dict.en_prefix.get(buf) else {
            return 0;
        };
        let dict = &self.dict;
        let mut hits = 0;
        for &i in idxs {
            let (word, freq) = &dict.english[i as usize];
            let exact = word == buf;
            insert_cand(
                pool,
                word.clone(),
                "en".to_string(),
                rank::TIER_ENGLISH_NO_CN,
                if exact { 1.5 } else { 1.0 },
                rank::norm10(*freq, dict.en_max),
                CandKind::English,
                suggestion_of(dict, word),
            );
            hits += 1;
        }
        hits
    }

    /// 有中文命中时的英文候选(修订 §5.C):缓冲本身是 mixed_auto_commit_top_n 内
    /// 的完整英文词,以 english_with_cn 层加入单个候选(层位低于五笔前缀与拼音)。
    fn english_with_cn_candidate(&self, buf: &str, pool: &mut HashMap<String, RawCand>) {
        let Some(&en_rank) = self.dict.en_rank.get(buf) else {
            return;
        };
        if en_rank as usize > self.cfg.mixed_auto_commit_top_n {
            return;
        }
        let Some(idxs) = self.dict.en_prefix.get(buf) else {
            return;
        };
        for &i in idxs {
            let (word, freq) = &self.dict.english[i as usize];
            if word == buf {
                insert_cand(
                    pool,
                    word.clone(),
                    "en".to_string(),
                    rank::TIER_ENGLISH_WITH_CN,
                    1.0,
                    rank::norm10(*freq, self.dict.en_max),
                    CandKind::English,
                    suggestion_of(&self.dict, word),
                );
                break;
            }
        }
    }
}

// ----------------------------------------------------------------------
// Plan 视图辅助(以"按键后的计划状态"为视角读页/提交)
// ----------------------------------------------------------------------

/// 计划状态的当前页切片。
fn plan_page_slice<'a>(eng: &Engine, p: &'a Plan) -> &'a [Candidate] {
    let ps = eng.page_size();
    let start = p.page * ps;
    if start >= p.cands.len() {
        return &[];
    }
    &p.cands[start..(start + ps).min(p.cands.len())]
}

/// 清空计划中的组合状态。
fn plan_clear(p: &mut Plan) {
    p.buf.clear();
    p.cands.clear();
    p.page = 0;
    p.cn_hit = false;
}

/// 结束本次组合并上屏 `text`;`learned` 表示该文本来自候选/自动直通,需要学习。
fn plan_commit(eng: &Engine, p: &mut Plan, text: &str, learned: bool) -> Vec<Effect> {
    if learned {
        p.learned = Some(text.to_string());
    }
    // 造词历史:只记汉字;本次 commit 不含汉字(标点/字母直通)时不打断
    // last_run——「你好,」的逗号不应让造词起点清零。
    let cjk: Vec<char> = text.chars().filter(|c| is_cjk(*c)).collect();
    if !cjk.is_empty() {
        p.last_run = cjk.len().min(RECENT_CAP);
        p.recent.extend(cjk.iter().copied());
        // 实耗键数 = 本组合敲的字母数,均摊到每个上屏字(词组提示比较基准)。
        let cost = (p.buf.chars().count() as u32 * COST_FIXED / cjk.len() as u32) as u16;
        p.recent_cost.extend(std::iter::repeat_n(cost, cjk.len()));
        let overflow = p.recent.len().saturating_sub(RECENT_CAP);
        if overflow > 0 {
            p.recent.drain(..overflow);
            p.recent_cost.drain(..overflow);
        }
    }
    plan_clear(p);
    let mut effects = vec![
        Effect::Commit(text.to_string()),
        Effect::Preedit(None),
        empty_cands(),
    ];
    // 词组效率提示排在清除之后:宿主候选条先隐藏再展示提示,
    // 并保留到下一次输入产生新效果流时才替换/清除。
    if let Some(hint) = eng.plan_phrase_hint(p) {
        effects.push(hint);
    }
    effects
}

/// 选中一条候选的上屏规划(数字/鼠标/空格确认共用):功能键候选(合同 §14)
/// → [`Effect::Action`](宿主执行功能,不上屏文本、不学习、不入造词历史);
/// 普通候选 → [`plan_commit`]。
fn plan_select(eng: &Engine, p: &mut Plan, cand: &Candidate, learned: bool) -> Vec<Effect> {
    if let CandKind::Action(i) = cand.kind {
        plan_clear(p);
        return vec![
            Effect::Action(i as usize),
            Effect::Preedit(None),
            empty_cands(),
        ];
    }
    plan_commit(eng, p, &cand.text, learned)
}

/// CJK 统一表意字符(含扩展 A/兼容区;造词只认汉字)。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF)
}

/// 造词库路径:与用户词典同目录的 `user_words.tsv`(目录由 user_dict 覆盖)。
fn user_words_path(cfg: &Config) -> std::path::PathBuf {
    let base = cfg
        .user_dict
        .clone()
        .unwrap_or_else(Config::default_user_dict_path);
    match base.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("user_words.tsv"),
        _ => std::path::PathBuf::from("user_words.tsv"),
    }
}

/// 组合中的效果流:[Preedit, Candidates]。
fn composition_effects(eng: &Engine, p: &Plan) -> Vec<Effect> {
    vec![
        Effect::Preedit(Some(p.buf.clone())),
        Effect::Candidates(Arc::new(plan_page_slice(eng, p).to_vec())),
    ]
}

/// 空候选页效果。
fn empty_cands() -> Effect {
    Effect::Candidates(Arc::new(Vec::new()))
}

impl Drop for Engine {
    fn drop(&mut self) {
        // 退出兜底:有未落盘的用户词就尽力写一次(合同 §4"每 64 次/退出时")。
        let _ = self.learner.save();
    }
}
