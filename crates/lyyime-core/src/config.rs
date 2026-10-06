//! 全局配置(`~/.config/lyyime/config.toml`),doctor / app / ibus 引擎共用(ARCHITECTURE.md §4)。
//!
//! 设计要点:
//! - 所有字段都有默认值,配置文件只写需要改的项(缺项回退默认);
//! - 文件缺失 → 返回默认配置(正常首次使用,不是错误);
//! - 文件损坏/取值非法 → 返回 `Err`(消息为人话),调用方可据此提示并回退默认值;
//! - [`Config::example_toml`] 输出带全部中文注释的模板,供安装器/设置界面生成初始配置。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::types::Mode;

/// 快速功能键(合同 §14):输入缓冲与 `trigger` 完全相等时候选条追加功能候选,
/// 数字键/鼠标点选后宿主执行 `command`,不上屏文本。
///
/// - `trigger`:小写字母 1–12 个(受缓冲上限与"数字键是选词键"约束);
/// - `label`:候选展示文本(如「打开配置」);注释固定为「功能键」;
/// - `command`:`@settings` / `@help` 为宿主内置功能,其余按 shell 命令执行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickAction {
    pub trigger: String,
    pub label: String,
    pub command: String,
}

impl QuickAction {
    /// trigger 合法性:小写字母 1–12 个(数字键是选词键,不允许进触发词)。
    pub fn trigger_valid(trigger: &str) -> bool {
        (1..=12).contains(&trigger.len())
            && trigger.bytes().all(|b| b.is_ascii_lowercase())
    }
}

/// 快速功能键条目上限(每页最多 9 个候选,功能键不该挤占整页)。
pub const QUICK_ACTIONS_MAX: usize = 8;

/// 回车/Shift 上屏英文原串后的模式去向(两键共用一套取值,§6):
/// - `Temp`:临时英文——仅本次原样上屏,保持中文模式;
/// - `English`:长久英文——上屏并切入英文模式(效果流末尾附
///   [`Effect::ModeChanged`](crate::types::Effect::ModeChanged),宿主同步
///   中英指示/XIM trigger;切回中文仍走 Shift 单击)。
///
/// 取值借鉴主流输入法(搜狗/QQ 拼音)的"回车上屏英文/Shift 切英文"习惯:
/// 回车天然是"把这一串字母当英文交出去",Shift 天然是"转入英文态"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnCommit {
    /// 临时:仅本次上屏,保持中文模式。
    Temp,
    /// 长久:上屏并切入英文模式。
    English,
}

impl EnCommit {
    /// TOML 取值解析:`temp`/`temporary` → Temp;`en`/`english`/`persist`
    /// → English;空串或未知值返回 None(调用方按各键默认值回退,宽恕手写)。
    fn from_toml(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "temp" | "temporary" => Some(Self::Temp),
            "en" | "english" | "persist" => Some(Self::English),
            _ => None,
        }
    }

    /// 规范写回值(example_toml / C 宿主写盘共用)。
    fn as_toml(self) -> &'static str {
        match self {
            Self::Temp => "temp",
            Self::English => "en",
        }
    }
}

/// 快速功能键默认表(合同 §14):设置两个触发词、截图(带热键提示)、帮助。
pub fn default_quick_actions() -> Vec<QuickAction> {
    vec![
        QuickAction {
            trigger: "peizhi".to_string(),
            label: "打开配置".to_string(),
            command: "@settings".to_string(),
        },
        QuickAction {
            trigger: "shezhi".to_string(),
            label: "设置".to_string(),
            command: "@settings".to_string(),
        },
        QuickAction {
            trigger: "jietu".to_string(),
            label: "截图".to_string(),
            command: "@shot".to_string(),
        },
        QuickAction {
            trigger: "bangzhu".to_string(),
            label: "帮助".to_string(),
            command: "@help".to_string(),
        },
    ]
}

