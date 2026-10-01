//! 菜单触发(文字上屏命中菜单功能 → 持续提示 → 显式功能键执行)。
//!
//! 独立纯状态机,**不属于** `Engine`/`Plan`:宿主(XIM/ibus)只在真实上屏
//! 文本产生后喂入 [`MenuTrigger::on_commit`],核心引擎的规划/重试/FFI
//! 两段式语义完全不受影响(短缓冲重试不会重复上屏,也不会误消耗待执行态)。
//!
//! 行为约定:
//! - 尾串 `tail`:最近一段**连续 CJK** 上屏文本(跨多次上屏拼接,定长上界);
//!   标点/拉丁/换行/其它非 CJK 字符均为硬边界,出现即截断(清空)尾串。
//! - 匹配两级:①**整段上屏文本精确等于某条别名**(允许含空格/拉丁/斜杠
//!   的混合名,如 `AI 助手`、`切换中/英文`)→ 直接命中;②否则尾串
//!   **最长后缀**命中目录项 `label`/`aliases` 之一。
//!   原始拉丁上屏(如 `shezhi`)不是任何别名且不含 CJK,永不命中。
//! - 黑名单:两级命中都按「最长者所属项」判定;命中黑名单项即整体不触发,
//!   **不回退**到更短的可用后缀/别名(如禁用 `输入统计` 后不会降级命中
//!   其它项)。
//! - 命中 → 返回提示 `匹配了菜单功能「{label}」,按 F{n} 进入该功能`,
//!   同时置起 `pending`(目录下标);`take_pending` 一次性取出。
//!   截屏项追加宿主注入的实际快捷键 `；也可按 {display}`(经
//!   [`MenuTrigger::set_shot_hotkey`] 注入,非法/空串不展示);英文项
//!   固定追加 `；也可单击 Shift 切换中英文`;其它项不发明快捷键。
//! - 每个非确认按键 → [`cancel_pending`](MenuTrigger::cancel_pending)
//!   (仅清待执行,保留尾串,允许跨上屏续接);退格/翻页/Esc/修饰键/直通文本/
//!   焦点与模式切换/引擎与配置重载/AI 进出 → [`reset`](MenuTrigger::reset)
//!   (尾串与待执行一起清)。
//! - 配置:`configure(enabled, key, disabled)`;`key` 仅收 1..=12(F1–F12),
//!   非法值回退默认 7;`disabled` 为逗号分隔的稳定 id,**逐 token 精确比较**,
//!   未知 id 原样保留(前向兼容)。
//! - 目录为静态可信白名单:只有 id/label/aliases + 内置动作枚举,
//!   不含任意命令;刻意不收录「退出/卸载/停用」类会打断输入的条目。
//!   `fix_ime`(修复输入法)默认进黑名单,须用户显式勾选启用。

/// 确认键合法范围:F1–F12(只收序号;默认 7 = F7)。
pub const MENU_KEY_MIN: u8 = 1;
pub const MENU_KEY_MAX: u8 = 12;
pub const MENU_KEY_DEFAULT: u8 = 7;
/// 默认黑名单:修复输入法属系统级操作,默认不允许文字触发。
pub const MENU_DISABLED_DEFAULT: &str = "fix_ime";
/// 尾串上界(字符数):远超最长目录别名,防长输入序列无限累积。
const TAIL_CAP: usize = 16;

/// 目录项动作(可信枚举;宿主按 id/下标分发,绝不落 shell)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// 打开主设置窗口。
    OpenSettings,
    /// 打开设置窗口指定页(0 基页码)。
    OpenSettingsPage(u8),
    /// 帮助提示。
    Help,
    /// 切到英文模式(非 toggle:已英文则无操作)。
    EnglishMode,
    /// 截屏助手。
    Shot,
    /// 直输模式悬浮窗(lyyime-float)。
    FloatWindow,
    /// 修复输入法诊断窗(带确认)。
    FixIme,
    /// 输入法管理。
    ManageIme,
    /// 重载词库。
    ReloadDict,
    /// 打开日志。
    OpenLog,
    /// 主窗口。
    MainWindow,
}

