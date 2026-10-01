//! 上屏后联想(本地静态联想)集成测试:
//! 中文上屏后候选条出现"可续接尾巴"(注释列留空,不占视觉宽度),
//! 空格/数字/点选续上屏,字母撤联想行并开始新组合,Esc/边界键取消联想且不产生陈旧上屏。
//!
//! 夹具词库(独立临时目录,不碰 tests/fixtures 共享数据):
//!   wubi:  a=你 b=好 c=中国 d=人 f/ff=子 g/gg=民 z=中国人民银行
//!          (ff/gg 供造词取 ≥2 码用,单码 f/g 保持空格上屏夹具语义)
//!   phrase: 你好1000 你们900 你好啊800 好啊700 好东西600 你弟800
//!           中国人民1000 中国人民银行900
//!   suggestion: 你好99999(跨源取最大频次) 好人4000 你弟800(与词组同频=平局)
//!
//! 期望联想(频次降序,同频尾巴文本升序):
//!   你   → [好(99999) 们(900) 好啊(800) 弟(800)]  无重复"你"(单字不入索引)
//!   好   → [人(4000) 啊(700) 东西(600)]
//!   你好 → [啊(800·长上下文) 人(4000) 东西(600)]  "啊"在"好啊"档位去重一次
//!   中国 → [人民(1000) 人民银行(900)]
//!   中国人民 → [银行(900)]  中国人民银行 → 无(完整最长词,无尾巴)

mod common;

use common::*;
use lyyime_core::{CandOp, Config, Effect, Engine, LKey};

/// 写测试词库并返回目录(每次调用一个独立 TempDir)。
fn pred_dir() -> TempDir {
    let td = TempDir::new();
    std::fs::write(
        td.join("wubi.tsv"),
        "a\t你\t100\nb\t好\t100\nc\t中国\t100\nd\t人\t100\nf\t子\t100\nff\t子\t100\ng\t民\t100\ngg\t民\t100\nz\t中国人民银行\t100\n",
    )
    .unwrap();
    std::fs::write(
        td.join("pinyin_phrase.tsv"),
        "你好\tni hao\t1000\n你们\tni men\t900\n你好啊\tni hao a\t800\n好啊\thao a\t700\n好东西\thao dong xi\t600\n你弟\tni di\t800\n中国人民\tzhong guo ren min\t1000\n中国人民银行\tzhong guo ren min yin hang\t900\n",
    )
    .unwrap();
    std::fs::write(td.join("suggestion.tsv"), "你好\t99999\n好人\t4000\n你弟\t800\n").unwrap();
    td
}

/// 联想引擎:用户数据文件落在词库夹具目录(dir 由调用方持有存活),
/// 不另起 TempDir——返回后目录若在调用前就 drop,引擎持有的路径成死链。
fn pred_engine(dir: &TempDir) -> Engine {
    engine_with_fixtures(
        &dir.path,
        Config {
            user_dict: Some(dir.join("user.tsv")),
            next_word_prediction: true,
            ..Config::default()
        },
    )
}

/// 联想引擎 + 预置屏蔽表(须在建引擎前落盘,set_config 才装载)。
fn pred_engine_blocked(dir: &TempDir, blocked: &[&str]) -> Engine {
    let mut text = String::new();
    for w in blocked {
        text.push_str(w);
        text.push('\n');
    }
    std::fs::write(dir.join("blocked.tsv"), text).unwrap();
    engine_with_fixtures(
        &dir.path,
        Config {
            user_dict: Some(dir.join("user.tsv")),
            next_word_prediction: true,
            ..Config::default()
        },
    )
}

/// 关闭联想的同夹具引擎(对照组):不显式置开关,直接吃 Config 默认(关)。
fn pred_engine_off(dir: &TempDir) -> Engine {
    engine_with_fixtures(
        &dir.path,
        Config {
            user_dict: Some(dir.join("user.tsv")),
            ..Config::default()
        },
    )
}

/// 敲五笔码并空格顶屏,返回最后一次按键的效果流。
fn type_commit(eng: &mut Engine, code: &str) -> Vec<Effect> {
    type_str(eng, code);
    eng.process_key(LKey::Space)
}

