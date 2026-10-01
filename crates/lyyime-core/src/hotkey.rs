//! IM 托管组合热键:解析 / 规范化 / 冲突自动升级(合同 §12 造词、§13 截屏)。
//!
//! 解析规格与 Mode B `xim/src/keysym_map.c lyy_hotkey_parse`、Mode A
//! `crates/lyyime-ibus/src/keysym.rs` 一致(Mode A 直接复用本模块):
//! `修饰+键` —— 修饰 ctrl/alt/super/shift 至少一个(别名 control/mod1/mod4/win),
//! 键名为单字符字面量、a-z/0-9、f1–f24、常用名(equal/minus/… )或 0x keysym。
//!
//! 冲突自动升级(用户语:"自动滑向下一级"):快捷键与其它 lyyime 快捷键
//! 占用同一 `修饰+键` 组合时,按 **原组合 → +alt → +alt+shift** 逐级尝试
//! 空闲组合(如 ctrl+equal → ctrl+alt+equal → ctrl+alt+shift+equal)。
//! - 设置窗保存路径:刚改动的一侧避让,最终值直接回写输入框(所见即所得);
//! - 配置文件加载路径:截屏热键让位(工具键让位打字键,[`resolve_config_hotkeys`]);
//! - 三级全占用:保持原值,由调用方以人话提示。
//!
//! 本模块是规格单源;Mode B 的 C 镜像(lyy_hotkey_canon/lyy_hotkey_escalate)
//! 与其逐条对齐,两端单测各自覆盖。

/// 热键修饰位(X/ibus 通用位值;与本 crate 无关,宿主各自比对事件态)
pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_CTRL: u32 = 1 << 2;
pub const MOD_ALT: u32 = 1 << 3; // MOD1
pub const MOD_SUPER: u32 = (1 << 6) | (1 << 26); // MOD4 | SUPER(ibus 兼容位)

const HOTKEY_MOD_NAMES: &[(&str, u32)] = &[
    ("ctrl", MOD_CTRL),
    ("control", MOD_CTRL),
    ("alt", MOD_ALT),
    ("mod1", MOD_ALT),
    ("super", MOD_SUPER),
    ("mod4", MOD_SUPER),
    ("win", MOD_SUPER),
    ("shift", MOD_SHIFT),
];

/// 单字符字面量与常用键名(别名紧随正名之后,规范化取首个=正名;
/// 与 C 版 LYY_HOTKEY_KEYS 同表)
const HOTKEY_KEY_NAMES: &[(&str, u32)] = &[
    ("equal", 0x3d), ("=", 0x3d),
    ("minus", 0x2d), ("-", 0x2d),
    ("grave", 0x60), ("`", 0x60),
    ("bracketleft", 0x5b), ("[", 0x5b),
    ("bracketright", 0x5d), ("]", 0x5d),
    ("semicolon", 0x3b), (";", 0x3b),
    ("apostrophe", 0x27), ("'", 0x27),
    ("comma", 0x2c), (",", 0x2c),
    ("period", 0x2e), (".", 0x2e),
    ("slash", 0x2f), ("/", 0x2f),
    ("backslash", 0x5c), ("\\", 0x5c),
    ("space", 0x20),
    ("tab", 0xff09),
    ("return", 0xff0d), ("enter", 0xff0d),
    ("escape", 0xff1b), ("esc", 0xff1b),
    ("left", 0xff51), ("up", 0xff52), ("right", 0xff53), ("down", 0xff54),
    ("insert", 0xff63),
    ("delete", 0xffff), ("del", 0xffff),
    ("home", 0xff50), ("end", 0xff57),
    ("pageup", 0xff55), ("pagedown", 0xff56),
    ("print", 0xff61),
];

