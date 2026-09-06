//! 引擎行为集成测试(合同 M1 硬指标):
//! 五笔全码/简码/前缀渐进/词组、拼音全拼/切分歧义/末音节不完整/简拼、
//! 中英混合、标点、数字选词、翻页、Enter/Esc/Backspace、学习排序、配置加载。

mod common;

use common::*;
use lyyime_core::{CandKind, Config, Effect, Engine, LKey, Mode};
use std::path::Path;

// ======================================================================
// 数据加载与降级
// ======================================================================

#[test]
fn 完整_fixtures_加载成功() {
    let eng = engine();
    assert!(eng.is_loaded());
}

#[test]
fn 数据目录不存在_空引擎仍可用() {
    let mut eng = Engine::new(Path::new("/nonexistent/lyyime-empty-dir")).unwrap();
    assert!(!eng.is_loaded());
    // 按键状态机照常:进缓冲 → 无候选 → 空格直通原字母。
    type_str(&mut eng, "abc");
    assert!(!eng.is_loaded());
    assert!(page_texts(&eng).is_empty());
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["abc".to_string()]);
}

#[test]
fn 单文件缺失_只降级英文通道() {
    let td = TempDir::new();
    for f in [
        "wubi.tsv",
        "pinyin_char.tsv",
        "pinyin_phrase.tsv",
        "suggestion.tsv",
    ] {
        std::fs::copy(fixtures().join(f), td.join(f)).unwrap();
    }
    let mut eng = Engine::new(&td.path).unwrap();
    assert!(eng.is_loaded(), "其余通道仍在");
    type_str(&mut eng, "hello");
    assert!(page_texts(&eng).is_empty(), "英文通道应被降级");
    let mut eng2 = Engine::new(&td.path).unwrap();
    type_str(&mut eng2, "a");
    assert_eq!(page_texts(&eng2).first().map(String::as_str), Some("工"));
}

#[test]
fn meta_json_缺失不影响加载() {
    let td = TempDir::new();
    std::fs::copy(fixtures().join("wubi.tsv"), td.join("wubi.tsv")).unwrap();
    let eng = Engine::new(&td.path).unwrap();
    assert!(eng.is_loaded());
}

// ======================================================================
// 五笔通道
// ======================================================================

#[test]
fn 五笔全码_单字_工() {
    let mut eng = engine();
    type_str(&mut eng, "a");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("工"));
    assert_eq!(eng.flush_page()[0].kind, CandKind::Wubi);
    assert_eq!(eng.flush_page()[0].comment, "a", "五笔候选注释应带编码");
}

#[test]
fn 五笔精确_优先于前缀() {
    let mut eng = engine();
    type_str(&mut eng, "aa");
    let page = page_texts(&eng);
    assert_eq!(
        page.first().map(String::as_str),
        Some("式"),
        "精确码 aa 应排第一"
    );
    assert!(
        page.contains(&"恭恭敬敬".to_string()),
        "aaaa 是 aa 的前缀扩展,应出现"
    );
}

#[test]
fn 四码后续字母_开启后先上屏当前选中() {
    let mut eng = engine_with(Config {
        commit_on_extra_after_four: true,
        // 本测单独验证“四码顶屏”:关掉四码唯一上屏,保证缓冲能停在四码。
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "aaaa");
    let top = page_texts(&eng).first().cloned().unwrap();
    let fx = eng.process_key(LKey::Char('g'));
    let commits_fx = commits(&fx);
    assert_eq!(commits_fx.first().map(String::as_str), Some(top.as_str()));
    assert_eq!(eng.buffer(), "g");
    assert_eq!(page_texts(&eng), vec!["一".to_string()]);
}

#[test]
fn 四码后续字母_默认继续前缀组词() {
    let mut eng = engine();
    type_str(&mut eng, "wgli");
    let fx = eng.process_key(LKey::Char('g'));
    assert!(commits(&fx).is_empty());
    assert_eq!(eng.buffer(), "wglig");
}

// ----------------------------------------------------------------------
// 四码唯一上屏(默认开启):恰好四码且候选唯一 → 免空格直接上屏
// ----------------------------------------------------------------------

#[test]
fn 四码唯一_免空格直接上屏() {
    let mut eng = engine();
    type_str(&mut eng, "wqv");
    assert_eq!(eng.buffer(), "wqv", "三码前保持组合");
    // 第 4 键 wqvb → 唯一候选「你好」自动上屏。
    let fx = eng.process_key(LKey::Char('b'));
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
    assert!(eng.buffer().is_empty());
    assert!(eng.flush_page().is_empty(), "上屏后候选一并清空");
    // 自动上屏后缓冲已空:空格放行原样输出,不再顶屏。
    assert!(is_pass(&eng.process_key(LKey::Space)));
}

#[test]
fn 四码唯一_关闭后保持渐进组合() {
    let mut eng = engine_with(Config {
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "wqv");
    let fx = eng.process_key(LKey::Char('b'));
    assert!(commits(&fx).is_empty(), "关闭后第 4 键不上屏");
    assert_eq!(eng.buffer(), "wqvb");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("你好"));
}

#[test]
fn 四码多候选_不自动上屏() {
    let mut eng = engine();
    // xian 同时命中拼音多候选(先/西安/西…),虽满四码但不唯一,不触发。
    let fx = type_str(&mut eng, "xian");
    assert!(commits(&fx).is_empty(), "多候选四码不得自动上屏");
    assert_eq!(eng.buffer(), "xian");
    assert!(eng.flush_page().len() > 1);
}

// ----------------------------------------------------------------------
// 词组效率提示(默认开启):上屏后最近几字有更省键的五笔词组 → Effect::Hint
// ----------------------------------------------------------------------

#[test]
fn 词组提示_逐字上屏后提示更省词组() {
    let mut eng = engine();
    // 逐字打「你好」:wqiy+空格 → 你,再 vbg+空格 → 好(共 7 个字母)。
    type_str(&mut eng, "wqiy");
    let fx1 = eng.process_key(LKey::Space);
    assert!(hints(&fx1).is_empty(), "只上屏一个字时无词组可提示");
    let fx2 = eng.process_key(LKey::Char('v'));
    assert!(hints(&fx2).is_empty());
    eng.process_key(LKey::Char('b'));
    eng.process_key(LKey::Char('g'));
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["好".to_string()]);
    let hs = hints(&fx);
    assert_eq!(hs.len(), 1, "恰一条提示:{hs:?}");
    assert!(hs[0].contains("你好") && hs[0].contains("wqvb"), "提示={}", hs[0]);
    // 提示位于效果流末尾(宿主先清组合显示再展示提示)。
    assert!(matches!(fx.last(), Some(Effect::Hint(_))), "顺序={fx:?}");
}

