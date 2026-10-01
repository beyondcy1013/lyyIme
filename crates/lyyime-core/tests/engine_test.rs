//! 引擎行为集成测试(合同 M1 硬指标):
//! 五笔全码/简码/前缀渐进/词组、拼音全拼/切分歧义/末音节不完整/简拼、
//! 中英混合、标点、数字选词、翻页、Enter/Esc/Backspace、学习排序、配置加载。

mod common;

use common::*;
use lyyime_core::{CandKind, CandOp, Config, Effect, EnCommit, Engine, LKey, Mode};
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
    // "thex" 全程无中文命中(英文通道缺失时也就没有任何候选)。
    type_str(&mut eng, "thex");
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
        // 本测单独验证“四码后首字母顶屏”:关掉四码自动上屏,保证缓冲能停在四码。
        commit_first_at_four: false,
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
fn 四码后续字母_仍有中文命中则继续组词() {
    // 四码顶屏只对五笔首选生效(§6):"niha" 的首选来自拼音通道,'o'
    // 不被顶屏——"nihaoma" 逐键命中 ni/niha/nihao/nihaoma 一路成词。
    let mut eng = engine();
    type_str(&mut eng, "nihaoma");
    assert_eq!(eng.buffer(), "nihaoma");
    assert!(page_texts(&eng).contains(&"你好吗".to_string()));
    // 而死码扩展("wgli" 在 "w" 之后即无中文命中)不进缓冲,见
    // 四码后继续输入死码_吞键保留候选。
}

#[test]
fn 四码后继续输入死码_吞键保留候选() {
    // "aaaa" 在夹具中只有唯一候选,关掉四码自动上屏让缓冲停在四码;
    // 同时关掉四码顶屏(默认开),否则第 5 键会先顶屏首选而非走死码路径。
    // 此后敲 'g' 把缓冲推进完全无候选的死胡同 → 吞键,候选状态原样保留。
    let mut eng = engine_with(Config {
        commit_first_at_four: false,
        commit_unique_four: false,
        commit_on_extra_after_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "aaaa");
    let before = page_texts(&eng);
    assert_eq!(before.first().map(String::as_str), Some("恭恭敬敬"));
    let fx = eng.process_key(LKey::Char('g'));
    assert!(commits(&fx).is_empty(), "死码字母不得产生上屏");
    assert!(fx.iter().any(|e| matches!(e, Effect::Consumed)), "fx={fx:?}");
    assert_eq!(eng.buffer(), "aaaa", "死码字母不进缓冲");
    assert_eq!(page_texts(&eng), before, "死码字母不得清空中文候选");
    // 连敲仍是吞键;空格照旧顶屏四码首选。
    eng.process_key(LKey::Char('x'));
    assert_eq!(eng.buffer(), "aaaa");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["恭恭敬敬".to_string()]);
}

#[test]
fn 不足四码的死码字母同样保留候选() {
    // 死码保护按"是否还有中文命中"判定,不限于四码:"wq" 有中文命中,
    // 'x' 使缓冲无候选 → 吞键;回退一格仍可选词。
    let mut eng = engine_with(Config {
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "wq");
    let before = page_texts(&eng);
    assert!(before.contains(&"你".to_string()));
    let fx = eng.process_key(LKey::Char('x'));
    assert!(commits(&fx).is_empty());
    assert_eq!(eng.buffer(), "wq");
    assert_eq!(page_texts(&eng), before);
    // Backspace 后正常回退组合。
    eng.process_key(LKey::Backspace);
    assert_eq!(eng.buffer(), "w");
}

#[test]
fn 四码唯一_英文候选不自动上屏() {
    // "hell" 在夹具中唯一命中英文词 hello:英文候选不参与四码唯一上屏,
    // 第 4 键保持组合展示候选,继续敲完 hello 后空格照常顶屏。
    let mut eng = engine();
    let fx = type_str(&mut eng, "hell");
    assert!(commits(&fx).is_empty(), "英文唯一候选不得自动上屏");
    assert_eq!(eng.buffer(), "hell");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("hello"));
    let fx = eng.process_key(LKey::Char('o'));
    assert!(commits(&fx).is_empty());
    assert_eq!(eng.buffer(), "hello");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["hello".to_string()]);
}

// ----------------------------------------------------------------------
// 四码唯一上屏(默认开启):恰好四码且候选唯一 → 免空格直接上屏
// ----------------------------------------------------------------------

#[test]
fn 四码唯一_免空格直接上屏() {
    // 本测断言"上屏后候选一并清空"与空缓冲空格直通的旧语义;
    // 上屏后联想属独立特性,这里关掉以隔离被测行为。
    let mut eng = engine_with(Config {
        next_word_prediction: false,
        ..Config::default()
    });
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
fn 四码首选_有重码不上屏_继续输入顶屏首选() {
    // 四码有重码禁止上屏(2026-09-28):gcft 同码命中「致/死难者」两个五笔
    // 候选,第 4 键保留组合等用户选词;直接继续输入则由四码顶屏(默认开)
    // 上屏首选,该字母开启新组合。
    let td = TempDir::new();
    let mut wubi = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wubi.push_str("gcft\t致\t900\ngcft\t死难者\t1200\n");
    std::fs::write(td.join("wubi.tsv"), &wubi).unwrap();
    let mut eng = engine_with_fixtures(&td.path, Config::default());
    let fx = type_str(&mut eng, "gcft");
    assert!(commits(&fx).is_empty(), "重码四码不得自动上屏:{fx:?}");
    assert_eq!(eng.buffer(), "gcft");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("致"));
    // 空格/数字照常可选重码;这里继续敲 'g' → 顶屏首选「致」,g 开新组合。
    let fx = eng.process_key(LKey::Char('g'));
    assert_eq!(commits(&fx), vec!["致".to_string()], "继续输入应顶屏首选");
    assert_eq!(eng.buffer(), "g");
    // 新组合 'g':精确命中「一」居首,gcft 前缀候选(死难者/致)随渐进跟排。
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("一"));
}

#[test]
fn 四码首选_混排候选算重码不上屏_继续输入顶屏() {
    // "重码"按候选条可见总数判定(2026-09-29 修订):xian 私有夹具给五笔
    // 唯一词条「舞」,但拼音通道同出 先/西安/先安——候选条多于一条即
    // 禁止自动上屏;继续输入字母仍顶屏五笔首选。
    let td = TempDir::new();
    for f in ["pinyin_char.tsv", "pinyin_phrase.tsv"] {
        std::fs::copy(fixtures().join(f), td.join(f)).unwrap();
    }
    let mut wubi = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wubi.push_str("xian\t舞\t900\n");
    std::fs::write(td.join("wubi.tsv"), &wubi).unwrap();
    let mut eng = engine_with_fixtures(&td.path, Config::default());
    let fx = type_str(&mut eng, "xian");
    assert!(commits(&fx).is_empty(), "混排多候选不得自动上屏:{fx:?}");
    assert_eq!(eng.buffer(), "xian");
    let page = page_texts(&eng);
    assert!(page.len() > 1 && page.first().map(String::as_str) == Some("舞"), "page={page:?}");
    // 继续敲 'g' → 顶屏首选「舞」,g 开新组合。
    let fx = eng.process_key(LKey::Char('g'));
    assert_eq!(commits(&fx), vec!["舞".to_string()]);
    assert_eq!(eng.buffer(), "g");
}

