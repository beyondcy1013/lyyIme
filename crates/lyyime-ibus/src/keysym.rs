//! keysym 常量与 keysym → LKey 映射(合同 ARCHITECTURE.md §2/§3)。
//! 数值与 python 旧引擎(已删除)及 X/ibus 修饰位掩码一致。

use lyyime_core::LKey;

// X keysym 常量
pub const KSYM_A: u32 = 0x61;
pub const KSYM_Z: u32 = 0x7a;
pub const KSYM_1: u32 = 0x31;
pub const KSYM_9: u32 = 0x39;
pub const KSYM_SPACE: u32 = 0x20;
pub const KSYM_RETURN: u32 = 0xff0d;
pub const KSYM_KP_ENTER: u32 = 0xff8d;
pub const KSYM_BACKSPACE: u32 = 0xff08;
pub const KSYM_ESCAPE: u32 = 0xff1b;
pub const KSYM_PAGE_UP: u32 = 0xff55;
pub const KSYM_PAGE_DOWN: u32 = 0xff56;
pub const KSYM_SHIFT_L: u32 = 0xffe1;
pub const KSYM_SHIFT_R: u32 = 0xffe2;
pub const KSYM_KP_1: u32 = 0xffb1;
pub const KSYM_KP_9: u32 = 0xffb9;

pub const SHIFT_KEYVALS: (u32, u32) = (KSYM_SHIFT_L, KSYM_SHIFT_R);

// 修饰位掩码(X/ibus 通用;数值与 IBus.ModifierType 一致)
pub const MASK_SHIFT: u32 = 1 << 0;
pub const MASK_LOCK: u32 = 1 << 1; // CapsLock:大写态字母直通英文(合同 §6)
pub const MASK_CTRL: u32 = 1 << 2;
pub const MASK_ALT: u32 = 1 << 3; // MOD1
pub const MASK_SUPER: u32 = (1 << 6) | (1 << 26); // MOD4 | SUPER
pub const MASK_RELEASE: u32 = 1 << 30;
/// 带 Ctrl/Alt/Super 的按键是应用快捷键:不算 Shift 单击、不进缓冲
pub const BLOCKING_MODS: u32 = MASK_CTRL | MASK_ALT | MASK_SUPER;

// InputPurpose 整型值(与 IBus.InputPurpose 一致)
pub const PURPOSE_PASSWORD: u32 = 8;
pub const PURPOSE_PIN: u32 = 9;

#[inline]
pub fn is_shift(keyval: u32) -> bool {
    keyval == SHIFT_KEYVALS.0 || keyval == SHIFT_KEYVALS.1
}

// ---------------------------------------------------------------------
// IM 托管组合热键(合同 §12 造词 / §13 截屏)
// 解析/规范化/冲突升级规格单源在 lyyime-core::hotkey(本模块 re-export),
// 与 Mode B keysym_map.c lyy_hotkey_parse 同规格,两端单测各自覆盖。
// ---------------------------------------------------------------------

/// 热键匹配用修饰位(只比这四位;CapsLock/NumLock 等不影响热键)
pub const HOTKEY_CLEAN_MODS: u32 = MASK_SHIFT | MASK_CTRL | MASK_ALT | MASK_SUPER;

pub use lyyime_core::hotkey::parse_hotkey;

/// 精确匹配:修饰位只比 HOTKEY_CLEAN_MODS 四位 + 键值相等。
#[inline]
pub fn hotkey_match(state: u32, keyval: u32, mods: u32, sym: u32) -> bool {
    (state & HOTKEY_CLEAN_MODS) == mods && keyval == sym
}