#[test]
fn 词组提示_同码词组打过不再提示() {
    let mut eng = engine();
    // wqvb 四码唯一自动上屏「你好」:实耗 4 键 = 词组编码 4 键,不更省 → 无提示。
    let fx = type_str(&mut eng, "wqvb");
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
    assert!(hints(&fx).is_empty(), "同码打过不应提示:{:?}", hints(&fx));
    // 空格确认路径同样不提示。
    let mut eng2 = engine_with(Config {
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng2, "wqvb");
    let fx2 = eng2.process_key(LKey::Space);
    assert_eq!(commits(&fx2), vec!["你好".to_string()]);
    assert!(hints(&fx2).is_empty());
}

#[test]
fn 词组提示_拼音打词组提示五笔编码() {
    let mut eng = engine();
    // nihao+空格 顶屏词组「你好」(5 个字母 > wqvb 4 键)→ 提示五笔编码。
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
    let hs = hints(&fx);
    assert!(hs.iter().any(|h| h.contains("你好") && h.contains("wqvb")), "提示={hs:?}");
}

#[test]
fn 词组提示_下一次输入产生新效果流无残留提示() {
    let mut eng = engine();
    type_str(&mut eng, "wqiy");
    eng.process_key(LKey::Space);
    type_str(&mut eng, "vbg");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(hints(&fx).len(), 1);
    // 后续任意输入:core 只出常规效果流,宿主据此替换/清除提示。
    let fx_next = eng.process_key(LKey::Char('a'));
    assert!(hints(&fx_next).is_empty());
    assert!(matches!(fx_next.first(), Some(Effect::Preedit(Some(_)))));
    let fx_esc = eng.process_key(LKey::Esc);
    assert!(hints(&fx_esc).is_empty());
}

#[test]
fn 词组提示_关闭后无提示() {
    let mut eng = engine_with(Config {
        phrase_hint: false,
        ..Config::default()
    });
    type_str(&mut eng, "wqiy");
    eng.process_key(LKey::Space);
    type_str(&mut eng, "vbg");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["好".to_string()]);
    assert!(hints(&fx).is_empty());
}

#[test]
fn 词组提示_字母直通不提示() {
    let mut eng = engine();
    // 原始字母上屏不含汉字:不进造词/提示历史。
    type_str(&mut eng, "abc");
    let fx = eng.process_key(LKey::Enter);
    assert_eq!(commits(&fx), vec!["abc".to_string()]);
    assert!(hints(&fx).is_empty());
}

#[test]
fn 五笔精确层_简码满分归一() {
    let mut eng = engine();
    let fx = type_str(&mut eng, "a");
    let top = &eng.flush_page()[0];
    assert_eq!(top.text, "工");
    // 层级词典序:工 处于五笔精确层(60),通道内最高频 → 归一满值 10。
    assert!(
        (top.score - 70.0).abs() < 1e-4,
        "精确层得分实为 {}",
        top.score
    );
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::Preedit(Some(s)) if s == "a")));
}

#[test]
fn 五笔前缀渐进_词组出现() {
    let mut eng = engine();
    type_str(&mut eng, "aaa");
    let page = page_texts(&eng);
    assert!(
        page.contains(&"恭恭敬敬".to_string()),
        "打 aaa 应渐进看到 aaaa 的词"
    );
    assert!(
        !page.contains(&"式".to_string()),
        "式 的码 aa 不是 aaa 的前缀"
    );
}

