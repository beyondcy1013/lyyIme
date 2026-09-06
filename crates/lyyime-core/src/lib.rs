//! # lyyime-core
//!
//! lyyIme 的纯逻辑输入引擎:码表加载、五笔/拼音/英文匹配排序、
//! 按键行为状态机、用户词学习、C ABI FFI。无 GUI / 无 X11 依赖,可 headless 测试。
//!
//! 两个宿主(Mode A ibus python 引擎、Mode B lyyime-app X11 外挂)只负责:
//! 按键采集与路由(翻译成 [`LKey`])、候选窗渲染、上屏执行([`Effect::Commit`])。
//!
//! ## 模块地图
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`types`] | LKey / Effect / Candidate / Mode / CandKind |
//! | [`config`] | Config 与 config.toml 读写(缺失/损坏回退默认并给出人话提示) |
//! | [`pinyin`] | 自写音节 DP 切分(含 xian = xi'an 歧义与末音节不完整) |
//! | [`rank`] | 合同 §5 的打分排序公式 |
//! | [`ffi`] | C ABI 导出(合同 §3)与效果流 JSON |
//! | [`punct`] | 中文标点映射(引号开合交替) |
//!
//! dict / engine / learner 为内部实现模块,通过 [`Engine`] 使用。
//!
//! ## 借鉴出处(按项目质量红线标注)
//!
//! - 码表按频次排序、候选渐进习惯:ibus-table(海峰86 码表引擎);
//! - 中英混打、简码优先、标点/Shift 交互:搜狗输入法、万能五笔的常用习惯;
//! - 拼音音节切分思路:fcitx5/libime 的切分器(此处为自写极简 DP 枚举版);
//! - FFI 效果流形态:ibus 引擎的 commit/update_preedit/update_lookup_table 事件模型。
//!
//! ## 最小示例
//!
//! ```no_run
//! use lyyime_core::{Effect, Engine, LKey, Mode};
//! use std::path::Path;
//!
//! let mut eng = Engine::new(Path::new("data/runtime")).unwrap();
//! for c in "nihao".chars() {
//!     let _ = eng.process_key(LKey::Char(c));
//! }
//! let effects = eng.process_key(LKey::Space); // 顶屏首选
//! assert!(matches!(effects.first(), Some(Effect::Commit(_))));
//! assert_eq!(eng.mode(), Mode::Chinese);
//! ```

pub mod config;
pub mod error;
pub mod ffi;
pub mod hotkey;
pub mod punct;
pub mod stats;

mod dict;
mod engine;
mod learner;
mod pinyin;
mod rank;
mod types;
mod user_words;

pub use config::Config;
pub use engine::Engine;
pub use error::Error;
pub use ffi::effects_json;
pub use punct::{to_chinese, QuoteState};
pub use types::{CandKind, Candidate, Effect, LKey, Mode};

#[cfg(test)]
mod tests {
    use super::*;

    /// 合同要求 Engine 为 Send(宿主可能跨线程持有)。
    #[test]
    fn engine_is_send() {
        fn requires_send<T: Send>() {}
        requires_send::<Engine>();
    }
}
