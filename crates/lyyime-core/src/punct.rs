//! 中文标点映射(合同 §6,默认集;可由宿主基于 [`to_chinese`] 自行扩展)。
//!
//! 映射习惯对齐搜狗/万能五笔默认行为:
//! `, . ? ! ; :` 转全角 `，。？！；：`,`[ ]` 转 `【】`,`( ) { }` 转全角括号,
//! 单/双引号按开合交替输出(`'` → `'`、`'`,`"` → `"`、`"`)。

/// 单/双引号的开合状态(跨按键持久,由 Engine 持有)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteState {
    /// 下一个单引号是否为开引号 `'`。
    pub single_open: bool,
    /// 下一个双引号是否为开引号 `"`。
    pub double_open: bool,
}

impl QuoteState {
    /// 初始状态:两种引号都从开引号开始。
    pub fn new() -> Self {
        Self {
            single_open: true,
            double_open: true,
        }
    }
}

impl Default for QuoteState {
    fn default() -> Self {
        Self::new()
    }
}

/// 把半角标点映射为中文标点。
///
/// 返回 `None` 表示无映射(该键应直通);引号输出后自动翻转开合状态。
/// 映射值一律用 Unicode 转义书写:全角/半角在源码里肉眼不可分,
/// 历史上映射表曾被写成半角恒等(中文态标点一直上半角),单测也照抄没拦住。
pub fn to_chinese(c: char, quotes: &mut QuoteState) -> Option<char> {
    let out = match c {
        ',' => '\u{FF0C}', // ，
        '.' => '\u{3002}', // 。
        '?' => '\u{FF1F}', // ？
        '!' => '\u{FF01}', // ！
        ';' => '\u{FF1B}', // ；
        ':' => '\u{FF1A}', // ：
        '\'' => {
            let open = quotes.single_open;
            quotes.single_open = !open;
            if open {
                '\u{2018}'
            } else {
                '\u{2019}'
            }
        }
        '"' => {
            let open = quotes.double_open;
            quotes.double_open = !open;
            if open {
                '\u{201C}'
            } else {
                '\u{201D}'
            }
        }
        '(' => '\u{FF08}', // （
        ')' => '\u{FF09}', // ）
        '[' => '\u{3010}', // 【
        ']' => '\u{3011}', // 】
        '{' => '\u{FF5B}', // ｛
        '}' => '\u{FF5D}', // ｝
        _ => return None,
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 常用标点映射正确() {
        let mut q = QuoteState::new();
        assert_eq!(to_chinese(',', &mut q), Some('\u{FF0C}'));
        assert_eq!(to_chinese('.', &mut q), Some('\u{3002}'));
        assert_eq!(to_chinese('?', &mut q), Some('\u{FF1F}'));
        assert_eq!(to_chinese('!', &mut q), Some('\u{FF01}'));
        assert_eq!(to_chinese(';', &mut q), Some('\u{FF1B}'));
        assert_eq!(to_chinese(':', &mut q), Some('\u{FF1A}'));
        assert_eq!(to_chinese('(', &mut q), Some('\u{FF08}'));
        assert_eq!(to_chinese(')', &mut q), Some('\u{FF09}'));
        assert_eq!(to_chinese('[', &mut q), Some('\u{3010}'));
        assert_eq!(to_chinese(']', &mut q), Some('\u{3011}'));
        assert_eq!(to_chinese('{', &mut q), Some('\u{FF5B}'));
        assert_eq!(to_chinese('}', &mut q), Some('\u{FF5D}'));
    }

    #[test]
    fn 映射值必须是全角而非半角恒等() {
        // 回归:映射表曾整体写成半角恒等,这里逐项断言"输出 != 输入"。
        let mut q = QuoteState::new();
        for c in [',', '.', '?', '!', ';', ':', '(', ')', '{', '}'] {
            let out = to_chinese(c, &mut q).expect("常用标点必须有映射");
            assert_ne!(c, out, "{c} 不应映射为自身");
        }
    }

    #[test]
    fn 引号按开合交替() {
        let mut q = QuoteState::new();
        assert_eq!(to_chinese('\'', &mut q), Some('\u{2018}'));
        assert_eq!(to_chinese('\'', &mut q), Some('\u{2019}'));
        assert_eq!(to_chinese('\'', &mut q), Some('\u{2018}'));
        assert_eq!(to_chinese('"', &mut q), Some('\u{201C}'));
        assert_eq!(to_chinese('"', &mut q), Some('\u{201D}'));
    }

    #[test]
    fn 未映射标点返回_none() {
        let mut q = QuoteState::new();
        assert_eq!(to_chinese('/', &mut q), None);
        assert_eq!(to_chinese('@', &mut q), None);
    }
}