#[test]
fn 五笔四码词组_完全命中第一() {
    // 本测验证四码词组排序本身:关掉四码唯一上屏,缓冲才能停在四码展示候选。
    let mut eng = engine_with(Config {
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "aaaa");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("恭恭敬敬"));
}

#[test]
fn 五笔空格顶屏首选() {
    let mut eng = engine_with(Config {
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "aaaa");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["恭恭敬敬".to_string()]);
    assert!(eng.buffer().is_empty());
}

// ======================================================================
// 拼音通道
// ======================================================================

#[test]
fn 拼音单字_全拼排序按频次() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let page = page_texts(&eng);
    assert_eq!(page, vec!["你".to_string(), "尼".to_string()]);
    // 反查:拼音候选注释为该字的五笔码;同字多码取最长全码 wqiy 而非简码 wq。
    assert_eq!(eng.flush_page()[0].comment, "wqiy");
    // 词不在五笔表时保留拼音注释兜底。
    assert_eq!(eng.flush_page()[1].comment, "ni");
}

#[test]
fn 拼音词组_全拼命中第一() {
    let mut eng = engine();
    type_str(&mut eng, "nihao");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("你好"));
    assert!(page.contains(&"好".to_string()), "末音节单字也应出现");
}

#[test]
fn 反查_拼音候选注释为五笔编码() {
    let mut eng = engine();
    type_str(&mut eng, "nihao");
    let page = eng.flush_page();
    let ni_hao = page
        .iter()
        .find(|c| c.text == "你好")
        .expect("你好 应在候选");
    assert_eq!(ni_hao.comment, "wqvb", "拼音词组应反查五笔编码");
    let hao = page.iter().find(|c| c.text == "好").expect("好 应在候选");
    assert_eq!(hao.comment, "vbg", "拼音单字应反查五笔编码");
}

#[test]
fn 反查_末音节不完整与简拼注释同样为五笔码() {
    let mut eng = engine();
    type_str(&mut eng, "niha");
    let page = eng.flush_page();
    let ni_hao = page
        .iter()
        .find(|c| c.text == "你好")
        .expect("你好 应在候选");
    assert_eq!(ni_hao.comment, "wqvb", "缺尾词组注释也应反查五笔编码");

    let mut eng = engine();
    type_str(&mut eng, "nh");
    let page = eng.flush_page();
    let ni_hao = page
        .iter()
        .find(|c| c.text == "你好")
        .expect("简拼应命中你好");
    assert_eq!(ni_hao.comment, "wqvb", "简拼候选注释也应反查五笔编码");
}

#[test]
fn 拼音三音节词组() {
    let mut eng = engine();
    type_str(&mut eng, "nihaoma");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("你好吗"));
}

#[test]
fn 拼音切分歧义_xian_同时命中两种切分() {
    let mut eng = engine();
    type_str(&mut eng, "xian");
    let page = page_texts(&eng);
    // 切分 [xian] → 单字"先";切分 [xi,an] → 词组"西安/先安"。
    assert!(page.contains(&"先".to_string()), "整体音节切分应命中先");
    assert_eq!(
        page.first().map(String::as_str),
        Some("西安"),
        "词组全拼应胜过单字"
    );
}

#[test]
fn 拼音末音节不完整_niha_词组命中且被完整切分压制() {
    let mut eng = engine();
    type_str(&mut eng, "niha");
    let page = page_texts(&eng);
    // "niha" 可完整切分为 ni+ha("哈",pinyin_full 层),按修订 §5 层位高于
    // 不完整尾音节词组"你好"(pinyin_partial 层);但不完整查询必须仍然命中。
    assert_eq!(page.first().map(String::as_str), Some("哈"));
    assert!(
        page.contains(&"你好".to_string()),
        "缺尾词组你好应命中:{page:?}"
    );
    let nh = page.iter().position(|t| t == "你好").unwrap();
    let h = page.iter().position(|t| *t == "好").unwrap();
    assert!(nh < h, "缺尾词组你好(音节多)应排在单字好之前");
    assert!(
        page.contains(&"海".to_string()),
        "不完整片段 h 的单字(hai 海)也应出现"
    );
}

#[test]
fn 拼音简拼_nh_命中词组且低权重无单字() {
    let mut eng = engine();
    type_str(&mut eng, "nh");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("你好"));
    assert!(
        page.contains(&"你好吗".to_string()),
        "渐进简拼应给出更长词组"
    );
}

#[test]
fn 简拼至少两键_单键不出词组() {
    let mut eng = engine();
    type_str(&mut eng, "n");
    let page = page_texts(&eng);
    assert!(!page.contains(&"你好".to_string()), "单键不应按简拼出词组");
    assert!(page.contains(&"你".to_string()), "单键按不完整音节出单字");
}

#[test]
fn 全拼词组排序高于单字和简拼() {
    let mut eng = engine();
    type_str(&mut eng, "nihao");
    let page = page_texts(&eng);
    let ni_hao = page.iter().position(|t| t == "你好").unwrap();
    let hao = page.iter().position(|t| *t == "好").unwrap();
    assert!(ni_hao < hao, "词组全拼(0.95)应排在末音节单字(0.90)之前");
}