/// 引擎配置(字段语义见各注释;与 config.toml 一一对应)。
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// 启动时的默认模式(cn/en);`Engine::set_config` 会把当前模式重置为该值。
    pub mode: Mode,
    /// 候选窗每页条数,1–10(数字键 1–9/0 选词,0 = 第 10 个;页不足 10 条时 0 吞键)。
    pub page_size: usize,
    pub pinyin_only: bool,
    /// 中英混合:缓冲无任何中文命中时,给出英文词候选。
    pub mixed_en: bool,
    /// 混合直通:缓冲本身是高频英文词(前 [`Config::en_freq_top_n`] 名)时,
    /// 遇标点自动上屏原词;空格始终是确认键,顶屏当前首选。
    pub mixed_auto_commit: bool,
    /// 中文态输出中文标点(关闭则标点一律直通)。
    pub cn_punct: bool,
    /// 用户词学习:累计到 user.tsv 并参与加成排序。
    pub learning: bool,
    /// 用户词典路径覆盖;None = 默认 `~/.local/share/lyyime/user.tsv`。
    pub user_dict: Option<PathBuf>,
    /// 词库目录覆盖;None = 由宿主决定(即 `Engine::new` 的入参)。
    pub data_dir: Option<PathBuf>,
    /// 英文自动上屏的高频词名次上限(词频表前 N 名)。
    pub en_freq_top_n: usize,
    /// 有中文命中时仍以英文候选/直通的名次上限(修订 §5.C:前 N 名完整英文词)。
    pub mixed_auto_commit_top_n: usize,
    /// 四码顶屏(默认开,借鉴极点五笔"四码顶屏"):恰好四码且首选是五笔
    /// 命中(Wubi/User)时,再输入字母先上屏首选,该字母开启新组合;
    /// 首选是拼音/英文/功能键或缓冲是触发词前缀时不顶屏,继续渐进组词。
    pub commit_on_extra_after_four: bool,
    /// 四码首选上屏(默认开,借鉴极点/QQ 五笔的"四码自动上屏"):恰好输入
    /// 四码、首选是五笔命中且候选条只此一条时,免空格直接上屏首选;
    /// 候选条多于一条(同码重码或拼音/简拼/英文混排候选)一律保留组合
    /// 等选词,或继续输入由 [`Config::commit_on_extra_after_four`] 顶屏
    /// 首选。首选是拼音/英文/功能键时不触发(拼音长码的中间态不被打断,
    /// "hell" 这类英文前缀词不会四键即上屏)。关闭后回退
    /// [`Config::commit_unique_four`] 的"仅候选唯一才上屏"判定。
    pub commit_first_at_four: bool,
    /// 四码唯一上屏:恰好输入四码、有中文命中且候选唯一时,免空格直接上屏
    /// 该候选(主流五笔的"四码唯一自动上屏"习惯);多候选或唯一候选是
    /// 英文词时不触发,保持混打渐进。`commit_first_at_four` 开启时本项
    /// 被覆盖(唯一五笔候选已直接上屏,无需判唯一)。
    pub commit_unique_four: bool,
    /// 回车上屏英文原串后的模式去向(默认 [`EnCommit::Temp`]:临时英文,
    /// 单个英文词的输入方式,上屏后保持中文模式)。
    pub enter_english: EnCommit,
    /// Shift 上屏英文原串后的模式去向(默认 [`EnCommit::English`]:上屏并
    /// 进入英文模式;临时英文走回车,或把本键配成 Temp 恢复旧版"仅上屏")。
    pub shift_english: EnCommit,
    /// 精确层单字按词频参与排位(默认开):全码/简码精确命中的单字不再
    /// 无条件恒居首位——按真实语料词频取频率档位,低频/生僻字可被更高频的
    /// 前缀词组反超(高频字仍居精确层顶部);关闭则恢复"恒居首位"旧行为。
    pub exact_char_freq_rank: bool,
    /// 词组效率提示:上屏后最近几个字若存在更省键的五笔词组,在候选条
    /// 提示「词 + 编码」,直到下一次输入才清除(借鉴万能五笔的高效词提示)。
    pub phrase_hint: bool,
    /// 上屏后联想(默认关):中文词上屏后,候选条按本地词典给出接下来可能
    /// 输入的词句尾巴(数字/空格/点选上屏尾巴,字母开始新组合,Esc 关闭);
    /// 纯本地词表反查,无网络/AI 调用。关闭后不再出联想行。
    pub next_word_prediction: bool,
    /// 造词热键(合同 §12):`修饰+键` 串,宿主解析;core 不消费该值,
    /// 收纳于此保证 config.toml 一份 schema 三端(doctor/ibus/xim)共用。
    /// 修饰:ctrl/alt/super/shift;键名:a-z 0-9 f1-f12 equal/minus/space 等。
    pub coin_hotkey: String,
    /// 截屏快捷键(合同 §13):拉起 `lyyime-shot` 框选截屏;写法同造词热键
    /// (修饰+键名)。core 不消费该值,仅集中 schema 供各宿主解析匹配。
    pub shot_hotkey: String,
    /// 快速功能键总开关(合同 §14):关闭后触发词不再产生功能候选。
    pub quick_actions_enabled: bool,
    /// 快速功能键列表(合同 §14);触发词整串命中时候选条追加功能候选。
    pub quick_actions: Vec<QuickAction>,
    /// 菜单触发总开关(上屏文字命中菜单功能名 → 提示后按确认键执行;默认开)。
    /// 与快速功能键不同源:本特性只匹配**真实上屏的中文文本**,不读字母缓冲;
    /// core 不消费该值,收纳于此保证 config.toml 一份 schema 三端共用。
    pub menu_trigger_enabled: bool,
    /// 菜单触发确认键:F1–F12 序号(默认 7 = F7;越界回退默认,不判整份损坏)。
    pub menu_trigger_key: usize,
    /// 菜单触发黑名单:逗号分隔的稳定 id(默认仅 `fix_ime` 修复输入法;
    /// 逐项精确比较,未知 id 原样保留以便前向兼容)。
    pub menu_trigger_disabled: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Chinese,
            page_size: 10,
            pinyin_only: false,
            mixed_en: true,
            mixed_auto_commit: true,
            cn_punct: true,
            learning: true,
            user_dict: None,
            data_dir: None,
            en_freq_top_n: 2000,
            mixed_auto_commit_top_n: 500,
            commit_on_extra_after_four: true,
            commit_first_at_four: true,
            commit_unique_four: true,
            enter_english: EnCommit::Temp,
            shift_english: EnCommit::English,
            exact_char_freq_rank: true,
            phrase_hint: true,
            next_word_prediction: false,
            coin_hotkey: "ctrl+equal".to_string(),
            shot_hotkey: "ctrl+alt+a".to_string(),
            quick_actions_enabled: true,
            quick_actions: default_quick_actions(),
            menu_trigger_enabled: true,
            menu_trigger_key: crate::menu_trigger::MENU_KEY_DEFAULT as usize,
            menu_trigger_disabled: crate::menu_trigger::MENU_DISABLED_DEFAULT.to_string(),
        }
    }
}

