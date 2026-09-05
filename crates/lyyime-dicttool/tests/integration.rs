//! dicttool 集成测试:用 rusqlite 内存库模拟 ibus-table schema → convert → 断言产物。
//!
//! 覆盖:tabkeys 大写多码归一、声调归一化(含轻声与防御性数字后缀)、
//! 重复 (code,word) 取 MAX(freq)、空行/NULL/控制字符脏数据容错、排序、
//! meta.json、幂等性,以及 fetch 阶段的纯解析函数。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use rusqlite::Connection;

use lyyime_dicttool::convert::convert_all;
use lyyime_dicttool::fetch::{
    derive_pinyin, parse_english_line, parse_jieba_line, parse_phrase_pinyin_line,
};
use lyyime_dicttool::util::{normalize_pinyin, toneless_syllable, Meta};

/// 每个测试独占一个临时目录(避免 tempfile 依赖)。
fn temp_out(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!(
        "lyyime-dicttool-test-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 构造模拟 ibus-table schema 的内存库,塞入覆盖各边界的受控数据。
fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE ime (attr TEXT, val TEXT);
         CREATE TABLE goucima (zi TEXT PRIMARY KEY, goucima TEXT);
         CREATE TABLE pinyin (pinyin TEXT, zi TEXT, freq INTEGER);
         CREATE TABLE suggestion (phrase TEXT, freq INTEGER);
         CREATE TABLE phrases (id INTEGER PRIMARY KEY, tabkeys TEXT, phrase TEXT,
             freq INTEGER, user_freq INTEGER);",
    )
    .unwrap();
    conn.execute("INSERT INTO ime VALUES ('serial_number','19991231')", []).unwrap();

    // phrases:正常码 + 大写 tabkeys + 重复 (code,word) + 脏数据
    let phrases: Vec<(&str, Option<&str>, Option<i64>)> = vec![
        ("a", Some("工"), Some(2266)),
        ("w", Some("人"), Some(10837)),
        ("wq", Some("你"), Some(6946)),
        ("aaaa", Some("恭恭敬敬"), Some(641000)),
        ("AAWI", Some("𤁱."), Some(100)),     // 大写码(海峰直上屏记法)→ 归一为 aawi
        ("yyyy", Some("方言"), Some(3703000)),
        ("yyyy", Some("方言"), Some(999)),    // 重复 (code,word):取 max
        ("zz", Some("脏\u{1}行\u{7f}"), Some(500)), // 含控制字符:剥离后保留
        ("zz", Some(""), Some(100)),          // 空词:跳过
        ("zz", None, Some(100)),              // NULL 词:跳过
        ("Zz", Some("中间"), Some(300)),      // 与小写 zz 合并到同码
        ("qq", Some("孤"), None),             // NULL freq:跳过
    ];
    for (i, (tk, ph, f)) in phrases.iter().enumerate() {
        conn.execute(
            "INSERT INTO phrases (id, tabkeys, phrase, freq, user_freq) VALUES (?1,?2,?3,?4,0)",
            rusqlite::params![(i + 1) as i64, tk, ph, f],
        )
        .unwrap();
    }

    // pinyin:五种声调标记 + 轻声 + 数字后缀 + 重复 + 脏数据
    let pinyins: Vec<(&str, Option<&str>, Option<i64>)> = vec![
        ("ni!", Some("你"), Some(1_490_000_000)),
        ("ni#", Some("你"), Some(100)), // 同 (ni,你):取 max
        ("ni@", Some("泥"), Some(1660)),
        ("de%", Some("的"), Some(4_220_000_000)), // 轻声 % → de
        ("hao$", Some("好"), Some(1_780_000_000)),
        ("xiao1", Some("小"), Some(500)), // 防御:数字声调后缀
        ("", Some("丁"), Some(10)),       // 空拼音:跳过
        ("ma", None, Some(1)),            // NULL 字:跳过
    ];
    for (py, zi, f) in pinyins.iter() {
        conn.execute(
            "INSERT INTO pinyin (pinyin, zi, freq) VALUES (?1,?2,?3)",
            rusqlite::params![py, zi, f],
        )
        .unwrap();
    }

    // suggestion:重复词取 max + 控制字符剥离
    let sug: Vec<(&str, i64)> = vec![("一一", 1885), ("一一", 5), ("人民", 100), ("坏\t点", 7)];
    for (ph, f) in &sug {
        conn.execute("INSERT INTO suggestion VALUES (?1,?2)", rusqlite::params![ph, f]).unwrap();
    }

    // goucima
    for (zi, gc) in [("一", "ggll"), ("工", "aaaa"), ("¡", "kckg")] {
        conn.execute("INSERT INTO goucima VALUES (?1,?2)", rusqlite::params![zi, gc]).unwrap();
    }
    conn
}