// ======================================================================
// 中英混合
// ======================================================================

#[test]
fn 无中文命中_出英文候选() {
    // 本测验证英文通道候选:关掉四码唯一上屏(hell 在夹具中唯一命中 hello,
    // 否则第 4 键会直接上屏)。
    let mut eng = engine_with(Config {
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "hello");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("hello"));
    assert_eq!(eng.flush_page()[0].kind, CandKind::English);
}

#[test]
fn 有中文命中_不出英文候选() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let page = page_texts(&eng);
    assert!(
        !page.contains(&"nice".to_string()),
        "中文命中时英文通道应关闭"
    );
}

#[test]
fn 一级简码_空格上屏当前首选() {
    // Space 是确认键:即使 a 同时是一级简码和英文词,也必须顶屏"工"。
    // 需要输出英文 a 时,应先 Shift 单击切到英文态。
    let mut eng = engine();
    type_str(&mut eng, "a");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("工"));
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["工".to_string()]);
    assert!(eng.buffer().is_empty());
}

#[test]
fn 有中文命中_空格也顶屏当前首选() {
    // 用户规则:不管缓冲多短,Space 只确认当前首选;英文用 Shift 态输入。
    let mut eng = engine();
    type_str(&mut eng, "he");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("和"));
    let fx = eng.process_key(LKey::Space);
    assert_eq!(
        commits(&fx),
        vec!["和".to_string()],
        "关闭直通后顶屏中文首选"
    );
}

#[test]
fn 英文自动直通_标点同样直通原词() {
    let mut eng = engine();
    type_str(&mut eng, "thex");
    let fx = eng.process_key(LKey::Punct(','));
    assert_eq!(commits(&fx), vec!["thex".to_string(), ",".to_string()]);
}

/// 构造仅含 english.tsv 的临时词库,目标词分别落在词频第 1999/2000/2001 名。
fn en_rank_fixture(td: &TempDir) -> Engine {
    write_en_rank_rows(td);
    engine_with_fixtures(&td.path, Config::default())
}

/// 只写 en_rank 词库行,供需要自定义配置的测试自建引擎。
fn write_en_rank_rows(td: &TempDir) {
    let mut rows = vec!["thextra	50000".to_string(), "thex	40000".to_string()];
    for i in 0..1996 {
        rows.push(format!("zz{:04}	{}", i, 30000 - i as u64)); // 第 3..=1998 名
    }
    rows.push("qword	1000".into()); // 第 1999 名
    rows.push("qworx	999".into()); // 第 2000 名(恰好达到 en_freq_top_n)
    rows.push("qwozz	998".into()); // 第 2001 名(超限)
    std::fs::write(td.join("english.tsv"), rows.join("\n")).unwrap();
}

#[test]
fn en_freq_top_n_边界_恰好第n名自动直通() {
    let td = TempDir::new();
    let mut eng = en_rank_fixture(&td);
    type_str(&mut eng, "qworx"); // 无中文命中,词频恰为第 2000 名
    assert!(
        page_texts(&eng).contains(&"qworx".to_string()),
        "恰好达到上限的词应作为英文候选:{:?}",
        page_texts(&eng)
    );
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["qworx".to_string()]);
}

#[test]
fn en_freq_top_n_边界_超过上限不直通() {
    // en_freq_top_n 只门控"自动直通",不影响候选资格(修订 §5.C 实现口径):
    // 第 2001 名的完整英文词仍以 english_no_cn 层给出候选,空格走顶屏而非直通。
    let td = TempDir::new();
    // 关掉四码唯一上屏:夹具里 qwoz 唯一命中 qwozz,否则第 4 键会直接上屏。
    write_en_rank_rows(&td);
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            commit_unique_four: false,
            ..Config::default()
        },
    );
    type_str(&mut eng, "qwozz");
    assert!(
        page_texts(&eng).contains(&"qwozz".to_string()),
        "长尾英文词仍应给出候选:{:?}",
        page_texts(&eng)
    );
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["qwozz".to_string()], "顶屏首选(即原词)");
}

#[test]
fn 英文态_一切按键直通() {
    let mut eng = engine();
    assert_eq!(eng.toggle_mode(), Mode::English);
    let fx = eng.process_key(LKey::Char('n'));
    assert!(is_pass(&fx));
    assert!(is_pass(&eng.process_key(LKey::Space)));
    assert!(is_pass(&eng.process_key(LKey::Punct(','))));
    assert!(is_pass(&eng.process_key(LKey::Digit(1))));
    assert!(is_pass(&eng.process_key(LKey::Enter)));
    assert_eq!(eng.mode(), Mode::English);
}

// ======================================================================
// 标点
// ======================================================================

#[test]
fn 中文态空缓冲_标点转中文标点() {
    let mut eng = engine();
    let fx = eng.process_key(LKey::Punct(','));
    assert_eq!(commits(&fx), vec![",".to_string()]);
    let fx = eng.process_key(LKey::Punct('?'));
    assert_eq!(commits(&fx), vec!["?".to_string()]);
    let fx = eng.process_key(LKey::Punct('['));
    assert_eq!(commits(&fx), vec!["【".to_string()]);
}