/// 目录项:稳定 id + 显示名 + 匹配别名(含去省略号的全称)。
pub struct MenuItem {
    pub id: &'static str,
    pub label: &'static str,
    /// 可匹配别名集合:整段上屏文本精确命中,或作为连续 CJK 尾串的最长
    /// 后缀命中;取最长者。含非 CJK 字符的别名(如 `切换中/英文`、
    /// `AI 助手`)只能靠「整段上屏文本精确等于别名」命中——尾串只含
    /// CJK,这类别名不参与后缀匹配。
    pub aliases: &'static [&'static str],
    pub action: MenuAction,
}

/// 功能目录(顺序即稳定下标,FFI `lyyime_menu_trigger_*` 的 index 与之对应)。
/// 不含退出/卸载/停用输入法类条目——文字触发不得意外打断输入。
pub static MENU_CATALOG: &[MenuItem] = &[
    MenuItem {
        id: "settings",
        label: "设置",
        aliases: &["设置", "配置", "打开配置"],
        action: MenuAction::OpenSettings,
    },
    MenuItem {
        id: "help",
        label: "帮助",
        aliases: &["帮助"],
        action: MenuAction::Help,
    },
    MenuItem {
        id: "english",
        label: "英文",
        aliases: &["英文", "英文模式", "切换中/英文"],
        action: MenuAction::EnglishMode,
    },
    MenuItem {
        id: "shot",
        label: "截屏",
        aliases: &["截屏", "截图"],
        action: MenuAction::Shot,
    },
    MenuItem {
        id: "float",
        label: "直输模式",
        aliases: &["直输模式", "悬浮窗"],
        action: MenuAction::FloatWindow,
    },
    MenuItem {
        id: "fix_ime",
        label: "修复输入法",
        aliases: &["修复输入法", "修复 Linux 中文输入法"],
        action: MenuAction::FixIme,
    },
    MenuItem {
        id: "manage_ime",
        label: "输入法管理",
        aliases: &["输入法管理"],
        action: MenuAction::ManageIme,
    },
    MenuItem {
        id: "reload_dict",
        label: "重载词库",
        aliases: &["重载词库"],
        action: MenuAction::ReloadDict,
    },
    MenuItem {
        id: "open_log",
        label: "打开日志",
        aliases: &["打开日志"],
        action: MenuAction::OpenLog,
    },
    MenuItem {
        id: "mainwin",
        label: "主窗口",
        aliases: &["主窗口", "显示主窗口"],
        action: MenuAction::MainWindow,
    },
    MenuItem {
        id: "settings_general",
        label: "常规",
        aliases: &["常规"],
        action: MenuAction::OpenSettingsPage(0),
    },
    MenuItem {
        id: "settings_input",
        label: "输入",
        aliases: &["输入"],
        action: MenuAction::OpenSettingsPage(1),
    },
    MenuItem {
        id: "settings_hotkey",
        label: "快捷键",
        aliases: &["快捷键"],
        action: MenuAction::OpenSettingsPage(2),
    },
    MenuItem {
        id: "settings_ai",
        label: "AI 助手",
        aliases: &["AI 助手", "人工智能助手"],
        action: MenuAction::OpenSettingsPage(3),
    },
    MenuItem {
        id: "settings_stats",
        label: "输入统计",
        aliases: &["输入统计"],
        action: MenuAction::OpenSettingsPage(4),
    },
    MenuItem {
        id: "settings_menu",
        label: "菜单触发",
        aliases: &["菜单触发"],
        action: MenuAction::OpenSettingsPage(5),
    },
];

