//! `fetch`:下载拼音词组与英文词频原始词典(缓存在 dicts/raw,可重跑),生成:
//! - pinyin_phrase.tsv `word\tpinyin\tfreq`(pinyin 为空格分隔的无声调音节)
//! - english.tsv       `word\tfreq`
//!
//! 来源选择(均 MIT 许可,github raw 可达,已实测):
//! 1. fxsjy/jieba dict.txt:约 34.9 万条“词 词频 词性”,提供词频;
//! 2. mozillazg/phrase-pinyin-data large_pinyin.txt:约 41 万条“词: 带调拼音”,提供拼音标注;
//!    两者求交:jieba 的词直接命中标注;未命中(多音字词等)用 pinyin_char.tsv 逐字反推,
//!    多音字取该字频次最高的读音(注释见 build_pinyin_phrase)。
//! 3. first20hours/google-10000-english 20k.txt:2 万英文词按使用频率排序(仅名次无频值,
//!    线性秩转换为频次,相对大小关系不变)。
//!
//! 失败策略:每个源按主/镜像 URL 依次尝试;全部失败则以非 0 退出码打印手动下载建议,绝不造假数据。

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::util::{is_cjk_word, toneless_syllable, AtomicWriter, Meta, Progress};

pub struct FetchOpts {
    pub out: PathBuf,
    pub cache: PathBuf,
}

struct Source {
    name: &'static str,
    license: &'static str,
    /// 依次尝试的下载地址(第一个为主源,其余为镜像)
    urls: &'static [&'static str],
    /// dicts/raw 下的缓存文件名
    cache_file: &'static str,
}

const SRC_PHRASE_PINYIN: Source = Source {
    name: "phrase-pinyin-data(large)",
    license: "MIT",
    urls: &[
        "https://raw.githubusercontent.com/mozillazg/phrase-pinyin-data/master/large_pinyin.txt",
        "https://cdn.jsdelivr.net/gh/mozillazg/phrase-pinyin-data@master/large_pinyin.txt",
    ],
    cache_file: "phrase-pinyin-large.txt",
};

const SRC_JIEBA: Source = Source {
    name: "jieba 词典 dict.txt",
    license: "MIT",
    urls: &[
        "https://raw.githubusercontent.com/fxsjy/jieba/master/jieba/dict.txt",
        "https://cdn.jsdelivr.net/gh/fxsjy/jieba@master/jieba/dict.txt",
    ],
    cache_file: "jieba-dict.txt",
};

const SRC_ENGLISH: Source = Source {
    name: "google-10000-english(20k)",
    license: "MIT",
    urls: &[
        "https://raw.githubusercontent.com/first20hours/google-10000-english/master/20k.txt",
        "https://cdn.jsdelivr.net/gh/first20hours/google-10000-english@master/20k.txt",
        "https://raw.githubusercontent.com/first20hours/google-10000-english/master/google-10000-english.txt",
    ],
    cache_file: "google-english-20k.txt",
};

pub fn run(opts: FetchOpts) -> Result<()> {
    let out = opts.out;
    let cache = opts.cache;
    std::fs::create_dir_all(&out)?;
    std::fs::create_dir_all(&cache)?;

    for src in [&SRC_PHRASE_PINYIN, &SRC_JIEBA, &SRC_ENGLISH] {
        ensure_cached(src, &cache)?;
    }

    // pinyin_char.tsv 逐字反推表(convert 必须先跑)
    let char_py = load_char_pinyin(&out)?;
    eprintln!("[fetch] 反推表:单字读音 {} 条", char_py.len());

    let phrase_n = build_pinyin_phrase(&cache, &out, &char_py)?;
    let english_n = build_english(&cache, &out)?;

    // 更新 meta.json(保留 convert 登记的来源与行数)
    let mut meta = Meta::load(&out.join("meta.json"))?
        .unwrap_or_else(|| Meta::new(Vec::new()));
    for (src, file) in [
        (&SRC_PHRASE_PINYIN, SRC_PHRASE_PINYIN.cache_file),
        (&SRC_JIEBA, SRC_JIEBA.cache_file),
        (&SRC_ENGLISH, SRC_ENGLISH.cache_file),
    ] {
        let entry = json!({
            "name": src.name,
            "url": src.urls[0],
            "license": src.license,
            "cache": file,
        });
        meta.sources.retain(|e| e.get("name") != entry.get("name"));
        meta.sources.push(entry);
    }
    meta.rows.insert("pinyin_phrase.tsv".into(), phrase_n);
    meta.rows.insert("english.tsv".into(), english_n);
    meta.built_at = crate::util::now_iso();
    meta.save(&out.join("meta.json"))?;

    eprintln!("[fetch] 完成:pinyin_phrase={} english={}", phrase_n, english_n);
    Ok(())
}