#[test]
fn 四码首选_拼音与英文中间态不打断() {
    // "niha" 是 nihao 的中间态、首选来自拼音通道:四码首选上屏只对
    // 五笔命中触发,拼音长码不被劫持;"hell" 首选英文候选同样不触发。
    // 四码顶屏(默认开)同样只对五笔首选生效:'o' 顶不动拼音首选,
    // 缓冲照常加长续拼 nihao。
    let mut eng = engine();
    let fx = type_str(&mut eng, "niha");
    assert!(commits(&fx).is_empty(), "拼音中间态不得四码上屏");
    assert_eq!(eng.buffer(), "niha");
    let fx = eng.process_key(LKey::Char('o'));
    assert!(commits(&fx).is_empty(), "拼音首选不得被顶屏:{fx:?}");
    assert_eq!(eng.buffer(), "nihao", "继续打完 nihao 正常组词");
    assert!(page_texts(&eng).contains(&"你好".to_string()));
}

#[test]
fn 四码首选_关闭后回退唯一上屏判定() {
    // 关掉 commit_first_at_four:多候选四码保留组合;候选唯一时仍由
    // commit_unique_four 上屏(两个开关独立的回退关系)。
    // 私有夹具:gcft 同时命中「致」与「死难者」两个重码。
    let td = TempDir::new();
    let mut wubi = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wubi.push_str("gcft\t致\t900\ngcft\t死难者\t1200\n");
    std::fs::write(td.join("wubi.tsv"), &wubi).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            commit_first_at_four: false,
            ..Config::default()
        },
    );
    let fx = type_str(&mut eng, "gcft");
    assert!(commits(&fx).is_empty(), "重码四码不上屏:{fx:?}");
    assert_eq!(eng.buffer(), "gcft");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["致".to_string()]);
    // 唯一候选仍走唯一上屏:wqiy 在夹具中只有一个候选「你」。
    let fx = type_str(&mut eng, "wqiy");
    assert_eq!(commits(&fx), vec!["你".to_string()], "唯一候选四码仍自动上屏");
}

#[test]
fn 四码自动上屏_全关后保持渐进组合() {
    // commit_first_at_four 与 commit_unique_four 都关掉,四码后缓冲保留候选。
    let mut eng = engine_with(Config {
        commit_first_at_four: false,
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
    // 词组提示与联想共用候选条位置:有联想行时不发提示;
    // 本测专验提示本身,关掉联想隔离变量。
    let mut eng = engine_with(Config {
        next_word_prediction: false,
        ..Config::default()
    });
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
    // 空格确认路径同样不提示(关四码自动上屏,缓冲才能停在四码等空格)。
    let mut eng2 = engine_with(Config {
        commit_first_at_four: false,
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
    let mut eng = engine_with(Config {
        next_word_prediction: false,
        ..Config::default()
    });
    // nihao+空格 顶屏词组「你好」(5 个字母 > wqvb 4 键)→ 提示五笔编码。
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
    let hs = hints(&fx);
    assert!(hs.iter().any(|h| h.contains("你好") && h.contains("wqvb")), "提示={hs:?}");
}

#[test]
fn 词组提示_下一次输入产生新效果流无残留提示() {
    let mut eng = engine_with(Config {
        next_word_prediction: false,
        ..Config::default()
    });
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
    // ("abc" 在 "a" 处即有中文命中,'b' 属死码会被吞;改用全程无命中的 "thex"。)
    type_str(&mut eng, "thex");
    let fx = eng.process_key(LKey::Enter);
    assert_eq!(commits(&fx), vec!["thex".to_string()]);
    assert!(hints(&fx).is_empty());
}

#[test]
fn 五笔精确层_单字按语料频次归一() {
    let mut eng = engine();
    let fx = type_str(&mut eng, "a");
    let top = &eng.flush_page()[0];
    assert_eq!(top.text, "工");
    // 层级词典序:工 处于五笔精确层(60);单字 norm 按真实语料频次归一,
    // 满值 10 只属于语料最高频字,故得分落在 [60,70) 区间。
    assert!(
        (60.0..70.0).contains(&top.score),
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
    // 本测验证四码词组排序本身:关掉四码自动上屏,缓冲才能停在四码展示候选。
    let mut eng = engine_with(Config {
        commit_first_at_four: false,
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "aaaa");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("恭恭敬敬"));
}

// ======================================================================
// 精确层单字按词频排位(exact_char_freq_rank,默认开;§5)
// ======================================================================

/// 带 GB2312 分档表与语料增补的临时词库:工(高频,语料 ~5e6)、
/// 氢(低频,语料 500;wubi aaa 全码 3000),验证低频全码单字让位词组。
fn engine_with_tier() -> Engine {
    let td = TempDir::new();
    for f in [
        "wubi.tsv",
        "pinyin_char.tsv",
        "pinyin_phrase.tsv",
        "suggestion.tsv",
        "english.tsv",
        "meta.json",
    ] {
        let src = fixtures().join(f);
        if src.exists() {
            std::fs::copy(&src, td.join(f)).unwrap();
        }
    }
    let mut py = std::fs::read_to_string(fixtures().join("pinyin_char.tsv")).unwrap();
    py.push_str("gong\t工\t5000000\nqing\t氢\t500\n");
    std::fs::write(td.join("pinyin_char.tsv"), py).unwrap();
    std::fs::write(td.join("char_tier.tsv"), "工\t1\n氢\t1\n").unwrap();
    let mut wb = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wb.push_str("aaa\t氢\t3000\n");
    std::fs::write(td.join("wubi.tsv"), wb).unwrap();
    engine_with_fixtures(&td.path, Config::default())
}

#[test]
fn 精确单字频率排位_低频字让位词组() {
    // 默认开:aaa 全码精确命中「氢」(语料 f≈0.4 < 0.5)按频率降档,
    // 首选让位 aaaa 前缀词组「恭恭敬敬」;氢仍在页内可翻选。
    let mut eng = engine_with_tier();
    type_str(&mut eng, "aaa");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("恭恭敬敬"), "{page:?}");
    assert!(page.contains(&"氢".to_string()), "氢应保留在候选页:{page:?}");
}

#[test]
fn 精确单字频率排位_高频字保持恒居首位() {
    // 语料常用字「工」(f≈0.99 ≥ 0.5)不经降档:a 的首选仍是工。
    let mut eng = engine_with_tier();
    type_str(&mut eng, "a");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("工"));
}

#[test]
fn 精确单字频率排位_关闭恢复恒居首位() {
    // 关闭开关:氢回到精确层顶部,aaa 首选恢复为氢(旧行为)。
    let td = TempDir::new();
    for f in [
        "wubi.tsv",
        "pinyin_char.tsv",
        "pinyin_phrase.tsv",
        "suggestion.tsv",
        "english.tsv",
        "meta.json",
    ] {
        let src = fixtures().join(f);
        if src.exists() {
            std::fs::copy(&src, td.join(f)).unwrap();
        }
    }
    let mut py = std::fs::read_to_string(fixtures().join("pinyin_char.tsv")).unwrap();
    py.push_str("qing\t氢\t500\n");
    std::fs::write(td.join("pinyin_char.tsv"), py).unwrap();
    std::fs::write(td.join("char_tier.tsv"), "氢\t1\n").unwrap();
    let mut wb = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wb.push_str("aaa\t氢\t3000\n");
    std::fs::write(td.join("wubi.tsv"), wb).unwrap();
    let mut eng = engine_with_fixtures(&td.path, Config {
        exact_char_freq_rank: false,
        ..Config::default()
    });
    type_str(&mut eng, "aaa");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("氢"), "{page:?}");
}

#[test]
fn 精确单字频率排位_无分档表不触发() {
    // 默认 fixtures 无 char_tier.tsv(旧数据目录):开关开也不降档(回退路径)。
    let mut eng = engine();
    type_str(&mut eng, "a");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("工"));
    let top = &eng.flush_page()[0];
    assert!(
        (60.0..70.0).contains(&top.score),
        "无分档表应保持精确层,实为 {}",
        top.score
    );
}

#[test]
fn 五笔空格顶屏首选() {
    let mut eng = engine_with(Config {
        commit_first_at_four: false,
        commit_unique_four: false,
        ..Config::default()
    });
    type_str(&mut eng, "aaaa");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["恭恭敬敬".to_string()]);
    assert!(eng.buffer().is_empty());
}