/// keysym → (LKey, 码点);无法识别时回 (LKey::Other, 0)。
///
/// 注:Digit 的数值经 char 参数按码点传递('1'=0x31),core 取 chr-0x30
/// 得 1..9(合同 §3 的补充约定)。
pub fn map_keyval(keyval: u32) -> (LKey, u32) {
    if (KSYM_A..=KSYM_Z).contains(&keyval) {
        return (LKey::Char(keyval as u8 as char), keyval);
    }
    // Shift 产生的大写键值:小写化入缓冲(CapsLock 大写态在 logic 层直通,不会到这里)
    if (0x41..=0x5a).contains(&keyval) {
        return (LKey::Char((keyval + 0x20) as u8 as char), keyval + 0x20);
    }
    if (KSYM_1..=KSYM_9).contains(&keyval) {
        return (LKey::Digit((keyval - KSYM_1 + 1) as u8), keyval);
    }
    // 小键盘数字也可选词
    if (KSYM_KP_1..=KSYM_KP_9).contains(&keyval) {
        return (LKey::Digit((keyval - KSYM_KP_1 + 1) as u8), 0x31 + (keyval - KSYM_KP_1));
    }
    if keyval == KSYM_SPACE {
        return (LKey::Space, 0);
    }
    if keyval == KSYM_RETURN || keyval == KSYM_KP_ENTER {
        return (LKey::Enter, 0);
    }
    if keyval == KSYM_BACKSPACE {
        return (LKey::Backspace, 0);
    }
    if keyval == KSYM_ESCAPE {
        return (LKey::Esc, 0);
    }
    if keyval == KSYM_PAGE_UP {
        return (LKey::PageUp, 0);
    }
    if keyval == KSYM_PAGE_DOWN {
        return (LKey::PageDown, 0);
    }
    // 其余可打印 ASCII 符号(标点、-=、空格之外的)一律按标点送 core
    if (0x21..=0x7e).contains(&keyval) {
        return (LKey::Punct(keyval as u8 as char), keyval);
    }
    (LKey::Other, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_letters_pass_through() {
        assert_eq!(map_keyval(0x61), (LKey::Char('a'), 0x61));
        assert_eq!(map_keyval(0x7a), (LKey::Char('z'), 0x7a));
    }

    #[test]
    fn uppercase_lowercased_into_buffer() {
        assert_eq!(map_keyval(0x41), (LKey::Char('a'), 0x61));
        assert_eq!(map_keyval(0x5a), (LKey::Char('z'), 0x7a));
    }

    #[test]
    fn digits_and_keypad() {
        assert_eq!(map_keyval(0x31), (LKey::Digit(1), 0x31));
        assert_eq!(map_keyval(0x39), (LKey::Digit(9), 0x39));
        assert_eq!(map_keyval(0xffb1), (LKey::Digit(1), 0x31));
        assert_eq!(map_keyval(0xffb9), (LKey::Digit(9), 0x39));
    }

    #[test]
    fn special_keys() {
        assert_eq!(map_keyval(KSYM_SPACE), (LKey::Space, 0));
        assert_eq!(map_keyval(KSYM_RETURN), (LKey::Enter, 0));
        assert_eq!(map_keyval(KSYM_KP_ENTER), (LKey::Enter, 0));
        assert_eq!(map_keyval(KSYM_BACKSPACE), (LKey::Backspace, 0));
        assert_eq!(map_keyval(KSYM_ESCAPE), (LKey::Esc, 0));
        assert_eq!(map_keyval(KSYM_PAGE_UP), (LKey::PageUp, 0));
        assert_eq!(map_keyval(KSYM_PAGE_DOWN), (LKey::PageDown, 0));
    }

    #[test]
    fn hotkey_parse_常用写法() {
        assert_eq!(parse_hotkey("ctrl+alt+a"), Some((MASK_CTRL | MASK_ALT, 0x61)));
        assert_eq!(parse_hotkey("ctrl+equal"), Some((MASK_CTRL, 0x3d)));
        assert_eq!(parse_hotkey("Ctrl + Shift + F5"), Some((MASK_CTRL | MASK_SHIFT, 0xffc2)));
        assert_eq!(parse_hotkey("alt+`"), Some((MASK_ALT, 0x60)));
        assert_eq!(parse_hotkey("super+0x31"), Some((MASK_SUPER, 0x31)));
        assert_eq!(parse_hotkey("ctrl+pagedown"), Some((MASK_CTRL, 0xff56)));
        // f13 跨段
        assert_eq!(parse_hotkey("ctrl+f13"), Some((MASK_CTRL, 0xffcc)));
    }

    #[test]
    fn hotkey_parse_非法拒绝() {
        // 无修饰(纯键与打字冲突一律拒绝)
        assert_eq!(parse_hotkey("a"), None);
        assert_eq!(parse_hotkey("equal"), None);
        assert_eq!(parse_hotkey("ctrl+"), None);
        assert_eq!(parse_hotkey("ctrl+ctrl"), None); // 键名不是修饰/键表成员
        assert_eq!(parse_hotkey(""), None);
        assert_eq!(parse_hotkey("mod1+"), None);
    }

    #[test]
    fn hotkey_match_只比四修饰位() {
        let (mods, sym) = parse_hotkey("ctrl+alt+a").unwrap();
        assert!(hotkey_match(MASK_CTRL | MASK_ALT, 0x61, mods, sym));
        // CapsLock/NumLock 等杂位不影响
        assert!(hotkey_match(MASK_CTRL | MASK_ALT | MASK_LOCK | (1 << 4), 0x61, mods, sym));
        assert!(!hotkey_match(MASK_CTRL, 0x61, mods, sym));
        assert!(!hotkey_match(MASK_CTRL | MASK_ALT, 0x62, mods, sym));
        // release 位被忽略
        assert!(hotkey_match(MASK_CTRL | MASK_ALT | MASK_RELEASE, 0x61, mods, sym));
    }

    #[test]
    fn printable_ascii_as_punct_others_pass() {
        assert_eq!(map_keyval(0x2c), (LKey::Punct(','), 0x2c));
        assert_eq!(map_keyval(0x3f), (LKey::Punct('?'), 0x3f));
        assert_eq!(map_keyval(0xff14), (LKey::Other, 0)); // 全角键值等不可识别
    }
}