/// 缓存命中则跳过;未命中按 URL 顺序下载(先写 .tmp 再改名)。
fn ensure_cached(src: &Source, cache: &Path) -> Result<()> {
    let dst = cache.join(src.cache_file);
    if dst.exists() && dst.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        eprintln!("[fetch] {} 已缓存于 {:?},跳过下载", src.name, dst);
        return Ok(());
    }
    let tmp = dst.with_extension("download.tmp");
    let mut errors = Vec::new();
    for url in src.urls {
        eprintln!("[fetch] 下载 {} <- {}", src.name, url);
        let st = Command::new("curl")
            .args(["-fSL", "--retry", "2", "--max-time", "600", "-o"])
            .arg(&tmp)
            .arg(url)
            .status();
        match st {
            Ok(s) if s.success() && tmp.metadata().map(|m| m.len() > 0).unwrap_or(false) => {
                std::fs::rename(&tmp, &dst).with_context(|| format!("改名到 {:?}", dst))?;
                return Ok(());
            }
            Ok(s) => errors.push(format!("{} -> 退出码 {:?}", url, s.code())),
            Err(e) => errors.push(format!("{} -> {}", url, e)),
        }
    }
    let _ = std::fs::remove_file(&tmp);
    bail!(
        "下载 {} 失败,全部源均不可达:\n  {}\n下一步建议:\n  1) 检查网络/代理(需要可达 raw.githubusercontent.com,镜像走 cdn.jsdelivr.net);\n  2) 手动下载上述任一 URL,保存为 {:?} 后重跑 dicttool fetch。\n禁止静默跳过或伪造数据,流程已中止。",
        src.name,
        errors.join("\n  "),
        dst
    )
}

/// 读 pinyin_char.tsv 建立单字 → (无声调读音, 频次) 表;多音字保留频次最高的读音。
fn load_char_pinyin(out: &Path) -> Result<HashMap<char, (String, u64)>> {
    let path = out.join("pinyin_char.tsv");
    let f = std::fs::File::open(&path).with_context(|| {
        format!(
            "打开 {:?} 失败:请先运行 `dicttool convert` 生成 pinyin_char.tsv",
            path
        )
    })?;
    let mut map: HashMap<char, (String, u64)> = HashMap::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        let mut it = line.split('\t');
        let (Some(py), Some(zi), Some(freq)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let Ok(freq) = freq.parse::<u64>() else { continue };
        let Some(ch) = zi.chars().next() else { continue };
        // 同字多读音:保留频次最高的
        match map.get(&ch) {
            Some((_, f)) if *f >= freq => {}
            _ => {
                map.insert(ch, (py.to_string(), freq));
            }
        }
    }
    Ok(map)
}

/// 解析 phrase-pinyin-data 的行 `词: 带调拼音`(跳过 # 注释行)。
pub fn parse_phrase_pinyin_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (word, pinyin) = line.split_once(':')?;
    let word = word.trim();
    if !is_cjk_word(word) || word.chars().count() < 2 {
        return None;
    }
    let mut sylls = Vec::new();
    for s in pinyin.split_whitespace() {
        sylls.push(toneless_syllable(s)?);
    }
    if sylls.is_empty() || sylls.len() != word.chars().count() {
        return None; // 音节数须与字数一致,防错位
    }
    Some((word.to_string(), sylls.join(" ")))
}

/// 解析 jieba dict.txt 的行 `词 词频 [词性]`。
pub fn parse_jieba_line(line: &str) -> Option<(String, u64)> {
    let mut it = line.split_whitespace();
    let word = it.next()?;
    let freq = it.next()?.parse::<u64>().ok()?;
    // 只要 ≥2 个汉字的纯中文词(单字拼音已有 pinyin_char;混入字母/符号的词不要)
    if freq == 0 || !is_cjk_word(word) || word.chars().count() < 2 {
        return None;
    }
    Some((word.to_string(), freq))
}

/// 逐字反推词组拼音:多音字取频次最高读音;任一字无读音返回 None。
pub fn derive_pinyin(
    word: &str,
    char_py: &HashMap<char, (String, u64)>,
) -> Option<String> {
    let mut parts = Vec::with_capacity(word.chars().count());
    for c in word.chars() {
        parts.push(char_py.get(&c)?.0.clone());
    }
    Some(parts.join(" "))
}

