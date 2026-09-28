//! 候选右键菜单集成测试(合同 §15):固定首位 / 删除词组 / 反查英文。
//! 覆盖 plan/apply 语义、持久化文件(pinned.tsv / blocked.tsv)、
//! 重启重载、再造词自动解屏蔽,以及 FFI `lyyime_cand_op`/`lyyime_cand_pinned`。

mod common;

use common::*;
use lyyime_core::ffi::{
    lyyime_cand, lyyime_cand_op, lyyime_cand_pinned, lyyime_free, lyyime_new,
    lyyime_process_key, LKEY_CHAR,
};
use lyyime_core::{CandOp, Config, Effect, Engine, LKey};
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Once;

/// 进程级 HOME 隔离:Engine::new 会先按默认路径读一次用户数据
/// (user_words/pinned/blocked),指向临时目录杜绝真实 HOME 数据污染。
fn isolated_home() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let td: &'static TempDir = Box::leak(Box::new(TempDir::new()));
        std::env::set_var("HOME", &td.path);
    });
}

/// 测试引擎:fixtures 词库 + 给定用户词典目录(固定/屏蔽表随之入 td);
/// 关掉四码自动上屏,保证敲满四码后候选仍在(右键操作要求"候选可见")。
fn eng_at(td: &TempDir) -> Engine {
    isolated_home();
    engine_with_fixtures(
        &fixtures(),
        Config {
            user_dict: Some(td.join("user.tsv")),
            commit_first_at_four: false,
            commit_unique_four: false,
            ..Config::default()
        },
    )
}

/// 当前页中某词的下标(0 起);不在返回 None。
fn idx_of(eng: &Engine, word: &str) -> Option<usize> {
    eng.flush_page().iter().position(|c| c.text == word)
}

#[test]
fn 固定首位_置顶带标记且持久化() {
    let td = TempDir::new();
    let mut eng = eng_at(&td);
    type_str(&mut eng, "dh"); // wubi 重码:到(3000)/稻(2000),自然序 到 在前
    let idx = idx_of(&eng, "稻").expect("候选应有 稻");
    assert!(idx > 0, "稻 应非自然首位(idx={idx})");
    assert_eq!(eng.cand_pinned(idx), Some(false));

    let fx = eng.cand_op(idx, CandOp::PinToggle);
    assert!(fx.iter().any(|e| matches!(e, Effect::Candidates(_))));
    assert_eq!(eng.flush_page()[0].text, "稻", "固定后应置顶");
    assert!(eng.flush_page()[0].comment.contains('固'), "应有固标记");
    assert_eq!(eng.cand_pinned(0), Some(true));

    // pinned.tsv 已落盘
    let pinned = std::fs::read_to_string(td.join("pinned.tsv")).unwrap();
    assert!(pinned.lines().any(|l| l == "dh\t稻"), "pinned.tsv: {pinned}");

    // 重启引擎(同数据目录):仍居首、菜单状态为已固定
    let mut eng2 = eng_at(&td);
    type_str(&mut eng2, "dh");
    assert_eq!(eng2.flush_page()[0].text, "稻");
    assert_eq!(eng2.cand_pinned(0), Some(true));

    // 同操作再触发 = 取消固定:恢复自然序、文件行消失
    let _ = eng2.cand_op(0, CandOp::PinToggle);
    assert_eq!(eng2.flush_page()[0].text, "到", "取消固定后应回落自然序");
    let pinned2 = std::fs::read_to_string(td.join("pinned.tsv")).unwrap();
    assert!(
        !pinned2.lines().any(|l| l.split('\t').next() == Some("dh")),
        "unpin 后 pinned.tsv 应无 dh 行: {pinned2}"
    );
}

#[test]
fn 删除词组_屏蔽写入且重启后仍屏蔽() {
    let td = TempDir::new();
    let mut eng = eng_at(&td);
    type_str(&mut eng, "wqvb");
    let idx = idx_of(&eng, "你好").expect("候选应有 你好");
    let fx = eng.cand_op(idx, CandOp::Delete);
    assert!(fx.iter().any(|e| matches!(e, Effect::Candidates(_))));
    assert!(idx_of(&eng, "你好").is_none(), "删除后候选应无 你好");

    let blocked = std::fs::read_to_string(td.join("blocked.tsv")).unwrap();
    assert!(blocked.lines().any(|l| l == "你好"), "blocked.tsv: {blocked}");

    // 重启引擎(同数据目录):屏蔽仍生效,且跨编码生效(拼音 nihao 同样查不到)
    let mut eng2 = eng_at(&td);
    type_str(&mut eng2, "wqvb");
    assert!(idx_of(&eng2, "你好").is_none(), "重启后 你好 应仍被屏蔽");
    let mut eng3 = eng_at(&td);
    type_str(&mut eng3, "nihao");
    assert!(idx_of(&eng3, "你好").is_none(), "拼音通道同样屏蔽 你好");
}

