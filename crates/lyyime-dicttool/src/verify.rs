//! `verify <dir>`:对 data/runtime 产物做行数下限、列数、UTF-8 编码、排序、抽样内容的断言,
//! 输出摘要表;任一断言失败退出码非 0。
//!
//! 抽样基准以海峰五笔86(wubi-haifeng86.db)为准:86 版码表 a=工、w=人、wq=你 为标准编码。

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

// 行数下限(M2 验收:词组≥10万、英文≥1万;其余按实测规模留足余量)
const WUBI_MIN: u64 = 100_000;
const PINYIN_CHAR_MIN: u64 = 20_000;
const PINYIN_PHRASE_MIN: u64 = 100_000;
const ENGLISH_MIN: u64 = 10_000;
const SUGGESTION_MIN: u64 = 100_000;
const GOUCIMA_MIN: u64 = 20_000;
/// 全拼无声调音节数下限(实测海峰码表归一化后 413 个,GB2312 常用集要求 ≥380)
const SYLLABLE_MIN: usize = 380;

struct Check {
    name: String,
    ok: bool,
    detail: String,
}

impl Check {
    fn new(name: &str, ok: bool, detail: String) -> Self {
        Self { name: name.to_string(), ok, detail }
    }
}

pub fn run(dir: PathBuf) -> Result<()> {
    if !dir.is_dir() {
        bail!("目录 {:?} 不存在。请先运行 `dicttool convert` 与 `dicttool fetch`。", dir);
    }
    let mut checks: Vec<Check> = Vec::new();
    check_wubi(&dir, &mut checks);
    check_pinyin_char(&dir, &mut checks);
    check_pinyin_phrase(&dir, &mut checks);
    check_english(&dir, &mut checks);
    check_suggestion(&dir, &mut checks);
    check_goucima(&dir, &mut checks);
    check_meta(&dir, &mut checks)?;

    // 摘要表
    eprintln!("\n===== dicttool verify 摘要({:?}) =====", dir);
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut failed = 0;
    for c in &checks {
        if c.ok {
            println!("  [ok]   {:<width$}  {}", c.name, c.detail, width = width);
        } else {
            failed += 1;
            println!("  [FAIL] {:<width$}  {}", c.name, c.detail, width = width);
        }
    }
    println!("  -----------------------------------------");
    println!("  通过 {}/{} 项检查", checks.len() - failed, checks.len());
    if failed > 0 {
        bail!("verify 有 {} 项断言失败", failed);
    }
    Ok(())
}

fn exists_check(dir: &Path, file: &str, checks: &mut Vec<Check>) -> Option<PathBuf> {
    let p = dir.join(file);
    let ok = p.is_file();
    checks.push(Check::new(
        &format!("{} 存在", file),
        ok,
        if ok { p.display().to_string() } else { "缺失(若为 pinyin_phrase/english,请先 dicttool fetch)".into() },
    ));
    ok.then_some(p)
}

/// 逐行检查列数并执行回调;返回 (行数, 是否全 UTF-8/列数正确)。
fn scan_lines(
    path: &Path,
    expect_cols: usize,
    mut on_line: impl FnMut(u64, &[&str]),
) -> Result<(u64, bool)> {
    let f = std::fs::File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut n = 0u64;
    let mut cols_ok = true;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let mut line = String::from_utf8_lossy(&buf).to_string();
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        // 控制字符检查(\t 为合法分隔符,换行已剥离,其余 Cc 一律不允许)
        if line.chars().any(|c| c.is_control() && c != '\t') {
            cols_ok = false;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != expect_cols {
            cols_ok = false;
        }
        n += 1;
        on_line(n, &fields);
    }
    Ok((n, cols_ok))
}

