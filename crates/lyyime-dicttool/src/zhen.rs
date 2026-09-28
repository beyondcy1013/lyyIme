//! `zhen`:由 ECDICT(stardict.db,英文→中文释义)反生成 中文→英文 反查表
//! `zh_en.tsv`(合同 §15 候选右键"反查英文"的运行时数据源)。
//!
//! 算法:
//! 1. 词面集 = 本项目词库的全体中文词面(wubi.tsv 词 + pinyin_phrase.tsv 词
//!    + pinyin_char.tsv 字 + suggestion.tsv 词)——反查覆盖率 = 候选空间,
//!    词表外的 zh 词不会被候选展示,也就不需要反查条目(表规模受控);
//! 2. 流式扫 stardict.translation,抽出"连续 CJK 词段"(汉字片段,≤8 字;
//!    ECDICT 释义形如 `n. 电脑, 电子计算机`,词段即单个义项);
//! 3. 词段 ∩ 词面集 → zh 键,值 = 英文词表(按 frq/bnc 词频升序,星级微调,
//!    每词 ≤8 条,大小写去重);
//! 4. 原子写 `zh \t en1 \t en2 …`(zh 升序)并登记 meta.json 行数。
//!
//! 流式约束:stardict 340 万行逐行处理,zh→en 映射聚合在内存
//! (词面集决定键上限 ≈ 词库规模,实测 ~20 万键)。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

use crate::util::{is_cjk_char, AtomicWriter, Meta, Progress};

/// 反查词段最大字数(更长多为释义短句而非词条)。
const ZH_MAX_LEN: usize = 8;
/// 每个中文词保留的英文词上限。
const EN_TOP: usize = 8;
/// 聚合阶段每键暂存上限(写盘前精排再截到 EN_TOP;粗截防异常行撑爆)。
const EN_CAP: usize = EN_TOP * 4;

pub const DEFAULT_STARDICT_DB: &str = "dicts/cache/stardict.db";

pub struct ZhenOpts {
    /// ECDICT/stardict.db 路径(en→zh 源)。
    pub stardict: PathBuf,
    /// runtime 目录(读词表 + 写 zh_en.tsv)。
    pub out: PathBuf,
}

pub fn run(opts: ZhenOpts) -> Result<()> {
    let vocab = load_zh_vocab(&opts.out)?;
    if vocab.is_empty() {
        anyhow::bail!(
            "中文词面集为空:{:?} 下缺 wubi.tsv/pinyin_phrase.tsv 等,请先 dicttool convert+fetch",
            opts.out
        );
    }
    eprintln!("[zhen] 中文词面集 {} 条", vocab.len());
    let conn = Connection::open_with_flags(
        &opts.stardict,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("以只读方式打开 {:?} 失败", opts.stardict))?;
    let n = zhen_from_conn(&conn, &vocab, &opts.out.join("zh_en.tsv"))?;

    // meta.json:登记行数 + 来源(与 convert 同一约定,已存在则合并)
    let mut meta = Meta::load(&opts.out.join("meta.json"))?.unwrap_or_else(|| Meta::new(Vec::new()));
    let src = serde_json::json!({"name": "ECDICT stardict.db 反生成 zh→en", "path": opts.stardict.display().to_string()});
    if !meta.sources.iter().any(|e| *e == src) {
        meta.sources.push(src);
    }
    meta.rows.insert("zh_en.tsv".into(), n);
    meta.built_at = crate::util::now_iso();
    meta.save(&opts.out.join("meta.json"))?;
    eprintln!("[zhen] 完成:zh_en.tsv {} 词 → {}", n, opts.out.display());
    Ok(())
}