#[test]
fn 删除用户造词_同时移出造词库() {
    let td = TempDir::new();
    let mut eng = eng_at(&td);
    // 先造词:你 好 → Ctrl+= → Enter 存「你好」(wqvb)
    for code in ["wq", "vbg"] {
        type_str(&mut eng, code);
        let _ = eng.process_key(LKey::Space);
    }
    let _ = eng.process_key(LKey::Coin);
    let _ = eng.process_key(LKey::Enter);
    assert!(td.join("user_words.tsv").is_file(), "造词应落盘");
    // 删除该词:候选消失 + user_words.tsv 中剔除 + blocked.tsv 收录
    type_str(&mut eng, "wqvb");
    let idx = idx_of(&eng, "你好").expect("造词后候选应有 你好");
    let _ = eng.cand_op(idx, CandOp::Delete);
    let uw = std::fs::read_to_string(td.join("user_words.tsv")).unwrap();
    assert!(!uw.lines().any(|l| l.split('\t').next() == Some("你好")),
            "user_words 应剔除 你好: {uw}");
    let blocked = std::fs::read_to_string(td.join("blocked.tsv")).unwrap();
    assert!(blocked.lines().any(|l| l == "你好"));
}

#[test]
fn 删除后再造词自动解除屏蔽() {
    let td = TempDir::new();
    let mut eng = eng_at(&td);
    type_str(&mut eng, "wqvb");
    let idx = idx_of(&eng, "你好").expect("候选应有 你好");
    let _ = eng.cand_op(idx, CandOp::Delete);
    assert!(idx_of(&eng, "你好").is_none());

    // 删除保留组合缓冲(用户可继续选词);再造前先 Esc 清缓冲,
    // 重新上屏 你/好 再造同一个词 → 解除屏蔽 + 词回来
    let _ = eng.process_key(LKey::Esc);
    for code in ["wq", "vbg"] {
        type_str(&mut eng, code);
        let _ = eng.process_key(LKey::Space);
    }
    let _ = eng.process_key(LKey::Coin);
    let fx = eng.process_key(LKey::Enter);
    assert!(notices(&fx).iter().any(|n| n.contains("已造词")), "{fx:?}");
    let blocked = std::fs::read_to_string(td.join("blocked.tsv")).unwrap();
    assert!(!blocked.lines().any(|l| l == "你好"), "再造词应解屏蔽: {blocked}");
    type_str(&mut eng, "wqvb");
    assert!(idx_of(&eng, "你好").is_some(), "再造词后 你好 应重新出现");
}

#[test]
fn 反查英文_候选页替换且可选词上屏() {
    let td = TempDir::new();
    let mut eng = eng_at(&td);
    type_str(&mut eng, "wqvb");
    let idx = idx_of(&eng, "你好").expect("候选应有 你好");
    let fx = eng.cand_op(idx, CandOp::EnLookup);
    assert!(fx.iter().any(|e| matches!(e, Effect::Candidates(_))));
    // 候选页 = zh_en 反查结果;注释带来源标记
    let texts = page_texts(&eng);
    assert_eq!(texts, vec!["hello".to_string(), "hi".to_string()]);
    assert!(eng.flush_page()[0].comment.contains("你好"), "注释应标来源词");
    // 点选反查结果 → 上屏英文词
    let fx2 = eng.select_candidate(0);
    assert!(has_commit(&fx2, "hello"));
    assert_eq!(eng.buffer(), "");
}

#[test]
fn 反查英文_非中文词与无结果给提示() {
    let td = TempDir::new();
    let mut eng = eng_at(&td);
    // 英文候选(无中文命中时的英文通道):反查给"不是中文词"提示
    type_str(&mut eng, "the");
    let idx = idx_of(&eng, "the").expect("英文候选应有 the");
    let fx = eng.cand_op(idx, CandOp::EnLookup);
    assert!(notices(&fx).iter().any(|n| n.contains("不是中文词")), "{fx:?}");

    // 中文词但 zh_en 未覆盖(夹具无「你」):给"没有结果"提示
    let mut eng2 = eng_at(&td);
    type_str(&mut eng2, "wq");
    let idx2 = idx_of(&eng2, "你").expect("候选应有 你");
    let fx2 = eng2.cand_op(idx2, CandOp::EnLookup);
    assert!(notices(&fx2).iter().any(|n| n.contains("没有英文反查结果")), "{fx2:?}");
}