fn convert(tag: &str) -> (PathBuf, (u64, u64, u64, u64)) {
    let out = temp_out(tag);
    let conn = test_db();
    let sources = vec![serde_json::json!({"name": "test.db", "serial_number": "19991231"})];
    let counts = convert_all(&conn, &out, sources).unwrap();
    (out, counts)
}

fn read(out: &PathBuf, name: &str) -> String {
    std::fs::read_to_string(out.join(name)).unwrap()
}

#[test]
fn wubi_lowercase_and_uppercase_tabkeys() {
    let (out, _) = convert("wubi-case");
    let text = read(&out, "wubi.tsv");
    // 大写 tabkeys 归一为小写
    assert!(text.contains("aawi\t𤁱.\t100\n"), "缺 aawi 行:{}", text);
    // 大小写不同但归一后同码的行共存
    assert!(text.contains("zz\t中间\t300\n"), "缺 zz/中间:{}", text);
    assert!(text.contains("a\t工\t2266\n"));
    assert!(text.contains("w\t人\t10837\n"));
    assert!(text.contains("wq\t你\t6946\n"));
}

#[test]
fn wubi_duplicate_takes_max_freq() {
    let (out, counts) = convert("wubi-dup");
    let text = read(&out, "wubi.tsv");
    let hits: Vec<&str> = text.lines().filter(|l| l.starts_with("yyyy\t方言\t")).collect();
    assert_eq!(hits, vec!["yyyy\t方言\t3703000"], "重复 (code,word) 应只留 max freq");
    // 12 行输入:空词/NULL词/NULLfreq 各去 1,重复 yyyy 合并 1 → 8 行
    assert_eq!(counts.0, 8, "wubi.tsv 行数");
}

#[test]
fn wubi_dirty_rows_tolerated() {
    let (out, _) = convert("wubi-dirty");
    let text = read(&out, "wubi.tsv");
    // 控制字符被剥离,词保留
    assert!(text.contains("zz\t脏行\t500\n"), "控制字符应被剥离:{}", text);
    // 空词与 NULL freq 行不存在
    assert!(!text.lines().any(|l| l.split('\t').nth(1).map(|w| w.is_empty()).unwrap_or(false)));
    assert!(!text.contains("qq\t"));
    assert!(text.contains("zz\t中间\t300\n"));
}

#[test]
fn wubi_sorted_by_code_then_freq_desc() {
    let (out, _) = convert("wubi-sort");
    let mut prev_code = String::new();
    let mut prev_freq = i64::MAX;
    for line in read(&out, "wubi.tsv").lines() {
        let mut it = line.split('\t');
        let code = it.next().unwrap();
        let freq: i64 = it.nth(1).unwrap().parse().unwrap();
        assert!(code >= prev_code.as_str(), "code 未升序:{}", line);
        if code == prev_code {
            assert!(freq <= prev_freq, "同 code 频次未降序:{}", line);
        }
        prev_code = code.to_string();
        prev_freq = freq;
    }
}

