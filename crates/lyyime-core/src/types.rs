//! 核心抽象类型:`LKey`(抽象逻辑键)、`Effect`(效果流)、`Candidate`(候选)、
//! `Mode`(中英模式)与 `CandKind`(候选来源)。
//!
//! core 不感知 X11/ibus:宿主把物理按键翻译成 `LKey` 喂进来,
//! core 返回一串按处理顺序排列的 `Effect`,由宿主负责渲染与上屏(见 ARCHITECTURE.md §1/§2)。

use std::sync::Arc;

/// 输入法工作模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// 中文模式:缓冲字母、出候选、中文标点。
    Chinese,
    /// 英文模式:一切按键直通(Pass),仅 Shift 单击等宿主行为除外。
    English,
}

/// 抽象逻辑键。宿主(ibus python / X11 app)负责把真实按键事件翻译成本类型,
/// 其中字母必须先小写化;core 对大写字母做防御性直通。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LKey {
    /// 小写字母 a–z(宿主负责小写化)。
    Char(char),
    /// 数字 '0'..'9' → 0..9,用于候选选词(0 = 第 10 个,页不足 10 条时吞键)。
    Digit(u8),
    /// 空格:顶屏首选 / 直通原字母 / 空缓冲时放行。
    Space,
    /// 回车:有缓冲上屏原始字母,空缓冲放行。
    Enter,
    /// 退格:删缓冲尾字符,空则放行。
    Backspace,
    /// Esc:清缓冲,空则放行。
    Esc,
    /// 翻上一页(宿主一般映射自 `-`)。
    PageUp,
    /// 翻下一页(宿主一般映射自 `=`)。
    PageDown,
    /// 标点原字符(半角),core 按 §6 决定中文标点/上屏/放行。
    Punct(char),
    /// Shift 按下。core 不判定"单击";宿主判定为单击后应直接调 [`crate::Engine::toggle_mode`]。
    ShiftPress,
    /// 造词热键(默认 Ctrl+=,`coin_hotkey` 可配置;宿主解析组合后送入本键)。
    /// 中文态空缓冲进入造词模式,组合中先上屏再进入(合同 §12)。
    Coin,
    /// 方向键 ←:造词模式少选一个字;非造词模式行为同 [`LKey::Other`]。
    ArrowLeft,
    /// 方向键 →:造词模式多选一个字;非造词模式行为同 [`LKey::Other`]。
    ArrowRight,
    /// 方向键 ↑:造词模式多选一个字;非造词模式行为同 [`LKey::Other`]。
    ArrowUp,
    /// 方向键 ↓:造词模式少选一个字;非造词模式行为同 [`LKey::Other`]。
    ArrowDown,
    /// 其余键:core 恒回 [`Effect::Pass`](有缓冲时先清缓冲)。
    Other,
}

/// 候选来源类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CandKind {
    /// 五笔通道命中(全码/简码/词组)。
    Wubi,
    /// 拼音通道命中(全拼/简拼/单字/词组)。
    Pinyin,
    /// 英文通道命中(中英混合)。
    English,
    /// 用户词(学习过、带 user.tsv 加成)。
    User,
    /// 快速功能键(合同 §14):值 = 配置列表 `quick_actions` 的下标;
    /// 选中(数字/鼠标/空格顶屏)产生 [`Effect::Action`],宿主执行功能,
    /// 不上屏文本、不学习。
    Action(u8),
}

/// 候选右键菜单操作(合同 §15):宿主(候选窗/悬浮窗)右键候选行触发,
/// 经 `Engine::cand_op` / FFI `lyyime_cand_op` 执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CandOp {
    /// 固定首位:当前缓冲编码下该词恒居候选第一;已固定时同操作取消固定。
    PinToggle,
    /// 删除词组:用户造词从 `user_words.tsv` 移除并加入屏蔽表
    /// (`blocked.tsv`);内置词条不改词典文件、只屏蔽显示。
    Delete,
    /// 反查英文:当前候选页替换为该中文词的英文反查结果
    /// (`zh_en.tsv`,可继续数字/点选上屏);无结果回 `Effect::Notice`。
    EnLookup,
}

/// 一条候选:上屏文本 + 注释(编码/拼音提示)+ 得分 + 来源。
#[derive(Clone, Debug)]
pub struct Candidate {
    /// 上屏文本(UTF-8)。
    pub text: String,
    /// 注释:五笔为编码、拼音为音节串、英文为 "en";供候选窗展示。
    pub comment: String,
    /// 排序得分(合同 §5 公式,越大越靠前)。
    pub score: f32,
    /// 来源类别。
    pub kind: CandKind,
}

/// 效果流:core 处理完一个键后要求宿主执行的动作序列。
///
/// `process_key` 返回列表的顺序即宿主处理顺序,典型形态:
/// `[Preedit, Candidates]`、`[Commit]`、`[Commit, Preedit(None)]`。
#[derive(Clone, Debug)]
pub enum Effect {
    /// 上屏文本(UTF-8),宿主负责注入;core 侧缓冲已清。
    Commit(String),
    /// 预编辑串更新(None = 清除)。
    Preedit(Option<String>),
    /// 当前候选页更新(页码/总页数信息由 FFI JSON 承载,见 §3)。
    Candidates(Arc<Vec<Candidate>>),
    /// 宿主必须把原键交给应用放行(ibus 返回 false;app 用 XTest 回放)。
    Pass,
    /// 吞掉该键但不产生可见效果。
    Consumed,
    /// 辅助区临时提示(造词成功/失败等;宿主展示数秒后自行清除)。
    Notice(String),
    /// 候选条效率提示(词组提示):刚上屏的几个字有更省键的词组时给出
    /// 「词 + 编码」。宿主展示在候选条且**不带定时**,直到下一次输入
    /// (任何产生效果流的按键)才替换/清除。
    Hint(String),
    /// 模式已变化,宿主更新中/EN 指示。
    ModeChanged(Mode),
    /// 快速功能键命中(合同 §14):值 = 配置列表 `quick_actions` 的下标。
    /// 宿主按 `command` 执行功能(`@settings`/`@help` 内置或 shell 命令),
    /// 不上屏任何文本;候选条随之清除(本效果流已含清除效果)。
    Action(usize),
}