#[test]
fn 四码单字_优先于同码词组与用户词() {
    // 私有夹具镜像真库场景(不污染共享夹具的前缀候选):gcft 同时精确命中
    // 「致」(900)与「死难者」(1200)——词组频次更高,学习 ×1.5 后单字仍须居首。
    let td = TempDir::new();
    let mut wubi = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wubi.push_str("gcft\t致\t900\ngcft\t死难者\t1200\n");
    std::fs::write(td.join("wubi.tsv"), &wubi).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            // 本测验证四码候选排序:关掉自动上屏,缓冲才能停在四码。
            commit_first_at_four: false,
            commit_unique_four: false,
            user_dict: Some(td.join("user.tsv")),
            ..Config::default()
        },
    );

    type_str(&mut eng, "gcft");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "致", "四码精确单字应排同码词组之前");
    assert_eq!(page[1].text, "死难者");

    // 选中词组学习后再打,单字仍居首,词组带 User 标记紧随其后。
    let fx = eng.process_key(LKey::Digit(2)); // 选中「死难者」→ 学习
    assert_eq!(commits(&fx), vec!["死难者".to_string()]);
    type_str(&mut eng, "gcft");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "致", "用户学习加成不得把同码词组抬到单字之前");
    assert_eq!(page[0].kind, CandKind::Wubi);
    assert_eq!(page[1].text, "死难者");
    assert_eq!(page[1].kind, CandKind::User);
}

#[test]
fn 四码生僻单字_按GB2312分档沉到词组后() {
    // 私有夹具镜像真库 thgj:牏(码表默认频 1000,语料填充值 58.5万,不在 GB2312)
    // 曾压过词组「处理」;分档表落实 一级字 > 词组 > 生僻字,生僻字仍可达。
    let td = TempDir::new();
    let mut wubi = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wubi.push_str("thgj\t牏\t1000\nthgj\t㸟\t95\nthgj\t处理\t500\nxxyy\t引\t1000\nxxyy\t引子\t1500\n");
    std::fs::write(td.join("wubi.tsv"), &wubi).unwrap();
    std::fs::write(td.join("char_tier.tsv"), "引\t1\n").unwrap();
    // 词频序下"常用字压词组"需要语料佐证:给引补一条高频语料(f→满值)。
    let mut py = std::fs::read_to_string(fixtures().join("pinyin_char.tsv")).unwrap();
    py.push_str("yin\t引\t900000000\n");
    std::fs::write(td.join("pinyin_char.tsv"), py).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            // 本测验证四码候选排序:关掉自动上屏,缓冲才能停在四码。
            commit_first_at_four: false,
            commit_unique_four: false,
            ..Config::default()
        },
    );

    type_str(&mut eng, "thgj");
    let texts = page_texts(&eng);
    assert_eq!(
        texts.first().map(String::as_str),
        Some("处理"),
        "生僻单字(表外)不得越过词组:{texts:?}"
    );
    assert!(
        texts.contains(&"牏".to_string()),
        "生僻字沉底但不消失,仍可翻页选出:{texts:?}"
    );
    eng.process_key(LKey::Esc); // 清缓冲再试下一段
    type_str(&mut eng, "xxyy");
    let texts = page_texts(&eng);
    assert_eq!(
        texts.first().map(String::as_str),
        Some("引"),
        "语料高频字按词频序压过词组:{texts:?}"
    );
}

#[test]
fn 四码单字_按语料频次排_生僻字沉底() {
    // 私有夹具镜像真库病态:海峰码表给生僻字默认频(~1000),码表频排序会让
    // 「靷」(语料 5e3)压过「引」(语料 9e8);语料外死字「齾」按表频 ×0.1 沉底。
    let td = TempDir::new();
    let mut wubi = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wubi.push_str("xxyy\t靷\t2000\nxxyy\t引\t1000\nxxyy\t齾\t3000\nxxyy\t引子\t1500\n");
    std::fs::write(td.join("wubi.tsv"), &wubi).unwrap();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "yin\t引\t900000000\nyin\t靷\t5000\n",
    )
    .unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            // 本测验证四码候选排序:关掉自动上屏,缓冲才能停在四码。
            commit_first_at_four: false,
            commit_unique_four: false,
            ..Config::default()
        },
    );

    type_str(&mut eng, "xxyy");
    let texts = page_texts(&eng);
    assert_eq!(
        texts.iter().take(3).map(String::as_str).collect::<Vec<_>>(),
        vec!["引", "靷", "齾"],
        "单字按语料频次排,语料外死字沉底:{texts:?}"
    );
    assert!(
        !texts[..3].contains(&"引子".to_string()),
        "词组(spec 1.0)不得越过单字组:{texts:?}"
    );
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
    let page = eng.flush_page();
    assert_eq!(page.first().map(|c| c.text.as_str()), Some("你好"));
    // 多音节缓冲不再出"只对应末音节"的孤立单字(旧行为会在 nihao 下出
    // 「好」);前缀候选「你」消费 ni(consumed=2),选中后剩余 hao 继续组词。
    assert!(
        !page.iter().any(|c| c.text == "好"),
        "nihao 不得出尾音节孤字 好:{page:?}"
    );
    let ni = page.iter().find(|c| c.text == "你").expect("前缀候选 你 应在列");
    assert_eq!(ni.consumed, 2, "前缀候选 你 只消费 ni");
    assert_eq!(ni.comment, "wqiy", "前缀候选注释同样反查五笔码");
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
    let ni = page.iter().find(|c| c.text == "你").expect("前缀候选 你 应在列");
    assert_eq!(ni.comment, "wqiy", "前缀单字候选应反查五笔编码");
    // 末音节单字的五笔反查在单音节缓冲下验证(ni → 你 见上,hao → 好)。
    let mut eng2 = engine();
    type_str(&mut eng2, "hao");
    let page2 = eng2.flush_page();
    let hao = page2.iter().find(|c| c.text == "好").expect("好 应在候选");
    assert_eq!(hao.comment, "vbg", "拼音单字应反查五笔编码");
    assert_eq!(hao.consumed, 0, "单音节命中消费整个缓冲");
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
    // 尾音节"an"的孤字(安)不得出现——它需要丢弃已敲的 xi 才说得通;
    // 对应能力由前缀候选承担(西 consumed=2,选中后剩 an 继续组词)。
    assert!(
        !page.contains(&"安".to_string()),
        "xian 不得出尾音节孤字 安:{page:?}"
    );
    let xi = eng
        .flush_page()
        .iter()
        .find(|c| c.text == "西")
        .expect("前缀候选 西 应在列");
    assert_eq!(xi.consumed, 2);
}