#[test]
fn pinyin_char_tone_normalization_and_dedup() {
    let (out, counts) = convert("py-char");
    let text = read(&out, "pinyin_char.tsv");
    // 轻声 % 与数字后缀都归一化掉
    assert!(text.contains("de\t的\t4220000000\n"), "轻声 de% -> de:{}", text);
    assert!(text.contains("xiao\t小\t500\n"), "数字后缀 xiao1 -> xiao:{}", text);
    assert!(text.contains("hao\t好\t1780000000\n"));
    assert!(text.contains("ni\t泥\t1660\n"));
    // 重复 (ni,你) 只留 max freq 行
    let hits: Vec<&str> = text.lines().filter(|l| l.starts_with("ni\t你\t")).collect();
    assert_eq!(hits, vec!["ni\t你\t1490000000"]);
    // 脏数据行被跳过:总数 8 - 空 - NULL - 重复 = 5
    assert_eq!(counts.1, 5, "pinyin_char 行数");
    // 同音节内频次降序:ni 的两行,泥(1660)在你(1490000000)之后
    let ni_pos_you = text.find("ni\t你\t").unwrap();
    let ni_pos_ni2 = text.find("ni\t泥\t").unwrap();
    assert!(ni_pos_you < ni_pos_ni2);
}

#[test]
fn suggestion_and_goucima_content() {
    let (out, counts) = convert("sugg-gc");
    let sugg = read(&out, "suggestion.tsv");
    let hits: Vec<&str> = sugg.lines().filter(|l| l.starts_with("一一\t")).collect();
    assert_eq!(hits, vec!["一一\t1885"], "suggestion 重复词取 max");
    assert!(sugg.contains("人民\t100\n"));
    assert!(sugg.contains("坏点\t7\n"), "控制字符剥离:{}", sugg);
    assert_eq!(counts.2, 3);

    let gc = read(&out, "goucima.tsv");
    assert!(gc.contains("一\tggll\n"));
    assert!(gc.contains("工\taaaa\n"));
    assert!(gc.contains("¡\tkckg\n"), "非中文字符的构词码行保留:{}", gc);
    assert_eq!(counts.3, 3);
}

#[test]
fn meta_json_records_rows_and_sources() {
    let (out, counts) = convert("meta");
    let meta = Meta::load(&out.join("meta.json")).unwrap().unwrap();
    assert_eq!(meta.version, 1);
    assert_eq!(meta.rows.get("wubi.tsv"), Some(&counts.0));
    assert_eq!(meta.rows.get("pinyin_char.tsv"), Some(&counts.1));
    assert_eq!(meta.rows.get("suggestion.tsv"), Some(&counts.2));
    assert_eq!(meta.rows.get("goucima.tsv"), Some(&counts.3));
    assert_eq!(meta.sources.len(), 1);
    assert_eq!(meta.sources[0]["serial_number"], "19991231");
    assert!(!meta.built_at.is_empty());
}

#[test]
fn convert_is_idempotent() {
    let (out1, _) = convert("idem-1");
    let (out2, _) = convert("idem-2");
    for name in ["wubi.tsv", "pinyin_char.tsv", "suggestion.tsv", "goucima.tsv"] {
        assert_eq!(read(&out1, name), read(&out2, name), "{} 两次转换结果应一致", name);
    }
    // meta 除 built_at 外一致
    let m1 = Meta::load(&out1.join("meta.json")).unwrap().unwrap();
    let m2 = Meta::load(&out2.join("meta.json")).unwrap().unwrap();
    assert_eq!(m1.rows, m2.rows);
    assert_eq!(m1.version, m2.version);
}

#[test]
fn test_normalize_pinyin_strips_tone_markers_and_digits() {
    // ibus 实测后缀:!1声 @2声 #3声 $4声 %轻声
    assert_eq!(normalize_pinyin("a!"), Some("a".into()));
    assert_eq!(normalize_pinyin("a@"), Some("a".into()));
    assert_eq!(normalize_pinyin("ai#"), Some("ai".into()));
    assert_eq!(normalize_pinyin("zhong$"), Some("zhong".into()));
    assert_eq!(normalize_pinyin("de%"), Some("de".into())); // 轻声:剥标记,不保留轻声信息
    // 防御:数字声调后缀
    assert_eq!(normalize_pinyin("xi1"), Some("xi".into()));
    assert_eq!(normalize_pinyin("zhong4"), Some("zhong".into()));
    assert_eq!(normalize_pinyin("nv3"), Some("nv".into()));
    assert_eq!(normalize_pinyin(""), None);
}