fn hotkey_keyval(name: &str) -> Option<u32> {
    // a-z / 0-9 单字符 → ASCII 键值
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0];
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    // f1–f24:XK_F1=0xffbe..F12=0xffc9,F13=0xffcc..F24=0xffd7
    if name.len() >= 2 && name.as_bytes()[0] == b'f' {
        if let Ok(n) = name[1..].parse::<u32>() {
            if (1..=12).contains(&n) {
                return Some(0xffbe + n - 1);
            }
            if (13..=24).contains(&n) {
                return Some(0xffcc + n - 13);
            }
        }
    }
    // 0x 十六进制 keysym 直写
    if let Some(hex) = name.strip_prefix("0x") {
        if let Ok(v) = u32::from_str_radix(hex, 16) {
            if v > 0 && v <= 0xff_ffff {
                return Some(v);
            }
        }
    }
    HOTKEY_KEY_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, sym)| *sym)
}

/// 解析 `修饰+键` 串 → (修饰位, 键值)。至少一个修饰(纯键热键与打字冲突);
/// 非法返回 None(宿主回退默认并记日志)。
pub fn parse_hotkey(spec: &str) -> Option<(u32, u32)> {
    let mut mods = 0u32;
    let mut parts: Vec<&str> = spec.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None; // 至少一个修饰 + 一个键
    }
    let key = parts.pop()?;
    for p in &parts {
        let p = p.to_lowercase();
        let bit = HOTKEY_MOD_NAMES
            .iter()
            .find(|(n, _)| n == &p)
            .map(|(_, b)| *b)?;
        mods |= bit;
    }
    let sym = hotkey_keyval(&key.to_lowercase())?;
    Some((mods, sym))
}

/// keysym → 标准键名(规范化/回显用;与解析表互逆,别名取正名)
fn hotkey_label(sym: u32) -> String {
    if (0x61..=0x7a).contains(&sym) || ('0'..='9').contains(&(sym as u8 as char)) {
        return (sym as u8 as char).to_string();
    }
    if (0xffbe..=0xffc9).contains(&sym) {
        return format!("f{}", sym - 0xffbe + 1);
    }
    if (0xffcc..=0xffd7).contains(&sym) {
        return format!("f{}", sym - 0xffcc + 13);
    }
    HOTKEY_KEY_NAMES
        .iter()
        .find(|(_, s)| *s == sym)
        .map(|(n, _)| (*n).to_string())
        .unwrap_or_else(|| format!("0x{sym:x}"))
}

/// (修饰位, 键值) → 规范化串:修饰固定顺序 ctrl+alt+super+shift + 标准键名。
fn hotkey_string(mods: u32, sym: u32) -> String {
    let mut s = String::new();
    for (name, bit) in [("ctrl", MOD_CTRL), ("alt", MOD_ALT), ("super", MOD_SUPER), ("shift", MOD_SHIFT)] {
        if mods & bit != 0 {
            if !s.is_empty() {
                s.push('+');
            }
            s.push_str(name);
        }
    }
    if !s.is_empty() {
        s.push('+');
    }
    s.push_str(&hotkey_label(sym));
    s
}

/// 规范化热键串:别名归一 + 修饰定序 + 键名标准形。
/// `"Ctrl + ="` 与 `"ctrl+equal"` 同归 `"ctrl+equal"`;非法返回 None。
pub fn canon_hotkey(spec: &str) -> Option<String> {
    parse_hotkey(spec).map(|(m, s)| hotkey_string(m, s))
}

/// 冲突自动升级:spec 与 occupied 中任一组合相同时,按
/// `原组合 → +alt → +alt+shift` 逐级尝试,返回首个空闲候选(规范化形);
/// spec 本身空闲则原样返回规范化形;三级全占用或 spec 非法返回 None。
/// occupied 中写法非法的项不构成占用(宿主对非法热键本就不拦截)。
pub fn escalate_hotkey(spec: &str, occupied: &[&str]) -> Option<String> {
    let (mods, sym) = parse_hotkey(spec)?;
    let occ: Vec<(u32, u32)> = occupied.iter().filter_map(|s| parse_hotkey(s)).collect();
    let free = |m: u32| !occ.iter().any(|&(om, os)| om == m && os == sym);
    for cand in [mods, mods | MOD_ALT, mods | MOD_ALT | MOD_SHIFT] {
        if free(cand) {
            return Some(hotkey_string(cand, sym));
        }
    }
    None
}