#[test]
fn 拼音末音节不完整_niha_词组命中且无尾部孤字() {
    let mut eng = engine();
    type_str(&mut eng, "niha");
    let page = eng.flush_page();
    // "niha" 的完整切分 ni+ha 没有词组/单字落点(末音节孤字已取消),
    // 缺尾词组"你好"(pinyin_partial 层)居首。
    assert_eq!(page.first().map(|c| c.text.as_str()), Some("你好"));
    for tail in ["哈", "海", "好"] {
        assert!(
            !page.iter().any(|c| c.text == tail),
            "niha 不得出尾音节孤字 {tail}:{page:?}"
        );
    }
    // 前缀候选 你(consumed=2)仍在列,选中后余 ha 继续组词。
    let ni = page.iter().find(|c| c.text == "你").expect("前缀候选 你 应在列");
    assert_eq!(ni.consumed, 2);
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
fn 全拼词组排序高于前缀候选和简拼() {
    let mut eng = engine();
    type_str(&mut eng, "nihao");
    let page = page_texts(&eng);
    let ni_hao = page.iter().position(|t| t == "你好").unwrap();
    // 前缀候选「你」只消费 ni,层级低于一切整缓冲命中,全拼词组必须在它之前。
    let ni = page.iter().position(|t| *t == "你").expect("前缀候选 你 应在列");
    assert!(ni_hao < ni, "词组全拼应排在前缀候选之前:{page:?}");
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
    assert_eq!(
        commits(&fx),
        vec!["thex".to_string(), "\u{FF0C}".to_string()]
    );
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
    assert_eq!(commits(&fx), vec!["\u{FF0C}".to_string()]);
    let fx = eng.process_key(LKey::Punct('?'));
    assert_eq!(commits(&fx), vec!["\u{FF1F}".to_string()]);
    let fx = eng.process_key(LKey::Punct('['));
    assert_eq!(commits(&fx), vec!["【".to_string()]);
}

#[test]
fn 中文态有缓冲_先上屏首选再上中文标点() {
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let fx = eng.process_key(LKey::Punct(','));
    assert_eq!(
        commits(&fx),
        vec!["你".to_string(), "\u{FF0C}".to_string()]
    );
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

#[test]
fn 中文态标点默认映射_逐个精确断言() {
    let mut eng = engine();
    for (input, want) in [
        (',', '\u{FF0C}'),
        ('.', '\u{3002}'),
        ('?', '\u{FF1F}'),
        ('!', '\u{FF01}'),
        (';', '\u{FF1B}'),
        (':', '\u{FF1A}'),
    ] {
        assert_eq!(
            commits(&eng.process_key(LKey::Punct(input))),
            vec![want.to_string()],
            "{input} 应映射为 {want}"
        );
    }
    assert_eq!(
        commits(&eng.process_key(LKey::Punct('\''))),
        vec!["\u{2018}".to_string()]
    );
    assert_eq!(
        commits(&eng.process_key(LKey::Punct('"'))),
        vec!["\u{201C}".to_string()]
    );
}

#[test]
fn 窄化标点开关_保留组合缓冲候选与模式() {
    // Ctrl+. 运行时切换走 set_chinese_punctuation:不重装载词表,
    // 不动缓冲/候选/模式——正在打的字原样保留。
    let mut eng = engine();
    type_str(&mut eng, "ni");
    let cands_before = page_texts(&eng);
    assert!(!cands_before.is_empty());
    eng.set_chinese_punctuation(false);
    assert_eq!(eng.buffer(), "ni", "窄化开关不得清缓冲");
    assert_eq!(page_texts(&eng), cands_before, "候选应原样保留");
    assert_eq!(eng.mode(), Mode::Chinese);
    // 关闭态标点键:有缓冲先提交组合,该键直通应用(ASCII 收尾)。
    let fx = eng.process_key(LKey::Punct(','));
    assert_eq!(commits(&fx), vec!["你".to_string()], "{fx:?}");
    assert!(matches!(fx.last(), Some(Effect::Pass)), "{fx:?}");
    // 切回中文标点:逗号上屏全角。
    eng.set_chinese_punctuation(true);
    assert_eq!(
        commits(&eng.process_key(LKey::Punct(','))),
        vec!["\u{FF0C}".to_string()]
    );
}

#[test]
fn 标点关闭时_ascii引号不得污染开合状态() {
    let mut eng = engine();
    // 中文态出一对双引号,开合位推进。
    assert_eq!(
        commits(&eng.process_key(LKey::Punct('"'))),
        vec!["\u{201C}".to_string()]
    );
    assert_eq!(
        commits(&eng.process_key(LKey::Punct('"'))),
        vec!["\u{201D}".to_string()]
    );
    // 关闭中文标点:ASCII 引号直通——禁用态不得翻转开合位,
    // 否则重开中文标点后首引号方向错乱。
    eng.set_chinese_punctuation(false);
    assert!(is_pass(&eng.process_key(LKey::Punct('"'))));
    assert!(is_pass(&eng.process_key(LKey::Punct('"'))));
    eng.set_chinese_punctuation(true);
    assert_eq!(
        commits(&eng.process_key(LKey::Punct('"'))),
        vec!["\u{201C}".to_string()],
        "开关复位后首引号应是开引号"
    );
}

#[test]
fn 标点开关翻转_返回值即新状态() {
    let mut eng = engine();
    assert!(eng.config().cn_punct, "默认中文标点");
    assert!(!eng.toggle_chinese_punctuation());
    assert!(is_pass(&eng.process_key(LKey::Punct(','))));
    assert!(eng.toggle_chinese_punctuation());
    assert_eq!(
        commits(&eng.process_key(LKey::Punct(','))),
        vec!["\u{FF0C}".to_string()]
    );
}

#[test]
fn set_config改写标点默认_引号配对复位() {
    let mut eng = engine();
    eng.process_key(LKey::Punct('"')); // 吃掉开引号位
    let mut cfg = eng.config().clone();
    cfg.cn_punct = false;
    eng.set_config(cfg.clone());
    assert!(is_pass(&eng.process_key(LKey::Punct('"'))));
    cfg.cn_punct = true;
    eng.set_config(cfg);
    assert_eq!(
        commits(&eng.process_key(LKey::Punct('"'))),
        vec!["\u{201C}".to_string()],
        "标点配置变更后引号应从头配对"
    );
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
fn 数字0_选第10个候选() {
    // 临时词库:音节 "a" 给 12 个单字(拼音通道),页大小 10 → 首页满 10 条。
    let td = TempDir::new();
    for f in [
        "wubi.tsv",
        "pinyin_phrase.tsv",
        "suggestion.tsv",
        "english.tsv",
        "meta.json",
    ] {
        let src = fixtures().join(f);
        if src.exists() {
            std::fs::copy(&src, td.join(f)).unwrap();
        }
    }
    let mut py = String::new();
    for (i, ch) in ['啊', '阿', '吖', '腌', '锕', '嗄', '垯', '怛', '妲', '汏', '垚', '炏']
        .iter()
        .enumerate()
    {
        py.push_str(&format!("a\t{}\t{}\n", ch, 1000 - i as u64));
    }
    std::fs::write(td.join("pinyin_char.tsv"), py).unwrap();
    let mut eng = engine_with_fixtures(&td.path, Config::default());
    type_str(&mut eng, "a");
    let page = page_texts(&eng);
    assert_eq!(page.len(), 10, "页大小默认 10:{page:?}");
    let tenth = page[9].clone();
    // 0 选中第 10 个;页内第 11 个不存在(翻页后另测)。
    let fx = eng.process_key(LKey::Digit(0));
    assert_eq!(commits(&fx), vec![tenth], "0 应选第 10 个候选");
    // 候选不足 10 条:0 无对象,吞键不提交。
    let mut eng2 = engine();
    type_str(&mut eng2, "ni"); // 2 个候选
    let fx = eng2.process_key(LKey::Digit(0));
    assert!(is_consumed(&fx));
    assert_eq!(commits(&fx), Vec::<String>::new());
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
    // 显式 5 条/页(h 开头的音节:ha/hao/hai/han/hei/he → 9 个单字,分两页);
    // 默认页大小 10 的翻页语义相同,不在此重复。
    let mut eng = engine_with(Config {
        page_size: 5,
        ..Config::default()
    });
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
    // 默认(enter_english = temp):临时英文,上屏后保持中文模式。
    assert!(!fx.iter().any(|e| matches!(e, Effect::ModeChanged(_))));
    assert_eq!(eng.mode(), Mode::Chinese);
    assert!(eng.buffer().is_empty());
    // 后续字母仍进中文组词缓冲(单个英文词输入完毕,继续打中文)。
    type_str(&mut eng, "ni");
    assert!(!page_texts(&eng).is_empty());
}

#[test]
fn enter_配置en_上屏并切英文模式() {
    let mut eng = engine_with(Config {
        enter_english: EnCommit::English,
        ..Config::default()
    });
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::Enter);
    assert_eq!(commits(&fx), vec!["nihao".to_string()]);
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::ModeChanged(Mode::English))));
    assert_eq!(eng.mode(), Mode::English);
    assert!(is_pass(&eng.process_key(LKey::Char('n'))));
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
    // "qwertyuioplk" 全程无中文命中("a" 起头的串会在死码处被吞键)。
    type_str(&mut eng, "qwertyuioplk"); // 12 个
    assert_eq!(eng.buffer(), "qwertyuioplk");
    let fx = eng.process_key(LKey::Char('m'));
    assert!(is_consumed(&fx));
    assert_eq!(eng.buffer(), "qwertyuioplk");
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
fn 大写字母_进入大写候选通道() {
    // 2026-09-28 需求:Shift 敲入的大写不再防御性直通,进入大写候选通道
    // (core 内部小写化组词,buf_raw 镜像原形;候选 = 大写 → 首字母大写 → 小写)。
    let mut eng = engine();
    let fx = eng.process_key(LKey::Char('A'));
    assert!(!is_pass(&fx), "大写字母应被消费进入大写候选");
    assert_eq!(eng.buffer(), "a", "内部以小写组词");
    assert_eq!(page_texts(&eng), ["A", "a"]);
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
fn shift_有缓冲上屏并切英文模式() {
    let mut eng = engine();
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::ShiftPress);
    assert_eq!(commits(&fx), vec!["nihao".to_string()]);
    // 新默认(shift_english = en):上屏即进入英文模式,效果流附 mode。
    assert!(fx
        .iter()
        .any(|e| matches!(e, Effect::ModeChanged(Mode::English))));
    assert_eq!(eng.mode(), Mode::English);
    assert!(eng.buffer().is_empty());
    assert!(page_texts(&eng).is_empty());
    // 英文态:后续字母直通。
    assert!(is_pass(&eng.process_key(LKey::Char('n'))));
    // 空缓冲的 Shift 仍由宿主判定单击后切模式;core 只吞键。
    assert!(is_consumed(&eng.process_key(LKey::ShiftPress)));
}

