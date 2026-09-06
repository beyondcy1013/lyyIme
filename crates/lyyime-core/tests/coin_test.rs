//! 造词模式集成测试(合同 §12):Ctrl+= 进入、方向键增减选字、Enter 存词、
//! Esc 取消、编码规则、user_words.tsv 持久化与重启加载。

mod common;

use common::*;
use lyyime_core::{Config, Effect, Engine, LKey};
use std::path::Path;

/// 依次上屏 一串 wubi 编码(每码后跟空格顶屏),返回全部效果。
fn type_words(eng: &mut Engine, words: &[&str]) -> Vec<Effect> {
    let mut all = Vec::new();
    for w in words {
        all.extend(type_str(eng, w));
        all.extend(eng.process_key(LKey::Space));
    }
    all
}

// ======================================================================
// 进入造词模式
// ======================================================================

#[test]
fn 造词初始选取最近一次上屏词() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["wqvb"]); // 你好
    let fx = eng.process_key(LKey::Coin);
    assert_eq!(last_preedit(&fx).as_deref(), Some("造词:你好"));
    assert_eq!(coin_candidate(&eng), Some(("你好".into(), "wqvb".into())));
}

#[test]
fn 单字上屏后造词自动凑二字起步() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["wq", "vbg"]); // 你 好
    let _ = eng.process_key(LKey::Coin);
    // last_run=1(好),自动带上前一字 → 你好。
    assert_eq!(coin_candidate(&eng), Some(("你好".into(), "wqvb".into())));
}

#[test]
fn 无历史时造词给提示() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    let fx = eng.process_key(LKey::Coin);
    assert!(!notices(&fx).is_empty() && notices(&fx)[0].contains("没有可造词"));
    assert!(fx.iter().any(|e| matches!(e, Effect::Consumed)));
    assert!(!fx.iter().any(|e| matches!(e, Effect::Pass)));
}

#[test]
fn 组合中按造词键先上屏再进入() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_str(&mut eng, "wq"); // 缓冲中有 wq(候选:你)
    let fx = eng.process_key(LKey::Coin);
    assert!(has_commit(&fx, "你"), "先按普通流程顶屏首选");
    assert_eq!(coin_candidate(&eng), Some(("你".into(), "wqiy".into())));
}

#[test]
fn 英文态造词键直通() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["wqvb"]);
    eng.toggle_mode();
    assert!(is_pass(&eng.process_key(LKey::Coin)));
}

// ======================================================================
// 方向键增减选字
// ======================================================================

#[test]
fn 方向键多选与少选一个字() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["dh", "dh", "wq", "vbg"]); // 到 稻 你 好
    let _ = eng.process_key(LKey::Coin);
    assert_eq!(coin_candidate(&eng).map(|(t, _)| t), Some("你好".into()));
    // → 多选一字
    let _ = eng.process_key(LKey::ArrowRight);
    assert_eq!(coin_candidate(&eng).map(|(t, _)| t), Some("到你好".into()));
    // ↑ 也是多选
    let _ = eng.process_key(LKey::ArrowUp);
    assert_eq!(
        coin_candidate(&eng).map(|(t, _)| t),
        Some("到到你好".into())
    );
    // ← 少选一字
    let _ = eng.process_key(LKey::ArrowLeft);
    assert_eq!(coin_candidate(&eng).map(|(t, _)| t), Some("到你好".into()));
    // ↓ 与退格同为少选
    let _ = eng.process_key(LKey::ArrowDown);
    assert_eq!(coin_candidate(&eng).map(|(t, _)| t), Some("你好".into()));
    let _ = eng.process_key(LKey::Backspace);
    // 已到二字下限:不再减
    assert_eq!(coin_candidate(&eng).map(|(t, _)| t), Some("你好".into()));
}

#[test]
fn 选长上限与历史容量() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    // 上屏 10 次"你好",历史 20 字。
    type_words(&mut eng, &["wqvb"; 10]);
    let _ = eng.process_key(LKey::Coin);
    for _ in 0..25 {
        let _ = eng.process_key(LKey::ArrowRight);
    }
    assert_eq!(
        coin_candidate(&eng).map(|(t, _)| t),
        Some("你好".repeat(10))
    );
}

#[test]
fn 非造词模式方向键放行() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    assert!(is_pass(&eng.process_key(LKey::ArrowLeft)));
    type_str(&mut eng, "wq");
    // 有缓冲:清缓冲后放行(同 Other 语义)。
    let fx = eng.process_key(LKey::ArrowRight);
    assert!(fx.iter().any(|e| matches!(e, Effect::Pass)));
    assert!(eng.buffer().is_empty());
    assert!(eng.flush_page().is_empty(), "候选页应一并清空");
}

// ======================================================================
// 存词、取消与编码规则
// ======================================================================