#[test]
fn 功能键候选_右键操作拒绝且菜单禁用() {
    let td = TempDir::new();
    let mut eng = eng_at(&td);
    type_str(&mut eng, "peizhi"); // 默认快速功能键触发词(§14 配置)
    let idx = eng
        .flush_page()
        .iter()
        .position(|c| matches!(c.kind, lyyime_core::CandKind::Action(_)));
    let Some(ai) = idx else {
        panic!("peizhi 应触发功能键候选");
    };
    assert_eq!(eng.cand_pinned(ai), None, "功能键行应禁用菜单");
    let fx = eng.cand_op(ai, CandOp::Delete);
    assert!(notices(&fx).iter().any(|n| n.contains("不支持")), "{fx:?}");
}

// ----------------------------------------------------------------------
// FFI 层冒烟:效果流 JSON 形态 + 固定状态查询 + -needed 两段式纪律
// ----------------------------------------------------------------------

#[test]
fn ffi_右键操作_固定删除反查全链路() {
    isolated_home();
    let dir = CString::new(fixtures().to_str().unwrap()).unwrap();
    let eng = unsafe { lyyime_new(dir.as_ptr()) };
    assert!(!eng.is_null());
    let mut buf = [0i8; 4096];
    // nihao(5 键拼音,避开四码自动上屏)→ 候选含 你好
    for c in b"nihao".iter() {
        let n = unsafe {
            lyyime_process_key(eng, LKEY_CHAR, *c as u32, buf.as_mut_ptr(), buf.len() as i64)
        };
        assert!(n > 0);
    }
    // 找 你好 下标(cand FFI)
    let mut nihao_idx = -1;
    for i in 0..9 {
        let mut tb = [0i8; 128];
        let n = unsafe { lyyime_cand(eng, i, tb.as_mut_ptr() as *mut c_char, 128) };
        if n <= 1 {
            continue;
        }
        let s = unsafe { std::ffi::CStr::from_ptr(tb.as_ptr()) }.to_str().unwrap();
        if s == "你好" {
            nihao_idx = i;
            break;
        }
    }
    assert!(nihao_idx >= 0, "nihao 候选应有 你好");
    assert_eq!(unsafe { lyyime_cand_pinned(eng, nihao_idx) }, 0);
    assert_eq!(unsafe { lyyime_cand_pinned(eng, 99) }, -1, "越界返回 -1");

    // op=1 固定首位 → 效果流含 cands, 你好 置顶, pinned=1
    let n = unsafe { lyyime_cand_op(eng, nihao_idx, 1, buf.as_mut_ptr(), buf.len() as i64) };
    assert!(n > 0);
    let json = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap().to_string();
    assert!(json.contains("\"t\":\"cands\""), "{json}");
    assert_eq!(unsafe { lyyime_cand_pinned(eng, 0) }, 1);
    let mut tb = [0i8; 128];
    unsafe { lyyime_cand(eng, 0, tb.as_mut_ptr() as *mut c_char, 128) };
    let first = unsafe { std::ffi::CStr::from_ptr(tb.as_ptr()) }.to_str().unwrap().to_string();
    assert_eq!(first, "你好");

    // op=3 反查英文 → 候选页替换为 hello/hi
    let n = unsafe { lyyime_cand_op(eng, 0, 3, buf.as_mut_ptr(), buf.len() as i64) };
    assert!(n > 0);
    let json = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap().to_string();
    assert!(json.contains("\"t\":\"cands\""), "{json}");
    let mut tb = [0i8; 128];
    unsafe { lyyime_cand(eng, 0, tb.as_mut_ptr() as *mut c_char, 128) };
    let first = unsafe { std::ffi::CStr::from_ptr(tb.as_ptr()) }.to_str().unwrap().to_string();
    assert_eq!(first, "hello");

    // 非法 op → consumed;非法下标 → consumed
    let n = unsafe { lyyime_cand_op(eng, 0, 77, buf.as_mut_ptr(), buf.len() as i64) };
    let json = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap().to_string();
    assert!(json.contains("\"t\":\"consumed\""), "{json}");

    // 容量不足 → -needed,引擎状态不变(仍可再试)
    let n = unsafe { lyyime_cand_op(eng, 0, 1, std::ptr::null_mut(), 0) };
    assert!(n < 0, "容量探测应返回 -needed,实得 {n}");

    unsafe { lyyime_free(eng) };
}