#[test]
fn shift_配置temp_仅上屏保持中文() {
    let mut eng = engine_with(Config {
        shift_english: EnCommit::Temp,
        ..Config::default()
    });
    type_str(&mut eng, "nihao");
    let fx = eng.process_key(LKey::ShiftPress);
    assert_eq!(commits(&fx), vec!["nihao".to_string()]);
    assert!(!fx.iter().any(|e| matches!(e, Effect::ModeChanged(_))));
    assert_eq!(eng.mode(), Mode::Chinese);
    // 后续字母仍进中文组词缓冲。
    type_str(&mut eng, "ni");
    assert!(!page_texts(&eng).is_empty());
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
fn 快速功能键_默认表四条含设置截图() {
    let mut eng = engine(); // Config::default 自带 peizhi/shezhi/jietu/bangzhu
    // 截图(jietu,配置下标 2):注释带配置的截屏热键,候选阶段即可看到
    type_str(&mut eng, "jietu");
    let page = eng.flush_page();
    let pos = page
        .iter()
        .position(|c| c.text == "截图")
        .expect("jietu 应出现截图功能候选");
    assert_eq!(page[pos].kind, CandKind::Action(2));
    assert_eq!(page[pos].comment, "功能键 热键:ctrl+alt+a");
    // 设置(shezhi,配置下标 1):注释保持「功能键」(无快捷键可提示)
    let mut eng = engine();
    type_str(&mut eng, "shezhi");
    let page = eng.flush_page();
    let pos = page
        .iter()
        .position(|c| c.text == "设置")
        .expect("shezhi 应出现设置功能候选");
    assert_eq!(page[pos].kind, CandKind::Action(1));
    assert_eq!(page[pos].comment, "功能键");
}

#[test]
fn 快速功能键_热键注释跟随配置() {
    let mut eng = engine_with(Config {
        shot_hotkey: "ctrl+shift+x".into(),
        ..Config::default()
    });
    type_str(&mut eng, "jietu");
    let page = eng.flush_page();
    let pos = page.iter().position(|c| c.text == "截图").unwrap();
    assert_eq!(page[pos].comment, "功能键 热键:ctrl+shift+x");
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
    // 契约是"不出功能候选":普通输入照常,自动上屏后还可能留联想行
    // (联想默认开)——不得断言候选全空,断言无 Action 候选/效果。
    let fx = type_str(&mut eng, "peizhi");
    assert!(
        eng.flush_page()
            .iter()
            .all(|c| !matches!(c.kind, CandKind::Action(_))),
        "总开关关闭后不得出现功能候选"
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::Action(_))));
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
fn 快速功能键_触发词前缀不被四码顶屏劫持() {
    // 缓冲恰是触发词前缀时,四码顶屏不得先上屏五笔首选——否则 5 字母
    // 触发词永远敲不完(§14 与死码保护同一纪律)。
    let mut eng = engine_action("aaaab", "打开配置", "@settings");
    // "aaaa" 是触发词 aaaab 的前缀:虽五笔唯一命中「恭恭敬敬」也不自动上屏。
    let fx = type_str(&mut eng, "aaaa");
    assert!(commits(&fx).is_empty(), "触发词前缀不得四码上屏:{fx:?}");
    assert_eq!(eng.buffer(), "aaaa");
    // 第 5 键 'b' 续成触发词:不顶屏「恭恭敬敬」,缓冲正常加长。
    let fx = eng.process_key(LKey::Char('b'));
    assert!(commits(&fx).is_empty(), "触发词前缀不得被顶屏:{fx:?}");
    assert_eq!(eng.buffer(), "aaaab");
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("打开配置"));
    // 空格确认触发功能候选。
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