/// 生成 pinyin_phrase.tsv。
///
/// 拼音来源优先级:phrase-pinyin-data 显式标注 > pinyin_char.tsv 逐字反推(derive_pinyin)。
/// 多音字反推规则:取该字在 pinyin_char.tsv 中频次最高的读音(单字频次是最可靠的歧义消解信号,
/// 如 重 zhong>chong、行 xing/hang);反推音节可能不合语境,故反推行保留但排序上天然受词频约束。
/// 任一字无读音的词直接跳过(宁缺毋滥)。
fn build_pinyin_phrase(
    cache: &Path,
    out: &Path,
    char_py: &HashMap<char, (String, u64)>,
) -> Result<u64> {
    eprintln!("[fetch] 解析 jieba 词频 ...");
    let mut jieba: HashMap<String, u64> = HashMap::new();
    {
        let f = std::fs::File::open(cache.join(SRC_JIEBA.cache_file))?;
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.starts_with('\u{feff}') {
                continue; // BOM
            }
            if let Some((w, f)) = parse_jieba_line(&line) {
                *jieba.entry(w).or_insert(0) += f;
            }
        }
    }
    eprintln!("[fetch] jieba 有效词 {} 条", jieba.len());

    eprintln!("[fetch] 解析 phrase-pinyin-data 拼音标注 ...");
    let mut annotated: HashMap<String, String> = HashMap::new();
    {
        let f = std::fs::File::open(cache.join(SRC_PHRASE_PINYIN.cache_file))?;
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.starts_with('\u{feff}') {
                continue;
            }
            if let Some((w, p)) = parse_phrase_pinyin_line(&line) {
                annotated.entry(w).or_insert(p);
            }
        }
    }
    eprintln!("[fetch] 拼音标注 {} 条", annotated.len());

    let mut prog = Progress::new("pinyin_phrase", 50_000);
    let mut rows: Vec<(String, String, u64)> = Vec::with_capacity(jieba.len());
    let mut derived = 0u64;
    let mut no_pinyin = 0u64;
    for (word, freq) in jieba {
        let pinyin = match annotated.get(&word) {
            Some(p) => p.clone(),
            None => match derive_pinyin(&word, char_py) {
                Some(p) => {
                    derived += 1;
                    p
                }
                None => {
                    no_pinyin += 1;
                    continue;
                }
            },
        };
        rows.push((word, pinyin, freq));
        prog.tick();
    }
    // 按 pinyin 升序、freq 降序输出,保证幂等与前缀扫描友好
    rows.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    let mut w = AtomicWriter::create(&out.join("pinyin_phrase.tsv"))?;
    for (word, pinyin, freq) in &rows {
        writeln!(w, "{}\t{}\t{}", word, pinyin, freq)?;
    }
    w.commit()?;
    eprintln!(
        "[fetch] pinyin_phrase.tsv:共 {} 行(反推读音 {} 行,无读音丢弃 {} 词)",
        rows.len(),
        derived,
        no_pinyin
    );
    Ok(rows.len() as u64)
}

/// 解析 google-10000-english 行(每行一个词,按使用频率排序)。
pub fn parse_english_line(line: &str) -> Option<String> {
    let w = line.trim().to_ascii_lowercase();
    if w.is_empty() || !w.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some(w)
}

/// 生成 english.tsv。
///
/// 来源只有“名次”没有频值:freq = 总行数 - 名次 + 1(第 1 名频次最高),
/// 保持相对大小关系,满足 core 的 log10 归一化与“高频前 N”判断。
fn build_english(cache: &Path, out: &Path) -> Result<u64> {
    let f = std::fs::File::open(cache.join(SRC_ENGLISH.cache_file))?;
    // 同词保留最好名次;频次随名次递减
    let mut best: BTreeMap<String, u64> = BTreeMap::new();
    let mut rank = 0u64;
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.starts_with('\u{feff}') {
            continue;
        }
        rank += 1;
        if let Some(word) = parse_english_line(&line) {
            let freq = rank; // 先记名次,后面统一反转
            best.entry(word).and_modify(|e| *e = (*e).min(freq)).or_insert(freq);
        }
    }
    let total = rank.max(1);
    let mut rows: Vec<(String, u64)> = best
        .into_iter()
        .map(|(word, r)| (word, total - r + 1))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut w = AtomicWriter::create(&out.join("english.tsv"))?;
    for (word, freq) in &rows {
        writeln!(w, "{}\t{}", word, freq)?;
    }
    w.commit()?;
    eprintln!("[fetch] english.tsv:共 {} 行(源排名 {} 词)", rows.len(), total);
    Ok(rows.len() as u64)
}

/// 供测试/文档引用:列出全部来源。
pub fn all_sources() -> Vec<(&'static str, &'static [&'static str], &'static str)> {
    vec![
        (SRC_PHRASE_PINYIN.name, SRC_PHRASE_PINYIN.urls, SRC_PHRASE_PINYIN.license),
        (SRC_JIEBA.name, SRC_JIEBA.urls, SRC_JIEBA.license),
        (SRC_ENGLISH.name, SRC_ENGLISH.urls, SRC_ENGLISH.license),
    ]
}