#[test]
fn 中文态有缓冲_先上屏首选再上中文标点() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let fx = eng.process_key(LKey::Punct(','));
    assert_eq!(commits(&fx), vec!["你".to_string(), ",".to_string()]);
    assert!(eng.buffer().is_empty());
}

#[test]
fn 中文态引号按开合交替() {
    let mut eng = engine();
    let first = commits(&eng.process_key(LKey::Punct('\'')));
    let second = commits(&eng.process_key(LKey::Punct('\'')));
    assert_eq!(first, vec!["\u{2018}".to_string()], "第一次应为开引号");
    assert_eq!(second, vec!["\u{2019}".to_string()], "第二次应为闭引号");
    let mut eng2 = engine();
    assert_eq!(
        commits(&eng2.process_key(LKey::Punct('"'))),
        vec!["\u{201C}".to_string()]
    );
    assert_eq!(
        commits(&eng2.process_key(LKey::Punct('"'))),
        vec!["\u{201D}".to_string()]
    );
}

#[test]
fn 未映射标点_直通放行() {
    let mut eng = engine();
    assert!(is_pass(&eng.process_key(LKey::Punct('/'))));
    assert!(is_pass(&eng.process_key(LKey::Punct('@'))));
}

#[test]
fn 关闭中文标点_直通放行() {
    let mut eng = engine_with(Config {
        cn_punct: false,
        ..Config::default()
    });
    assert!(is_pass(&eng.process_key(LKey::Punct(','))));
}

// ======================================================================
// 数字选词 / 翻页
// ======================================================================

#[test]
fn 数字选词_选当前页第n个() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let fx = eng.process_key(LKey::Digit(2));
    assert_eq!(commits(&fx), vec!["尼".to_string()]);
}

#[test]
fn 数字选词_越界吞掉不提交() {
    let mut eng = engine();
    type_str(&mut eng, "ni"); // 只有 2 个候选
    let fx = eng.process_key(LKey::Digit(9));
    assert!(is_consumed(&fx));
    assert_eq!(commits(&fx), Vec::<String>::new());
    assert_eq!(eng.buffer(), "ni", "组合不应被破坏");
}

#[test]
fn 数字键_无候选放行() {
    let mut eng = engine();
    assert!(is_pass(&eng.process_key(LKey::Digit(1))));
    type_str(&mut eng, "zzz");
    assert!(is_pass(&eng.process_key(LKey::Digit(3))));
}

#[test]
fn select_candidate_api_选中与越界() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let fx = eng.select_candidate(0);
    assert_eq!(commits(&fx), vec!["你".to_string()]);
    let fx = eng.select_candidate(9);
    assert!(is_consumed(&fx));
}

#[test]
fn 翻页_等号下一页减号上一页() {
    let mut eng = engine();
    type_str(&mut eng, "h"); // h 开头的音节:ha/hao/hai/han/hei/he → 9 个单字
    assert_eq!(eng.page_count(), 2, "page_size=5,9 个候选应有两页");
    // 同层按归一频率排序:和(6000000)第一,好(5000)随其后。
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("和"));
    assert!(page_texts(&eng).contains(&"好".to_string()));
    let fx = eng.process_key(LKey::PageDown);
    assert!(fx.iter().any(|e| matches!(e, Effect::Consumed)));
    let page2 = page_texts(&eng);
    assert_eq!(
        page2.first().map(String::as_str),
        Some("汉"),
        "第二页应以汉开头"
    );
    eng.process_key(LKey::PageUp);
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("和"));
}

#[test]
fn 翻页_单页时放行() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    assert_eq!(eng.page_count(), 1);
    assert!(is_pass(&eng.process_key(LKey::PageDown)));
    assert!(is_pass(&eng.process_key(LKey::PageUp)));
}

#[test]
fn 翻页_无候选放行() {
    let mut eng = engine();
    assert!(is_pass(&eng.process_key(LKey::PageDown)));
}

// ======================================================================
// Enter / Esc / Backspace / 缓冲状态
// ======================================================================

#[test]
fn enter_有缓冲上屏原字母() {
    let mut eng = engine();
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::Enter);
    assert_eq!(commits(&fx), vec!["nihao".to_string()]);
    assert!(eng.buffer().is_empty());
}

#[test]
fn enter_空缓冲放行() {
    let mut eng = engine();
    assert!(is_pass(&eng.process_key(LKey::Enter)));
}

#[test]
fn esc_清缓冲并吞键() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let fx = eng.process_key(LKey::Esc);
    assert!(fx.iter().any(|e| matches!(e, Effect::Consumed)));
    assert!(eng.buffer().is_empty());
    assert!(page_texts(&eng).is_empty());
    // 空缓冲时 Esc 放行。
    assert!(is_pass(&eng.process_key(LKey::Esc)));
}

#[test]
fn backspace_删尾字符() {
    let mut eng = engine();
    type_str(&mut eng, "nih");
    let fx = eng.process_key(LKey::Backspace);
    assert_eq!(eng.buffer(), "ni");
    assert_eq!(last_preedit(&fx).as_deref(), Some("ni"));
    assert!(page_texts(&eng).contains(&"你".to_string()));
}

