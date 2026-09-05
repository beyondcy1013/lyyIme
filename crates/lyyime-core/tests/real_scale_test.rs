//! 真实词库尺度回归测试(跨源频率错配修复验收,fixtures-real-scale/)。
//!
//! 数据特征:拼音单字 ~1e9(ibus 尺度)、拼音词组 ~1e3(jieba 尺度)、
//! 五笔 ~1e3、英文 ~2e4 —— 复现旧加性公式下的 4 个严重问题:
//! 跨尺度碾压、精确被前缀淹没、英文通道被门控死、伪切分(xia+n)污染候选。

mod common;

use common::*;
use lyyime_core::{CandKind, Config, LKey};

// ----------------------------------------------------------------------
// 问题 2:五笔精确(含简码)必须压过同码前缀词组
// ----------------------------------------------------------------------

#[test]
fn 真尺度_a_首选工_简码不丢() {
    let mut eng = engine_real();
    type_str(&mut eng, "a");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "工", "a 码精确的工必须第一");
    assert_eq!(page[0].kind, CandKind::Wubi);
    assert!(
        page[0].score >= 60.0 && page[0].score < 71.0,
        "应处五笔精确层:{}",
        page[0].score
    );
    // 前缀词组仍在列(层级在前缀层),只是不再压过精确。
    let texts = page_texts(&eng);
    assert!(texts.contains(&"工作".to_string()) && texts.contains(&"世界".to_string()));
}

#[test]
fn 真尺度_wq_首选我们() {
    let mut eng = engine_real();
    type_str(&mut eng, "wq");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("我们"));
    assert!(
        page_texts(&eng).contains(&"您家".to_string()),
        "wq 前缀词仍在渐进候选"
    );
}

// ----------------------------------------------------------------------
// 问题 1:拼音单字(1e9)不得碾压词组(1e3)
// ----------------------------------------------------------------------

#[test]
fn 真尺度_nihao_首选你好() {
    let mut eng = engine_real();
    type_str(&mut eng, "nihao");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "你好", "词组全拼必须排在末音节单字之前");
    assert!(
        page[0].score >= 50.0 && page[0].score < 60.0,
        "应处拼音完整层"
    );
    assert!(
        page_texts(&eng).contains(&"好".to_string()),
        "末音节单字仍在列"
    );
}

#[test]
fn 真尺度_nihao_空格顶屏你好() {
    let mut eng = engine_real();
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
}

#[test]
fn 真尺度_woaini_首选我爱你() {
    let mut eng = engine_real();
    type_str(&mut eng, "woaini");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("我爱你"));
}

#[test]
fn 真尺度_zhongguo_首选中国() {
    let mut eng = engine_real();
    type_str(&mut eng, "zhongguo");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("中国"));
}

// ----------------------------------------------------------------------
// 问题 4:伪切分(xia+n)不得污染 xian 候选
// ----------------------------------------------------------------------

#[test]
fn 真尺度_xian_完整音节候选在列且无伪切分字() {
    let mut eng = engine_real();
    type_str(&mut eng, "xian");
    let mut all = page_texts(&eng);
    assert_eq!(
        all.first().map(String::as_str),
        Some("西安"),
        "xi'an 词组应居首"
    );
    eng.process_key(LKey::PageDown); // 翻到第二页
    all.extend(page_texts(&eng));
    for expect in ["先", "县", "现"] {
        assert!(
            all.contains(&expect.to_string()),
            "完整音节 xian 的字 {expect} 应在列:{all:?}"
        );
    }
    for bad in ["年", "您", "能"] {
        assert!(
            !all.contains(&bad.to_string()),
            "伪切分 xia+n 的字 {bad} 不得出现:{all:?}"
        );
    }
}

// ----------------------------------------------------------------------
// 问题 3:英文通道放宽 —— 有中文命中时高频完整英文词仍可用
// ----------------------------------------------------------------------

#[test]
fn 真尺度_the_英文候选在列且标点直通() {
    let mut eng = engine_real();
    type_str(&mut eng, "the");
    let texts = page_texts(&eng);
    assert!(
        texts.contains(&"the".to_string()),
        "the 应作为英文候选出现:{texts:?}"
    );
    let fx = eng.process_key(LKey::Punct(','));
    assert_eq!(
        commits(&fx),
        vec!["the".to_string(), ",".to_string()],
        "高频英文词遇标点直通"
    );
}

#[test]
fn 真尺度_he_有中文命中英文仍在列且空格顶中文() {
    let mut eng = engine_real();
    type_str(&mut eng, "he");
    let texts = page_texts(&eng);
    assert!(texts.contains(&"和".to_string()), "中文命中正常在列");
    assert!(
        texts.contains(&"he".to_string()),
        "top-500 英文词应作为 english_with_cn 层在列:{texts:?}"
    );
    let fx = eng.process_key(LKey::Space);
    assert_eq!(
        commits(&fx),
        vec!["和".to_string()],
        "Space 确认当前首选,英文需 Shift 态输入"
    );
}

#[test]
fn 真尺度_超出top上限的英文词_有中文命中时不出现() {
    // he 词频第 2 名;把 mixed_auto_commit_top_n 压到 1 后不再以 with_cn 层出现。
    let mut eng = engine_with_fixtures(
        &fixtures_real(),
        Config {
            mixed_auto_commit_top_n: 1,
            ..Config::default()
        },
    );
    type_str(&mut eng, "he");
    assert!(
        !page_texts(&eng).contains(&"he".to_string()),
        "超出 top_n 的英文词不应在有中文命中时出现:{:?}",
        page_texts(&eng)
    );
}