/// 保留组合:Ctrl+.(句点)固定为中文态中英文标点切换键,不可被
/// 造词/截屏热键占用(配置加载时自动让位,设置窗保存时直接拒绝)。
pub const PUNCT_TOGGLE_HOTKEY: &str = "ctrl+period";

/// 配置内冲突自愈(加载路径):先把占用了保留键 Ctrl+. 的造词/截屏热键
/// 逐级让位(+alt → +alt+shift,避开另一侧已占组合),再造词与截屏热键
/// 规范化相同时截屏热键按升级阶梯让位(工具键让位打字键),就地改写
/// `cfg.*_hotkey`。返回人话说明(无冲突或写法非法时为空 —— 非法热键
/// 由宿主按各自合同回退默认,不参与冲突)。
pub fn resolve_config_hotkeys(cfg: &mut crate::config::Config) -> Vec<String> {
    let mut notes = Vec::new();
    // 保留键让位:Ctrl+. 是内置标点切换键,造词/截屏热键配成它时
    // 在加载路径自动让位(原组合 → +alt → +alt+shift),不静默共存;
    // 让位候选须避开另一侧字段已占组合(如另一侧已是 ctrl+alt+period
    // 则落到 ctrl+alt+shift+period)。先于两两互斥判定执行。
    for is_coin in [true, false] {
        let (label, cur, other) = if is_coin {
            ("造词", cfg.coin_hotkey.clone(), cfg.shot_hotkey.clone())
        } else {
            ("截屏", cfg.shot_hotkey.clone(), cfg.coin_hotkey.clone())
        };
        if canon_hotkey(&cur).as_deref() != Some(PUNCT_TOGGLE_HOTKEY) {
            continue;
        }
        // 保留键自身也计入占用:否则另一侧已占 +alt 档时让位会原样
        // 返回 ctrl+period(它恰是"空闲"候选),等于没让位。
        match escalate_hotkey(&cur, &[other.as_str(), PUNCT_TOGGLE_HOTKEY]) {
            Some(next) => {
                notes.push(format!(
                    "{label}快捷键 {cur} 已保留给中英文标点切换,已自动改为 {next}(可在设置中修改)"
                ));
                if is_coin {
                    cfg.coin_hotkey = next;
                } else {
                    cfg.shot_hotkey = next;
                }
            }
            None => notes.push(format!(
                "{label}快捷键 {cur} 已保留给中英文标点切换且无法自动升级,请修改其中一项"
            )),
        }
    }
    let conflict = match (
        parse_hotkey(&cfg.coin_hotkey),
        parse_hotkey(&cfg.shot_hotkey),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => return notes,
    };
    if !conflict {
        return notes;
    }
    let coin = cfg.coin_hotkey.clone();
    match escalate_hotkey(&cfg.shot_hotkey, &[coin.as_str()]) {
        Some(next) => {
            notes.push(format!(
                "截屏快捷键 {} 与造词快捷键 {} 冲突,已自动改为 {}(可在设置中修改)",
                cfg.shot_hotkey, coin, next
            ));
            cfg.shot_hotkey = next;
        }
        None => notes.push(format!(
            "截屏快捷键 {} 与造词快捷键 {} 冲突且无法自动升级,请修改其中一项",
            cfg.shot_hotkey, coin
        )),
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parse_hotkey_规格与两端一致() {
        assert_eq!(parse_hotkey("ctrl+alt+a"), Some((MOD_CTRL | MOD_ALT, 0x61)));
        assert_eq!(parse_hotkey("ctrl+equal"), Some((MOD_CTRL, 0x3d)));
        assert_eq!(parse_hotkey("Ctrl + Shift + F5"), Some((MOD_CTRL | MOD_SHIFT, 0xffc2)));
        assert_eq!(parse_hotkey("alt+`"), Some((MOD_ALT, 0x60)));
        assert_eq!(parse_hotkey("super+0x31"), Some((MOD_SUPER, 0x31)));
        // 纯键(无修饰)与打字冲突,一律拒绝
        assert_eq!(parse_hotkey("a"), None);
        assert_eq!(parse_hotkey("equal"), None);
        assert_eq!(parse_hotkey("ctrl+foo"), None);
        assert_eq!(parse_hotkey(""), None);
    }

    #[test]
    fn canon_hotkey_别名与定序() {
        assert_eq!(canon_hotkey("ctrl+equal").as_deref(), Some("ctrl+equal"));
        assert_eq!(canon_hotkey("Ctrl + =").as_deref(), Some("ctrl+equal"));
        assert_eq!(canon_hotkey("ctrl+=").as_deref(), Some("ctrl+equal"));
        assert_eq!(canon_hotkey("control+equal").as_deref(), Some("ctrl+equal"));
        assert_eq!(canon_hotkey("Shift + mod4 + A").as_deref(), Some("super+shift+a"));
        assert_eq!(canon_hotkey("alt+mod1+a").as_deref(), Some("alt+a"));
        assert_eq!(canon_hotkey("ctrl+f13").as_deref(), Some("ctrl+f13"));
        assert_eq!(canon_hotkey("ctrl+0x31").as_deref(), Some("ctrl+1"));
        assert_eq!(canon_hotkey("ctrl+enter").as_deref(), Some("ctrl+return"));
        assert_eq!(canon_hotkey("ctrl+非法键").as_deref(), None);
    }

    #[test]
    fn escalate_hotkey_逐级加alt再加shift() {
        // 原组合空闲:原样返回(规范化形)
        assert_eq!(
            escalate_hotkey("ctrl+equal", &["ctrl+alt+a"]).as_deref(),
            Some("ctrl+equal")
        );
        // 用户示例:CTRL+= 被占 → +ALT → 还不行 → +SHIFT
        assert_eq!(
            escalate_hotkey("ctrl+equal", &["ctrl+equal"]).as_deref(),
            Some("ctrl+alt+equal")
        );
        assert_eq!(
            escalate_hotkey("ctrl+equal", &["ctrl+equal", "ctrl+alt+equal"]).as_deref(),
            Some("ctrl+alt+shift+equal")
        );
        // 三级全占用:放弃并交由调用方提示
        assert_eq!(
            escalate_hotkey(
                "ctrl+equal",
                &["ctrl+equal", "ctrl+alt+equal", "ctrl+alt+shift+equal"]
            ),
            None
        );
        // 别名写法同样构成占用(Ctrl + = ≡ ctrl+equal)
        assert_eq!(
            escalate_hotkey("ctrl+equal", &["Ctrl + ="]).as_deref(),
            Some("ctrl+alt+equal")
        );
        // 已含 alt 的组合:下一级直接 +shift
        assert_eq!(
            escalate_hotkey("alt+a", &["alt+a"]).as_deref(),
            Some("alt+shift+a")
        );
        // occupied 写法非法:不构成占用
        assert_eq!(
            escalate_hotkey("ctrl+equal", &["乱写"]).as_deref(),
            Some("ctrl+equal")
        );
        assert_eq!(escalate_hotkey("乱写", &[]), None);
    }

    #[test]
    fn resolve_config_hotkeys_截屏让位打字键() {
        // 默认配置无冲突:原样返回
        let mut cfg = Config::default();
        assert!(resolve_config_hotkeys(&mut cfg).is_empty());
        assert_eq!(cfg.shot_hotkey, "ctrl+alt+a");

        // 显式冲突(非默认值):截屏热键逐级升级
        let mut cfg = Config::default();
        cfg.coin_hotkey = "ctrl+alt+a".into();
        let notes = resolve_config_hotkeys(&mut cfg);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("ctrl+alt+shift+a"), "note={}", notes[0]);
        assert_eq!(cfg.shot_hotkey, "ctrl+alt+shift+a");

        // 用户示例:造词想要 Ctrl+= 而被占用 → 加载路径下仍是截屏让位
        let mut cfg = Config::default();
        cfg.shot_hotkey = "ctrl+equal".into();
        let notes = resolve_config_hotkeys(&mut cfg);
        assert_eq!(cfg.shot_hotkey, "ctrl+alt+equal");
        assert!(!notes.is_empty());
        // 造词保持 Ctrl+= 不变
        assert_eq!(cfg.coin_hotkey, "ctrl+equal");

        // 三级全占用(两侧都是顶格组合,阶梯无级可升):配置不变,给人话说明
        let mut cfg = Config::default();
        cfg.coin_hotkey = "ctrl+alt+shift+a".into();
        cfg.shot_hotkey = "ctrl+alt+shift+a".into();
        let notes = resolve_config_hotkeys(&mut cfg);
        assert_eq!(cfg.shot_hotkey, "ctrl+alt+shift+a");
        assert!(notes[0].contains("无法自动升级"), "note={}", notes[0]);

        // 任一写法非法:不参与冲突(宿主按各自合同回退默认)
        let mut cfg = Config::default();
        cfg.coin_hotkey = "a".into();
        cfg.shot_hotkey = "a".into();
        assert!(resolve_config_hotkeys(&mut cfg).is_empty());
    }

    #[test]
    fn resolve_config_hotkeys_保留键标点切换让位() {
        // Ctrl+. 保留给中英文标点切换:造词配成它 → +alt 让位
        let mut cfg = Config::default();
        cfg.coin_hotkey = "ctrl+period".into();
        let notes = resolve_config_hotkeys(&mut cfg);
        assert_eq!(cfg.coin_hotkey, "ctrl+alt+period");
        assert!(notes.iter().any(|n| n.contains("标点切换")), "{notes:?}");

        // 截屏配成它 → 同样让位;另一侧若已占 ctrl+alt+period 则落到
        // ctrl+alt+shift+period
        let mut cfg = Config::default();
        cfg.coin_hotkey = "ctrl+alt+period".into();
        cfg.shot_hotkey = "ctrl+period".into();
        resolve_config_hotkeys(&mut cfg);
        assert_eq!(cfg.shot_hotkey, "ctrl+alt+shift+period");
        assert_eq!(cfg.coin_hotkey, "ctrl+alt+period");

        // 两侧都配成保留键:各让一级,互不冲突
        let mut cfg = Config::default();
        cfg.coin_hotkey = "ctrl+period".into();
        cfg.shot_hotkey = "ctrl+period".into();
        resolve_config_hotkeys(&mut cfg);
        assert_eq!(cfg.coin_hotkey, "ctrl+alt+period");
        assert_eq!(cfg.shot_hotkey, "ctrl+alt+shift+period");

        // 别名写法(Ctrl + . / ctrl+.)同归保留键,同样让位
        let mut cfg = Config::default();
        cfg.coin_hotkey = "Ctrl + .".into();
        resolve_config_hotkeys(&mut cfg);
        assert_eq!(cfg.coin_hotkey, "ctrl+alt+period");
    }

    #[test]
    fn hotkey_string_与解析互逆() {
        for spec in ["ctrl+equal", "ctrl+alt+a", "super+shift+f5", "alt+0x2f"] {
            let (m, s) = parse_hotkey(spec).unwrap();
            let canon = hotkey_string(m, s);
            assert_eq!(parse_hotkey(&canon), Some((m, s)), "spec={spec}");
        }
    }
}
