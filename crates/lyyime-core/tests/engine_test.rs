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
    for f in ["wubi.tsv", "pinyin_char.tsv", "pinyin_phrase.tsv", "suggestion.tsv"] {
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
    assert_eq!(page.first().map(String::as_str), Some("式"), "精确码 aa 应排第一");
    assert!(page.contains(&"恭恭敬敬".to_string()), "aaaa 是 aa 的前缀扩展,应出现");
}

#[test]
fn 五笔精确层_简码满分归一() {
    let mut eng = engine();
    let fx = type_str(&mut eng, "a");
    let top = &eng.flush_page()[0];
    assert_eq!(top.text, "工");
    // 层级词典序:工 处于五笔精确层(60),通道内最高频 → 归一满值 10。
    assert!((top.score - 70.0).abs() < 1e-4, "精确层得分实为 {}", top.score);
    assert!(fx.iter().any(|e| matches!(e, Effect::Preedit(Some(s)) if s == "a")));
}

#[test]
fn 五笔前缀渐进_词组出现() {
    let mut eng = engine();
    type_str(&mut eng, "aaa");
    let page = page_texts(&eng);
    assert!(page.contains(&"恭恭敬敬".to_string()), "打 aaa 应渐进看到 aaaa 的词");
    assert!(!page.contains(&"式".to_string()), "式 的码 aa 不是 aaa 的前缀");
}

#[test]
fn 五笔四码词组_完全命中第一() {
    let mut eng = engine();
    type_str(&mut eng, "aaaa");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("恭恭敬敬"));
}

#[test]
fn 五笔空格顶屏首选() {
    let mut eng = engine();
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
    assert_eq!(eng.flush_page()[0].comment, "ni");
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
    assert_eq!(page.first().map(String::as_str), Some("西安"), "词组全拼应胜过单字");
}

#[test]
fn 拼音末音节不完整_niha_词组命中且被完整切分压制() {
    let mut eng = engine();
    type_str(&mut eng, "niha");
    let page = page_texts(&eng);
    // "niha" 可完整切分为 ni+ha("哈",pinyin_full 层),按修订 §5 层位高于
    // 不完整尾音节词组"你好"(pinyin_partial 层);但不完整查询必须仍然命中。
    assert_eq!(page.first().map(String::as_str), Some("哈"));
    assert!(page.contains(&"你好".to_string()), "缺尾词组你好应命中:{page:?}");
    let nh = page.iter().position(|t| t == "你好").unwrap();
    let h = page.iter().position(|t| *t == "好").unwrap();
    assert!(nh < h, "缺尾词组你好(音节多)应排在单字好之前");
    assert!(page.contains(&"海".to_string()), "不完整片段 h 的单字(hai 海)也应出现");
}

#[test]
fn 拼音简拼_nh_命中词组且低权重无单字() {
    let mut eng = engine();
    type_str(&mut eng, "nh");
    let page = page_texts(&eng);
    assert_eq!(page.first().map(String::as_str), Some("你好"));
    assert!(page.contains(&"你好吗".to_string()), "渐进简拼应给出更长词组");
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
    let mut eng = engine();
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
    assert!(!page.contains(&"nice".to_string()), "中文命中时英文通道应关闭");
}

#[test]
fn 英文自动直通_空格上屏原词() {
    // 修订 §5.C:有中文命中时,top-500 完整英文词仍直通原词。
    // "he" 有拼音命中(和/喝),也是 top-500 英文词 → 空格直通 he。
    let mut eng = engine();
    type_str(&mut eng, "he");
    let texts = page_texts(&eng);
    assert!(texts.contains(&"和".to_string()), "中文命中在列:{texts:?}");
    assert!(texts.contains(&"he".to_string()), "英文 with_cn 层在列:{texts:?}");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["he".to_string()]);
}

#[test]
fn 英文自动直通_关闭时空格顶屏首选() {
    let mut eng = engine_with(Config { mixed_auto_commit: false, ..Config::default() });
    type_str(&mut eng, "he");
    let fx = eng.process_key(LKey::Space);
    assert_eq!(commits(&fx), vec!["和".to_string()], "关闭直通后顶屏中文首选");
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
    let mut rows = vec!["thextra	50000".to_string(), "thex	40000".to_string()];
    for i in 0..1996 {
        rows.push(format!("zz{:04}	{}", i, 30000 - i as u64)); // 第 3..=1998 名
    }
    rows.push("qword	1000".into()); // 第 1999 名
    rows.push("qworx	999".into()); // 第 2000 名(恰好达到 en_freq_top_n)
    rows.push("qwozz	998".into()); // 第 2001 名(超限)
    std::fs::write(td.join("english.tsv"), rows.join("\n")).unwrap();
    engine_with_fixtures(&td.path, Config::default())
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
    let mut eng = en_rank_fixture(&td);
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
    assert_eq!(commits(&eng2.process_key(LKey::Punct('"'))), vec!["\u{201C}".to_string()]);
    assert_eq!(commits(&eng2.process_key(LKey::Punct('"'))), vec!["\u{201D}".to_string()]);
}

#[test]
fn 未映射标点_直通放行() {
    let mut eng = engine();
    assert!(is_pass(&eng.process_key(LKey::Punct('/'))));
    assert!(is_pass(&eng.process_key(LKey::Punct('@'))));
}

#[test]
fn 关闭中文标点_直通放行() {
    let mut eng = engine_with(Config { cn_punct: false, ..Config::default() });
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
    assert_eq!(page2.first().map(String::as_str), Some("汉"), "第二页应以汉开头");
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
    assert!(!td.join("user.tsv").exists(), "不足 64 次且未 flush,不应落盘");
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
    assert!(text.lines().any(|l| l.starts_with("你\t2\t")), "实为:{text}");
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
    assert!(text.lines().any(|l| l.starts_with("工\t64\t")), "64 次应触发 write-behind");
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
    assert_eq!(page_texts(&eng).first().map(String::as_str), Some("到"), "无加成,频次高者在前");
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
    assert_eq!(page_texts(&eng2).first().map(String::as_str), Some("稻"), "重启后应从 user.tsv 恢复加成");
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
    assert!(err.message().contains("损坏"), "应提示配置损坏:{}", err.message());
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
    eng.set_config(Config { page_size: 3, mode: Mode::English, ..Config::default() });
    assert_eq!(eng.config().page_size, 3);
    assert_eq!(eng.mode(), Mode::English, "config.mode 是启动默认模式,set_config 会重置");
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
    std::fs::write(
        td.join("wubi.tsv"),
        "z\t甲\t100\nz\t乙\t100\n",
    )
    .unwrap();
    std::fs::write(td.join("suggestion.tsv"), "乙\t5000\n甲\t10\n").unwrap();
    let mut eng = Engine::new(&td.path).unwrap();
    type_str(&mut eng, "z");
    let page = page_texts(&eng);
    assert_eq!(page, vec!["乙".to_string(), "甲".to_string()], "同分时 suggestion 高者在前");
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