/// TOML 中间层:字段全可选,缺项落到默认值。
/// 注意:`#[serde(default)]` 以本类型 `Default` 为底,而这里手动实现为
/// `Config::default()` 的镜像——保证只写一项的配置文件,其余项仍是官方默认(而非零值)。
#[derive(Serialize, Deserialize, Debug)]
#[serde(default)]
struct ConfigToml {
    mode: String,
    page_size: usize,
    pinyin_only: bool,
    mixed_en: bool,
    mixed_auto_commit: bool,
    /// 旧别名(早期 core 写盘键名);规范键 `chinese_punct` 缺失时才生效。
    #[serde(skip_serializing_if = "Option::is_none")]
    cn_punct: Option<bool>,
    /// 规范键名(XIM 设置窗写盘键名);与旧别名 `cn_punct` 并存时以本键
    /// 为准——设置界面保存的值优先(见 [`ConfigToml::into_config`])。
    #[serde(skip_serializing_if = "Option::is_none")]
    chinese_punct: Option<bool>,
    learning: bool,
    user_dict: Option<String>,
    data_dir: Option<String>,
    en_freq_top_n: usize,
    mixed_auto_commit_top_n: usize,
    /// 规范键名;`None` 时回退别名 `commit_after_four`(XIM 设置窗写盘键名)
    /// 或内置默认,见 [`ConfigToml::into_config`]。
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_on_extra_after_four: Option<bool>,
    /// `commit_on_extra_after_four` 的别名(XIM 写盘键名);规范键缺失时才生效。
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_after_four: Option<bool>,
    commit_first_at_four: bool,
    commit_unique_four: bool,
    enter_english: String,
    shift_english: String,
    exact_char_freq_rank: bool,
    phrase_hint: bool,
    next_word_prediction: bool,
    coin_hotkey: String,
    shot_hotkey: String,
    quick_actions_enabled: bool,
    quick_actions: Vec<QuickAction>,
    menu_trigger_enabled: bool,
    menu_trigger_key: usize,
    menu_trigger_disabled: String,
}

impl Default for ConfigToml {
    fn default() -> Self {
        let mut t = ConfigToml::from(&Config::default());
        // 带别名的键以 Option 呈现,缺失(None)才可被别名/内置默认接管。
        t.commit_on_extra_after_four = None;
        t.commit_after_four = None;
        t.cn_punct = None;
        t.chinese_punct = None;
        t
    }
}

impl From<&Config> for ConfigToml {
    fn from(c: &Config) -> Self {
        Self {
            mode: match c.mode {
                Mode::Chinese => "cn".to_string(),
                Mode::English => "en".to_string(),
            },
            page_size: c.page_size,
            pinyin_only: c.pinyin_only,
            mixed_en: c.mixed_en,
            mixed_auto_commit: c.mixed_auto_commit,
            cn_punct: Some(c.cn_punct),
            chinese_punct: None,
            learning: c.learning,
            user_dict: c
                .user_dict
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            data_dir: c
                .data_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            en_freq_top_n: c.en_freq_top_n,
            mixed_auto_commit_top_n: c.mixed_auto_commit_top_n,
            commit_on_extra_after_four: Some(c.commit_on_extra_after_four),
            commit_after_four: None,
            commit_first_at_four: c.commit_first_at_four,
            commit_unique_four: c.commit_unique_four,
            enter_english: c.enter_english.as_toml().to_string(),
            shift_english: c.shift_english.as_toml().to_string(),
            exact_char_freq_rank: c.exact_char_freq_rank,
            phrase_hint: c.phrase_hint,
            next_word_prediction: c.next_word_prediction,
            coin_hotkey: c.coin_hotkey.clone(),
            shot_hotkey: c.shot_hotkey.clone(),
            quick_actions_enabled: c.quick_actions_enabled,
            quick_actions: c.quick_actions.clone(),
            menu_trigger_enabled: c.menu_trigger_enabled,
            menu_trigger_key: c.menu_trigger_key,
            menu_trigger_disabled: c.menu_trigger_disabled.clone(),
        }
    }
}