/// 联想页文本(提交后候选条即联想行)。
fn pred_texts(eng: &Engine) -> Vec<String> {
    page_texts(eng)
}

fn has_pred_cands(effects: &[Effect]) -> bool {
    effects.iter().any(|e| match e {
        Effect::Candidates(list) => !list.is_empty(),
        _ => false,
    })
}

fn last_cands_empty(effects: &[Effect]) -> bool {
    match effects.iter().rev().find(|e| matches!(e, Effect::Candidates(_))) {
        Some(Effect::Candidates(list)) => list.is_empty(),
        _ => false,
    }
}

// ======================================================================
// 基本行为:显式开启、联想行内容、三种选择方式
// ======================================================================

#[test]
fn 显式开启_上屏你后出现联想行() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    let fx = type_commit(&mut eng, "a"); // a→你
    assert!(has_commit(&fx, "你"));
    assert!(has_pred_cands(&fx), "上屏后应有联想候选: {fx:?}");
    assert_eq!(
        pred_texts(&eng),
        vec!["好", "们", "好啊", "弟"],
        "联想尾巴应按频次降序+文本升序"
    );
    assert!(
        eng.flush_page().iter().all(|c| c.comment.is_empty()),
        "联想行不再带注释(联想标记已移除)"
    );
}

#[test]
fn 默认关闭_上屏后候选条为空() {
    let dir = pred_dir();
    let mut eng = pred_engine_off(&dir);
    let fx = type_commit(&mut eng, "a");
    assert!(has_commit(&fx, "你"));
    assert!(!has_pred_cands(&fx), "关闭联想不应出现候选: {fx:?}");
    assert!(pred_texts(&eng).is_empty());
    // 空格照常放行(空缓冲无候选)。
    assert!(is_pass(&eng.process_key(LKey::Space)));
}

#[test]
fn 空格选中联想首选_只上屏尾巴() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a"); // 你
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["好".to_string()], "空格应只上屏尾巴");
    // 续接:上下文"你好",长上下文先出"啊" → [啊 人 东西]
    assert_eq!(pred_texts(&eng), vec!["啊", "人", "东西"]);
}

#[test]
fn 数字选中联想_越界数字直通不吞() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a"); // 你 → [好 们 好啊 弟]
    let fx = eng.process_key(LKey::Digit(2));
    assert_eq!(commits(&fx), vec!["们".to_string()], "数字 2 应上屏第二尾巴");
    // 续接:们 无下文 → 联想结束,空格放行
    assert!(is_pass(&eng.process_key(LKey::Space)));

    // 越界数字:联想行撤下、按键放行(数字交给应用)。
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    let fx = eng.process_key(LKey::Digit(9));
    assert!(
        matches!(fx.last(), Some(Effect::Pass)),
        "越界数字应直通: {fx:?}"
    );
    assert!(last_cands_empty(&fx), "越界后联想行应撤下");
    assert!(pred_texts(&eng).is_empty());
    // 上下文亦作废:再上屏 好 → 按"好"而非"你N好"继续联想
    type_commit(&mut eng, "b");
    assert_eq!(pred_texts(&eng), vec!["人", "啊", "东西"]);
}

#[test]
fn 点选联想_鼠标路径同样只上屏尾巴() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    let fx = eng.select_candidate(1); // 们
    assert_eq!(commits(&fx), vec!["们".to_string()]);
    // 点选越界行:吞键不产生上屏
    let fx = eng.select_candidate(9);
    assert!(is_consumed(&fx) || !has_pred_cands(&fx));
}

#[test]
fn 联想续接_中国到人民银行到银行() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    let fx = type_commit(&mut eng, "c"); // 中国
    assert!(has_commit(&fx, "中国"));
    assert_eq!(pred_texts(&eng), vec!["人民", "人民银行"]);
    // 空格选"人民" → 续出"银行"
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["人民".to_string()]);
    assert_eq!(pred_texts(&eng), vec!["银行"]);
    // 数字 1 选"银行" → 中国人民银行已到完整最长词,无新联想
    let fx = eng.process_key(LKey::Digit(1));
    assert_eq!(commits(&fx), vec!["银行".to_string()]);
    assert!(pred_texts(&eng).is_empty(), "完整最长词后不应有联想");
}