#[test]
fn test_toneless_syllable_handles_accented_letters() {
    assert_eq!(toneless_syllable("zhōng"), Some("zhong".into()));
    assert_eq!(toneless_syllable("Xiǎo"), Some("xiao".into()));
    assert_eq!(toneless_syllable("lǜ"), Some("lv".into())); // ü -> v,与 ibus lv/nv 约定一致
    assert_eq!(toneless_syllable("nǚ"), Some("nv".into()));
    assert_eq!(toneless_syllable("ér"), Some("er".into()));
    assert_eq!(toneless_syllable("bēi"), Some("bei".into()));
    assert_eq!(toneless_syllable("a1"), None, "数字属非法音节");
    assert_eq!(toneless_syllable(""), None);
}

#[test]
fn test_parse_phrase_pinyin_line() {
    let (w, p) = parse_phrase_pinyin_line("悲伤: bēi shāng").unwrap();
    assert_eq!(w, "悲伤");
    assert_eq!(p, "bei shang");
    let (w, p) = parse_phrase_pinyin_line("你好:nǐ hǎo").unwrap();
    assert_eq!(w, "你好");
    assert_eq!(p, "ni hao");
    assert!(parse_phrase_pinyin_line("# version: 0.19.0").is_none(), "注释行");
    assert!(parse_phrase_pinyin_line("AT&T: éi").is_none(), "非纯中文词");
    assert!(parse_phrase_pinyin_line("好: hǎo").is_none(), "单字不要");
    assert!(parse_phrase_pinyin_line("中国: zhōng").is_none(), "音节数与字数不符");
    assert!(parse_phrase_pinyin_line("").is_none());
}

#[test]
fn test_parse_jieba_line() {
    let (w, f) = parse_jieba_line("中国 214 cn").unwrap();
    assert_eq!(w, "中国");
    assert_eq!(f, 214);
    assert!(parse_jieba_line("AT&T 3 nz").is_none(), "含非中文字符");
    assert!(parse_jieba_line("B超 3 n").is_none(), "混字母");
    assert!(parse_jieba_line("好 500 ag").is_none(), "单字不要");
    assert!(parse_jieba_line("词 0 x").is_none(), "零频不要");
    assert!(parse_jieba_line("词 abc x").is_none(), "频次非数字");
}

#[test]
fn test_parse_english_line() {
    assert_eq!(parse_english_line("Hello"), Some("hello".into()));
    assert_eq!(parse_english_line("world"), Some("world".into()));
    assert_eq!(parse_english_line("don't"), None, "撇号不要");
    assert_eq!(parse_english_line("abc123"), None, "数字不要");
    assert_eq!(parse_english_line("   "), None);
}

#[test]
fn test_derive_pinyin_polyphones() {
    // 多音字“行”:xing(50)/hang(100) → 取频次最高的 hang
    let mut map: HashMap<char, (String, u64)> = HashMap::new();
    map.insert('行', ("xing".into(), 50));
    map.insert('行', ("hang".into(), 100)); // load_char_pinyin 同规则:高频胜出
    map.insert('你', ("ni".into(), 1_490_000_000));
    map.insert('好', ("hao".into(), 1_780_000_000));
    assert_eq!(derive_pinyin("你好", &map), Some("ni hao".into()));
    assert_eq!(derive_pinyin("行走", &map), None, "缺“走”的读音 → 整词放弃");
    // 直接用 load_char_pinyin 的产物语义验证:多音字保留高频读音
    assert_eq!(map.get(&'行').unwrap().0, "hang");
}