impl Config {
    /// 读取 TOML 配置文件。
    ///
    /// - 文件不存在:`Ok(Config::default())`(首次使用,属正常);
    /// - 文件损坏/字段非法:`Err`(人话消息,提示可回退默认);
    /// - 部分字段缺失:缺失项取默认值。
    pub fn load(path: &Path) -> Result<Config, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => {
                return Err(Error::new(format!(
                    "无法读取配置文件 {}: {e}(请检查文件权限)",
                    path.display()
                )))
            }
        };
        let raw: ConfigToml = toml::from_str(&text).map_err(|e| {
            Error::new(format!(
                "配置文件 {} 已损坏: {e};可修正该文件或删除后重启,程序将回退默认配置",
                path.display()
            ))
        })?;
        raw.into_config(path)
    }

    /// 生成带全部中文注释的默认配置模板(供安装器/设置界面初始化 config.toml)。
    pub fn example_toml() -> String {
        let d = Config::default();
        let mut s = String::new();
        s.push_str("# lyyIme 配置文件(~/.config/lyyime/config.toml)\n");
        s.push_str("# 全部字段可省略,省略即使用下方默认值;修改保存后对宿主即时生效。\n\n");
        let _ = writeln!(
            s,
            "# 启动默认模式:cn = 中文,en = 英文(Shift 单击可随时切换)"
        );
        let _ = writeln!(
            s,
            "mode = \"{}\"",
            if d.mode == Mode::Chinese { "cn" } else { "en" }
        );
        let _ = writeln!(s, "\n# 候选窗每页条数(1–10,数字键 1–9/0 选词,0 = 第 10 个)");
        let _ = writeln!(s, "page_size = {}", d.page_size);
        let _ = writeln!(
            s,
            "\n# 输入方案:true = 纯拼音(候选只出拼音,不出五笔/英文,四码规则不生效);"
        );
        let _ = writeln!(s, "# false = 五笔/拼音混输(默认)");
        let _ = writeln!(s, "pinyin_only = {}", d.pinyin_only);
        let _ = writeln!(s, "\n# 中英混合:输入无中文命中时给出英文词候选");
        let _ = writeln!(s, "mixed_en = {}", d.mixed_en);
        let _ = writeln!(
            s,
            "\n# 混合直通:高频英文词(见 en_freq_top_n)遇标点自动上屏原词"
        );
        let _ = writeln!(s, "mixed_auto_commit = {}", d.mixed_auto_commit);
        let _ = writeln!(
            s,
            "\n# 中文态输出中文标点(如 ， 。 ？ ！;关闭则标点原样直通;Ctrl+. 可临时切换;"
        );
        let _ = writeln!(s, "# 旧别名 cn_punct 仍被识别,两键并存时以本键为准)");
        let _ = writeln!(s, "chinese_punct = {}", d.cn_punct);
        let _ = writeln!(s, "\n# 用户词学习:上屏候选累计词频,越用越顺手");
        let _ = writeln!(s, "learning = {}", d.learning);
        let _ = writeln!(
            s,
            "\n# 用户词典路径覆盖(默认 ~/.local/share/lyyime/user.tsv;留空使用默认)"
        );
        let _ = writeln!(
            s,
            "user_dict = {}",
            d.user_dict
                .as_ref()
                .map(|p| format!("\"{}\"", p.display()))
                .unwrap_or_else(|| "\"\"".into())
        );
        let _ = writeln!(s, "\n# 词库目录覆盖(留空使用宿主默认,如 data/runtime/)");
        let _ = writeln!(
            s,
            "data_dir = {}",
            d.data_dir
                .as_ref()
                .map(|p| format!("\"{}\"", p.display()))
                .unwrap_or_else(|| "\"\"".into())
        );
        let _ = writeln!(
            s,
            "\n# 英文自动上屏的高频词名次上限(词频表前 N 名才自动直通)"
        );
        let _ = writeln!(s, "en_freq_top_n = {}", d.en_freq_top_n);
        let _ = writeln!(
            s,
            "\n# 中英混合:有中文命中时,词频前 N 名的完整英文词仍给出候选并可直接直通"
        );
        let _ = writeln!(s, "mixed_auto_commit_top_n = {}", d.mixed_auto_commit_top_n);
        let _ = writeln!(
            s,
            "\n# 四码顶屏(默认开):恰好四码且首选是五笔命中时,再输入字母先上屏首选,"
        );
        let _ = writeln!(
            s,
            "# 该字母开始新组合;首选是拼音/英文时不顶屏,继续渐进组词(XIM 写盘键名"
        );
        let _ = writeln!(s, "# commit_after_four,与本键同义)");
        let _ = writeln!(
            s,
            "commit_on_extra_after_four = {}",
            d.commit_on_extra_after_four
        );
        let _ = writeln!(
            s,
            "\n# 四码首选上屏(默认开):恰好四码且首选是五笔命中、候选条只此一条时免空格"
        );
        let _ = writeln!(
            s,
            "# 直接上屏首选;候选多于一条(重码或混排候选)保留组合等选词/顶屏;首选是拼音/英文时不触发"
        );
        let _ = writeln!(s, "# (不打断拼音长码与英文单词)");
        let _ = writeln!(
            s,
            "commit_first_at_four = {}",
            d.commit_first_at_four
        );
        let _ = writeln!(
            s,
            "\n# 四码唯一上屏:恰好四码且候选唯一时免空格直接上屏(commit_first_at_four"
        );
        let _ = writeln!(s, "# 开启时本项不生效;关闭后多候选仍需空格/数字选词)");
        let _ = writeln!(s, "commit_unique_four = {}", d.commit_unique_four);
        let _ = writeln!(
            s,
            "\n# 回车上屏英文原串后的去向:temp = 临时(默认,上屏后保持中文模式,"
        );
        let _ = writeln!(
            s,
            "# 作为单个英文词的输入方式);en = 长久(上屏并切入英文模式)"
        );
        let _ = writeln!(
            s,
            "enter_english = \"{}\"",
            d.enter_english.as_toml()
        );
        let _ = writeln!(
            s,
            "\n# Shift 上屏英文原串后的去向:en = 长久(默认,上屏并进入英文模式,"
        );
        let _ = writeln!(
            s,
            "# 之后按键直通,Shift 单击切回中文);temp = 临时(仅上屏,保持中文)"
        );
        let _ = writeln!(
            s,
            "shift_english = \"{}\"",
            d.shift_english.as_toml()
        );
        let _ = writeln!(
            s,
            "\n# 精确层单字按词频排位(默认开):四码/简码精确命中的单字不再恒居"
        );
        let _ = writeln!(
            s,
            "# 首位——按真实语料词频取档,低频/生僻字可被更高频的词组反超;"
        );
        let _ = writeln!(s, "# 关闭则恢复\"精确单字恒居首位\"的旧行为");
        let _ = writeln!(s, "exact_char_freq_rank = {}", d.exact_char_freq_rank);
        let _ = writeln!(
            s,
            "\n# 词组效率提示:上屏后最近几个字有更省键的五笔词组时,候选条提示词组与编码"
        );
        let _ = writeln!(s, "phrase_hint = {}", d.phrase_hint);
        let _ = writeln!(
            s,
            "\n# 上屏后联想(默认关):中文词上屏后候选条给出接下来可能输入的词句尾巴"
        );
        let _ = writeln!(
            s,
            "# (数字/空格/点选上屏,字母开始新组合,Esc 关闭;纯本地词表,无网络调用)"
        );
        let _ = writeln!(
            s,
            "next_word_prediction = {}",
            d.next_word_prediction
        );
        let _ = writeln!(
            s,
            "\n# 造词快捷键:上屏汉字后按此键进入造词模式(方向键 →/↑ 多选一字、"
        );
        let _ = writeln!(
            s,
            "# ←/↓/退格 少选一字,回车存词、Esc 取消;写法:修饰(ctrl/alt/super/shift)+键名"
        );
        let _ = writeln!(s, "coin_hotkey = \"{}\"", d.coin_hotkey);
        let _ = writeln!(
            s,
            "\n# 截屏快捷键:按下拉起框选截屏(拖拽选区,存图片目录并复制剪贴板;"
        );
        let _ = writeln!(
            s,
            "# Esc 取消、Enter 确认、双击整屏);写法同造词快捷键,如 ctrl+alt+a、ctrl+shift+x"
        );
        let _ = writeln!(s, "shot_hotkey = \"{}\"", d.shot_hotkey);
        let _ = writeln!(
            s,
            "\n# 快速功能键(合同 §14):输入触发词(整串小写字母)时候选条追加"
        );
        let _ = writeln!(
            s,
            "# 功能候选,数字键/鼠标点选执行,不上屏文本;command 为 @settings/"
        );
        let _ = writeln!(
            s,
            "# @help(宿主内置)或任意 shell 命令;总开关关闭则整表不生效。"
        );
        let _ = writeln!(s, "quick_actions_enabled = {}", d.quick_actions_enabled);
        let _ = writeln!(
            s,
            "\n# 菜单触发(与快速功能键不同源):上屏的中文文字结尾命中菜单功能名时,"
        );
        let _ = writeln!(
            s,
            "# 候选条提示「按 Fn 进入该功能」,按确认键执行一次;继续输入/切换窗口即取消"
        );
        let _ = writeln!(s, "menu_trigger_enabled = {}", d.menu_trigger_enabled);
        let _ = writeln!(s, "# 确认键:F1–F12 的功能键序号(默认 7 = F7,无修饰单独按)");
        let _ = writeln!(s, "menu_trigger_key = {}", d.menu_trigger_key);
        let _ = writeln!(
            s,
            "# 禁用项:逗号分隔的功能稳定 id(默认仅禁 fix_ime 修复输入法;"
        );
        let _ = writeln!(
            s,
            "# 逐项精确匹配,未知 id 保留;只影响本特性,不影响快速功能键)"
        );
        let _ = writeln!(
            s,
            "menu_trigger_disabled = \"{}\"",
            d.menu_trigger_disabled
        );
        for a in &d.quick_actions {
            let _ = writeln!(s, "\n[[quick_actions]]");
            let _ = writeln!(s, "trigger = \"{}\"", a.trigger);
            let _ = writeln!(s, "label = \"{}\"", a.label);
            let _ = writeln!(s, "command = \"{}\"", a.command);
        }
        s
    }

    /// 取默认用户词典路径:`~/.local/share/lyyime/user.tsv`(无 HOME 时退回当前目录下的相对路径)。
    pub fn default_user_dict_path() -> PathBuf {
        match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => PathBuf::from(h).join(".local/share/lyyime/user.tsv"),
            _ => PathBuf::from(".local/share/lyyime/user.tsv"),
        }
    }

    /// 取默认配置文件路径:`~/.config/lyyime/config.toml`(无 HOME 时返回 None)。
    pub fn default_config_path() -> Option<PathBuf> {
        match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => Some(PathBuf::from(h).join(".config/lyyime/config.toml")),
            _ => None,
        }
    }
}