/// 目录下标 → 稳定 id(越界返回 None)。
pub fn catalog_id(index: usize) -> Option<&'static str> {
    MENU_CATALOG.get(index).map(|m| m.id)
}

/// CJK 统一表意字符(与 engine.rs 造词历史同口径:基本区+扩展A+兼容区+扩展B)。
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF)
}

/// 菜单触发状态机:尾串 + 一次性待执行项。
#[derive(Debug, Clone)]
pub struct MenuTrigger {
    /// 最近连续 CJK 上屏尾串(≤ TAIL_CAP 字)。
    tail: String,
    /// 待执行目录下标(一次性;take 即消费)。
    pending: Option<usize>,
    enabled: bool,
    /// 确认键 F 序号(1..=12)。
    key: u8,
    /// 黑名单稳定 id 集合(逗号分隔源串已拆分)。
    disabled: Vec<String>,
    /// 宿主注入的实际截屏快捷键展示串(如 `Ctrl+Alt+A`);
    /// None = 未注入/非法,截屏提示只显示 F 确认键。
    shot_shortcut: Option<String>,
}

impl Default for MenuTrigger {
    fn default() -> Self {
        Self::new()
    }
}

impl MenuTrigger {
    pub fn new() -> Self {
        let mut t = Self {
            tail: String::new(),
            pending: None,
            enabled: true,
            key: MENU_KEY_DEFAULT,
            disabled: Vec::new(),
            shot_shortcut: None,
        };
        t.configure(true, MENU_KEY_DEFAULT, MENU_DISABLED_DEFAULT);
        t
    }

    /// 配置:开关 / 确认键序号(1..=12,非法回退默认) / 逗号分隔黑名单。
    /// 配置变更视为硬边界:尾串与待执行一起复位。
    pub fn configure(&mut self, enabled: bool, key: u8, disabled: &str) {
        self.enabled = enabled;
        self.key = if (MENU_KEY_MIN..=MENU_KEY_MAX).contains(&key) {
            key
        } else {
            MENU_KEY_DEFAULT
        };
        self.disabled = disabled
            .split(',')
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        self.reset();
    }

