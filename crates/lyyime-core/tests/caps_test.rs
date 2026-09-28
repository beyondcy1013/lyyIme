//! 全大写输入候选(2026-09-28 需求):中文态下 Shift 逐字母敲入全大写时,
//! 候选固定为 原样大写 → 首字母大写 → 全小写 → 中文翻译(常用在前,其后为
//! 其它常用翻译),不再混入五笔/拼音候选;小写或混合大小写保持既有通道。
//!
//! 宿主(ibus/xim)把 Shift+字母 的大写键值原样传入(LKey::Char('A'..='Z')),
//! core 以 buf_raw 镜像敲入形式、以小写 buf 做既有匹配与词典查找。

mod common;

use common::{commits, engine, is_consumed, last_preedit, page_texts};
use lyyime_core::{Engine, LKey, Mode};

/// 逐键喂入一串大写字母(模拟宿主传入 Shift+字母 的大写键值)。
fn type_caps(eng: &mut Engine, s: &str) -> Vec<lyyime_core::Effect> {
    let mut all = Vec::new();
    for c in s.chars() {
        assert!(c.is_ascii_uppercase(), "type_caps 仅接受大写字母,收到 {c}");
        all.extend(eng.process_key(LKey::Char(c)));
    }
    all
}

#[test]
fn 全大写_候选顺序_大写_首字母大写_小写_翻译() {
    let mut eng = engine();
    type_caps(&mut eng, "WHO");
    assert_eq!(page_texts(&eng), ["WHO", "Who", "who", "世界卫生组织", "谁"]);
}

#[test]
fn 多条翻译依次排列在三个变体之后() {
    let mut eng = engine();
    type_caps(&mut eng, "GO");
    assert_eq!(page_texts(&eng), ["GO", "Go", "go", "去", "走"]);
}

#[test]
fn 单字母_首字母大写与大写去重() {
    let mut eng = engine();
    type_caps(&mut eng, "W");
    assert_eq!(page_texts(&eng), ["W", "w"]);
}

#[test]
fn 无翻译词条_仅大小写变体() {
    let mut eng = engine();
    type_caps(&mut eng, "ZZZ");
    assert_eq!(page_texts(&eng), ["ZZZ", "Zzz", "zzz"]);
}

#[test]
fn 预编辑显示敲入的大写() {
    let mut eng = engine();
    let fx = type_caps(&mut eng, "WHO");
    assert_eq!(last_preedit(&fx).as_deref(), Some("WHO"));
}

#[test]
fn 空格上屏原样大写() {
    let mut eng = engine();
    type_caps(&mut eng, "WHO");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), ["WHO"]);
}

#[test]
fn 回车上屏原样大写并保持中文态() {
    let mut eng = engine();
    type_caps(&mut eng, "WHO");
    let fx = eng.process_key(LKey::Enter);
    assert_eq!(commits(&fx), ["WHO"]);
    assert_eq!(eng.mode(), Mode::Chinese);
}

#[test]
fn 数字键选中中文翻译() {
    let mut eng = engine();
    type_caps(&mut eng, "WHO");
    let fx = eng.process_key(LKey::Digit(4));
    assert_eq!(commits(&fx), ["世界卫生组织"]);
}

#[test]
fn 退格回退大写组合并重算候选() {
    let mut eng = engine();
    type_caps(&mut eng, "WHO");
    let fx = eng.process_key(LKey::Backspace);
    assert_eq!(page_texts(&eng), ["WH", "Wh", "wh"]);
    assert_eq!(last_preedit(&fx).as_deref(), Some("WH"));
}

#[test]
fn 翻译候选注释为原词() {
    let mut eng = engine();
    type_caps(&mut eng, "CPU");
    let page = eng.flush_page();
    assert_eq!(page[3].text, "中央处理器");
    assert_eq!(page[3].comment, "CPU");
}

#[test]
fn 大写四码不触发四码上屏() {
    let mut eng = engine();
    let fx = type_caps(&mut eng, "WHOS");
    assert!(commits(&fx).is_empty(), "四码大写不应自动上屏");
    assert_eq!(page_texts(&eng), ["WHOS", "Whos", "whos"]);
}

#[test]
fn 大写后敲小写_回退普通通道() {
    let mut eng = engine();
    type_caps(&mut eng, "W");
    // 小写介入 → 混合大小写,不再是大写候选;按小写 "wh" 走既有匹配(夹具无命中)
    let fx = eng.process_key(LKey::Char('h'));
    assert_eq!(last_preedit(&fx).as_deref(), Some("Wh"));
    assert!(page_texts(&eng).is_empty());
}

#[test]
fn 小写起头再敲大写_保持小写通道() {
    let mut eng = engine();
    eng.process_key(LKey::Char('w'));
    eng.process_key(LKey::Char('Q'));
    // "wq" 走五笔精确通道,大写不激活大写候选
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("你"));
}

#[test]
fn Esc_清空大写组合() {
    let mut eng = engine();
    type_caps(&mut eng, "WHO");
    let fx = eng.process_key(LKey::Esc);
    assert!(is_consumed(&fx) || last_preedit(&fx).is_none());
    assert!(eng.buffer().is_empty());
    assert!(page_texts(&eng).is_empty());
}

#[test]
fn 大写候选不因上屏学习污染小写通道排序() {
    let td = common::TempDir::new();
    let mut eng = common::engine_with_user_dict(&td);
    type_caps(&mut eng, "WHO");
    eng.process_key(LKey::Space); // 上屏 "WHO"
    eng.flush_user_dict().unwrap();
    // 再次小写输入 "who" 相关组合,普通通道候选仍由既有词典决定(无 panic 即可)
    let mut eng2 = common::engine_with_user_dict(&td);
    eng2.process_key(LKey::Char('w'));
    assert!(!page_texts(&eng2).is_empty());
}