impl ConfigToml {
    /// 校验并转换为 `Config`;非法取值给出人话错误。
    fn into_config(self, path: &Path) -> Result<Config, Error> {
        let mode = match self.mode.trim().to_lowercase().as_str() {
            "" | "cn" | "chinese" | "zh" => Mode::Chinese,
            "en" | "english" => Mode::English,
            other => {
                return Err(Error::new(format!(
                    "配置文件 {} 中 mode = \"{other}\" 不合法,可选值:cn / en",
                    path.display()
                )))
            }
        };
        if self.page_size == 0 || self.page_size > 10 {
            return Err(Error::new(format!(
                "配置文件 {} 中 page_size = {} 不合法,需在 1–10 之间(数字键 1–9/0 选词)",
                path.display(),
                self.page_size
            )));
        }
        if self.en_freq_top_n == 0 {
            return Err(Error::new(format!(
                "配置文件 {} 中 en_freq_top_n = 0 不合法,至少为 1",
                path.display()
            )));
        }
        if self.mixed_auto_commit_top_n == 0 {
            return Err(Error::new(format!(
                "配置文件 {} 中 mixed_auto_commit_top_n = 0 不合法,至少为 1",
                path.display()
            )));
        }
        Ok(Config {
            mode,
            page_size: self.page_size,
            pinyin_only: self.pinyin_only,
            mixed_en: self.mixed_en,
            mixed_auto_commit: self.mixed_auto_commit,
            // 规范键 chinese_punct(XIM 设置窗写盘键名)优先,旧别名
            // cn_punct 兜底,再落内置默认 true——设置界面保存值优先。
            cn_punct: self.chinese_punct.or(self.cn_punct).unwrap_or(true),
            learning: self.learning,
            user_dict: self
                .user_dict
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from),
            data_dir: self
                .data_dir
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from),
            en_freq_top_n: self.en_freq_top_n,
            mixed_auto_commit_top_n: self.mixed_auto_commit_top_n,
            // 规范键优先,XIM 写盘的别名 commit_after_four 兜底,再落内置默认。
            commit_on_extra_after_four: self
                .commit_on_extra_after_four
                .or(self.commit_after_four)
                .unwrap_or(Config::default().commit_on_extra_after_four),
            commit_first_at_four: self.commit_first_at_four,
            commit_unique_four: self.commit_unique_four,
            // 未知/空取值按各键默认回退(回车 temp / Shift en),不判整份损坏
            enter_english: EnCommit::from_toml(&self.enter_english)
                .unwrap_or(EnCommit::Temp),
            shift_english: EnCommit::from_toml(&self.shift_english)
                .unwrap_or(EnCommit::English),
            exact_char_freq_rank: self.exact_char_freq_rank,
            phrase_hint: self.phrase_hint,
            next_word_prediction: self.next_word_prediction,
            coin_hotkey: {
                let hk = self.coin_hotkey.trim().to_string();
                if hk.is_empty() {
                    "ctrl+equal".to_string()
                } else {
                    hk
                }
            },
            shot_hotkey: {
                let hk = self.shot_hotkey.trim().to_string();
                if hk.is_empty() {
                    "ctrl+alt+a".to_string()
                } else {
                    hk
                }
            },
            quick_actions_enabled: self.quick_actions_enabled,
            quick_actions: validate_quick_actions(&self.quick_actions, path)?,
            menu_trigger_enabled: self.menu_trigger_enabled,
            // 越界序号回退默认(宿主下拉框只出 F1–F12;手写非法值宽恕处理)
            menu_trigger_key: if (1..=12).contains(&self.menu_trigger_key) {
                self.menu_trigger_key
            } else {
                Config::default().menu_trigger_key
            },
            menu_trigger_disabled: self.menu_trigger_disabled,
        })
    }
}