#[test]
fn 回车存词编码正确并可立即打出() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["wqvb"]); // 你好
    let _ = eng.process_key(LKey::Coin);
    let fx = eng.process_key(LKey::Enter);
    assert!(notices(&fx)
        .iter()
        .any(|s| s.contains("已造词:你好(wqvb)")));
    // 立即可用编码打出(内存索引已并入;wqvb 唯一四码,免空格直接上屏)。
    let fx = type_str(&mut eng, "wqvb");
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
    // user_words.tsv 已落盘。
    let content =
        std::fs::read_to_string(td.join("user_words.tsv")).expect("user_words.tsv 应已生成");
    assert!(
        content.lines().any(|l| l.starts_with("你好\twqvb\t")),
        "落盘内容缺造词条目: {content}"
    );
}

#[test]
fn 三字词四字词多字词取码规则() {
    // 直接经由存词路径校验取码:到(dh) 你(wqiy) 好(vbg) → 3字词 d+w+vb。
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["dh", "wq", "vbg"]); // 到 你 好
    let _ = eng.process_key(LKey::Coin);
    // 初始选取=最近一次上屏(好),扩到三字。
    let _ = eng.process_key(LKey::ArrowRight);
    assert_eq!(coin_candidate(&eng).map(|(t, _)| t), Some("到你好".into()));
    let fx = eng.process_key(LKey::Enter);
    assert!(notices(&fx).iter().any(|s| s.contains("(dwvb)")));
}

#[test]
fn 造词跨重启仍可打出() {
    let td = TempDir::new();
    {
        let mut eng = engine_with_user_dict(&td);
        type_words(&mut eng, &["wqvb"]);
        let _ = eng.process_key(LKey::Coin);
        let _ = eng.process_key(LKey::Enter);
    }
    // 新引擎(同数据目录):user_words.tsv 随加载并入词库。
    let mut eng2 = engine_with_user_dict(&td);
    let fx = type_str(&mut eng2, "wqvb");
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
}

#[test]
fn esc_取消造词模式() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["wqvb"]);
    let _ = eng.process_key(LKey::Coin);
    let fx = eng.process_key(LKey::Esc);
    assert_eq!(last_preedit(&fx), None);
    assert!(eng.flush_page().is_empty());
    // 取消后字母照常进缓冲。
    type_str(&mut eng, "wq");
    assert_eq!(eng.buffer(), "wq");
}

#[test]
fn 造词模式中数字放行标点取消并输出中文标点() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["wqvb"]);
    let _ = eng.process_key(LKey::Coin);
    // 数字:无候选页 → 原样放行,退出造词。
    assert!(is_pass(&eng.process_key(LKey::Digit(3))));
    assert!(eng.flush_page().is_empty(), "退出造词后不应残留候选页");
    // 重新进入后按逗号:取消造词,标点按当前合同输出。
    let _ = eng.process_key(LKey::Coin);
    let fx = eng.process_key(LKey::Punct(','));
    assert!(has_commit(&fx, "\u{FF0C}"), "标点效果流: {:?}", commits(&fx));
    assert!(eng.flush_page().is_empty());
}

#[test]
fn 缺码字造词给失败提示() {
    // 临时词库:中 只进拼音表、不进五笔表 → 拼音上屏「中」后造词必失败。
    let td = TempDir::new();
    std::fs::copy(fixtures().join("wubi.tsv"), td.join("wubi.tsv")).unwrap();
    std::fs::copy(fixtures().join("pinyin_char.tsv"), td.join("pinyin_char.tsv")).unwrap();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        std::fs::read_to_string(fixtures().join("pinyin_char.tsv")).unwrap() + "zhong\t中\t900\n",
    )
    .unwrap();
    let mut eng = Engine::new(&td.path).unwrap();
    // 本测验证拼音上屏与造词失败提示:关掉四码唯一上屏——夹具小词库里
    // zhon(zhong 的前缀)候选唯一,会在第 4 键把「中」提前上屏。
    eng.set_config(Config {
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "zhong");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("中"));
    let _ = eng.process_key(LKey::Space);
    let _ = eng.process_key(LKey::Coin);
    let fx = eng.process_key(LKey::Enter);
    assert!(notices(&fx)
        .iter()
        .any(|s| s.contains("「中」不在五笔码表")));
    assert!(!Path::new(&td.join("user_words.tsv")).exists() || {
        // 文件不存在=未落盘;存在则不得包含失败的词
        let c = std::fs::read_to_string(td.join("user_words.tsv")).unwrap();
        !c.contains("中")
    });
}

#[test]
fn 造词模式点选候选被吞() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_words(&mut eng, &["wqvb"]);
    let _ = eng.process_key(LKey::Coin);
    assert!(is_consumed(&eng.select_candidate(0)));
}