#[test]
fn 完整最长词上屏后无联想() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    let fx = type_commit(&mut eng, "z"); // z→中国人民银行
    assert!(has_commit(&fx, "中国人民银行"));
    assert!(pred_texts(&eng).is_empty());
    // 空格照常放行
    assert!(is_pass(&eng.process_key(LKey::Space)));
}

#[test]
fn 最长上下文优先_上下文逐字累积() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a"); // 你
    type_commit(&mut eng, "b"); // 好 → 上下文"你好"
    // len2"你好"先命中 啊;len1"好"的"啊"被去重,剩 人/东西
    assert_eq!(pred_texts(&eng), vec!["啊", "人", "东西"]);
}

#[test]
fn 屏蔽词与屏蔽尾巴都过滤() {
    let dir = pred_dir();
    let mut eng = pred_engine_blocked(&dir, &["你们", "啊"]);
    type_commit(&mut eng, "a"); // 你
    // 你们整词被屏蔽 → 们 不出现;"啊"是屏蔽尾巴但 你→好啊 不含此尾巴,保留
    assert_eq!(pred_texts(&eng), vec!["好", "好啊", "弟"]);
    // 上下文"你好":len2 尾巴"啊"被屏蔽 → 落到 len1 的 人/东西
    type_commit(&mut eng, "b");
    assert_eq!(pred_texts(&eng), vec!["人", "东西"]);
}

// ======================================================================
// 边界与取消:Esc / 字母 / 标点 / 控制键 / 复位 / 开关
// ======================================================================

#[test]
fn esc_取消联想_吞一次键后正常直通() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    let fx = eng.process_key(LKey::Esc);
    assert!(
        matches!(fx.last(), Some(Effect::Consumed)),
        "Esc 应吞键撤联想: {fx:?}"
    );
    assert!(last_cands_empty(&fx));
    assert!(pred_texts(&eng).is_empty());
    // 上下文作废:Esc 后再空格放行,不会偷偷上屏旧尾巴
    assert!(is_pass(&eng.process_key(LKey::Space)));
    // 数字也不再是联想选择(直通)
    let fx = eng.process_key(LKey::Digit(1));
    assert!(is_pass(&fx));
}

#[test]
fn 字母撤联想行_直接开始新组合() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a"); // 你 → 联想行
    let fx = eng.process_key(LKey::Char('b'));
    assert_eq!(eng.buffer(), "b", "字母应进入新组合缓冲");
    assert!(commits(&fx).is_empty(), "字母不得上屏任何联想尾巴");
    // 上下文保留:上屏 好 后按"你好"续联想,联想首为"啊"(长上下文优先)
    let fx = eng.process_key(LKey::Space);
    assert!(has_commit(&fx, "好"));
    assert_eq!(pred_texts(&eng), vec!["啊", "人", "东西"]);
}

#[test]
fn 字母可接无命中键_联想仍被撤下() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    // 'x' 无任何命中:进缓冲(死码保护只对有中文命中的组合生效,
    // 联想态无缓冲),缓冲为 x、无候选
    let fx = eng.process_key(LKey::Char('x'));
    assert!(commits(&fx).is_empty());
    assert!(pred_texts(&eng).is_empty() || eng.buffer() == "x");
    assert_eq!(eng.buffer(), "x");
}

#[test]
fn 标点是硬边界_联想行撤下且不留尾巴() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    let fx = eng.process_key(LKey::Punct(','));
    // 中文逗号上屏;联想候选必须在该帧被撤下(最后一条 Candidates 为空)
    assert!(has_commit(&fx, "，"), "应上屏中文逗号: {fx:?}");
    assert!(last_cands_empty(&fx), "标点后不得残留联想行: {fx:?}");
    assert!(pred_texts(&eng).is_empty());
    // 上下文作废:再空格放行;上屏"中国"后只按"中国"联想而非"你中国"
    assert!(is_pass(&eng.process_key(LKey::Space)));
    // 上屏"好":上下文已随标点作废 → 按"好"联想 [人 啊 东西];
    // 若上下文残留成"你好"则首联想会是"啊"——区分得出。
    type_commit(&mut eng, "b");
    assert_eq!(pred_texts(&eng), vec!["人", "啊", "东西"]);
}