    /// 宿主注入当前实际截屏快捷键;非法/空串不展示替代快捷键。
    pub fn set_shot_hotkey(&mut self, spec: &str) {
        let shortcut = crate::hotkey::canon_hotkey(spec).map(|s| {
            s.split('+').map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            }).collect::<Vec<_>>().join("+")
        });
        if shortcut != self.shot_shortcut {
            self.reset();
            self.shot_shortcut = shortcut;
        }
    }

    /// 配置的确认键序号(1..=12)。
    pub fn key(&self) -> u8 {
        self.key
    }

    /// 当前待执行目录下标(不消费)。
    pub fn pending(&self) -> Option<usize> {
        self.pending
    }

    /// 当前待执行项的提示文本(与 on_commit 返回值同格式)。
    pub fn hint(&self) -> Option<String> {
        self.pending.map(|i| self.hint_text(i))
    }

    /// 真实上屏文本喂入:更新尾串并按最长后缀匹配;命中返回提示文本并置
    /// 起待执行项,未命中/被禁/关闭时返回 None(待执行一并清掉)。
    pub fn on_commit(&mut self, text: &str) -> Option<String> {
        self.pending = None;
        // 尾串追加后只保留末尾连续 CJK 段:标点/拉丁/换行等非 CJK 即断。
        self.tail.push_str(text);
        let cut = self
            .tail
            .char_indices()
            .rev()
            .find(|(_, c)| !is_cjk(*c))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        if cut > 0 {
            self.tail.drain(..cut);
        }
        // 定长上界:只留最近 TAIL_CAP 个字符。
        let excess = self.tail.chars().count().saturating_sub(TAIL_CAP);
        if excess > 0 {
            let byte = self
                .tail
                .char_indices()
                .nth(excess)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.tail.drain(..byte);
        }
        if !self.enabled {
            return None;
        }
        // ① 整段上屏文本精确等于某条别名:支持含空格/拉丁/斜杠的混合名
        //    (「AI 助手」「切换中/英文」「修复 Linux 中文输入法」),
        //    取目录中该别名最长者;命中黑名单项即整体不触发,不回退到
        //    尾串后缀上的更短可用项。
        if !text.is_empty() {
            let mut exact: Option<(usize, usize)> = None; // (别名字符数, 目录下标)
            for (i, item) in MENU_CATALOG.iter().enumerate() {
                for alias in item.aliases {
                    let len = alias.chars().count();
                    if len > 0 && text == *alias
                        && exact.map_or(true, |(bl, _)| len > bl)
                    {
                        exact = Some((len, i));
                    }
                }
            }
            if let Some((_, i)) = exact {
                if self.disabled.iter().any(|d| d == MENU_CATALOG[i].id) {
                    return None;
                }
                self.pending = Some(i);
                return Some(self.hint_text(i));
            }
        }
        if self.tail.is_empty() {
            return None;
        }
        // ② 尾串最长后缀命中:取别名最长者,等长取目录靠前项;
        //    命中黑名单项同样整体不触发(不回退更短后缀)。
        let mut best: Option<(usize, usize)> = None;
        for (i, item) in MENU_CATALOG.iter().enumerate() {
            for alias in item.aliases {
                let len = alias.chars().count();
                if len == 0 || !self.tail.ends_with(alias) {
                    continue;
                }
                if best.map_or(true, |(bl, _)| len > bl) {
                    best = Some((len, i));
                }
            }
        }
        let (_, i) = best?;
        if self.disabled.iter().any(|d| d == MENU_CATALOG[i].id) {
            return None;
        }
        self.pending = Some(i);
        Some(self.hint_text(i))
    }

    /// 每个非确认按键调用:仅清待执行,保留尾串(允许跨上屏续接)。
    /// 返回 true 表示确实清掉了待执行项(供宿主记日志/清提示)。
    pub fn cancel_pending(&mut self) -> bool {
        self.pending.take().is_some()
    }

    /// 硬边界:尾串与待执行一起清空。返回 true 表示曾有待执行项。
    pub fn reset(&mut self) -> bool {
        let had = self.pending.is_some();
        self.tail.clear();
        self.pending = None;
        had
    }

    /// 一次性取出待执行目录下标(不删文本、不重复上屏)。
    pub fn take_pending(&mut self) -> Option<usize> {
        self.pending.take()
    }

    /// 命中提示文本:`匹配了菜单功能「{label}」,按 F{n} 进入该功能`。
    fn hint_text(&self, index: usize) -> String {
        let label = MENU_CATALOG
            .get(index)
            .map(|m| m.label)
            .unwrap_or("?");
        let mut hint = format!("匹配了菜单功能「{label}」,按 F{} 进入该功能", self.key);
        match MENU_CATALOG.get(index).map(|m| m.action) {
            Some(MenuAction::Shot) => {
                if let Some(shortcut) = &self.shot_shortcut {
                    hint.push_str(&format!("；也可按 {shortcut}"));
                }
            }
            Some(MenuAction::EnglishMode) => hint.push_str("；也可单击 Shift 切换中英文"),
            _ => {}
        }
        hint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_index() -> usize {
        MENU_CATALOG.iter().position(|m| m.id == "settings").unwrap()
    }

    fn catalog_index(id: &str) -> usize {
        MENU_CATALOG.iter().position(|m| m.id == id).unwrap()
    }

    #[test]
    fn 目录项_无退出卸载类条目_且id唯一() {
        let mut ids: Vec<&str> = MENU_CATALOG.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MENU_CATALOG.len(), "目录 id 必须唯一");
        for id in ids {
            assert!(!id.contains("quit") && !id.contains("uninstall"),
                "目录不得收录退出/卸载类条目:{id}");
        }
        // 16 项目录与既定顺序一致(settings=0 … settings_menu=15)
        assert_eq!(MENU_CATALOG.len(), 16);
        assert_eq!(MENU_CATALOG[0].id, "settings");
        assert_eq!(MENU_CATALOG[15].id, "settings_menu");
    }

    #[test]
    fn 单次上屏设置_提示f7且一次性取出() {
        let mut t = MenuTrigger::new();
        let hint = t.on_commit("设置").unwrap();
        assert_eq!(hint, "匹配了菜单功能「设置」,按 F7 进入该功能");
        assert_eq!(t.pending(), Some(settings_index()));
        assert_eq!(t.take_pending(), Some(settings_index()));
        assert_eq!(t.take_pending(), None, "待执行只能取一次");
        assert_eq!(t.hint(), None);
    }

    #[test]
    fn 分字上屏_设加置_命中() {
        let mut t = MenuTrigger::new();
        assert!(t.on_commit("设").is_none(), "半个词不应命中");
        let hint = t.on_commit("置").unwrap();
        assert!(hint.contains("「设置」"), "{hint}");
        assert_eq!(t.pending(), Some(settings_index()));
    }

    #[test]
    fn 帮助与英文_命中() {
        let mut t = MenuTrigger::new();
        t.on_commit("帮助");
        assert_eq!(t.pending(), Some(catalog_index("help")));
        t.reset();
        t.on_commit("英文");
        assert_eq!(t.pending(), Some(catalog_index("english")));
    }

    #[test]
    fn 最长后缀优先_输入统计胜过输入() {
        let mut t = MenuTrigger::new();
        let hint = t.on_commit("输入统计").unwrap();
        assert!(hint.contains("「输入统计」"), "{hint}");
        assert_eq!(t.pending(), Some(catalog_index("settings_stats")));
        // 单独「输入」命中输入页
        t.reset();
        t.on_commit("输入");
        assert_eq!(t.pending(), Some(catalog_index("settings_input")));
    }

    #[test]
    fn 边界_标点_非cjk_换行_截断尾串() {
        let mut t = MenuTrigger::new();
        // 中文标点上屏(单独 commit)截断:设/置/， 后 tail 为空
        t.on_commit("设");
        t.on_commit("置");
        t.on_commit("，");
        assert!(t.on_commit("").is_none());
        assert_eq!(t.pending(), None);
        assert!(t.tail.is_empty(), "标点后尾串应清空");
        // 混合文本中间切断:「设abc置」尾串只剩「置」
        t.reset();
        t.on_commit("设abc置");
        assert_eq!(t.tail, "置");
        // 换行同样是边界
        t.reset();
        t.on_commit("设置\n帮助");
        assert_eq!(t.tail, "帮助");
        assert_eq!(t.pending(), Some(catalog_index("help")));
    }

    #[test]
    fn 拉丁上屏_不命中() {
        let mut t = MenuTrigger::new();
        assert!(t.on_commit("shezhi").is_none(), "原始拉丁串永不命中");
        assert_eq!(t.pending(), None);
        assert!(t.tail.is_empty());
    }

    #[test]
    fn 黑名单_帮助禁用无提示无待执行() {
        let mut t = MenuTrigger::new();
        t.configure(true, 7, "help");
        assert!(t.on_commit("帮助").is_none());
        assert_eq!(t.pending(), None);
        assert_eq!(t.take_pending(), None);
        // 逗号分隔 token 精确匹配:help 不误伤 help2 形态(无此项),
        // 且黑名单不影响其它条目
        t.on_commit("设置");
        assert_eq!(t.pending(), Some(settings_index()));
    }

    #[test]
    fn 全局关闭_无提示() {
        let mut t = MenuTrigger::new();
        t.configure(false, 7, "");
        assert!(t.on_commit("设置").is_none());
        assert_eq!(t.pending(), None);
    }

    #[test]
    fn 自定义确认键_f8() {
        let mut t = MenuTrigger::new();
        t.configure(true, 8, "");
        let hint = t.on_commit("设置").unwrap();
        assert_eq!(hint, "匹配了菜单功能「设置」,按 F8 进入该功能");
        assert_eq!(t.key(), 8);
        // 越界序号回退默认 7
        t.configure(true, 0, "");
        assert_eq!(t.key(), 7);
        t.configure(true, 13, "");
        assert_eq!(t.key(), 7);
    }

    #[test]
    fn 非确认键_取消待执行但保留尾串() {
        let mut t = MenuTrigger::new();
        t.on_commit("设置");
        assert_eq!(t.pending(), Some(settings_index()));
        assert!(t.cancel_pending(), "有待执行应返回 true");
        assert_eq!(t.pending(), None);
        assert_eq!(t.tail, "设置", "取消只清待执行,尾串保留");
        // 尾串还在:续接「帮助」→「设置帮助」命中帮助
        t.on_commit("帮助");
        assert_eq!(t.pending(), Some(catalog_index("help")));
    }

    #[test]
    fn 复位_清尾串与待执行() {
        let mut t = MenuTrigger::new();
        t.on_commit("设");
        t.on_commit("置");
        assert!(t.reset(), "有待执行时 reset 返回 true");
        assert!(t.tail.is_empty());
        assert_eq!(t.pending(), None);
        assert!(!t.reset(), "已无待执行返回 false");
    }

    #[test]
    fn 尾串定长_旧字被淘汰() {
        let mut t = MenuTrigger::new();
        // 连续单字上屏超过 TAIL_CAP:尾串只留最近 16 字
        for _ in 0..TAIL_CAP + 4 {
            assert!(t.on_commit("打").is_none());
        }
        assert_eq!(t.tail.chars().count(), TAIL_CAP);
        // 尾部续接目录名仍命中
        t.on_commit("帮助");
        assert_eq!(t.pending(), Some(catalog_index("help")));
    }

    #[test]
    fn 默认黑名单_修复输入法默认不触发() {
        let mut t = MenuTrigger::new();
        assert!(t.on_commit("修复输入法").is_none(),
            "fix_ime 默认在黑名单内");
        assert_eq!(t.pending(), None);
        // 显式移出黑名单后命中
        t.configure(true, 7, "");
        t.on_commit("修复输入法");
        assert_eq!(t.pending(), Some(catalog_index("fix_ime")));
    }

    #[test]
    fn 空格与别名_空格本身是边界() {
        let mut t = MenuTrigger::new();
        t.on_commit("设");
        t.on_commit(" ");
        t.on_commit("置");
        assert_eq!(t.tail, "置", "空格上屏截断尾串");
        assert_eq!(t.pending(), None);
    }

    #[test]
    fn 整段精确别名_混合名命中() {
        let mut t = MenuTrigger::new();
        // 含拉丁+空格的混合名:整段上屏文本精确等于别名 → 命中设置 AI 页
        let hint = t.on_commit("AI 助手").unwrap();
        assert!(hint.contains("「AI 助手」"), "{hint}");
        assert_eq!(t.pending(), Some(catalog_index("settings_ai")));
        // 中文别名同样精确命中(也走尾串后缀,殊途同归)
        t.reset();
        t.on_commit("人工智能助手");
        assert_eq!(t.pending(), Some(catalog_index("settings_ai")));
        // 「切换中/英文」整段精确 → english
        t.reset();
        t.on_commit("切换中/英文");
        assert_eq!(t.pending(), Some(catalog_index("english")));
        // 「修复 Linux 中文输入法」整段精确(移出默认黑名单后)
        t.configure(true, 7, "");
        t.on_commit("修复 Linux 中文输入法");
        assert_eq!(t.pending(), Some(catalog_index("fix_ime")));
    }

    #[test]
    fn 精确别名_部分串与任意拉丁仍不命中() {
        let mut t = MenuTrigger::new();
        // 别名前缀/子串不构成整段精确,也不是 CJK 尾串后缀 → 不命中
        assert!(t.on_commit("AI").is_none());
        assert!(t.on_commit("AI 助").is_none());
        assert!(t.on_commit("shezhi").is_none());
        assert_eq!(t.pending(), None);
    }

    #[test]
    fn 黑名单_精确别名禁用不回退更短后缀() {
        let mut t = MenuTrigger::new();
        // 禁用 settings_ai:「AI 助手」精确命中禁用项 → 整体不触发
        t.configure(true, 7, "settings_ai");
        assert!(t.on_commit("AI 助手").is_none());
        assert_eq!(t.pending(), None);
        // 禁用 settings_stats:「输入统计」不得降级命中其它项
        t.configure(true, 7, "settings_stats");
        assert!(t.on_commit("输入统计").is_none());
        assert_eq!(t.pending(), None);
        // 解除后恢复命中
        t.configure(true, 7, "");
        t.on_commit("输入统计");
        assert_eq!(t.pending(), Some(catalog_index("settings_stats")));
    }

    #[test]
    fn 未知黑名单id_原样保留且不报错() {
        let mut t = MenuTrigger::new();
        t.configure(true, 7, "future_entry, help ,,");
        assert!(t.on_commit("帮助").is_none(), "help 被禁");
        t.on_commit("设置");
        assert!(t.pending().is_some(), "未知 id 不干扰正常匹配");
    }

    #[test]
    fn shortcut_hint_uses_effective_key() {
        let shot = catalog_index("shot");
        let english = catalog_index("english");

        // 默认档:注入实际截屏键 → 提示=基础 F7 + 快捷键展示
        let mut t = MenuTrigger::new();
        t.set_shot_hotkey("ctrl+alt+a");
        let hint = t.on_commit("截屏").unwrap();
        assert_eq!(hint, "匹配了菜单功能「截屏」,按 F7 进入该功能；也可按 Ctrl+Alt+A");
        assert_eq!(t.pending(), Some(shot));

        // 配置键改 F8 + 别名规范化的快捷键写法 → 展示形 Ctrl+Shift+F9
        t.configure(true, 8, "");
        t.set_shot_hotkey("Shift + control + F9");
        let hint = t.on_commit("截图").unwrap();
        assert_eq!(hint, "匹配了菜单功能「截屏」,按 F8 进入该功能；也可按 Ctrl+Shift+F9");

        // 英文项固定追加 Shift 文案(与快捷键注入无关)
        let hint = t.on_commit("英文").unwrap();
        assert_eq!(hint, "匹配了菜单功能「英文」,按 F8 进入该功能；也可单击 Shift 切换中英文");
        assert_eq!(t.pending(), Some(english));

        // 其它项不受影响:设置仍只有 F8
        let hint = t.on_commit("设置").unwrap();
        assert_eq!(hint, "匹配了菜单功能「设置」,按 F8 进入该功能");

        // 非法 spec → 清掉替代快捷键,截屏只显示 F8(不追加后缀)
        t.set_shot_hotkey("a");
        let hint = t.on_commit("截屏").unwrap();
        assert_eq!(hint, "匹配了菜单功能「截屏」,按 F8 进入该功能");

        // setter 变更即硬边界:置起 pending 后改快捷键须复位
        assert_eq!(t.pending(), Some(shot));
        t.set_shot_hotkey("ctrl+alt+a");
        assert_eq!(t.pending(), None, "快捷键变更必须复位待执行");
        assert_eq!(t.hint(), None);

        // 黑名单含 shot → 截屏不产生提示
        t.configure(true, 8, "shot");
        assert_eq!(t.on_commit("截屏"), None);
        assert_eq!(t.pending(), None);
    }
}