#[test]
fn backspace_空缓冲放行() {
    let mut eng = engine();
    assert!(is_pass(&eng.process_key(LKey::Backspace)));
}

#[test]
fn 缓冲上限12字母_超出吞掉() {
    let mut eng = engine();
    type_str(&mut eng, "abcdefghijkl"); // 12 个
    assert_eq!(eng.buffer(), "abcdefghijkl");
    let fx = eng.process_key(LKey::Char('m'));
    assert!(is_consumed(&fx));
    assert_eq!(eng.buffer(), "abcdefghijkl");
}

#[test]
fn 其它键_有缓冲先清缓冲再放行() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let fx = eng.process_key(LKey::Other);
    assert!(fx.iter().any(|e| matches!(e, Effect::Pass)));
    assert!(eng.buffer().is_empty());
    assert!(is_pass(&eng.process_key(LKey::Other)));
}

#[test]
fn 大写字母_防御性直通() {
    let mut eng = engine();
    assert!(is_pass(&eng.process_key(LKey::Char('A'))));
    assert!(eng.buffer().is_empty());
}

#[test]
fn shiftpress_吞键_切模式由宿主调_toggle_mode() {
    let mut eng = engine();
    assert!(is_consumed(&eng.process_key(LKey::ShiftPress)));
    assert_eq!(eng.mode(), Mode::Chinese);
    assert_eq!(eng.toggle_mode(), Mode::English);
    assert_eq!(eng.toggle_mode(), Mode::Chinese);
}

#[test]
fn shift_有缓冲先上屏英文原串() {
    let mut eng = engine();
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::ShiftPress);
    assert_eq!(commits(&fx), vec!["nihao".to_string()]);
    assert!(eng.buffer().is_empty());
    assert!(page_texts(&eng).is_empty());
    // 空缓冲的 Shift 仍由宿主判定单击后切模式;core 只吞键。
    assert!(is_consumed(&eng.process_key(LKey::ShiftPress)));
    assert_eq!(eng.mode(), Mode::Chinese);
}

#[test]
fn reset_清空组合() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    eng.reset();
    assert!(eng.buffer().is_empty());
    assert!(page_texts(&eng).is_empty());
    assert!(is_pass(&eng.process_key(LKey::Space)));
}

// ======================================================================
// 学习(user.tsv + write-behind + 排序回归)
// ======================================================================

#[test]
fn 学习后排序提升并标记用户词() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    // dh:到(3000)> 稻(2000)。
    type_str(&mut eng, "dh");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("到"));
    let fx = eng.process_key(LKey::Digit(2)); // 选中"稻"
    assert_eq!(commits(&fx), vec!["稻".to_string()]);
    // 再次输入:稻 ×1.5 加成反超。
    type_str(&mut eng, "dh");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "稻");
    assert_eq!(page[0].kind, CandKind::User);
}

#[test]
fn 学习写盘_依赖显式_flush() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    type_str(&mut eng, "ni");
    eng.process_key(LKey::Space); // 上屏"你"
    assert!(
        !td.join("user.tsv").exists(),
        "不足 64 次且未 flush,不应落盘"
    );
    eng.flush_user_dict().unwrap();
    let text = std::fs::read_to_string(td.join("user.tsv")).unwrap();
    let line = text.lines().next().unwrap();
    let mut cols = line.split('\t');
    assert_eq!(cols.next(), Some("你"));
    assert_eq!(cols.next(), Some("1"), "一次上屏 freq_extra=1");
    assert!(!cols.next().unwrap().is_empty(), "应有 last_used_epoch");
}

#[test]
fn 学习累计_多次上屏词频递增() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    for _ in 0..2 {
        type_str(&mut eng, "ni");
        eng.process_key(LKey::Space);
    }
    eng.flush_user_dict().unwrap();
    let text = std::fs::read_to_string(td.join("user.tsv")).unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("你\t2\t")),
        "实为:{text}"
    );
}

#[test]
fn write_behind_满64次自动落盘() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    for _ in 0..64 {
        type_str(&mut eng, "a");
        eng.process_key(LKey::Space); // 顶屏"工"
    }
    let text = std::fs::read_to_string(td.join("user.tsv")).unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("工\t64\t")),
        "64 次应触发 write-behind"
    );
}

#[test]
fn 关闭学习_不写盘不加载() {
    let td = TempDir::new();
    let mut eng = engine_with(Config {
        learning: false,
        user_dict: Some(td.join("user.tsv")),
        ..Config::default()
    });
    type_str(&mut eng, "ni");
    eng.process_key(LKey::Space);
    eng.flush_user_dict().unwrap();
    assert!(!td.join("user.tsv").exists(), "learning=false 不应写盘");
    type_str(&mut eng, "dh");
    assert_eq!(
        page_texts(&eng).first().map(String::as_str),
        Some("到"),
        "无加成,频次高者在前"
    );
}

