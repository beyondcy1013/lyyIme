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

/// 快速功能键默认表(合同 §14):配置/帮助两个内置功能。
pub fn default_quick_actions() -> Vec<QuickAction> {
    vec![
        QuickAction {
            trigger: "peizhi".to_string(),
            label: "打开配置".to_string(),
            command: "@settings".to_string(),
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
    /// 候选窗每页条数,1–9(数字键选词)。
    pub page_size: usize,
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
    /// 满足四码后,再输入字母先上屏当前选中,剩余字母开启新组合。
    pub commit_on_extra_after_four: bool,
    /// 四码唯一上屏:恰好输入四码且候选唯一时,免空格直接上屏该候选
    /// (主流五笔的"四码唯一自动上屏"习惯);多候选不触发,保持混打渐进。
    pub commit_unique_four: bool,
    /// 词组效率提示:上屏后最近几个字若存在更省键的五笔词组,在候选条
    /// 提示「词 + 编码」,直到下一次输入才清除(借鉴万能五笔的高效词提示)。
    pub phrase_hint: bool,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Chinese,
            page_size: 5,
            mixed_en: true,
            mixed_auto_commit: true,
            cn_punct: true,
            learning: true,
            user_dict: None,
            data_dir: None,
            en_freq_top_n: 2000,
            mixed_auto_commit_top_n: 500,
            commit_on_extra_after_four: false,
            commit_unique_four: true,
            phrase_hint: true,
            coin_hotkey: "ctrl+equal".to_string(),
            shot_hotkey: "ctrl+alt+a".to_string(),
            quick_actions_enabled: true,
            quick_actions: default_quick_actions(),
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
    mixed_en: bool,
    mixed_auto_commit: bool,
    cn_punct: bool,
    learning: bool,
    user_dict: Option<String>,
    data_dir: Option<String>,
    en_freq_top_n: usize,
    mixed_auto_commit_top_n: usize,
    commit_on_extra_after_four: bool,
    commit_unique_four: bool,
    phrase_hint: bool,
    coin_hotkey: String,
    shot_hotkey: String,
    quick_actions_enabled: bool,
    quick_actions: Vec<QuickAction>,
}

impl Default for ConfigToml {
    fn default() -> Self {
        ConfigToml::from(&Config::default())
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
            mixed_en: c.mixed_en,
            mixed_auto_commit: c.mixed_auto_commit,
            cn_punct: c.cn_punct,
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
            commit_on_extra_after_four: c.commit_on_extra_after_four,
            commit_unique_four: c.commit_unique_four,
            phrase_hint: c.phrase_hint,
            coin_hotkey: c.coin_hotkey.clone(),
            shot_hotkey: c.shot_hotkey.clone(),
            quick_actions_enabled: c.quick_actions_enabled,
            quick_actions: c.quick_actions.clone(),
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
        let _ = writeln!(s, "\n# 候选窗每页条数(1–9,数字键选词)");
        let _ = writeln!(s, "page_size = {}", d.page_size);
        let _ = writeln!(s, "\n# 中英混合:输入无中文命中时给出英文词候选");
        let _ = writeln!(s, "mixed_en = {}", d.mixed_en);
        let _ = writeln!(
            s,
            "\n# 混合直通:高频英文词(见 en_freq_top_n)遇标点自动上屏原词"
        );
        let _ = writeln!(s, "mixed_auto_commit = {}", d.mixed_auto_commit);
        let _ = writeln!(s, "\n# 中文态输出中文标点(如 , 。 ?;关闭则标点原样直通)");
        let _ = writeln!(s, "cn_punct = {}", d.cn_punct);
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
            "\n# 满足四码后,再输入字母先上屏当前选中,后续字母开始新组合"
        );
        let _ = writeln!(
            s,
            "commit_on_extra_after_four = {}",
            d.commit_on_extra_after_four
        );
        let _ = writeln!(
            s,
            "\n# 四码唯一上屏:恰好四码且候选唯一时免空格直接上屏(多候选仍需空格/数字)"
        );
        let _ = writeln!(s, "commit_unique_four = {}", d.commit_unique_four);
        let _ = writeln!(
            s,
            "\n# 词组效率提示:上屏后最近几个字有更省键的五笔词组时,候选条提示词组与编码"
        );
        let _ = writeln!(s, "phrase_hint = {}", d.phrase_hint);
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
        if self.page_size == 0 || self.page_size > 9 {
            return Err(Error::new(format!(
                "配置文件 {} 中 page_size = {} 不合法,需在 1–9 之间(数字键选词)",
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
            mixed_en: self.mixed_en,
            mixed_auto_commit: self.mixed_auto_commit,
            cn_punct: self.cn_punct,
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
            commit_on_extra_after_four: self.commit_on_extra_after_four,
            commit_unique_four: self.commit_unique_four,
            phrase_hint: self.phrase_hint,
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
        assert!(text.contains("page_size = 5"));
        let raw: ConfigToml = toml::from_str(&text).unwrap();
        let cfg = raw.into_config(Path::new("x")).unwrap();
        assert_eq!(cfg, Config::default());
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
    fn quick_actions_缺项回默认表() {
        let cfg: Config = toml::from_str::<ConfigToml>("mode = \"cn\"")
            .unwrap()
            .into_config(Path::new("x"))
            .unwrap();
        assert!(cfg.quick_actions_enabled);
        assert_eq!(cfg.quick_actions, default_quick_actions());
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
}