/// 词面集:runtime TSV 的中文词/字列。文件缺失跳过(部分产物也能跑)。
fn load_zh_vocab(out: &Path) -> Result<HashSet<String>> {
    let mut vocab = HashSet::new();
    // (文件, 词面所在列下标)
    for (name, col) in [
        ("wubi.tsv", 1usize),
        ("pinyin_phrase.tsv", 0),
        ("pinyin_char.tsv", 1),
        ("suggestion.tsv", 0),
    ] {
        let p = out.join(name);
        let Ok(f) = File::open(&p) else {
            eprintln!("[zhen] 提示:{name} 缺失,跳过该词面来源");
            continue;
        };
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(w) = line.split('\t').nth(col) {
                if !w.is_empty() && w.chars().all(is_cjk_char) {
                    vocab.insert(w.to_string());
                }
            }
        }
    }
    Ok(vocab)
}

/// 英文词排序键:frq/bnc 越小越常用(两库都在取小者偏保守);
/// 均无词频 → 8_000_000 起,再按 collins/oxford 星级微调靠前。
fn en_rank(frq: i64, bnc: i64, collins: i64, oxford: i64) -> u64 {
    let f = match (frq > 0, bnc > 0) {
        (true, true) => frq.min(bnc) as u64,
        (true, false) => frq as u64,
        (false, true) => bnc as u64,
        (false, false) => 8_000_000,
    };
    f.saturating_sub(collins.max(0) as u64 * 10 + oxford.max(0) as u64 * 20)
}