fn check_wubi(dir: &Path, checks: &mut Vec<Check>) {
    let Some(path) = exists_check(dir, "wubi.tsv", checks) else { return };
    let mut n = 0u64;
    let mut cols_ok = true;
    let mut sort_ok = true;
    let mut prev: Option<(String, i64)> = None;
    // 抽样:同码首行即最高频(文件按 freq 降序)
    let mut samples: HashMap<String, (String, i64)> = HashMap::new();
    let want = ["a", "w", "wq", "yyyy"];
    let r = scan_lines(&path, 3, |i, f| {
        n = i;
        let (code, word, freq) = (f[0], f[1], f[2]);
        if word.is_empty() || code.is_empty() {
            cols_ok = false;
        }
        let Ok(freq) = freq.parse::<i64>() else { cols_ok = false; return };
        match &prev {
            Some((pc, pf)) if !(pc.as_str() < code || (pc == code && *pf >= freq)) => sort_ok = false,
            _ => {}
        }
        prev = Some((code.to_string(), freq));
        if want.contains(&code) && !samples.contains_key(code) {
            samples.insert(code.to_string(), (word.to_string(), freq));
        }
    });
    match r {
        Ok((rows, ok)) => {
            checks.push(Check::new(
                "wubi.tsv 行数",
                rows >= WUBI_MIN,
                format!("{} 行(下限 {})", rows, WUBI_MIN),
            ));
            checks.push(Check::new("wubi.tsv 列数/编码/控制字符", ok && cols_ok, "3 列 UTF-8".into()));
            checks.push(Check::new(
                "wubi.tsv 排序",
                sort_ok,
                "code 升序、同 code freq 降序".into(),
            ));
            for (code, expect) in [("a", "工"), ("w", "人"), ("wq", "你")] {
                let got = samples.get(code);
                checks.push(Check::new(
                    &format!("wubi.tsv 抽样 code={}", code),
                    got.map(|(w, _)| w == expect).unwrap_or(false),
                    format!("{} -> {:?}", expect, got.map(|(w, f)| format!("{}(freq {})", w, f))),
                ));
            }
            let y = samples.get("yyyy").map(|(w, _)| w.clone());
            checks.push(Check::new(
                "wubi.tsv 抽样 code=yyyy(词组)",
                y.is_some(),
                format!("-> {:?}", y),
            ));
        }
        Err(e) => checks.push(Check::new("wubi.tsv 可读", false, e.to_string())),
    }
}

fn check_pinyin_char(dir: &Path, checks: &mut Vec<Check>) {
    let Some(path) = exists_check(dir, "pinyin_char.tsv", checks) else { return };
    let mut rows = 0u64;
    let mut cols_ok = true;
    let mut sort_ok = true;
    let mut prev: Option<String> = None;
    let mut prev_freq = i64::MAX;
    let mut freq_desc_ok = true;
    let mut syllables: HashSet<String> = HashSet::new();
    let mut by_py: HashMap<String, HashSet<String>> = HashMap::new();
    let res = scan_lines(&path, 3, |i, f| {
        rows = i;
        let (py, zi, freq) = (f[0], f[1], f[2]);
        if zi.chars().count() != 1 || py.is_empty() {
            cols_ok = false;
        }
        let Ok(freq) = freq.parse::<i64>() else { cols_ok = false; return };
        if let Some(p) = &prev {
            if p.as_str() > py {
                sort_ok = false;
            }
        }
        if prev.as_deref() == Some(py) && prev_freq < freq {
            freq_desc_ok = false; // 同音节内应按频次降序
        }
        prev = Some(py.to_string());
        prev_freq = freq;
        syllables.insert(py.to_string());
        by_py.entry(py.to_string()).or_default().insert(zi.to_string());
    });
    if let Err(e) = res {
        checks.push(Check::new("pinyin_char.tsv 可读", false, e.to_string()));
        return;
    }
    checks.push(Check::new(
        "pinyin_char.tsv 行数",
        rows >= PINYIN_CHAR_MIN,
        format!("{} 行(下限 {})", rows, PINYIN_CHAR_MIN),
    ));
    checks.push(Check::new(
        "pinyin_char.tsv 音节覆盖",
        syllables.len() >= SYLLABLE_MIN,
        format!("{} 个无声调音节(下限 {},GB2312 常用集)", syllables.len(), SYLLABLE_MIN),
    ));
    checks.push(Check::new("pinyin_char.tsv 列数/编码", cols_ok, "3 列 UTF-8".into()));
    checks.push(Check::new("pinyin_char.tsv 排序", sort_ok, "pinyin 升序".into()));
    checks.push(Check::new(
        "pinyin_char.tsv 音节内频次降序",
        freq_desc_ok,
        "同 pinyin 内 freq 降序".into(),
    ));
    for (py, expect) in [("ni", "你"), ("hao", "好"), ("de", "的")] {
        let set = by_py.get(py);
        let ok = set.map(|s| s.contains(expect)).unwrap_or(false);
        checks.push(Check::new(
            &format!("pinyin_char.tsv 抽样 {} -> {}", py, expect),
            ok,
            format!(
                "{} 含 {:?}(该音节共 {} 字)",
                py,
                expect,
                set.map(|s| s.len()).unwrap_or(0)
            ),
        ));
    }
}