#[test]
fn 用户词典_重启后加载生效() {
    let td = TempDir::new();
    {
        let mut eng = engine_with_user_dict(&td);
        type_str(&mut eng, "dh");
        eng.process_key(LKey::Digit(2)); // 学习"稻"
        eng.flush_user_dict().unwrap();
    } // drop(顺带落盘,但不依赖)
    let mut eng2 = engine_with_user_dict(&td);
    type_str(&mut eng2, "dh");
    assert_eq!(
        page_texts(&eng2).first().map(String::as_str),
        Some("稻"),
        "重启后应从 user.tsv 恢复加成"
    );
}

// ======================================================================
// 配置加载
// ======================================================================

#[test]
fn 配置缺失_回退默认值() {
    let td = TempDir::new();
    let cfg = Config::load(&td.join("no-such-config.toml")).unwrap();
    assert_eq!(cfg, Config::default());
}

#[test]
fn 配置部分字段_缺失项取默认() {
    let td = TempDir::new();
    let p = td.join("config.toml");
    std::fs::write(&p, "page_size = 7\nmode = \"en\"\n").unwrap();
    let cfg = Config::load(&p).unwrap();
    assert_eq!(cfg.page_size, 7);
    assert_eq!(cfg.mode, Mode::English);
    assert!(cfg.mixed_en, "未写出的开关应保持默认 true 而非零值");
    assert_eq!(cfg.en_freq_top_n, 2000);
}

#[test]
fn 配置损坏_报人话错误() {
    let td = TempDir::new();
    let p = td.join("config.toml");
    std::fs::write(&p, "这不是 toml [[[\npage_size = ").unwrap();
    let err = Config::load(&p).unwrap_err();
    assert!(
        err.message().contains("损坏"),
        "应提示配置损坏:{}",
        err.message()
    );
}

#[test]
fn 配置取值非法_报人话错误() {
    let td = TempDir::new();
    let p = td.join("config.toml");
    std::fs::write(&p, "page_size = 20\n").unwrap();
    let err = Config::load(&p).unwrap_err();
    assert!(err.message().contains("page_size"), "{}", err.message());

    std::fs::write(&p, "mode = \"kr\"\n").unwrap();
    let err = Config::load(&p).unwrap_err();
    assert!(err.message().contains("mode"), "{}", err.message());
}

#[test]
fn set_config_生效并重置默认模式() {
    let mut eng = engine();
    type_str(&mut eng, "ni"); // 留下组合
    eng.set_config(Config {
        page_size: 3,
        mode: Mode::English,
        ..Config::default()
    });
    assert_eq!(eng.config().page_size, 3);
    assert_eq!(
        eng.mode(),
        Mode::English,
        "config.mode 是启动默认模式,set_config 会重置"
    );
    assert!(eng.buffer().is_empty(), "切模式应清空组合");
}

// ======================================================================
// 排序细节(兜底词频/同码简码奖励)
// ======================================================================

#[test]
fn 同分候选_按通用词频兜底排序() {
    // dh 下的 到/稻 频次不同,不足以触发兜底;用 suggestion 中被加成的"稻"验证
    // suggestion 兜底:构造两个同频词条的临时词库。
    let td = TempDir::new();
    std::fs::write(td.join("wubi.tsv"), "z\t甲\t100\nz\t乙\t100\n").unwrap();
    std::fs::write(td.join("suggestion.tsv"), "乙\t5000\n甲\t10\n").unwrap();
    let mut eng = Engine::new(&td.path).unwrap();
    type_str(&mut eng, "z");
    let page = page_texts(&eng);
    assert_eq!(
        page,
        vec!["乙".to_string(), "甲".to_string()],
        "同分时 suggestion 高者在前"
    );
}

#[test]
fn 学习加成_用户词优先于更高频普通词() {
    let td = TempDir::new();
    let mut eng = engine_with_user_dict(&td);
    // 你好(3000)本就第一;改用 是吗 vs 你好吗:先学"你好吗"(低频词组)。
    type_str(&mut eng, "nihaoma");
    let fx = eng.process_key(LKey::Space); // 上屏"你好吗"
    assert_eq!(commits(&fx), vec!["你好吗".to_string()]);
    // "ni" 简拼渐进里 你好吗 原本排在 你好 之后;加成后仍不及全拼 你好,
    // 这里验证的是简拼通道内的提升:输入 nh 时 你好吗(×1.5)反超 你好。
    type_str(&mut eng, "nh");
    let page = page_texts(&eng);
    let nhm = page.iter().position(|t| t == "你好吗").unwrap();
    let nh = page.iter().position(|t| t == "你好").unwrap();
    assert!(nhm < nh, "学习后低频词组应在简拼通道反超,实为 {page:?}");
}

// ======================================================================
// 快速功能键(合同 §14)
// ======================================================================

use lyyime_core::QuickAction;

/// 配置单条功能键的引擎。
fn engine_action(trigger: &str, label: &str, command: &str) -> Engine {
    engine_with(Config {
        quick_actions: vec![QuickAction {
            trigger: trigger.to_string(),
            label: label.to_string(),
            command: command.to_string(),
        }],
        ..Config::default()
    })
}