// ======================================================================
// 拼音前缀候选(缺词兜底):consumed>0 的候选只消费缓冲开头若干字节,
// 选中后立即上屏前缀、余下后缀留在组合里继续编辑;边界键无损收尾。
// ======================================================================

/// 缺词夹具:jie→截/接、ping→屏、pin→品;不写词组/五笔/英文表——
/// 词库没有「截屏」正是前缀消费路径的确定性来源。
/// 用户数据(user/pinned/blocked)放 ud/ 子目录,与词库目录分开。
fn jieping_engine() -> (TempDir, Engine) {
    let td = TempDir::new();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\npin\t品\t2000\n",
    )
    .unwrap();
    std::fs::create_dir_all(td.join("ud")).unwrap();
    let eng = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user.tsv")),
            ..Config::default()
        },
    );
    (td, eng)
}

#[test]
fn 前缀候选_三种选词路径都只上屏前缀() {
    for select in 0..3 {
        let (_td, mut eng) = jieping_engine();
        let fx = type_str(&mut eng, "jieping");
        assert!(commits(&fx).is_empty(), "键入过程不得产生上屏:{fx:?}");
        assert_eq!(eng.buffer(), "jieping");
        assert_eq!(
            page_texts(&eng),
            vec!["截".to_string(), "接".to_string()]
        );
        assert_eq!(eng.flush_page()[0].consumed, 3);
        let fx = match select {
            0 => eng.process_key(LKey::Space),
            1 => eng.process_key(LKey::Digit(1)),
            _ => eng.select_candidate(0),
        };
        assert_eq!(
            commits(&fx),
            vec!["截".to_string()],
            "选词路径 {select} 应只上屏前缀"
        );
        assert_eq!(eng.buffer(), "ping", "余下后缀留在组合缓冲");
        assert_eq!(last_preedit(&fx).as_deref(), Some("ping"));
        assert_eq!(
            page_texts(&eng),
            vec!["屏".to_string()],
            "后缀应继续出候选"
        );
        let fx = eng.process_key(LKey::Space);
        assert_eq!(commits(&fx), vec!["屏".to_string()]);
        assert!(eng.buffer().is_empty());
    }
}

#[test]
fn 前缀候选_部分后缀_jiepin与jiepi() {
    // jiepin:完整音节后缀 pin。
    let (_td, mut eng) = jieping_engine();
    type_str(&mut eng, "jiepin");
    assert_eq!(
        page_texts(&eng),
        vec!["截".to_string(), "接".to_string()]
    );
    let fx = eng.process_key(LKey::Digit(2));
    assert_eq!(commits(&fx), vec!["接".to_string()]);
    assert_eq!(eng.buffer(), "pin");
    // pin 是完整音节、也是 ping 的前缀:整音节补全候选 品 与 屏 都合法。
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("品"));
    assert!(page_texts(&eng).contains(&"屏".to_string()));
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["品".to_string()]);

    // jiepi:不完整但语法可续的后缀;满四键(jiep)不得自动上屏。
    let (_td, mut eng) = jieping_engine();
    let fx = type_str(&mut eng, "jiepi");
    assert!(commits(&fx).is_empty(), "前缀候选不得触发四码上屏:{fx:?}");
    assert_eq!(eng.buffer(), "jiepi");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["截".to_string()]);
    assert_eq!(eng.buffer(), "pi");
    eng.process_key(LKey::Char('n'));
    assert_eq!(eng.buffer(), "pin");
    // 同上:完整音节 pin 的补全候选同时含 品/屏。
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("品"));
    assert!(page_texts(&eng).contains(&"屏".to_string()));
}

#[test]
fn 前缀词组候选_nihaoni_消费五字节() {
    let mut eng = engine(); // 共享夹具:你好(ni hao)
    type_str(&mut eng, "nihaoni");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "你好");
    assert_eq!(page[0].consumed, 5, "前缀词组候选只消费 nihao");
    // 不得出现对应尾部音节的孤立单字(它们要丢弃前段才说得通)。
    for tail in ["哈", "好", "海", "哦"] {
        assert!(
            !page.iter().any(|c| c.text == tail),
            "nihaoni 不得出尾部孤字 {tail}:{page:?}"
        );
    }
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["你好".to_string()]);
    assert_eq!(eng.buffer(), "ni");
    assert_eq!(page_texts(&eng), vec!["你".to_string(), "尼".to_string()]);
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["你".to_string()]);
    assert!(eng.buffer().is_empty());
}

#[test]
fn 前缀候选_混合大小写_预编辑保留敲入原形() {
    let (_td, mut eng) = jieping_engine();
    for c in "jIePing".chars() {
        eng.process_key(LKey::Char(c));
    }
    assert_eq!(eng.buffer(), "jieping", "内部按小写组词");
    let fx = eng.select_candidate(0);
    assert_eq!(commits(&fx), vec!["截".to_string()]);
    assert_eq!(eng.buffer(), "ping");
    assert_eq!(
        last_preedit(&fx).as_deref(),
        Some("Ping"),
        "预编辑保留大写敲入原形"
    );
    // Enter 收尾:上屏的也是敲入原形(含大写)。
    let fx = eng.process_key(LKey::Enter);
    assert_eq!(commits(&fx), vec!["Ping".to_string()]);
}