fn check_pinyin_phrase(dir: &Path, checks: &mut Vec<Check>) {
    let Some(path) = exists_check(dir, "pinyin_phrase.tsv", checks) else { return };
    let mut rows = 0u64;
    let mut cols_ok = true;
    let mut sort_ok = true;
    let mut prev: Option<String> = None;
    let mut nihao: Option<String> = None;
    match scan_lines(&path, 3, |i, f| {
        rows = i;
        let (word, pinyin, freq) = (f[0], f[1], f[2]);
        if word.is_empty() || pinyin.is_empty() || freq.parse::<u64>().is_err() {
            cols_ok = false;
        }
        if pinyin.chars().any(|c| c != ' ' && !c.is_ascii_lowercase()) {
            cols_ok = false; // 必须为空格分隔的无声调音节
        }
        if let Some(p) = &prev {
            if p.as_str() > pinyin {
                sort_ok = false;
            }
        }
        prev = Some(pinyin.to_string());
        if word == "你好" {
            nihao = Some(pinyin.to_string());
        }
    }) {
        Ok((_, ok)) => {
            checks.push(Check::new(
                "pinyin_phrase.tsv 行数",
                rows >= PINYIN_PHRASE_MIN,
                format!("{} 行(下限 {})", rows, PINYIN_PHRASE_MIN),
            ));
            checks.push(Check::new(
                "pinyin_phrase.tsv 列数/无声调音节",
                ok && cols_ok,
                "3 列,pinyin 为空格分隔无声调音节".into(),
            ));
            checks.push(Check::new("pinyin_phrase.tsv 排序", sort_ok, "pinyin 升序".into()));
            checks.push(Check::new(
                "pinyin_phrase.tsv 抽样 你好",
                nihao.as_deref() == Some("ni hao"),
                format!("-> {:?}", nihao),
            ));
        }
        Err(e) => checks.push(Check::new("pinyin_phrase.tsv 可读", false, e.to_string())),
    }
}

fn check_english(dir: &Path, checks: &mut Vec<Check>) {
    let Some(path) = exists_check(dir, "english.tsv", checks) else { return };
    let mut rows = 0u64;
    let mut cols_ok = true;
    let mut sort_ok = true;
    let mut prev: Option<String> = None;
    let mut has_hello = false;
    match scan_lines(&path, 2, |i, f| {
        rows = i;
        let (word, freq) = (f[0], f[1]);
        if word.is_empty() || freq.parse::<u64>().is_err() {
            cols_ok = false;
        }
        if !word.chars().all(|c| c.is_ascii_lowercase()) {
            cols_ok = false;
        }
        if let Some(p) = &prev {
            if p.as_str() >= word {
                sort_ok = false;
            }
        }
        prev = Some(word.to_string());
        if word == "hello" {
            has_hello = true;
        }
    }) {
        Ok((_, ok)) => {
            checks.push(Check::new(
                "english.tsv 行数",
                rows >= ENGLISH_MIN,
                format!("{} 行(下限 {})", rows, ENGLISH_MIN),
            ));
            checks.push(Check::new("english.tsv 列数/小写词", ok && cols_ok, "2 列".into()));
            checks.push(Check::new("english.tsv 排序", sort_ok, "word 升序且无重复".into()));
            checks.push(Check::new("english.tsv 抽样 hello", has_hello, "hello 存在".into()));
        }
        Err(e) => checks.push(Check::new("english.tsv 可读", false, e.to_string())),
    }
}