#[test]
fn 快速功能键_触发词整串命中追加功能候选() {
    let mut eng = engine_action("peizhi", "打开配置", "@settings");
    // 部分触发词不出现功能候选(整串匹配才提示)
    type_str(&mut eng, "peizh");
    assert!(!page_texts(&eng).iter().any(|t| t == "打开配置"));
    eng.process_key(LKey::Char('i'));
    let page = eng.flush_page();
    assert_eq!(page[0].text, "打开配置", "peizhi 无词库命中,功能候选置顶");
    assert_eq!(page[0].kind, CandKind::Action(0));
    assert_eq!(page[0].comment, "功能键");
}

#[test]
fn 快速功能键_紧跟首选不顶替普通候选() {
    // "nihao" 命中词库「你好」:功能候选排第 2,首选仍是普通候选。
    let mut eng = engine_action("nihao", "打开配置", "@settings");
    type_str(&mut eng, "nihao");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "你好");
    assert_eq!(page[1].text, "打开配置");
    assert_eq!(page[1].kind, CandKind::Action(0));
    // 空格确认首选仍是普通候选(功能键不劫持顶屏)。
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
}

#[test]
fn 快速功能键_数字选中产生Action且不学习() {
    let td = TempDir::new();
    let mut eng = engine_with(Config {
        user_dict: Some(td.join("user.tsv")),
        quick_actions: vec![QuickAction {
            trigger: "nihao".to_string(),
            label: "打开配置".to_string(),
            command: "@settings".to_string(),
        }],
        ..Config::default()
    });
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::Digit(2));
    assert!(
        matches!(fx.first(), Some(Effect::Action(0))),
        "数字选功能键应产生 Action 效果,实为 {fx:?}"
    );
    assert!(commits(&fx).is_empty(), "功能键不上屏文本");
    assert!(eng.flush_page().is_empty(), "功能键选中后候选条清除");
    // 不污染学习数据:用户词典保持为空。
    eng.flush_user_dict().unwrap();
    assert_eq!(
        std::fs::read_to_string(td.join("user.tsv")).unwrap_or_default(),
        ""
    );
}

#[test]
fn 快速功能键_select_candidate_点选产生Action() {
    let mut eng = engine_action("nihao", "打开配置", "@settings");
    type_str(&mut eng, "nihao");
    let fx = eng.select_candidate(1); // 第 2 条 = 功能键
    assert!(matches!(fx.first(), Some(Effect::Action(0))), "{fx:?}");
    // 越界点选吞掉,不受功能键影响。
    type_str(&mut eng, "ni");
    assert!(matches!(eng.select_candidate(99).as_slice(), [Effect::Consumed]));
}

#[test]
fn 快速功能键_多条触发词按配置顺序() {
    let mut eng = engine_with(Config {
        quick_actions: vec![
            QuickAction {
                trigger: "ni".into(),
                label: "功能一".into(),
                command: "@settings".into(),
            },
            QuickAction {
                trigger: "ni".into(),
                label: "功能二".into(),
                command: "@help".into(),
            },
        ],
        ..Config::default()
    });
    type_str(&mut eng, "ni");
    let texts = page_texts(&eng);
    let p1 = texts.iter().position(|t| t == "功能一").unwrap();
    let p2 = texts.iter().position(|t| t == "功能二").unwrap();
    assert_eq!((p1, p2), (1, 2), "功能候选按配置顺序紧跟首选,实为 {texts:?}");
}

#[test]
fn 快速功能键_总开关关闭不提示() {
    let mut eng = engine_with(Config {
        quick_actions_enabled: false,
        quick_actions: vec![QuickAction {
            trigger: "peizhi".to_string(),
            label: "打开配置".to_string(),
            command: "@settings".to_string(),
        }],
        ..Config::default()
    });
    type_str(&mut eng, "peizhi");
    assert!(page_texts(&eng).is_empty());
}

#[test]
fn 快速功能键_唯一功能候选不触发四码唯一上屏() {
    // 4 字母触发词且无词库命中:四码唯一上屏不得把功能键自动打出去。
    let mut eng = engine_action("abcd", "打开配置", "@settings");
    let fx = type_str(&mut eng, "abcd");
    assert!(
        commits(&fx).is_empty() && !fx.iter().any(|e| matches!(e, Effect::Action(_))),
        "唯一候选是功能键时不自动上屏,{fx:?}"
    );
    // 用户空格确认后才触发。
    let fx = eng.process_key(LKey::Space);
    assert!(matches!(fx.first(), Some(Effect::Action(0))));
}

#[test]
fn 快速功能键_标点收尾退回原始字母不上屏功能标签() {
    // 触发词已命中功能候选时打标点:退回原始字母收尾,不把标签当文本上屏。
    let mut eng = engine_action("zzz", "打开配置", "@settings");
    type_str(&mut eng, "zzz");
    let fx = eng.process_key(LKey::Punct(','));
    let cs = commits(&fx);
    assert_eq!(cs.first().map(String::as_str), Some("zzz"), "退回原始字母 {cs:?}");
    assert!(!cs.iter().any(|c| c == "打开配置"), "功能标签不得上屏 {cs:?}");
}