#[test]
fn 前缀选词后_退格Esc回车Shift逐键行为() {
    let (_td, mut eng) = jieping_engine();
    type_str(&mut eng, "jieping");
    eng.process_key(LKey::Space); // 截 → ping
    let fx = eng.process_key(LKey::Backspace);
    assert_eq!(eng.buffer(), "pin");
    assert_eq!(last_preedit(&fx).as_deref(), Some("pin"));
    // pin 音节补全:品 首选,屏 同列(pin 也是 ping 的前缀)。
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("品"));
    assert!(page_texts(&eng).contains(&"屏".to_string()));
    // Esc:只清未选后缀,不得重复上屏已选的前缀。
    let fx = eng.process_key(LKey::Esc);
    assert!(commits(&fx).is_empty(), "Esc 不得再次上屏 截:{fx:?}");
    assert!(eng.buffer().is_empty());

    // Enter:上屏剩余原串(默认临时去向,保持中文模式)。
    type_str(&mut eng, "jieping");
    eng.process_key(LKey::Space);
    let fx = eng.process_key(LKey::Enter);
    assert_eq!(commits(&fx), vec!["ping".to_string()]);
    assert!(eng.buffer().is_empty());
    assert_eq!(eng.mode(), Mode::Chinese);

    // Shift:上屏剩余原串并按配置切英文(默认 shift_english=en)。
    type_str(&mut eng, "jieping");
    eng.process_key(LKey::Space);
    let fx = eng.process_key(LKey::ShiftPress);
    assert_eq!(commits(&fx), vec!["ping".to_string()]);
    assert_eq!(eng.mode(), Mode::English);
}

#[test]
fn 前缀候选_标点收尾_无损拼接原后缀且不学习() {
    let (td, mut eng) = jieping_engine();
    type_str(&mut eng, "jieping"); // 首选是前缀候选「截」
    let fx = eng.process_key(LKey::Punct(','));
    assert_eq!(
        commits(&fx),
        vec!["截ping".to_string(), "\u{FF0C}".to_string()],
        "标点收尾 = 前缀文本 + 原始后缀 + 中文标点:{fx:?}"
    );
    assert!(eng.buffer().is_empty());
    // 拼接串不是用户确认过的词:绝不得入学习库(否则学到「截ping」)。
    eng.flush_user_dict().unwrap();
    let learned = std::fs::read_to_string(td.join("ud/user.tsv")).unwrap_or_default();
    assert!(
        !learned
            .lines()
            .any(|l| l.split('\t').next() == Some("截ping")),
        "拼接串不得进 user.tsv:{learned}"
    );

    // 关闭中文标点:同样无损拼接,标点键放行给应用。
    let (_td2, mut eng2) = jieping_engine();
    eng2.set_chinese_punctuation(false);
    type_str(&mut eng2, "jieping");
    let fx = eng2.process_key(LKey::Punct(','));
    assert_eq!(commits(&fx), vec!["截ping".to_string()], "{fx:?}");
    assert!(matches!(fx.last(), Some(Effect::Pass)), "{fx:?}");
}

#[test]
fn 前缀候选_学习与固定不改变消费量() {
    let (td, mut eng) = jieping_engine();
    type_str(&mut eng, "jieping");
    let fx = eng.select_candidate(0);
    assert_eq!(commits(&fx), vec!["截".to_string()]);
    eng.process_key(LKey::Esc); // 丢弃 ping 余串
    // 学习后「截」标 User 且加成,消费量仍 = 3。
    type_str(&mut eng, "jieping");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "截");
    assert_eq!(page[0].kind, CandKind::User, "学习后应标用户词");
    assert_eq!(page[0].consumed, 3, "学习不得改变消费量");
    eng.process_key(LKey::Esc);

    // 固定「接」到 jieping 首位:消费量同样保持 3,重启引擎依旧。
    type_str(&mut eng, "jieping");
    let idx = page_texts(&eng)
        .iter()
        .position(|t| t == "接")
        .expect("候选应有 接");
    eng.cand_op(idx, CandOp::PinToggle);
    assert_eq!(eng.flush_page()[0].text, "接", "固定后应居首");
    assert_eq!(eng.flush_page()[0].consumed, 3);
    drop(eng);
    let mut eng2 = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user.tsv")),
            ..Config::default()
        },
    );
    type_str(&mut eng2, "jieping");
    let page = eng2.flush_page();
    assert_eq!(page[0].text, "接", "重启后固定仍居首");
    assert_eq!(page[0].consumed, 3, "固定不得改变消费量");
    let fx = eng2.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["接".to_string()]);
    assert_eq!(eng2.buffer(), "ping");
}

#[test]
fn 前缀候选_单页一条翻页后选第二候选仍留后缀() {
    let td = TempDir::new();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\npin\t品\t2000\n",
    )
    .unwrap();
    std::fs::create_dir_all(td.join("ud")).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user.tsv")),
            page_size: 1,
            ..Config::default()
        },
    );
    type_str(&mut eng, "jieping");
    assert_eq!(eng.page_count(), 2);
    assert_eq!(page_texts(&eng), vec!["截".to_string()]);
    eng.process_key(LKey::PageDown);
    assert_eq!(page_texts(&eng), vec!["接".to_string()]);
    let fx = eng.process_key(LKey::Digit(1));
    assert_eq!(commits(&fx), vec!["接".to_string()]);
    assert_eq!(eng.buffer(), "ping");
    assert_eq!(page_texts(&eng), vec!["屏".to_string()]);
}

#[test]
fn 前缀候选屏蔽后_语法可续缓冲仍接收_死码仍吞键() {
    let (td, eng) = jieping_engine();
    drop(eng); // 屏蔽表须在引擎装载前写好,先建一个只为拿到隔离目录
    // 屏蔽 jie 的全部单字:词库缺词/屏蔽 ≠ 拼音非法,可续拼写必须收下。
    std::fs::write(td.join("ud/blocked.tsv"), "截\n接\n").unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user.tsv")),
            ..Config::default()
        },
    );
    type_str(&mut eng, "jieping");
    assert!(
        page_texts(&eng).is_empty(),
        "屏蔽后无候选:{:?}",
        page_texts(&eng)
    );
    assert_eq!(
        eng.buffer(),
        "jieping",
        "候选为空但语法可续,缓冲不得丢键"
    );
    // 真正的死码扩展仍被吞:'q' 不在本夹具的音节前缀里。
    let fx = eng.process_key(LKey::Char('q'));
    assert!(is_consumed(&fx), "{fx:?}");
    assert_eq!(eng.buffer(), "jieping", "jiepingq 属死码应吞键");
}