/// 从一段释义文本抽出"连续 CJK 词段"(汉字片段,去重保序)。
fn zh_segments(text: &str) -> Vec<String> {
    let mut segs = Vec::<String>::new();
    let mut cur = String::new();
    for c in text.chars() {
        if is_cjk_char(c) {
            cur.push(c);
        } else if !cur.is_empty() {
            if !segs.iter().any(|s| *s == cur) {
                segs.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if !cur.is_empty() && !segs.iter().any(|s| *s == cur) {
        segs.push(cur);
    }
    segs
}

/// 流式生成 zh_en.tsv(连接可注入,单测用内存库)。
pub fn zhen_from_conn(
    conn: &Connection,
    vocab: &HashSet<String>,
    out_path: &Path,
) -> Result<u64> {
    let mut map: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT word, translation, frq, bnc, collins, oxford
         FROM stardict WHERE translation IS NOT NULL AND translation <> ''",
    )?;
    let mut rows = stmt.query([])?;
    let mut prog = Progress::new("zhen/scan", 500_000);
    while let Some(row) = rows.next()? {
        prog.tick();
        let word: String = row.get(0)?;
        // 头词只收 ASCII(短语/连字符/撇号可):过滤日文罗马字/法语地名等
        // 非英文转写(Hichisō/Trois-Rivières),同类常带 ASCII 孪生条目兜底
        if !word.is_ascii()
            || word.is_empty()
            || !word.chars().any(|c| c.is_ascii_alphabetic())
        {
            continue;
        }
        let tr: String = row.get(1)?;
        let frq: i64 = row.get(2).unwrap_or(0);
        let bnc: i64 = row.get(3).unwrap_or(0);
        let collins: i64 = row.get(4).unwrap_or(0);
        let oxford: i64 = row.get(5).unwrap_or(0);
        let rank = en_rank(frq, bnc, collins, oxford);
        for seg in zh_segments(&tr) {
            if seg.chars().count() > ZH_MAX_LEN || !vocab.contains(&seg) {
                continue;
            }
            let v = map.entry(seg).or_default();
            if v.iter().any(|(_, w)| w.eq_ignore_ascii_case(&word)) {
                continue;
            }
            if v.len() < EN_CAP {
                v.push((rank, word.clone()));
            }
        }
    }
    prog.finish();

    // zh 升序写出;en 按 rank 升序,大小写去重后截 EN_TOP
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    let mut w = AtomicWriter::create(out_path)?;
    let mut n = 0u64;
    for zh in keys {
        let mut v = map.remove(&zh).unwrap_or_default();
        v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut seen: HashSet<String> = HashSet::new();
        let mut ens: Vec<String> = Vec::new();
        for (_, en) in v {
            if seen.insert(en.to_lowercase()) {
                ens.push(en);
            }
        }
        ens.truncate(EN_TOP);
        if ens.is_empty() {
            continue;
        }
        write!(w, "{zh}")?;
        for en in &ens {
            write!(w, "\t{en}")?;
        }
        writeln!(w)?;
        n += 1;
    }
    w.commit()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE stardict(
                id INTEGER PRIMARY KEY, word TEXT UNIQUE COLLATE NOCASE,
                sw TEXT, phonetic TEXT, definition TEXT, translation TEXT,
                pos TEXT, collins INTEGER, oxford INTEGER, tag TEXT,
                bnc INTEGER, frq INTEGER, exchange TEXT, detail TEXT, audio TEXT);",
        )
        .unwrap();
        conn
    }

    fn ins(conn: &Connection, word: &str, tr: &str, frq: i64, bnc: i64, collins: i64, oxford: i64) {
        conn.execute(
            "INSERT INTO stardict(word, translation, frq, bnc, collins, oxford)
             VALUES (?1,?2,?3,?4,?5,?6)",
            (word, tr, frq, bnc, collins, oxford),
        )
        .unwrap();
    }

    fn tmp_path(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lyyime-zhen-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        p
    }

    #[test]
    fn 词段抽取_词性与标点剥离() {
        let segs = zh_segments("n. 苹果, 家伙\n[医] 苹果\nadj. 好的, 优良的");
        assert!(segs.contains(&"苹果".to_string()));
        assert!(segs.contains(&"家伙".to_string()));
        assert!(segs.contains(&"好的".to_string()));
        assert!(segs.contains(&"优良的".to_string())); // 词段不切义项内部
        // 同词段去重
        assert_eq!(segs.iter().filter(|s| *s == "苹果").count(), 1);
    }

    #[test]
    fn 反查表生成_过滤排序与上限() {
        let conn = mk_conn();
        ins(&conn, "apple", "n. 苹果, 家伙", 100, 80, 5, 1);
        ins(&conn, "pear", "n. 梨", 200, 0, 0, 0);
        ins(&conn, "apples", "n. 苹果", 50, 0, 0, 0);
        ins(&conn, "pineapple", "n. 菠萝", 900, 0, 0, 0);
        ins(&conn, "longgloss", "这是一个非常长的释义短句不应该入表", 10, 10, 0, 0);
        let vocab: HashSet<String> = ["苹果", "梨", "菠萝", "伙计"]
            .iter().map(|s| s.to_string()).collect();
        let out = tmp_path("zh_en.tsv");
        let n = zhen_from_conn(&conn, &vocab, &out).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        // 苹果 → apple(min(100,80)-星级=rank10) 先于 apples(frq50);
        // Apple/apples 大小写去重各留一条
        assert!(text.contains("苹果\tapple\tapples"), "{text}");
        assert!(text.contains("梨\tpear"));
        assert!(text.contains("菠萝\tpineapple"));
        assert!(!text.contains("伙计"), "词面外源词不入表?词面有的才入");
        assert!(!text.contains("longgloss"), "释义短句词段不入表");
        // zh 升序
        let zhs: Vec<&str> = text.lines().map(|l| l.split('\t').next().unwrap()).collect();
        let mut sorted = zhs.clone();
        sorted.sort();
        assert_eq!(zhs, sorted);
        assert_eq!(n, 3);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn 词面过滤_只收词库内的中文词() {
        let conn = mk_conn();
        ins(&conn, "apple", "n. 苹果, 家伙", 100, 80, 0, 0);
        let vocab: HashSet<String> = ["苹果"].iter().map(|s| s.to_string()).collect();
        let out = tmp_path("zh_en2.tsv");
        zhen_from_conn(&conn, &vocab, &out).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("苹果"));
        assert!(!text.contains("家伙"), "非词库词面不入表: {text}");
        let _ = std::fs::remove_file(&out);
    }
}