#[test]
fn 硬边界清单_回车退格方向键_other_shift_均清联想() {
    for key in [
        LKey::Enter,
        LKey::Backspace,
        LKey::ArrowLeft,
        LKey::ArrowRight,
        LKey::ArrowUp,
        LKey::ArrowDown,
        LKey::Other,
        LKey::ShiftPress,
    ] {
        let dir = pred_dir();
        let mut eng = pred_engine(&dir);
        type_commit(&mut eng, "a");
        let fx = eng.process_key(key);
        assert!(
            pred_texts(&eng).is_empty(),
            "{key:?} 后联想应清空: {fx:?}"
        );
        // 空格/数字不再上屏尾巴
        let fx = eng.process_key(LKey::Space);
        assert!(
            !has_commit(&fx, "好"),
            "{key:?} 边界后空格不得上屏旧联想: {fx:?}"
        );
    }
}

#[test]
fn reset_与模式切换是硬边界() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    eng.reset();
    assert!(pred_texts(&eng).is_empty());
    assert!(is_pass(&eng.process_key(LKey::Space)));

    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    eng.toggle_mode(); // 切英文:上下文作废
    assert_eq!(eng.mode(), lyyime_core::Mode::English);
    eng.toggle_mode(); // 切回中文
    let fx = eng.process_key(LKey::Space);
    assert!(is_pass(&fx), "模式切换后空格不得上屏旧联想");
}

#[test]
fn 热关闭联想_已展示的联想行立即撤下() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    assert_eq!(pred_texts(&eng).len(), 4);
    let mut cfg = eng.config().clone();
    cfg.next_word_prediction = false;
    eng.set_config(cfg);
    assert!(pred_texts(&eng).is_empty(), "关闭后联想行应立即撤下");
    assert!(is_pass(&eng.process_key(LKey::Space)));
    // 再上屏也不出联想
    type_commit(&mut eng, "a");
    assert!(pred_texts(&eng).is_empty());
}

// ======================================================================
// 联想页翻页 / 组合无关路径不串扰
// ======================================================================

#[test]
fn 联想页可翻页_页大小1() {
    let dir = pred_dir();
    let ud = TempDir::new();
    let mut eng = engine_with_fixtures(
        &dir.path,
        Config {
            user_dict: Some(ud.join("user.tsv")),
            page_size: 1,
            next_word_prediction: true,
            ..Config::default()
        },
    );
    type_commit(&mut eng, "a");
    assert_eq!(pred_texts(&eng), vec!["好"]);
    // 翻到第 2 页:们;数字 1 选当前页首选 = 们
    let fx = eng.process_key(LKey::PageDown);
    assert!(!is_pass(&fx), "有下一页应消费");
    assert_eq!(pred_texts(&eng), vec!["们"]);
    let fx = eng.process_key(LKey::Digit(1));
    assert_eq!(commits(&fx), vec!["们".to_string()]);
}

#[test]
fn 联想行不支持固定与右键操作() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    assert_eq!(eng.cand_pinned(0), None, "联想行无码可固定");
    let fx = eng.cand_op(0, CandOp::Delete);
    assert!(is_consumed(&fx), "联想行右键操作应吞键不动作");
    assert_eq!(pred_texts(&eng).len(), 4, "联想行应原样保留");
}

#[test]
fn 联想尾巴不写入学习记录() {
    let dir = pred_dir();
    let ud = TempDir::new();
    let user_tsv = ud.join("user.tsv");
    let mut eng = engine_with_fixtures(
        &dir.path,
        Config {
            user_dict: Some(user_tsv.clone()),
            next_word_prediction: true,
            ..Config::default()
        },
    );
    type_commit(&mut eng, "a"); // 上屏 你(学习)
    eng.process_key(LKey::Space); // 空格上屏联想尾巴 好(不得学习半截尾巴)
    eng.flush_user_dict().unwrap();
    let content = std::fs::read_to_string(&user_tsv).unwrap_or_default();
    assert!(content.lines().any(|l| l.starts_with("你\t")), "正常上屏应学习: {content}");
    assert!(
        !content.lines().any(|l| l.starts_with("好\t")),
        "联想尾巴不得写学习记录: {content}"
    );
}