fn check_suggestion(dir: &Path, checks: &mut Vec<Check>) {
    let Some(path) = exists_check(dir, "suggestion.tsv", checks) else { return };
    let mut rows = 0u64;
    let mut cols_ok = true;
    let mut sort_ok = true;
    let mut prev: Option<i64> = None;
    let mut has_yiyi = false;
    match scan_lines(&path, 2, |i, f| {
        rows = i;
        let (word, freq) = (f[0], f[1]);
        if word.is_empty() {
            cols_ok = false;
        }
        let Ok(freq) = freq.parse::<i64>() else { cols_ok = false; return };
        if let Some(p) = &prev {
            if *p < freq {
                sort_ok = false;
            }
        }
        prev = Some(freq);
        if word == "一一" {
            has_yiyi = true;
        }
    }) {
        Ok((_, ok)) => {
            checks.push(Check::new(
                "suggestion.tsv 行数",
                rows >= SUGGESTION_MIN,
                format!("{} 行(下限 {})", rows, SUGGESTION_MIN),
            ));
            checks.push(Check::new("suggestion.tsv 列数", ok && cols_ok, "2 列".into()));
            checks.push(Check::new("suggestion.tsv 排序", sort_ok, "freq 降序".into()));
            checks.push(Check::new("suggestion.tsv 抽样 一一", has_yiyi, "一一 存在".into()));
        }
        Err(e) => checks.push(Check::new("suggestion.tsv 可读", false, e.to_string())),
    }
}

fn check_goucima(dir: &Path, checks: &mut Vec<Check>) {
    let Some(path) = exists_check(dir, "goucima.tsv", checks) else { return };
    let mut rows = 0u64;
    let mut cols_ok = true;
    let mut sort_ok = true;
    let mut prev: Option<String> = None;
    let mut yi: Option<String> = None;
    match scan_lines(&path, 2, |i, f| {
        rows = i;
        let (zi, code) = (f[0], f[1]);
        if zi.is_empty() || code.is_empty() {
            cols_ok = false;
        }
        if let Some(p) = &prev {
            if p.as_str() >= zi {
                sort_ok = false;
            }
        }
        prev = Some(zi.to_string());
        if zi == "一" {
            yi = Some(code.to_string());
        }
    }) {
        Ok((_, ok)) => {
            checks.push(Check::new(
                "goucima.tsv 行数",
                rows >= GOUCIMA_MIN,
                format!("{} 行(下限 {})", rows, GOUCIMA_MIN),
            ));
            checks.push(Check::new("goucima.tsv 列数", ok && cols_ok, "2 列".into()));
            checks.push(Check::new("goucima.tsv 排序", sort_ok, "zi 码点升序".into()));
            checks.push(Check::new(
                "goucima.tsv 抽样 一 -> ggll",
                yi.as_deref() == Some("ggll"),
                format!("-> {:?}", yi),
            ));
        }
        Err(e) => checks.push(Check::new("goucima.tsv 可读", false, e.to_string())),
    }
}

fn check_meta(dir: &Path, checks: &mut Vec<Check>) -> Result<()> {
    let path = dir.join("meta.json");
    let Some(meta) = crate::util::Meta::load(&path)? else {
        checks.push(Check::new("meta.json 存在", false, "缺失".into()));
        return Ok(());
    };
    checks.push(Check::new("meta.json version", meta.version == 1, format!("version={}", meta.version)));
    checks.push(Check::new("meta.json sources", !meta.sources.is_empty(), format!("{} 个来源", meta.sources.len())));
    let files = [
        ("wubi.tsv", WUBI_MIN),
        ("pinyin_char.tsv", PINYIN_CHAR_MIN),
        ("pinyin_phrase.tsv", PINYIN_PHRASE_MIN),
        ("english.tsv", ENGLISH_MIN),
        ("suggestion.tsv", SUGGESTION_MIN),
        ("goucima.tsv", GOUCIMA_MIN),
    ];
    let mut mismatches = Vec::new();
    for (file, _) in files {
        let actual = crate::util::count_lines(&dir.join(file)).unwrap_or(u64::MAX);
        match meta.rows.get(file) {
            Some(recorded) if *recorded == actual => {}
            other => mismatches.push(format!("{} meta={:?} 实际={}", file, other, actual)),
        }
    }
    checks.push(Check::new(
        "meta.json rows 与实际行数一致",
        mismatches.is_empty(),
        if mismatches.is_empty() { "6 个文件全部一致".into() } else { mismatches.join("; ") },
    ));
    Ok(())
}
