//! `query` 子命令:对 runtime TSV 做独立前缀扫描,打印前 9 个候选。
//!
//! 自包含实现(只依赖本 crate 与 std,不依赖 lyyime-core——后者在并行开发中)。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

const TOP: usize = 9;

pub fn run(kind: &str, q: &str, out: &PathBuf) -> Result<()> {
    let dir: &Path = out;
    match kind {
        "wubi" => query_wubi(dir, q),
        "pinyin" => query_pinyin(dir, q),
        "en" | "english" => query_en(dir, q),
        "zh_en" | "zhene" => query_zh_en(dir, q),
        _ => bail!("未知查询类型 {:?},可用:wubi | pinyin | en | zh_en", kind),
    }
}

struct Cand {
    text: String,
    /// 附注(码 / 拼音)
    comment: String,
    freq: u64,
    /// 0=完全命中 1=前缀命中 2=兜底
    tier: u8,
}

fn open(dir: &Path, file: &str) -> Result<BufReader<std::fs::File>> {
    let p = dir.join(file);
    Ok(BufReader::new(
        std::fs::File::open(&p).with_context(|| format!("打开 {:?} 失败(先跑 convert/fetch)", p))?,
    ))
}

fn print_cands(q: &str, mut cands: Vec<Cand>) {
    cands.sort_by(|a, b| a.tier.cmp(&b.tier).then(b.freq.cmp(&a.freq)));
    if cands.is_empty() {
        println!("query {:?}: 无候选", q);
        return;
    }
    println!("query {:?} 前 {} 个候选:", q, TOP.min(cands.len()));
    for (i, c) in cands.into_iter().take(TOP).enumerate() {
        println!("  {}. {}\t[{}]\tfreq={}", i + 1, c.text, c.comment, c.freq);
    }
}

/// 五笔:code == q 完全命中优先,其后 code 前缀命中(ARCHITECTURE.md §5.2)。
fn query_wubi(dir: &Path, q: &str) -> Result<()> {
    let q = q.to_ascii_lowercase();
    if q.is_empty() || !q.chars().all(|c| c.is_ascii_lowercase()) {
        bail!("五笔码只接受小写字母,例如:dicttool query wubi yyyy");
    }
    let mut cands = Vec::new();
    for line in open(dir, "wubi.tsv")?.lines() {
        let line = line?;
        let mut it = line.split('\t');
        let (Some(code), Some(word), Some(freq)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        if code == q {
            cands.push(Cand { text: word.into(), comment: code.into(), freq: freq.parse().unwrap_or(0), tier: 0 });
        } else if code.starts_with(&q) {
            cands.push(Cand { text: word.into(), comment: code.into(), freq: freq.parse().unwrap_or(0), tier: 1 });
        }
    }
    print_cands(&q, cands);
    Ok(())
}

/// 拼音:词组完全命中(pinyin_phrase 忽略音节空格后 == 查询)> 整查询恰为一个音节时的
/// 单字候选 > 前缀命中(对齐 core §5.3 的全拼优先、渐进候选在后)。
fn query_pinyin(dir: &Path, q: &str) -> Result<()> {
    let key: String = q.split_whitespace().collect::<Vec<_>>().join("").to_ascii_lowercase();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_lowercase()) {
        bail!("拼音只接受小写字母,例如:dicttool query pinyin nihao");
    }
    let mut cands = Vec::new();
    for line in open(dir, "pinyin_phrase.tsv")?.lines() {
        let line = line?;
        let mut it = line.split('\t');
        let (Some(word), Some(pinyin), Some(freq)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let joined: String = pinyin.split_whitespace().collect();
        if joined == key {
            cands.push(Cand { text: word.into(), comment: pinyin.into(), freq: freq.parse().unwrap_or(0), tier: 0 });
        } else if joined.starts_with(&key) {
            cands.push(Cand { text: word.into(), comment: pinyin.into(), freq: freq.parse().unwrap_or(0), tier: 2 });
        }
    }
    // 单音节查询:补单字候选(频次降序,与词组前缀候选按 tier 分层)
    if pinyin_char_syllables(dir)?.iter().any(|s| s == &key) {
        for line in open(dir, "pinyin_char.tsv")?.lines() {
            let line = line?;
            let mut it = line.split('\t');
            let (Some(py), Some(zi), Some(freq)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if py == key {
                cands.push(Cand { text: zi.into(), comment: py.into(), freq: freq.parse().unwrap_or(0), tier: 1 });
            }
        }
    }
    print_cands(&q, cands);
    Ok(())
}

/// pinyin_char.tsv 中的全部音节(去重)。
fn pinyin_char_syllables(dir: &Path) -> Result<Vec<String>> {
    let mut seen = Vec::new();
    let mut last = String::new();
    for line in open(dir, "pinyin_char.tsv")?.lines() {
        let line = line?;
        if let Some(py) = line.split('\t').next() {
            if py != last {
                seen.push(py.to_string());
                last = py.to_string();
            }
        }
    }
    Ok(seen)
}

/// 反查英文:zh_en.tsv 精确命中整行(en 按内置词频序),供 §15 右键菜单联调。
fn query_zh_en(dir: &Path, q: &str) -> Result<()> {
    let mut cands = Vec::new();
    for line in open(dir, "zh_en.tsv")?.lines() {
        let line = line?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split('\t');
        let Some(zh) = it.next() else { continue };
        if zh != q {
            continue;
        }
        for en in it {
            cands.push(Cand { text: en.into(), comment: zh.into(), freq: 0, tier: 0 });
        }
    }
    print_cands(&q, cands);
    Ok(())
}

/// 英文:english.tsv 词前缀,按词频降序。
fn query_en(dir: &Path, q: &str) -> Result<()> {
    let q = q.to_ascii_lowercase();
    if q.is_empty() || !q.chars().all(|c| c.is_ascii_lowercase()) {
        bail!("英文查询只接受小写字母,例如:dicttool query en hello");
    }
    let mut cands = Vec::new();
    for line in open(dir, "english.tsv")?.lines() {
        let line = line?;
        let mut it = line.split('\t');
        let (Some(word), Some(freq)) = (it.next(), it.next()) else {
            continue;
        };
        if word.starts_with(&q) {
            let exact = word == q;
            cands.push(Cand {
                text: word.into(),
                comment: "en".into(),
                freq: freq.parse().unwrap_or(0),
                tier: if exact { 0 } else { 1 },
            });
        }
    }
    print_cands(&q, cands);
    Ok(())
}