#[test]
fn 造词键在联想态正常进入_联想行撤下() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a"); // 你 → 联想行
    type_commit(&mut eng, "b"); // 好(字母路径:联想已撤、新组合上屏)
    let fx = eng.process_key(LKey::Coin);
    assert_eq!(last_preedit(&fx).as_deref(), Some("造词:你好"));
    // 造词态里没有联想行(候选条是造词预览)
    assert!(
        !pred_texts(&eng).iter().any(|t| t == "人"),
        "造词态不得残留联想行"
    );
}

#[test]
fn 新造词并入联想索引_即时生效() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    // 词库里没有"子民":先上屏 子、民 各一次建立造词历史
    type_commit(&mut eng, "f"); // 子
    assert!(pred_texts(&eng).is_empty(), "子 暂无可联想");
    type_commit(&mut eng, "g"); // 民
    // 造词自动带上前字,初始即选中"子民",Enter 存词
    let fx = eng.process_key(LKey::Coin);
    assert_eq!(last_preedit(&fx).as_deref(), Some("造词:子民"));
    let fx = eng.process_key(LKey::Enter);
    assert!(
        notices(&fx).iter().any(|n| n.contains("已造词:子民")),
        "造词应成功: {fx:?}"
    );
    // 再上屏"子":新词入联想索引 → 联想"民"
    let fx = type_commit(&mut eng, "f");
    assert!(has_commit(&fx, "子"));
    assert_eq!(pred_texts(&eng), vec!["民"], "新造词应即时进入联想索引");
}

#[test]
fn 英文上屏与空缓冲不受影响() {
    let dir = pred_dir();
    // 英文直通(回车上屏原字母):不产生联想、不建立中文上下文。
    // 用无命中字母组合(词库无 x 起头的码/音节)。
    let mut eng = pred_engine(&dir);
    type_str(&mut eng, "xq");
    let fx = eng.process_key(LKey::Enter);
    assert!(has_commit(&fx, "xq"));
    assert!(pred_texts(&eng).is_empty());
    // 空缓冲按键照常
    assert!(is_pass(&eng.process_key(LKey::Space)));
    assert!(is_pass(&eng.process_key(LKey::Digit(1))));
}

#[test]
fn 联想单页时翻页键吞键_联想行保持() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a"); // 4 条联想,默认页大小 10 → 只有一页
    for key in [LKey::PageDown, LKey::PageUp] {
        let fx = eng.process_key(key);
        assert!(
            is_consumed(&fx),
            "单页联想 {key:?} 应吞键保持原页(不得直通进应用): {fx:?}"
        );
        assert!(commits(&fx).is_empty(), "{key:?} 不得上屏: {fx:?}");
        assert_eq!(
            pred_texts(&eng),
            vec!["好", "们", "好啊", "弟"],
            "{key:?} 后联想行应保持"
        );
    }
}

#[test]
fn set_config换用户目录_联想行随索引重建撤下() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a");
    assert_eq!(pred_texts(&eng).len(), 4);
    // 换到另一个存活目录:set_config 重装载造词库 → 联想索引重建,
    // 正在展示的联想行与旧上下文一并撤下,不留旧尾巴供误选。
    let ud2 = TempDir::new();
    let mut cfg = eng.config().clone();
    cfg.user_dict = Some(ud2.join("user.tsv"));
    eng.set_config(cfg);
    assert!(eng.flush_page().is_empty(), "换目录后旧联想行必须撤下");
    let fx = eng.process_key(LKey::Digit(1));
    assert!(is_pass(&fx), "旧联想行撤下后数字应直通: {fx:?}");
    assert!(commits(&fx).is_empty(), "不得上屏旧联想尾巴: {fx:?}");
}

#[test]
fn 联想选中后再输入数字_新联想正常替换() {
    let dir = pred_dir();
    let mut eng = pred_engine(&dir);
    type_commit(&mut eng, "a"); // 你 → [好 们 好啊 弟]
    eng.process_key(LKey::Space); // 好 → 上下文"你好":[啊 人 东西]
    let fx = eng.process_key(LKey::Digit(1)); // 啊(长上下文档首位)
    assert_eq!(commits(&fx), vec!["啊".to_string()]);
    assert!(pred_texts(&eng).is_empty(), "啊 无下文联想");
}
