//! 输入引擎:按键行为状态机 + 候选生成与排序(合同 §5/§6,§3 v1.1 重试纪律)。
//!
//! 宿主只做三件事:把真实按键翻译成 [`LKey`](一般需先小写化字母)、
//! 按 [`Effect`] 顺序渲染/上屏、焦点切换时调 [`Engine::reset`]。
//!
//! 行为速查(细则见 ARCHITECTURE.md §6):
//! - a–z:进缓冲,更新 preedit/候选(≤12 字母;英文态直通;原组合有中文命中
//!   而本键会推进完全无候选的死胡同时吞键,原样保留候选状态);
//! - 1–9:选当前页第 N 个候选;0:选第 10 个(页不足 10 条时吞掉);
//!   无候选放行数字;越界吞掉;
//! - Space:有候选顶屏首选;有缓冲无候选直通原字母;空缓冲放行;
//! - Enter:有缓冲上屏原始字母(单个英文词的输入方式;去向按
//!   `enter_english` 配置,默认临时——上屏后保持中文模式);空缓冲放行;
//! - Backspace:删尾;Esc:清缓冲;`-`/`=`(PageUp/PageDown):翻页;
//! - 标点:中文态空缓冲出中文标点;有缓冲先上屏首选再补中文标点;英文态放行;
//! - 四码首选上屏(可配置,默认开):恰好四码、首选是五笔命中且同码
//!   无重码(五笔/用户候选唯一)时免空格直接上屏首选;有重码不上屏,
//!   留候选等选词或继续输入顶屏;首选是拼音/英文或缓冲命中功能键
//!   触发词(前缀)不触发;关闭后回退"四码唯一上屏"——中文候选唯一
//!   时才免空格上屏(唯一候选是英文词不触发);
//! - 四码顶屏(可配置,默认开):缓冲恰四码且首选是五笔命中时再敲字母,
//!   先上屏首选、该字母开新组合;拼音/英文/功能键首选与触发词前缀
//!   不顶屏,继续渐进组词;
//! - 词组提示(可配置):上屏后最近几字有更省键的五笔词组时,效果流在
//!   清除类效果之后追加 [`Effect::Hint`](候选条展示,下一次输入才清除);
//! - 快速功能键(可配置,合同 §14):缓冲与触发词完全相等时候选条追加
//!   功能候选(`CandKind::Action`,紧跟首选之后);数字/鼠标/空格选中产生
//!   [`Effect::Action`](宿主执行功能,不上屏文本、不学习),普通候选
//!   不会被功能键顶替首选位置;
//! - ShiftPress:有缓冲上屏英文原串,并按 `shift_english` 配置决定去向
//!   (默认切英文模式——效果流附 [`Effect::ModeChanged`],宿主同步指示/
//!   trigger;配成临时则仅上屏,英文短词改用回车);空缓冲吞键,宿主判定
//!   单击后调 [`Engine::toggle_mode`];
//! - 拼音前缀候选(缺词兜底):缓冲能切成"合法音节前缀 + 可续接后缀"时,
//!   词库虽无整词仍给出前缀候选(`Candidate.consumed` = 前缀字节数);
//!   选中(空格/数字/点选共用)只上屏候选文本,剩余后缀以原敲入形态留在
//!   组合里继续编辑/选词;标点、造词等边界键按"首选文本 + 原始后缀"
//!   无损收尾,不猜后缀汉字;学习/上屏历史只记消费掉的前缀。
//! - 其它键:Pass(有缓冲先清缓冲)。
//!
//! ## 两段式按键处理(合同 §3 v1.1 重试纪律)
//!
//! `process_key` = [`Engine::plan_key`](纯读取,产出效果流与下一状态 Plan)
//! + [`Engine::apply_plan`](落内部状态)。FFI 层先把效果流 JSON 写入宿主缓冲,
//! **确认写得下才 apply**;返回 -needed 时内部状态保持不变,宿主扩容重试同一键
//! 不会二次生效。Rust 宿主走 `process_key` 时两步合并,行为不变。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{Config, EnCommit, QuickAction};
use crate::dict::{char_corpus_freq, char_tier_of, suggestion_of, DictIndex};
use crate::learner::Learner;
use crate::pinyin;
use crate::prediction::{self, PredictionIndex};
use crate::punct::{to_chinese, QuoteState};
use crate::rank;
use crate::rank::exact_char_tier;
use crate::types::{CandKind, CandOp, Candidate, Effect, LKey, Mode};
use crate::user_words::UserWords;
use crate::wordops::{BlockList, PinTable, ZhEn};

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
/// 排序按 (tier, spec, norm) 词典序——层间不可跨越(修订版 §5;精确层单字
/// 在 `exact_char_freq_rank` 开启时按语料词频取档,见 rank::exact_char_tier);
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
    /// 消费语义同 [`Candidate::consumed`]:0 = 整缓冲;非零 = 只消费开头
    /// 这么多 ASCII 字节(前缀候选,选中后剩余后缀留在组合内继续编辑)。
    consumed: usize,
}

/// 向候选池插入一条候选(默认消费整个缓冲),同词去重规则见
/// [`insert_cand_with_consumed`]。
#[allow(clippy::too_many_arguments)]
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
    insert_cand_with_consumed(pool, text, comment, tier, spec, norm, kind, sug, 0);
}

/// 同词保留 (tier, spec, norm) 词典序更大者;层级打平时消费更多者优先
/// (consumed=0 视为消费整个缓冲,即 usize::MAX,整缓冲候选压过前缀候选)。
/// 替换时连同 kind 一并更新 consumed——消费量与候选来源绑定,不随排序漂移。
#[allow(clippy::too_many_arguments)]
fn insert_cand_with_consumed(
    pool: &mut HashMap<String, RawCand>,
    text: String,
    comment: String,
    tier: f32,
    spec: f32,
    norm: f32,
    kind: CandKind,
    sug: u64,
    consumed: usize,
) {
    // 0 = 整缓冲,平级比较时记为最大消费。
    fn cons_ord(c: usize) -> usize {
        if c == 0 {
            usize::MAX
        } else {
            c
        }
    }
    if let Some(e) = pool.get_mut(&text) {
        if (tier, spec, norm) > (e.tier, e.spec, e.norm)
            || ((tier, spec, norm) == (e.tier, e.spec, e.norm)
                && cons_ord(consumed) > cons_ord(e.consumed))
        {
            e.tier = tier;
            e.spec = spec;
            e.norm = norm;
            e.comment = comment;
            e.kind = kind;
            e.consumed = consumed;
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
            consumed,
        },
    );
}

/// 一次按键的计划:按键生效后的全部可变状态。
///
/// 由 [`Engine::plan_key`] 纯读取产出,经 [`Engine::apply_plan`] 落地;
/// FFI 层借此实现"JSON 写得下才生效"的有状态重试纪律。
pub(crate) struct Plan {
    pub(crate) buf: String,
    /// 敲入原形镜像(保留大写;与 buf 等长)。
    pub(crate) buf_raw: String,
    pub(crate) cands: Vec<Candidate>,
    pub(crate) page: usize,
    pub(crate) cn_hit: bool,
    pub(crate) quotes: QuoteState,
    /// 按键生效后的模式(回车/Shift 上屏英文可切英文模式;其余键不变)。
    pub(crate) mode: Mode,
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
    /// §15 右键操作的计划态覆盖:plan 阶段不动真实表,recompute 时以
    /// 覆盖集代替 `self.pins` / `self.blocked` 读;None = 用真实表。
    pub(crate) pins_override: Option<HashMap<String, String>>,
    pub(crate) blocked_override: Option<HashSet<String>>,
    /// §15 apply 阶段执行的持久化动作(固定/取消固定/删除词组)。
    pub(crate) persist_op: Option<PersistOp>,
    /// 上屏后联想的上下文(最近上屏的连续 CJK 尾串,≤6 字);与造词
    /// recent/recent_cost 相互独立。
    pub(crate) prediction_context: String,
    /// 当前候选行是否为联想行(true 时空格/数字/点选选中的是"接下来
    /// 的词句尾巴",按尾巴上屏;字母/边界键按联想规则分派)。
    pub(crate) predicting: bool,
}