#[test]
fn 前缀候选_选中后联想不盖未完成组合_结束后上下文仍可联想() {
    let td = TempDir::new();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\npin\t品\t2000\n",
    )
    .unwrap();
    // suggestion 供联想索引:上屏「屏」后应能联想尾巴「幕」。
    std::fs::write(td.join("suggestion.tsv"), "屏幕\t9000\n").unwrap();
    std::fs::create_dir_all(td.join("ud")).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            next_word_prediction: true,
            user_dict: Some(td.join("ud/user.tsv")),
            ..Config::default()
        },
    );
    type_str(&mut eng, "jieping");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["截".to_string()]);
    // 联想/提示不得盖住未完成组合:候选是 ping 的真实候选,preedit 是 ping。
    assert_eq!(eng.buffer(), "ping");
    assert_eq!(page_texts(&eng), vec!["屏".to_string()]);
    // 整词收齐后上下文照常续接:上屏「屏」→ 联想尾巴「幕」。
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["屏".to_string()]);
    assert_eq!(
        page_texts(&eng),
        vec!["幕".to_string()],
        "组合结束后应按上下文继续联想:{fx:?}"
    );
}

// ======================================================================
// 内嵌补充词表(data/pinyin_supplement.tsv)与前缀候选的相互作用。
// 补充表由 dict.rs 装载时合并:仅当 base 词组文件非空、主词库无该词、
// 且标注音节全部存在时才补入;本组用例锁定这三条门槛。
// ======================================================================

#[test]
fn 补充词表_主词组非空且缺词时补入_整词优先于前缀候选() {
    // base 词组文件非空(接屏 jie ping)且不含「截屏」→ 内嵌补充表补入
    // 截屏(jie ping):与 base 词同层整缓冲候选(consumed=0),
    // 词库补齐后前缀兜底自动让位,这正是"缺词兜底"的设计意图。
    let td = TempDir::new();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\n",
    )
    .unwrap();
    std::fs::write(td.join("pinyin_phrase.tsv"), "接屏\tjie ping\t30\n").unwrap();
    std::fs::create_dir_all(td.join("ud")).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user.tsv")),
            ..Config::default()
        },
    );
    type_str(&mut eng, "jieping");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "接屏", "同层内按词频接屏应在前:{page:?}");
    assert_eq!(page[0].consumed, 0);
    let jp = page
        .iter()
        .position(|c| c.text == "截屏")
        .expect("补充词 截屏 应在列:{page:?}");
    assert_eq!(page[jp].consumed, 0, "补充词是整缓冲候选,不是前缀候选");
    // 前缀兜底候选仍在列(层级低于一切整缓冲候选)。
    assert!(
        page.iter().any(|c| c.text == "截" && c.consumed == 3),
        "前缀候选 截 应仍在列:{page:?}"
    );
    // 选中补充词:整词一次上屏(消费整个 jieping)。
    let fx = eng.select_candidate(jp);
    assert_eq!(commits(&fx), vec!["截屏".to_string()], "{fx:?}");
    assert!(eng.buffer().is_empty());
}

#[test]
fn 补充词表_主词库已有该词则不补入_jieping不冒错词() {
    // base 已有「截屏」但标注读音是 jie pin → 补充行(词面相同)被跳过;
    // 打 jieping 不得冒出与缓冲不符的 截屏,它只在 jiepin 下整词命中。
    let td = TempDir::new();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\npin\t品\t2000\n",
    )
    .unwrap();
    std::fs::write(td.join("pinyin_phrase.tsv"), "截屏\tjie pin\t40\n").unwrap();
    std::fs::create_dir_all(td.join("ud")).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user.tsv")),
            ..Config::default()
        },
    );
    type_str(&mut eng, "jieping");
    let page = eng.flush_page();
    assert!(
        !page.iter().any(|c| c.text == "截屏"),
        "jieping 不得出现补充词 截屏(base 已有同名词):{page:?}"
    );
    assert_eq!(
        page_texts(&eng),
        vec!["截".to_string(), "接".to_string()],
        "仍是缺词兜底的前缀候选形态"
    );
    // 对照:jiepin 下 base 词正常整词命中(证明词确已装载)。
    let mut eng2 = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user2.tsv")),
            ..Config::default()
        },
    );
    type_str(&mut eng2, "jiepin");
    assert_eq!(eng2.flush_page()[0].text, "截屏");
    assert_eq!(eng2.flush_page()[0].consumed, 0);
}

#[test]
fn 补充词表_主词组文件缺失或为空不激活() {
    // jieping_engine 不写 pinyin_phrase.tsv:补充表不激活,
    // jieping 保持缺词兜底形态(只有 consumed=3 的前缀候选)。
    let (_td, mut eng) = jieping_engine();
    type_str(&mut eng, "jieping");
    assert_eq!(page_texts(&eng), vec!["截".to_string(), "接".to_string()]);

    // 词组文件存在但为空同理:补充表仍不激活。
    let td = TempDir::new();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\n",
    )
    .unwrap();
    std::fs::write(td.join("pinyin_phrase.tsv"), "").unwrap();
    std::fs::create_dir_all(td.join("ud")).unwrap();
    let mut eng = engine_with_fixtures(
        &td.path,
        Config {
            user_dict: Some(td.join("ud/user.tsv")),
            ..Config::default()
        },
    );
    type_str(&mut eng, "jieping");
    assert_eq!(
        page_texts(&eng),
        vec!["截".to_string(), "接".to_string()],
        "空词组文件不得激活补充表"
    );
}

#[test]
fn 补充词表_不扩展到小词库没有的音节() {
    // 夹具音节表只有 ni/hao:补充词 截屏 需要 jie/ping 音节,
    // 均不存在 → 补充行被过滤,小词库行为与无补充表时一致。
    let td = TempDir::new();
    std::fs::write(td.join("pinyin_char.tsv"), "ni\t你\t6000\nhao\t好\t5000\n")
        .unwrap();
    std::fs::write(td.join("pinyin_phrase.tsv"), "你好\tni hao\t9000\n").unwrap();
    std::fs::create_dir_all(td.join("ud")).unwrap();
    let mk = || {
        engine_with_fixtures(
            &td.path,
            Config {
                user_dict: Some(td.join("ud/user.tsv")),
                ..Config::default()
            },
        )
    };
    let mut eng = mk();
    type_str(&mut eng, "nihao");
    let page = eng.flush_page();
    assert_eq!(page[0].text, "你好");
    assert_eq!(page[0].consumed, 0);
    assert!(
        page.iter().any(|c| c.text == "你" && c.consumed == 2),
        "前缀候选 你 应在列:{page:?}"
    );
    assert!(!page.iter().any(|c| c.text == "截屏"), "{page:?}");
    // 词库没有的音节照旧无候选、原串直通(补充表没有凭空造出 jie/ping)。
    let mut eng = mk();
    type_str(&mut eng, "jieping");
    assert!(page_texts(&eng).is_empty());
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["jieping".to_string()], "{fx:?}");
}