/// 快速功能键条目校验(合同 §14):格式非法给人话错误;上限 [`QUICK_ACTIONS_MAX`]。
fn validate_quick_actions(list: &[QuickAction], path: &Path) -> Result<Vec<QuickAction>, Error> {
    if list.len() > QUICK_ACTIONS_MAX {
        return Err(Error::new(format!(
            "配置文件 {} 中 quick_actions 有 {} 条,最多 {} 条",
            path.display(),
            list.len(),
            QUICK_ACTIONS_MAX
        )));
    }
    for a in list {
        if !QuickAction::trigger_valid(&a.trigger) {
            return Err(Error::new(format!(
                "配置文件 {} 中 quick_actions 触发词 \"{}\" 不合法:需 1–12 个小写字母",
                path.display(),
                a.trigger
            )));
        }
        if a.label.trim().is_empty() || a.command.trim().is_empty() {
            return Err(Error::new(format!(
                "配置文件 {} 中 quick_actions \"{}\" 的 label/command 不能为空",
                path.display(),
                a.trigger
            )));
        }
    }
    Ok(list.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_toml_可被完整解析回默认值() {
        let text = Config::example_toml();
        assert!(text.contains("page_size = 10"));
        let raw: ConfigToml = toml::from_str(&text).unwrap();
        let cfg = raw.into_config(Path::new("x")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn page_size_边界与非法值() {
        // 1 与 10 合法;0 与 11 判损坏(人话错误)。
        for ok in [1usize, 9, 10] {
            let text = format!("page_size = {ok}");
            let cfg: Config = toml::from_str::<ConfigToml>(&text)
                .unwrap()
                .into_config(Path::new("x"))
                .unwrap();
            assert_eq!(cfg.page_size, ok);
        }
        for bad in [0usize, 11, 99] {
            let text = format!("page_size = {bad}");
            let r = toml::from_str::<ConfigToml>(&text)
                .map_err(|e| Error::new(e.to_string()))
                .and_then(|raw| raw.into_config(Path::new("x")));
            assert!(r.is_err(), "page_size = {bad} 应判不合法");
        }
    }

    #[test]
    fn shot_hotkey_默认值与空串回退() {
        assert_eq!(Config::default().shot_hotkey, "ctrl+alt+a");
        // 缺项与空串都落默认值
        let cfg: Config = toml::from_str::<ConfigToml>("mode = \"cn\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert_eq!(cfg.shot_hotkey, "ctrl+alt+a");
        let cfg2: Config = toml::from_str::<ConfigToml>("shot_hotkey = \"\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert_eq!(cfg2.shot_hotkey, "ctrl+alt+a");
        // 自定义值原样保留
        let cfg3: Config = toml::from_str::<ConfigToml>("shot_hotkey = \"ctrl+shift+x\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert_eq!(cfg3.shot_hotkey, "ctrl+shift+x");
    }

    #[test]
    fn 英文上屏去向_缺省与解析() {
        // 默认:回车临时(单个英文词),Shift 切英文模式(§6)。
        assert_eq!(Config::default().enter_english, EnCommit::Temp);
        assert_eq!(Config::default().shift_english, EnCommit::English);
        // 精确单字按词频排位默认开。
        assert!(Config::default().exact_char_freq_rank);
        // 缺项/空串/未知值:按各键默认回退,不判整份损坏(宽恕手写拼写)
        for (text, want_enter, want_shift) in [
            ("", EnCommit::Temp, EnCommit::English),
            ("mode = \"cn\"", EnCommit::Temp, EnCommit::English),
            (
                "enter_english = \"junk\"\nshift_english = \"\"",
                EnCommit::Temp,
                EnCommit::English,
            ),
        ] {
            let cfg: Config = toml::from_str::<ConfigToml>(text)
                .unwrap()
                .into_config(Path::new("x"))
                .unwrap();
            assert_eq!(cfg.enter_english, want_enter, "text={text}");
            assert_eq!(cfg.shift_english, want_shift, "text={text}");
        }
        // 规范值与同义词(大小写/空白宽容)
        let cfg: Config = toml::from_str::<ConfigToml>(
            "enter_english = \" EN \"\nshift_english = \"temporary\"",
        )
        .unwrap()
        .into_config(Path::new("x"))
        .unwrap();
        assert_eq!(cfg.enter_english, EnCommit::English);
        assert_eq!(cfg.shift_english, EnCommit::Temp);
        // persist 同义词
        let cfg: Config = toml::from_str::<ConfigToml>("enter_english = \"persist\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert_eq!(cfg.enter_english, EnCommit::English);
    }

    #[test]
    fn quick_actions_缺项回默认表() {
        let cfg: Config = toml::from_str::<ConfigToml>("mode = \"cn\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(cfg.quick_actions_enabled);
        assert_eq!(cfg.quick_actions, default_quick_actions());
    }

    #[test]
    fn 四码顶屏_别名键兼容与规范键优先() {
        // XIM 设置窗写盘键名 commit_after_four 与规范键
        // commit_on_extra_after_four 同义;规范键缺失时别名生效,两键同写规范键优先。
        let cfg: Config = toml::from_str::<ConfigToml>("commit_after_four = false")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(!cfg.commit_on_extra_after_four);
        let cfg: Config = toml::from_str::<ConfigToml>(
            "commit_on_extra_after_four = true\ncommit_after_four = false",
        )
        .unwrap()
        .into_config(Path::new("x"))
        .unwrap();
        assert!(cfg.commit_on_extra_after_four);
        // 缺省 = 内置默认(顶屏开)。
        let cfg: Config = toml::from_str::<ConfigToml>("mode = \"cn\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(cfg.commit_on_extra_after_four);
    }

    #[test]
    fn 中文标点_别名键兼容与规范键优先() {
        // 缺省 = 内置默认(中文标点开)。
        let cfg: Config = toml::from_str::<ConfigToml>("mode = \"cn\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(cfg.cn_punct);
        // 规范键 chinese_punct(XIM 设置窗写盘键名)。
        let cfg: Config = toml::from_str::<ConfigToml>("chinese_punct = false")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(!cfg.cn_punct);
        // 旧别名 cn_punct 单独使用时仍生效。
        let cfg: Config = toml::from_str::<ConfigToml>("cn_punct = false")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(!cfg.cn_punct);
        // 两键矛盾:无论书写顺序,规范键(设置界面保存值)优先。
        for text in [
            "cn_punct = true\nchinese_punct = false",
            "chinese_punct = false\ncn_punct = true",
        ] {
            let cfg: Config = toml::from_str::<ConfigToml>(text)
                .unwrap()
                .into_config(Path::new("x"))
                .unwrap();
            assert!(!cfg.cn_punct, "规范键应覆盖旧别名: {text}");
        }
    }

    #[test]
    fn quick_actions_自定义表解析与非法触发词() {
        let text = r#"
quick_actions_enabled = false
[[quick_actions]]
trigger = "rizhi"
label = "看日志"
command = "xfce4-terminal -e 'tail -f ~/.local/share/lyyime/logs/xim.log'"
[[quick_actions]]
trigger = "wenjian"
label = "帮助"
command = "@help"
"#;
        let cfg: Config = toml::from_str::<ConfigToml>(text)
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(!cfg.quick_actions_enabled);
        assert_eq!(cfg.quick_actions.len(), 2);
        assert_eq!(cfg.quick_actions[0].trigger, "rizhi");
        assert_eq!(cfg.quick_actions[1].command, "@help");

        // 触发词含数字/大写/超长:整份配置判损坏(人话错误,宿主回退默认)
        for bad in ["Peizhi", "pe-i", "peizhi1", "a".repeat(13).as_str()] {
            let text = format!("[[quick_actions]]\ntrigger = \"{bad}\"\nlabel = \"x\"\ncommand = \"@help\"\n");
            let r = toml::from_str::<ConfigToml>(&text)
                .map_err(|e| Error::new(e.to_string()))
                .and_then(|raw| raw.into_config(Path::new("x")));
            assert!(r.is_err(), "trigger {bad} 应判不合法");
        }
    }

    #[test]
    fn 菜单触发_默认值与解析() {
        // 默认:开 / F7 / 黑名单仅 fix_ime
        let d = Config::default();
        assert!(d.menu_trigger_enabled);
        assert_eq!(d.menu_trigger_key, 7);
        assert_eq!(d.menu_trigger_disabled, "fix_ime");

        // 缺项回默认;显式值解析(含未知 id 原样保留)
        let cfg: Config = toml::from_str::<ConfigToml>(
            "menu_trigger_enabled = false\n\
             menu_trigger_key = 8\n\
             menu_trigger_disabled = \"help,future_entry\"",
        )
        .unwrap()
        .into_config(Path::new("x"))
        .unwrap();
        assert!(!cfg.menu_trigger_enabled);
        assert_eq!(cfg.menu_trigger_key, 8);
        assert_eq!(cfg.menu_trigger_disabled, "help,future_entry");

        // 键序号越界宽恕回退默认 7(与 C 宿主钳制一致,不判整份损坏)
        for bad in [0usize, 13, 99] {
            let cfg: Config = toml::from_str::<ConfigToml>(&format!(
                "menu_trigger_key = {bad}"
            ))
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
            assert_eq!(cfg.menu_trigger_key, 7, "menu_trigger_key = {bad} 应回退默认");
        }

        // example_toml 包含三键且完整回读
        let text = Config::example_toml();
        assert!(text.contains("menu_trigger_enabled = true"));
        assert!(text.contains("menu_trigger_key = 7"));
        assert!(text.contains("menu_trigger_disabled = \"fix_ime\""));
    }

    #[test]
    fn 上屏后联想_默认关与显式开启回读() {
        assert!(!Config::default().next_word_prediction);
        // 缺项回默认(关)
        let cfg: Config = toml::from_str::<ConfigToml>("mode = \"cn\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(!cfg.next_word_prediction);
        // 显式 true / false 均按书写解析
        let cfg: Config = toml::from_str::<ConfigToml>("next_word_prediction = true")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(cfg.next_word_prediction);
        let cfg: Config = toml::from_str::<ConfigToml>("next_word_prediction = false")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(!cfg.next_word_prediction);
        // example_toml 含该键且完整回读为默认
        let text = Config::example_toml();
        assert!(text.contains("next_word_prediction = false"));
        let raw: ConfigToml = toml::from_str(&text).unwrap();
        assert_eq!(raw.into_config(Path::new("x")).unwrap(), Config::default());
    }
    #[test]
    fn 纯拼音方案_默认关与回读() {
        assert!(!Config::default().pinyin_only);
        let cfg: Config = toml::from_str::<ConfigToml>("mode = \"cn\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(!cfg.pinyin_only);
        let cfg: Config = toml::from_str::<ConfigToml>("pinyin_only = true")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(cfg.pinyin_only);
        let text = Config::example_toml();
        assert!(text.contains("pinyin_only = false"));
        let raw: ConfigToml = toml::from_str(&text).unwrap();
        assert_eq!(raw.into_config(Path::new("x")).unwrap(), Config::default());
    }
}