/// §15 右键菜单操作在 apply_plan 阶段的持久化动作(plan 纯演算不落盘)。
pub(crate) enum PersistOp {
    Pin { code: String, word: String },
    Unpin { code: String },
    Delete { word: String },
}

impl Plan {
    fn unchanged(eng: &Engine) -> Self {
        Self {
            buf: eng.buf.clone(),
            buf_raw: eng.buf_raw.clone(),
            cands: eng.cands.clone(),
            page: eng.page,
            cn_hit: eng.cn_hit,
            quotes: eng.quotes,
            mode: eng.mode,
            learned: None,
            recent: eng.recent.clone(),
            recent_cost: eng.recent_cost.clone(),
            last_run: eng.last_run,
            coin: eng.coin,
            user_word: None,
            pins_override: None,
            blocked_override: None,
            persist_op: None,
            prediction_context: eng.prediction_context.clone(),
            predicting: eng.predicting,
        }
    }
}

/// lyyIme 输入引擎(Send:宿主可能跨线程持有,但所有 API 非线程安全,需自行加锁)。
pub struct Engine {
    dict: DictIndex,
    cfg: Config,
    mode: Mode,
    buf: String,
    /// 缓冲的敲入原形(与 buf 等长,保留大写;全大写输入候选用,见 caps_candidates)。
    buf_raw: String,
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
    /// 固定首位表(§15,与造词库同目录 `pinned.tsv`)。
    pins: PinTable,
    /// 屏蔽词表(§15 右键"删除词组";`blocked.tsv`)。
    blocked: BlockList,
    /// 中→英反查表(§15,词典目录 `zh_en.tsv`;惰性:首次反查才读盘,
    /// 缺失文件记空表不再重试)。
    zh_en: RefCell<Option<ZhEn>>,
    /// 词库目录(`zh_en.tsv` 所在,Engine::new 的 data_dir)。
    data_dir: PathBuf,
    /// 上屏后联想索引(本地词表,建引擎时一次合并;造词并入即时更新)。
    prediction_index: PredictionIndex,
    /// 联想上下文:最近上屏的连续 CJK 尾串(≤[`prediction::MAX_CTX_CHARS`] 字)。
    prediction_context: String,
    /// 当前候选行是否为联想行。
    predicting: bool,
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
        // §15 固定/屏蔽表与造词库同目录(用户数据区)。
        let mut pins = PinTable::new();
        pins.load(&wordops_path(&cfg, "pinned.tsv"));
        let mut blocked = BlockList::new();
        blocked.load(&wordops_path(&cfg, "blocked.tsv"));
        // 联想索引在用户词并入之后构建(造词也参与联想)。
        let prediction_index = PredictionIndex::new(&dict);
        Ok(Self {
            dict,
            cfg,
            mode: Mode::Chinese,
            buf: String::new(),
            buf_raw: String::new(),
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
            pins,
            blocked,
            zh_en: RefCell::new(None),
            data_dir: data_dir.to_path_buf(),
            prediction_index,
            prediction_context: String::new(),
            predicting: false,
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
        if self
            .user_words
            .set_path_and_load(&mut self.dict, user_words_path(&cfg))
        {
            // 词典索引变了(新目录的造词并入):联想索引随之重建,
            // 旧上下文作废(避免跨目录词表错配);正在展示的联想行一并撤下。
            self.prediction_index = PredictionIndex::new(&self.dict);
            self.prediction_context.clear();
            if self.predicting {
                self.predicting = false;
                self.cands.clear();
                self.page = 0;
            }
        }
        // §15 固定/屏蔽表同目录:配置下发即重载(目录迁移与外部改动都覆盖)。
        self.pins.load(&wordops_path(&cfg, "pinned.tsv"));
        self.blocked.load(&wordops_path(&cfg, "blocked.tsv"));
        if cfg.mode != self.mode {
            self.mode = cfg.mode;
            self.clear_buf();
        }
        // 中文标点默认值被改写时复位引号开合状态(新配置的首个引号
        // 应从头开始配对,不继承旧配置下的半对状态)。
        if cfg.cn_punct != self.cfg.cn_punct {
            self.quotes = QuoteState::new();
        }
        // 联想开关转关:正在展示的联想行与上下文一并作废(组合缓冲不动)。
        if !cfg.next_word_prediction {
            self.prediction_context.clear();
            if self.predicting {
                self.predicting = false;
                self.cands.clear();
                self.page = 0;
            }
        }
        self.cfg = cfg;
    }

    /// 当前配置快照。
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// 窄化设置中文标点开关:只改 `cn_punct` 并复位引号配对状态,
    /// 不动组合缓冲/候选/联想上下文/模式(运行时 Ctrl+. 切换走这里,
    /// 避免 set_config 顺带重装载词表或打断正在输入的组合)。
    pub fn set_chinese_punctuation(&mut self, enabled: bool) {
        if self.cfg.cn_punct != enabled {
            self.cfg.cn_punct = enabled;
            self.quotes = QuoteState::new();
        }
    }

    /// 翻转中文标点开关并返回新状态(Ctrl+. 热键路径)。
    pub fn toggle_chinese_punctuation(&mut self) -> bool {
        self.set_chinese_punctuation(!self.cfg.cn_punct);
        self.cfg.cn_punct
    }

    pub fn set_pinyin_only(&mut self, on: bool) -> bool {
        if self.cfg.pinyin_only == on {
            return false;
        }
        self.cfg.pinyin_only = on;
        self.clear_buf();
        true
    }
    pub fn set_learning(&mut self, on: bool) {
        if self.cfg.learning != on {
            self.cfg.learning = on;
            self.learner.set_enabled(on);
        }
    }
    pub fn set_quick_actions_enabled(&mut self, on: bool) {
        self.cfg.quick_actions_enabled = on;
    }
    pub fn set_mixed_en(&mut self, on: bool) {
        self.cfg.mixed_en = on;
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
    // §15 候选右键菜单:固定首位 / 删除词组 / 反查英文
    // ------------------------------------------------------------------

    /// 当前页第 `idx` 个候选的固定状态:
    /// - `Some(true)`  = 普通候选,已是本缓冲码的固定词(菜单显示"取消固定");
    /// - `Some(false)` = 普通候选,未固定(菜单显示"固定首位");
    /// - `None`        = 功能键候选或越界,宿主应禁用整个右键菜单。
    pub fn cand_pinned(&self, idx: usize) -> Option<bool> {
        // 联想行不是编码候选:无码可固定/删除,右键菜单禁用。
        if self.predicting {
            return None;
        }
        let cand = self.flush_page().get(idx)?;
        if matches!(cand.kind, CandKind::Action(_)) {
            return None;
        }
        Some(self.pins.is_pinned(&self.buf, &cand.text))
    }

    /// 右键菜单操作(Rust 宿主直调版):内部走 `plan_cand_op` → `apply_plan` 两段式。
    pub fn cand_op(&mut self, idx: usize, op: CandOp) -> Vec<Effect> {
        let (effects, plan) = self.plan_cand_op(idx, op);
        self.apply_plan(plan);
        effects
    }

    /// §15 的规划态版本(FFI 两段式重试纪律,同 `plan_select_candidate`):
    /// 固定/删除在 plan 阶段只用 `pins_override`/`blocked_override` 预演重算,
    /// 真实表在 `apply_plan`(JSON 确认写入宿主缓冲后)才改动+落盘;
    /// 反查英文把计划候选页替换为 zh_en 结果(可继续数字/点选上屏)。
    pub(crate) fn plan_cand_op(&self, idx: usize, op: CandOp) -> (Vec<Effect>, Plan) {
        let p = Plan::unchanged(self);
        // 联想行同理不开放右键操作(没有码可固定;尾巴删除会误伤整词)。
        if self.coin.is_some() || self.predicting {
            return (vec![Effect::Consumed], p);
        }
        let Some(cand) = plan_page_slice(self, &p).get(idx).cloned() else {
            return (vec![Effect::Consumed], p);
        };
        if matches!(cand.kind, CandKind::Action(_)) {
            return (
                vec![
                    Effect::Notice("功能键候选不支持右键操作".into()),
                    Effect::Consumed,
                ],
                p,
            );
        }
        match op {
            CandOp::PinToggle => self.plan_pin_toggle(p, &cand),
            CandOp::Delete => self.plan_delete(p, &cand),
            CandOp::EnLookup => self.plan_en_lookup(p, &cand),
        }
    }

    /// 固定首位/取消固定:固定表按键码 code(=当前缓冲)存词,
    /// 重算后该词恒居第一(注释带"固"标记)。
    fn plan_pin_toggle(&self, mut p: Plan, cand: &Candidate) -> (Vec<Effect>, Plan) {
        let code = p.buf.clone();
        if code.is_empty() {
            return (vec![Effect::Consumed], p);
        }
        if !self.pins.prepare() {
            let msg = format!("固定失败:无法写入 {}", self.pins.path_display());
            return (vec![Effect::Notice(msg), Effect::Consumed], p);
        }
        let mut map = self.pins.map().clone();
        if self.pins.is_pinned(&code, &cand.text) {
            map.remove(&code);
            p.persist_op = Some(PersistOp::Unpin { code });
        } else {
            map.insert(code.clone(), cand.text.clone());
            p.persist_op = Some(PersistOp::Pin {
                code,
                word: cand.text.clone(),
            });
        }
        p.pins_override = Some(map);
        self.recompute_into(&mut p);
        (composition_effects(self, &p), p)
    }

    /// 删除词组:用户造词剔除 + 词面入屏蔽表(内置词条不改词典文件),
    /// 重算后该词不再出现在任何编码的候选里。
    fn plan_delete(&self, mut p: Plan, cand: &Candidate) -> (Vec<Effect>, Plan) {
        if !self.blocked.prepare() || !self.user_words.prepare() {
            let msg = format!("删除失败:无法写入 {}", self.blocked.path_display());
            return (vec![Effect::Notice(msg), Effect::Consumed], p);
        }
        let word = cand.text.clone();
        let mut set = self.blocked.words().clone();
        set.insert(word.clone());
        p.blocked_override = Some(set);
        p.persist_op = Some(PersistOp::Delete { word });
        self.recompute_into(&mut p);
        (composition_effects(self, &p), p)
    }

    /// 反查英文:当前候选页替换为该中文词的英文反查结果(zh_en.tsv),
    /// 注释标出来源词;缓冲与页码保持,退格/改码即还原正常候选。
    fn plan_en_lookup(&self, mut p: Plan, cand: &Candidate) -> (Vec<Effect>, Plan) {
        let word = cand.text.clone();
        if !word.chars().any(is_cjk) {
            return (
                vec![
                    Effect::Notice(format!("「{word}」不是中文词,无英文反查")),
                    Effect::Consumed,
                ],
                p,
            );
        }
        let ens = self.zh_en_lookup(&word);
        if ens.is_empty() {
            return (
                vec![
                    Effect::Notice(format!("「{word}」没有英文反查结果")),
                    Effect::Consumed,
                ],
                p,
            );
        }
        p.cands = ens
            .iter()
            .map(|en| Candidate {
                text: en.clone(),
                comment: format!("←{word}"),
                score: 0.0,
                kind: CandKind::English,
                consumed: 0,
            })
            .collect();
        p.page = 0;
        (
            vec![Effect::Candidates(Arc::new(
                plan_page_slice(self, &p).to_vec(),
            ))],
            p,
        )
    }

    /// `zh_en.tsv` 惰性装载:首次"反查英文"才读盘(文件可能较大,
    /// 不拖慢引擎启动);缺失记空表(查询落空 → 提示,不再重试)。
    fn zh_en_lookup(&self, word: &str) -> Vec<String> {
        let mut slot = self.zh_en.borrow_mut();
        if slot.is_none() {
            *slot = Some(ZhEn::load(&self.data_dir.join("zh_en.tsv")));
        }
        slot.as_ref()
            .map(|z| z.lookup(word).to_vec())
            .unwrap_or_default()
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
        // 联想态按键分派(先于普通路径):候选条里是"接下来的词句尾巴"。
        // 空格/数字/点选 = 上屏所选尾巴(可连选续接);翻页键翻联想页;
        // Esc = 关联想(上下文一并作废);字母 = 撤联想行、保留上下文、
        // 开始新组合;其余键 = 硬边界(联想与上下文清空)后按普通规则处理。
        if p.predicting {
            match key {
                LKey::Space => return (self.plan_space(&mut p), p),
                LKey::Digit(n) => {
                    let idx = if n == 0 { 9 } else { (n - 1) as usize };
                    if let Some(c) = plan_page_slice(self, &p).get(idx).cloned() {
                        return (plan_select(self, &mut p, &c, true), p);
                    }
                    // 越界数字:撤销联想行并直通(数字交给应用,不吞)。
                    p.predicting = false;
                    p.cands.clear();
                    p.page = 0;
                    p.prediction_context.clear();
                    return (
                        vec![Effect::Preedit(None), empty_cands(), Effect::Pass],
                        p,
                    );
                }
                LKey::PageUp | LKey::PageDown => {
                    // 单页联想不放行:翻页键(-/= 等)放行会落进应用而联想行
                    // 仍在原位,吞键保持原页(多页时仍走 plan_page 翻页)。
                    if p.cands.len() <= self.page_size() {
                        return (vec![Effect::Consumed], p);
                    }
                    return (self.plan_page(&mut p, key == LKey::PageDown), p);
                }
                LKey::Esc => {
                    p.predicting = false;
                    p.cands.clear();
                    p.page = 0;
                    p.prediction_context.clear();
                    return (
                        vec![Effect::Preedit(None), empty_cands(), Effect::Consumed],
                        p,
                    );
                }
                LKey::Char(c) => {
                    p.predicting = false;
                    p.cands.clear();
                    p.page = 0;
                    let mut fx = vec![Effect::Preedit(None), empty_cands()];
                    fx.extend(self.plan_char(&mut p, c));
                    return (fx, p);
                }
                LKey::Coin => {
                    // 造词:撤联想行后正常进入(上下文与造词历史都保留)。
                    p.predicting = false;
                    p.cands.clear();
                    p.page = 0;
                    let mut fx = vec![Effect::Preedit(None), empty_cands()];
                    fx.extend(self.plan_coin_start(&mut p));
                    return (fx, p);
                }
                _ => {
                    p.predicting = false;
                    p.cands.clear();
                    p.page = 0;
                    p.prediction_context.clear();
                    let mut fx = vec![Effect::Preedit(None), empty_cands()];
                    fx.extend(self.plan_key_normal(&mut p, key));
                    return (fx, p);
                }
            }
        }
        let effects = self.plan_key_normal(&mut p, key);
        // 输入边界截断联想上下文:标点/回车/退格/方向/Esc/Other/Shift 按下,
        // 或效果流出现直通/功能动作/模式切换(直通即边界——宿主放行后
        // 后续按键与本次上屏不再连续)。
        let boundary = matches!(
            key,
            LKey::Punct(_)
                | LKey::Enter
                | LKey::Backspace
                | LKey::Esc
                | LKey::ArrowLeft
                | LKey::ArrowRight
                | LKey::ArrowUp
                | LKey::ArrowDown
                | LKey::Other
                | LKey::ShiftPress
        ) || effects.iter().any(|e| {
            matches!(
                e,
                Effect::Pass | Effect::Action(_) | Effect::ModeChanged(_)
            )
        });
        if boundary {
            p.prediction_context.clear();
        }
        (effects, p)
    }

    /// 普通(非联想态)按键分派:原 [`Engine::plan_key`] 的主匹配体。
    fn plan_key_normal(&self, p: &mut Plan, key: LKey) -> Vec<Effect> {
        match key {
            LKey::Coin => self.plan_coin_start(p),
            _ if self.coin.is_some() => self.plan_coin_key(p, key),
            LKey::Char(c) => self.plan_char(p, c),
            LKey::Digit(n) => self.plan_digit(p, n),
            LKey::Space => self.plan_space(p),
            LKey::Enter => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    // 合同 §6:Enter 上屏原始字母——单个英文词的输入方式;
                    // 模式去向按 enter_english(默认临时:保持中文模式)。
                    let to = self.cfg.enter_english;
                    self.plan_commit_raw_en(p, to)
                }
            }
            LKey::Backspace => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    p.buf.pop();
                    p.buf_raw.pop();
                    p.page = 0;
                    self.recompute_into(p);
                    composition_effects(self, p)
                }
            }
            LKey::Esc => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    plan_clear(p);
                    vec![Effect::Preedit(None), empty_cands(), Effect::Consumed]
                }
            }
            LKey::PageUp | LKey::PageDown => self.plan_page(p, key == LKey::PageDown),
            LKey::Punct(c) => self.plan_punct(p, c),
            LKey::ShiftPress => {
                // Shift 按下:有缓冲上屏英文原串,去向按 shift_english
                // (默认 English:上屏即进入英文模式,临时英文改用回车);
                // 空缓冲由宿主判定 Shift 单击后切换中英模式。
                if p.buf.is_empty() {
                    vec![Effect::Consumed]
                } else {
                    let to = self.cfg.shift_english;
                    self.plan_commit_raw_en(p, to)
                }
            }
            LKey::Other => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    plan_clear(p);
                    vec![Effect::Preedit(None), empty_cands(), Effect::Pass]
                }
            }
            // 非造词模式的方向键:无组词语义,行为同 Other(有缓冲先清缓冲)。
            LKey::ArrowLeft | LKey::ArrowRight | LKey::ArrowUp | LKey::ArrowDown => {
                if p.buf.is_empty() {
                    vec![Effect::Pass]
                } else {
                    plan_clear(p);
                    vec![Effect::Preedit(None), empty_cands(), Effect::Pass]
                }
            }
        }
    }

    /// 把计划落为引擎内部状态(FFI 在 JSON 确认写入后调用)。
    pub(crate) fn apply_plan(&mut self, plan: Plan) {
        self.buf = plan.buf;
        self.buf_raw = plan.buf_raw;
        self.cands = plan.cands;
        self.page = plan.page;
        self.cn_hit = plan.cn_hit;
        self.quotes = plan.quotes;
        self.mode = plan.mode;
        self.recent = plan.recent;
        self.recent_cost = plan.recent_cost;
        self.last_run = plan.last_run;
        self.coin = plan.coin;
        self.prediction_context = plan.prediction_context;
        self.predicting = plan.predicting;
        if let Some((word, code)) = plan.user_word {
            // 重新造词视为解除屏蔽(§15:删过的词再造回来要能再见到)。
            let _ = self.blocked.remove(&word);
            // 内存索引即时生效;落盘失败不回滚(plan 阶段已做可写预检,
            // 此处属罕见竞态),保 dirty 让下次造词重试。
            if let Err(e) = self.user_words.add(&mut self.dict, &word, &code) {
                eprintln!("lyyime-core: {e}");
            } else {
                // 造的词并入联想索引(频次取词库内该词的实际值,查不到给 0)。
                let freq = self
                    .dict
                    .wubi
                    .iter()
                    .filter(|e| e.word == word)
                    .map(|e| e.freq)
                    .max()
                    .unwrap_or(0);
                self.prediction_index.insert(&word, freq);
            }
        }
        // §15 右键操作的持久化落地(plan 只演算;JSON 确认写出后才到这里)。
        match plan.persist_op {
            Some(PersistOp::Pin { code, word }) => {
                if let Err(e) = self.pins.pin(&code, &word) {
                    eprintln!("lyyime-core: 固定首位落盘失败: {e}");
                }
            }
            Some(PersistOp::Unpin { code }) => {
                if let Err(e) = self.pins.unpin(&code) {
                    eprintln!("lyyime-core: 取消固定落盘失败: {e}");
                }
            }
            Some(PersistOp::Delete { word }) => {
                let _ = self.user_words.remove(&word);
                if let Err(e) = self.blocked.add(&word) {
                    eprintln!("lyyime-core: 删除词组落盘失败: {e}");
                }
                self.pins.drop_word(&word);
            }
            None => {}
        }
        if let Some(word) = plan.learned {
            self.learner.record(&word);
        }
    }

    fn plan_char(&self, p: &mut Plan, c: char) -> Vec<Effect> {
        // 宿主传小写(普通五笔/拼音);Shift 产生的大写键值原样传入,
        // 全大写敲入走大写候选通道(见 recompute_into/caps_candidates),
        // 混合大小写按小写处理(与旧行为一致)。非字母防御性直通。
        if !c.is_ascii_lowercase() && !c.is_ascii_uppercase() {
            return vec![Effect::Pass];
        }
        if p.buf.chars().count() >= MAX_BUF {
            // 缓冲已满:吞掉,维持现有组合(合同 §5.1 ≤12)。
            return vec![Effect::Consumed];
        }
        // 四码顶屏(默认开,借鉴极点五笔"四码顶屏"):恰好四码且首选是
        // 当前码的五笔精确命中时,再来的字母先顶屏首选、该字母开启新组合;
        // 首选是拼音/英文/功能键或缓冲恰是功能键触发词前缀时不顶屏,
        // 继续渐进组词——"niha"+'o' 续拼 nihao、"hell"+'o' 续拼 hello
        // 与长触发词都不被劫持。
        if !self.cfg.pinyin_only
            && self.cfg.commit_on_extra_after_four
            && p.buf.chars().count() == 4
        {
            let trigger_prefix = self.cfg.quick_actions_enabled
                && self
                    .cfg
                    .quick_actions
                    .iter()
                    .any(|a| a.trigger.starts_with(p.buf.as_str()));
            if !trigger_prefix {
                let top_text = plan_page_slice(self, p)
                    .first()
                    .filter(|top| self.plan_wubi_hit(p, top))
                    .map(|top| top.text.clone());
                if let Some(text) = top_text {
                    let mut effects = plan_commit(self, p, &text, true);
                    // 顶屏后立刻进入新组合:联想行不得盖在新组合候选上;
                    // 上下文保留——新词上屏后续接同一上下文窗口。
                    strip_prediction_tail(p, &mut effects, false);
                    p.buf.push(c.to_ascii_lowercase());
                    p.buf_raw.push(c);
                    p.page = 0;
                    self.recompute_into(p);
                    effects.extend(composition_effects(self, p));
                    return effects;
                }
            }
        }
        p.buf.push(c.to_ascii_lowercase());
        p.buf_raw.push(c);
        p.page = 0;
        self.recompute_into(p);
        // 死码保护:原组合还有中文命中时,本键把缓冲推进完全无候选且
        // 拼音语法上也不可达的死胡同,则该字母不进缓冲、原样保留候选状态
        // ——四码未选继续敲击不再把候选清空,避免随后空格/标点把整串字母
        // 当英文直通上屏。
        // 注意:无候选不等于拼音非法——词库缺词时缓冲可能仍是可续接的合法
        // 拼音(如前缀候选被屏蔽后只剩拼写);只要语法上还能续拼就必须收下
        // 该键,交给后续输入或边界键原样处理;只有当前音节表不支持的后缀
        // (如最小夹具无 q 起头音节时的 `jieq`)才走吞键保护。
        if !self.cfg.pinyin_only
            && self.cn_hit
            && p.cands.is_empty()
            && !pinyin::can_continue(&p.buf, &self.dict.syllables, &self.dict.syllable_prefixes)
        {
            // 快速功能键触发词的前缀不受死码保护(§14):触发词允许是任意
            // 小写字母串,若中间态无候选会被吞键,触发词将永远敲不完。
            let trigger_prefix = self.cfg.quick_actions_enabled
                && self
                    .cfg
                    .quick_actions
                    .iter()
                    .any(|a| a.trigger.starts_with(p.buf.as_str()));
            if !trigger_prefix {
                p.buf.pop();
                p.buf_raw.pop();
                p.cands = self.cands.clone();
                p.page = self.page;
                p.cn_hit = self.cn_hit;
                return vec![Effect::Consumed];
            }
        }
        // 四码上屏(借鉴极点/QQ 五笔的"四码自动上屏",两个开关):
        // - commit_first_at_four(默认开):首选是五笔命中且候选条只此一条时
        //   免空格直接上屏;候选条多于一条(同码重码或拼音/简拼混排候选)
        //   一律保留组合,交给空格/数字选词或继续输入由
        //   commit_on_extra_after_four 顶屏;
        // - commit_unique_four:候选唯一时免空格上屏(混打渐进的弱顶屏)。
        // 首选是拼音/英文时不触发前者:"niha" 是 nihao 的中间态、四键不该
        // 劫持拼音长码,"hell" 不该四键上屏英文前缀词。缓冲是快速功能键
        // 触发词(或其前缀)时同样不上屏——功能候选须经用户确认(§14)。
        if !self.cfg.pinyin_only
            && p.buf.chars().count() == 4
            && !p.cands.is_empty()
            && p.cands[0].consumed == 0
            && !matches!(p.cands[0].kind, CandKind::Action(_))
            && !(self.cfg.quick_actions_enabled
                && self
                    .cfg
                    .quick_actions
                    .iter()
                    .any(|a| a.trigger.starts_with(p.buf.as_str())))
        {
            // 首选是五笔命中按词条归属判定(wubi_exact 索引成员),不依赖
            // 候选 kind——学习过的拼音词被标 User 不算五笔命中;"有重码"
            // 按候选条可见总数判定:五笔同码重码与拼音/简拼/英文混排候选
            // 都算重码,多于一条即不自动上屏。
            let first_wubi = self.plan_wubi_hit(p, &p.cands[0]);
            let auto = (self.cfg.commit_first_at_four && first_wubi && p.cands.len() == 1)
                || (self.cfg.commit_unique_four && p.cn_hit && p.cands.len() == 1);
            if auto {
                let text = p.cands[0].text.clone();
                return plan_commit(self, p, &text, true);
            }
        }
        composition_effects(self, p)
    }

    fn plan_digit(&self, p: &mut Plan, n: u8) -> Vec<Effect> {
        let page = plan_page_slice(self, p);
        if page.is_empty() {
            // 无候选:数字原样放行(合同 §6)。
            return vec![Effect::Pass];
        }
        // 0 = 第 10 个候选(主流输入法惯例);页不足 10 条时无此候选,吞键。
        let idx = if n == 0 { 9 } else { (n - 1) as usize };
        if idx >= page.len() {
            // 越界:吞掉,避免数字被注入到正在组合的文本流中。
            return vec![Effect::Consumed];
        }
        let cand = page[idx].clone();
        plan_select(self, p, &cand, true)
    }

    fn plan_space(&self, p: &mut Plan) -> Vec<Effect> {
        // 联想态空缓冲:空格确认联想首选(尾巴上屏);非联想态空缓冲放行。
        if p.buf.is_empty() && !p.predicting {
            return vec![Effect::Pass];
        }
        // Space 是确认键:只要有候选,一律上屏当前选中(首选),不做英文抢占。
        // 英文输入走 Shift 单击切换后的英文直通态;无候选时仍保留原字母兜底。
        // 选中首选是功能键时同样产生 Action 效果(空格=确认首选)。
        if let Some(top) = plan_page_slice(self, p).first().cloned() {
            plan_select(self, p, &top, true)
        } else {
            // 有缓冲无候选:直通原始字母(中英混合,合同 §6)。
            let raw = raw_display(p);
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
        if !self.cfg.cn_punct {
            // 用户关闭中文标点:一律直通(有缓冲同样先提交组合)。
            // 必须先判禁用再碰 to_chinese/QuoteState——ASCII 引号若在
            // 禁用态翻转了引号状态,会污染下一次中文引号对的开合方向。
            return self.plan_commit_then(p, Effect::Pass);
        }
        let mut quotes = p.quotes;
        let punct = to_chinese(c, &mut quotes);
        if punct.is_some() {
            p.quotes = quotes;
        }
        let Some(punct) = punct else {
            // 未映射标点:有缓冲先提交组合再放行,空缓冲直接放行。
            return self.plan_commit_then(p, Effect::Pass);
        };
        if p.buf.is_empty() {
            // 中文态空缓冲:输出对应中文标点(合同 §6)。
            return vec![Effect::Commit(punct.to_string())];
        }
        // 有缓冲:Commit(首选)+ Commit(中文标点)。
        let text = self.finish_text(p);
        let learned = self.learn_worthy(p);
        let mut effects = plan_commit(self, p, &text, learned);
        effects.insert(1, Effect::Commit(punct.to_string()));
        // 标点紧跟在上屏之后:联想行不得残留(候选条不能停在陈旧联想上)。
        strip_prediction_tail(p, &mut effects, true);
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
            consumed: 0,
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
            // 上屏后进入造词展示:联想行让位给造词候选。
            strip_prediction_tail(p, &mut effects, false);
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
        // tail(直通等)在上屏之后:联想行不得残留。
        strip_prediction_tail(p, &mut effects, true);
        effects.push(tail);
        effects
    }

    /// 上屏缓冲原串(英文直通,不学习),并按 `to` 决定模式去向:
    /// [`EnCommit::English`] 时计划态切英文,效果流末尾附
    /// [`Effect::ModeChanged`](宿主同步中英指示/XIM trigger)。
    fn plan_commit_raw_en(&self, p: &mut Plan, to: EnCommit) -> Vec<Effect> {
        let raw = raw_display(p);
        let mut effects = plan_commit(self, p, &raw, false);
        if to == EnCommit::English {
            p.mode = Mode::English;
            effects.push(Effect::ModeChanged(Mode::English));
        }
        effects
    }

    /// 本次组合结束是否值得学习(来自候选顶屏)。
    /// 只有"页内首选且消费整个缓冲"的普通候选才计:前缀候选的组合收尾
    /// 会把未选后缀原样拼在上屏文本里(如 截ping),这种拼接串不是用户
    /// 确认过的词,绝不能让原始后缀混进学习库。
    fn learn_worthy(&self, p: &Plan) -> bool {
        match plan_page_slice(self, p).first() {
            Some(top) => !matches!(top.kind, CandKind::Action(_)) && top.consumed == 0,
            None => false,
        }
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
        if self.cfg.pinyin_only {
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
    /// 首选是前缀候选(consumed>0)时,上屏 = 候选文本 + 剩余后缀的敲入
    /// 原形(如 截 + ping → "截ping"),不猜测后缀对应的汉字——后缀的
    /// 转换只能由用户显式选词完成,边界键只做无损拼接。
    fn finish_text(&self, p: &Plan) -> String {
        match plan_page_slice(self, p).first() {
            Some(top) if !matches!(top.kind, CandKind::Action(_)) => {
                let mut text = top.text.clone();
                if top.consumed > 0 && top.consumed < p.buf.len() {
                    text.push_str(&raw_display(p)[top.consumed..]);
                }
                text
            }
            _ => raw_display(p),
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
        self.buf_raw.clear();
        self.cands.clear();
        self.page = 0;
        self.cn_hit = false;
        // 造词模式不跨焦点延续(预编辑已随 reset 消失,继续吞键会卡输入)。
        self.coin = None;
        // 联想与会话同生死:reset/模式切换等硬边界连同上下文一并作废。
        self.predicting = false;
        self.prediction_context.clear();
    }

    // ------------------------------------------------------------------
    // 候选生成与排序(修订 §5:层级主导的词典序)
    // ------------------------------------------------------------------

    /// 依据计划缓冲重算候选:wubi → pinyin → english(修订 §5.C 门控)。
    /// 只读引擎配置/词库/学习数据,结果写入 plan(纯函数语义,配合两段式处理)。
    fn recompute_into(&self, p: &mut Plan) {
        // 重算 = 新组合候选:联想标记绝不带入(顶屏续打等路径经此重算)。
        p.predicting = false;
        let buf = p.buf.clone();
        let mut pool: HashMap<String, RawCand> = HashMap::new();
        let mut cn_hits = 0usize;
        // 全大写敲入(需求 2026-09-28):候选固定为 大写原样 → 首字母大写 →
        // 全小写 → 中文翻译,跳过五笔/拼音/英文池(大写敲入是"要英文形态"的
        // 明确信号,混入中文候选反而违背预期);翻译缺失时仅大小写变体。
        if !buf.is_empty() && is_all_upper(p) {
            p.cn_hit = false;
            p.cands = self.caps_candidates(p);
            return;
        }
        if !buf.is_empty() {
            if !self.cfg.pinyin_only {
                cn_hits += self.wubi_candidates(&buf, &mut pool);
            }
            cn_hits += self.pinyin_candidates(&buf, &mut pool);
            // 英文通道(修订 §5.C):
            // - 无中文命中 → 前缀候选(不限名次)进 english_no_cn 层;
            // - 有中文命中 → 仅当缓冲本身是 mixed_auto_commit_top_n 内的完整英文词,
            //   以 english_with_cn 层加入单个候选。
            if self.cfg.mixed_en && !self.cfg.pinyin_only {
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
                consumed: rc.consumed,
            })
            .collect();
        // §15 屏蔽词过滤(右键"删除词组"):计划态可用 blocked_override 预演,
        // apply 前真实表不变——FFI 重试不产生副作用。
        {
            let blocked = p.blocked_override.as_ref().unwrap_or(self.blocked.words());
            if !blocked.is_empty() {
                p.cands.retain(|c| !blocked.contains(c.text.as_str()));
            }
        }
        self.insert_quick_actions(p, &buf);
        // §15 固定首位( pinned.tsv ):当前缓冲码的固定词仍在候选里就置顶,
        // 注释尾缀"固"标记,让用户知道为何它恒居第一。
        let pinned = p
            .pins_override
            .as_ref()
            .unwrap_or_else(|| self.pins.map())
            .get(&buf)
            .cloned();
        if let Some(word) = pinned {
            if let Some(pos) = p.cands.iter().position(|c| c.text == word) {
                let mut c = p.cands.remove(pos);
                c.comment = format!("{}·固", c.comment);
                p.cands.insert(0, c);
            }
        }
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
                    comment: self.action_comment(a),
                    score: 0.0,
                    kind: CandKind::Action(i as u8),
                    consumed: 0,
                },
            );
            off += 1;
        }
    }

    /// 功能候选注释:普通为「功能键」;截图(@shot)附配置的热键,
    /// 候选阶段即可看到快捷方式(选中后宿主还会再提示一次,§14)。
    fn action_comment(&self, a: &QuickAction) -> String {
        match a.command.as_str() {
            "@shot" => format!("功能键 热键:{}", self.cfg.shot_hotkey),
            _ => "功能键".to_string(),
        }
    }

    /// 候选是否是当前缓冲码的五笔精确命中(含用户造词——造词时已并入
    /// wubi_exact 索引)。按词条归属判定而非候选 `kind`:学习过的拼音词
    /// 被标 User 不算五笔命中,学习过的五笔词仍算。四码上屏的"首选是
    /// 五笔命中"与四码顶屏的门控共用。
    /// 前缀候选(consumed>0)永不视为五笔命中——即使词面恰好在五笔表内,
    /// 它也只消费缓冲开头,不能当整码命中触发四码上屏/顶屏。
    fn plan_wubi_hit(&self, p: &Plan, cand: &Candidate) -> bool {
        cand.consumed == 0
            && self.dict.wubi_exact.get(&p.buf).is_some_and(|idxs| {
                idxs.iter()
                    .any(|&i| self.dict.wubi[i as usize].word == cand.text)
            })
    }

    /// 五笔通道(§5.2):完全同码(code == buffer,简码奖励并入该层)与前缀渐进分两路查询。
    ///
    /// 精确命中直接查 exact 索引,不受前缀桶截断影响——真实词库中 `a`/`wq` 这类
    /// 高频前缀桶按频次取前 32 时可能把精确码挤出,必须独立保证"精确 > 前缀"。
    ///
    /// 精确层单字按词频排位(可配置 `exact_char_freq_rank`,默认开,依据真实语料):
    /// 语料常用字(归一频率 f ≥ 0.5)保持精确层,但层内 spec 与词组同档 1.0——
    /// 四码重码时由词频 norm 定次序,"不常用单字恒居词组之前"的特权取消;
    /// 低频/生僻字(f < 0.5)再按 [`rank::exact_char_tier`] 频率降档,让位更高频的
    /// 前缀词组。语料未覆盖的 GB2312 表内字取中性 0.5(不降档),表外生僻字取
    /// 低档 0.15 且 spec 0.5 沉在词组之后。关闭开关(或无分档表)恢复旧行为:
    /// 单字恒居精确层顶,层内 spec 按 GB2312 分档 一级 3.0 > 二级 2.5 >
    /// 词组 1.0 > 生僻 0.5。档内单字 norm 按真实语料频次归一,语料未覆盖按
    /// 表频 ×0.1;语料通道缺失时整体回退码表频。用户学习 ×1.5 只在组内生效。
    fn wubi_candidates(&self, buf: &str, pool: &mut HashMap<String, RawCand>) -> usize {
        let dict = &self.dict;
        let mut hits = 0;
        if let Some(idxs) = dict.wubi_exact.get(buf) {
            for &i in idxs {
                let e = &dict.wubi[i as usize];
                let single = e.word.chars().count() == 1;
                let table_norm = rank::norm10(e.freq, dict.wubi_max);
                let tiered = single && !dict.char_tier.is_empty();
                let tier = if tiered { char_tier_of(dict, &e.word) } else { None };
                // 精确层内单字的 spec(exact_char_freq_rank 开):与词组同档 1.0,
                // 由词频 norm 定次序——"四码符合的单字"不再靠档位恒居词组之前,
                // 不常用单字可被高频词组反超(默认词频序);表外生僻字仍沉词组
                // 之后(0.5)。开关关闭或无分档表时恢复旧行为:单字按 GB2312
                // 分档恒居词组之前(3.0/2.5)。
                let spec = match tier {
                    Some(_) if self.cfg.exact_char_freq_rank => 1.0,
                    Some(1) => 3.0,
                    Some(_) => 2.5,
                    None if tiered => 0.5,
                    None if single => 2.0,
                    None => 1.0,
                };
                // 频率档位:单字 + 开关开 + 有分档表时按真实语料归一词频判定;
                // 常用字(f≥0.5)保持精确层恒居首位(体验零变化),低频/生僻字
                // 按频率降档让位词组;语料未覆盖的表内字取中性 0.5(不降),
                // 表外生僻字取低档 0.15。
                let tier_base = if single && self.cfg.exact_char_freq_rank && tiered {
                    let f = match char_corpus_freq(dict, &e.word) {
                        Some(freq) => rank::norm10(freq, dict.char_corpus_max) / 10.0,
                        None if tier == Some(1) || tier == Some(2) => 0.5,
                        None => 0.15,
                    };
                    if f >= 0.5 {
                        rank::TIER_WUBI_EXACT
                    } else {
                        exact_char_tier(f)
                    }
                } else {
                    rank::TIER_WUBI_EXACT
                };
                // 档内单字按语料频;生僻档(表外字,语料是填充值)与无档位表时按表频。
                let corpus_ranked = single
                    && !dict.char_corpus.is_empty()
                    && (!tiered || tier.is_some());
                let norm = if corpus_ranked {
                    match char_corpus_freq(dict, &e.word) {
                        Some(f) => rank::norm10(f, dict.char_corpus_max),
                        None => table_norm * 0.1,
                    }
                } else {
                    table_norm
                };
                insert_cand(
                    pool,
                    e.word.clone(),
                    e.code.clone(),
                    tier_base,
                    spec,
                    norm,
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
            // 末音节单字只允许在"整个缓冲恰好是一个音节"时出(seg.len()==1):
            // 多音节切分下展示尾音节单字会丢弃前面已敲音节(如 nihao 里出
            // 孤立的"好"),前缀路径见下方 ⑤。
            if seg.len() == 1 {
                if let Some(list) = dict.py_chars.get(buf) {
                    for (ch, freq) in list {
                        insert_cand(
                            pool,
                            ch.to_string(),
                            self.wubi_comment(&ch.to_string(), buf.to_string()),
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

        // ⑤ 前缀候选(缺词兜底):缓冲能切成"合法音节前缀 + 可续接后缀"时,
        // 给前缀部分的词组/单字候选——选中只上屏前缀、剩余后缀留在组合里
        // 继续组词(如 jieping 缺"截屏"词时仍可先上屏「截」再选「屏」)。
        // 层级压在简拼之下:任何完整切分/补全/简拼候选都排它前面;
        // 后缀必须语法可续(can_continue),否则不是"前缀+余量"而是死码。
        for k in pinyin::prefix_ends(buf, &dict.syllables) {
            if k >= buf.len()
                || !pinyin::can_continue(
                    &buf[k..],
                    &dict.syllables,
                    &dict.syllable_prefixes,
                )
            {
                continue;
            }
            for seg in pinyin::segmentations(&buf[..k], &dict.syllables, SEGS_CAP) {
                let joined = pinyin::seg_joined(&buf[..k], &seg);
                if let Some(idxs) = dict.py_exact.get(&joined) {
                    for &i in idxs {
                        let phrase = &dict.phrases[i as usize];
                        insert_cand_with_consumed(
                            pool,
                            phrase.word.clone(),
                            self.wubi_comment(&phrase.word, joined.clone()),
                            rank::TIER_PINYIN_ABBREV - 1.0,
                            k as f32,
                            rank::norm10(phrase.freq, dict.py_phrase_max),
                            CandKind::Pinyin,
                            suggestion_of(dict, &phrase.word),
                            k,
                        );
                        hits += 1;
                    }
                }
                if seg.len() == 1 {
                    if let Some(chars) = dict.py_chars.get(&buf[..k]) {
                        for (ch, freq) in chars {
                            let text = ch.to_string();
                            insert_cand_with_consumed(
                                pool,
                                text.clone(),
                                self.wubi_comment(&text, buf[..k].to_string()),
                                rank::TIER_PINYIN_ABBREV - 1.0,
                                k as f32,
                                rank::norm10(*freq, dict.py_char_max),
                                CandKind::Pinyin,
                                suggestion_of(dict, &text),
                                k,
                            );
                            hits += 1;
                        }
                    }
                }
            }
        }
        hits
    }

    /// 反查五笔注释:候选注释统一为五笔编码——拼音命中的词显示其五笔码
    /// (拼音打字也能看到五笔编码,便于学习反查);词不在五笔表时保留
    /// 原通道注释(拼音)兜底。
    fn wubi_comment(&self, word: &str, fallback: String) -> String {
        if self.cfg.pinyin_only {
            return fallback;
        }
        match self.dict.wubi_rev.get(word) {
            Some(code) => code.clone(),
            None => fallback,
        }
    }

    /// 英文通道(修订 §5.C):无中文命中时的前缀候选。
    ///
    /// 候选不限名次(否则长尾词在真实 20k 词表下永不出现,如 hello 列第 ~2400 名)。
    /// 全大写输入候选(需求 2026-09-28):候选顺序固定为
    /// 原样大写 → 首字母大写 → 全小写 → 中文翻译(en_trans 词典序即常用序,
    /// 候选 5、6 起为其它常用翻译);单字母/重复变体去重。
    /// 候选不经排序池,此处顺序即最终顺序;翻译候选以原词作注释。
    fn caps_candidates(&self, p: &Plan) -> Vec<Candidate> {
        let raw = p.buf_raw.clone();
        let lower = p.buf.clone();
        let mut title = lower.clone();
        if let Some(f) = title.get_mut(..1) {
            f.make_ascii_uppercase();
        }
        let mut out: Vec<Candidate> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut push = |text: String, comment: &str, score: f32| {
            if seen.insert(text.clone()) {
                out.push(Candidate {
                    text,
                    comment: comment.to_string(),
                    score,
                    kind: CandKind::English,
                    consumed: 0,
                });
            }
        };
        push(raw.clone(), "en", 60.3);
        push(title, "en", 60.2);
        push(lower.clone(), "en", 60.1);
        for t in self.dict.en_trans.get(&lower).into_iter().flatten() {
            push(t.clone(), &raw, 60.0);
        }
        out.truncate(MAX_CANDS);
        out
    }

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

/// 清空计划中的组合状态(联想行也在其列;联想上下文由调用方按需取舍)。
fn plan_clear(p: &mut Plan) {
    p.buf.clear();
    p.buf_raw.clear();
    p.cands.clear();
    p.page = 0;
    p.cn_hit = false;
    p.predicting = false;
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
    // 联想上下文:上屏文本全为 CJK 时续接(保留尾部 MAX_CTX_CHARS 字),
    // 含标点/西文/数字的上屏一律打断(标点直通 commit 不走本函数,
    // 由 plan_key 的边界清理兜底)。
    if !text.is_empty() && text.chars().all(is_cjk) {
        p.prediction_context.push_str(text);
        let keep = prediction::MAX_CTX_CHARS;
        let len = p.prediction_context.chars().count();
        if len > keep {
            p.prediction_context = p.prediction_context.chars().skip(len - keep).collect();
        }
    } else {
        p.prediction_context.clear();
    }
    // plan_clear 已复位联想标记:本键若是联想尾巴上屏,清缓冲后才决定
    // 是否再出下一批联想;落空时 predicting 不得残留(否则空候选页上
    // 空格会走错分支)。
    plan_clear(p);
    // 上屏后联想:命中的可续接尾巴占候选条原位(可空格/数字/点选续打);
    // 无联想才出词组效率提示——提示写在候选条同一位置,不能盖掉可点选的联想行。
    let preds = if eng.cfg.next_word_prediction
        && p.mode == Mode::Chinese
        && !p.prediction_context.is_empty()
    {
        eng.prediction_index
            .candidates(&p.prediction_context, &eng.blocked, usize::MAX)
    } else {
        Vec::new()
    };
    let mut effects = vec![Effect::Commit(text.to_string()), Effect::Preedit(None)];
    if preds.is_empty() {
        effects.push(empty_cands());
        // 词组效率提示排在清除之后:宿主候选条先隐藏再展示提示,
        // 并保留到下一次输入产生新效果流时才替换/清除。
        if let Some(hint) = eng.plan_phrase_hint(p) {
            effects.push(hint);
        }
    } else {
        p.cands = preds;
        p.page = 0;
        p.predicting = true;
        effects.push(Effect::Candidates(Arc::new(
            plan_page_slice(eng, p).to_vec(),
        )));
    }
    effects
}

/// plan_commit 已发出联想页、本帧还要继续追加效果(标点/顶屏续打/造词
/// 入口等)时调用:撤销联想状态与效果流里的联想候选,末尾补空候选清除,
/// 保证宿主候选条不会在标点等后续效果之后残留陈旧联想行。
/// `boundary` = 本键属于输入边界时把联想上下文一并作废(标点直通等);
/// 顶屏续打(随后即开始新组合)保留上下文。
fn strip_prediction_tail(p: &mut Plan, effects: &mut Vec<Effect>, boundary: bool) {
    if !p.predicting {
        return;
    }
    if let Some(pos) = effects
        .iter()
        .rposition(|e| matches!(e, Effect::Candidates(_)))
    {
        effects.remove(pos);
    }
    p.predicting = false;
    p.cands.clear();
    p.page = 0;
    if boundary {
        p.prediction_context.clear();
    }
    effects.push(empty_cands());
}

/// 选中一条候选的上屏规划(数字/鼠标/空格确认共用):功能键候选(合同 §14)
/// → [`Effect::Action`](宿主执行功能,不上屏文本、不学习、不入造词历史);
/// 前缀候选(consumed>0)→ 只上屏候选文本、剩余后缀留在组合里继续编辑;
/// 普通候选 → [`plan_commit`]。
fn plan_select(eng: &Engine, p: &mut Plan, cand: &Candidate, learned: bool) -> Vec<Effect> {
    if let CandKind::Action(i) = cand.kind {
        plan_clear(p);
        // 功能动作是输入边界:点选路径不经 plan_key 的边界清理,
        // 联想上下文在此一并作废。
        p.prediction_context.clear();
        return vec![
            Effect::Action(i as usize),
            Effect::Preedit(None),
            empty_cands(),
        ];
    }
    let learn = learned && !p.predicting;
    // 前缀候选:只消费缓冲开头 consumed 个 ASCII 字节——先把缓冲截成前缀
    // 长度再上屏(学习/实耗键数按前缀计,后缀不算入),随后把原始后缀
    // 放回缓冲并重算候选。空格/数字/鼠标点选共用本路径,后缀保持可编辑。
    if cand.consumed > 0 && cand.consumed < p.buf.len() {
        let rest = p.buf[cand.consumed..].to_string();
        let rest_raw = p.buf_raw[cand.consumed..].to_string();
        p.buf.truncate(cand.consumed);
        p.buf_raw.truncate(cand.consumed);
        let mut effects = plan_commit(eng, p, &cand.text, learn);
        // 上屏后联想/提示绝不能盖住未完成的组合:只保留真正的 Commit,
        // 剩余编码的 preedit/候选由本帧末尾的效果重绘。
        effects.retain(|e| matches!(e, Effect::Commit(_)));
        plan_clear(p);
        p.buf = rest;
        p.buf_raw = rest_raw;
        eng.recompute_into(p);
        effects.extend(composition_effects(eng, p));
        return effects;
    }
    // 联想行上屏的是"尾巴"而非整词:不写学习记录(半截尾巴入学会污染
    // 用户词典排序;上屏统计仍按实际 commit 计)。
    plan_commit(eng, p, &cand.text, learn)
}

/// CJK 统一表意字符(含扩展 A/兼容区;造词只认汉字)。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF)
}

/// 用户数据区文件路径:与用户词典同目录(user_words/pinned/blocked 共目录,
/// 目录由 user_dict 覆盖推导)。
fn wordops_path(cfg: &Config, name: &str) -> std::path::PathBuf {
    let base = cfg
        .user_dict
        .clone()
        .unwrap_or_else(Config::default_user_dict_path);
    match base.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
        _ => std::path::PathBuf::from(name),
    }
}

/// 造词库路径:与用户词典同目录的 `user_words.tsv`(目录由 user_dict 覆盖)。
fn user_words_path(cfg: &Config) -> std::path::PathBuf {
    wordops_path(cfg, "user_words.tsv")
}

/// 缓冲是否处于全大写敲入态:buf_raw 非空且全部字符为大写字母。
/// (混合大小写一经出现即回退普通通道,is_all_upper 随之变 false。)
fn is_all_upper(p: &Plan) -> bool {
    !p.buf_raw.is_empty() && p.buf_raw.chars().all(|c| c.is_ascii_uppercase())
}

/// 组合的展示/原样上屏文本:全大写敲入保留大写原形,其余为小写缓冲。
fn raw_display(p: &Plan) -> String {
    if p.buf_raw.is_empty() {
        p.buf.clone()
    } else {
        p.buf_raw.clone()
    }
}

/// 组合中的效果流:[Preedit, Candidates]。
fn composition_effects(eng: &Engine, p: &Plan) -> Vec<Effect> {
    vec![
        Effect::Preedit(Some(raw_display(p))),
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
