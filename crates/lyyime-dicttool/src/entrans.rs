//! `entrans`:由 ECDICT(stardict.db,英文→中文释义)生成 英文→中文 翻译表
//! `en_trans.tsv`(全大写输入候选的"中文翻译"数据源,2026-09-28 需求)。
//!
//! 算法(与 [`zhen`] 同源约定,流式一遍扫库):
//! 1. 键宇宙 = runtime english.tsv 全体词 ∪ stardict 全大写词(缩略语词形)
//!    ∪ 内置常用缩略语人工校对表(见 `acronyms.tsv`;WHO/GDP 等 ECDICT 缺词或
//!    义项歧义词的兜底,人工校对义项恒排该词翻译列表最前);
//! 2. 流式扫 stardict 纯字母词行(NOCASE 库中 'cpu'/'CPU' 两种词形都可能是
//!    释义来源,统一归并到小写键),释义按行清洗:剥词性/领域标签、保留含
//!    中文的义项行、去重保序;
//! 3. 每键 ≤6 条,原子写 `词 \t 译1 \t 译2 …`(词升序)并登记 meta.json 行数。
//!
//! 运行前提:`dicts/cache/stardict.db`(下载见项目 README / ECDICT release),
//! 缺库时与 `zhen` 同样直接报错给出手动下载建议,绝不静默跳过。

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

use crate::util::{is_cjk_char, AtomicWriter, Meta, Progress};

/// 每个词保留的翻译条数上限(候选 4 及其后为翻译,页容量足够)。
const TRANS_TOP: usize = 6;
/// 聚合阶段每键暂存上限(写盘前统一截到 TRANS_TOP;粗截防异常行撑爆)。
const TRANS_CAP: usize = 12;

pub const DEFAULT_STARDICT_DB: &str = "dicts/cache/stardict.db";

/// 内置常用缩略语人工校对表(随 crate 编译期嵌入)。
pub const ACRONYMS_TSV: &str = include_str!("acronyms.tsv");

pub struct EntransOpts {
    /// ECDICT/stardict.db 路径(en→zh 源)。
    pub stardict: PathBuf,
    /// runtime 目录(读 english.tsv 词表 + 写 en_trans.tsv)。
    pub out: PathBuf,
}

pub fn run(opts: EntransOpts) -> Result<()> {
    let conn = Connection::open_with_flags(
        &opts.stardict,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| {
        format!(
            "以只读方式打开 {:?} 失败:ECDICT 词典缺失,请手动下载 \
             https://github.com/skywind3000/ECDICT/releases (ecdict-sqlite-28.zip,解压出 stardict.db) 后重试",
            opts.stardict
        )
    })?;
    let universe = load_en_vocab(&opts.out)?;
    eprintln!("[entrans] 英文词表 {} 条(缺失时仅缩略语词形)", universe.len());
    let n = entrans_from_conn(&conn, &universe, &opts.out.join("en_trans.tsv"))?;

    // meta.json:登记行数 + 来源(与 convert/zhen 同一约定,已存在则合并)
    let mut meta = Meta::load(&opts.out.join("meta.json"))?.unwrap_or_else(|| Meta::new(Vec::new()));
    let src = serde_json::json!({
        "name": "ECDICT stardict.db 反生成 en→zh 翻译(+内置缩略语校对表)",
        "path": opts.stardict.display().to_string(),
        "license": "MIT",
    });
    if !meta.sources.iter().any(|e| *e == src) {
        meta.sources.push(src);
    }
    meta.rows.insert("en_trans.tsv".into(), n as u64);
    meta.built_at = crate::util::now_iso();
    meta.save(&opts.out.join("meta.json"))?;
    eprintln!("[entrans] 完成:en_trans.tsv {} 词 → {}", n, opts.out.display());
    Ok(())
}

/// 英文词表:english.tsv 第一列(词必须小写)。文件缺失 → 空集
/// (此时键宇宙仍含 stardict 全大写词与内置缩略语,功能可用但覆盖小)。
fn load_en_vocab(out: &Path) -> Result<HashSet<String>> {
    let mut vocab = HashSet::new();
    let p = out.join("english.tsv");
    let Ok(f) = File::open(&p) else {
        eprintln!("[entrans] 提示:english.tsv 缺失,请先 dicttool convert+fetch;键宇宙降级为缩略语词形");
        return Ok(vocab);
    };
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(w) = line.split('\t').next() {
            if w.chars().all(|c| c.is_ascii_lowercase()) && !w.is_empty() {
                vocab.insert(w.to_string());
            }
        }
    }
    Ok(vocab)
}

/// 从 stardict 连接生成 en_trans.tsv,返回写入词数(测试可传内存库)。
pub fn entrans_from_conn(
    conn: &Connection,
    universe: &HashSet<String>,
    dst: &Path,
) -> Result<usize> {
    // 键 → 已清洗义项(保序去重);BTreeMap 保证输出按词升序,产物幂等。
    let mut keys: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut stmt = conn.prepare(
        "SELECT word, translation FROM stardict
         WHERE translation IS NOT NULL
           AND word NOT GLOB '*[^A-Za-z]*'
           AND length(word) BETWEEN 1 AND 24",
    )?;
    let mut rows = stmt.query([])?;
    let mut prog = Progress::new("en_trans", 50_000);
    while let Some(r) = rows.next()? {
        let word: String = r.get(0)?;
        let raw: String = r.get(1)?;
        prog.tick();
        let all_caps = word.chars().all(|c| c.is_ascii_uppercase());
        let lower = word.to_lowercase();
        // 收录两类词:词表内小写词(小写词的大写敲入形态)与全大写缩略语词形;
        // 其余(专有名词/混合词形)既打不出来也不该出现在候选里。
        if !all_caps && !universe.contains(&lower) {
            continue;
        }
        let entry = keys.entry(lower).or_default();
        for t in translation_lines(&raw) {
            if !entry.contains(&t) {
                entry.push(t);
            }
        }
        entry.truncate(TRANS_CAP);
    }
    let n = write_en_trans(keys, dst)?;
    Ok(n)
}

/// 并入缩略语人工校对义项(恒排最前)、截断、原子写出;返回写入词数。
fn write_en_trans(keys: BTreeMap<String, Vec<String>>, dst: &Path) -> Result<usize> {
    let acronyms = load_acronyms(ACRONYMS_TSV);
    let mut keys = merge_with_acronyms(keys, &acronyms, TRANS_TOP);
    keys.retain(|_, v| !v.is_empty());
    let mut w = AtomicWriter::create(dst)?;
    let mut n = 0usize;
    for (word, trans) in keys.iter_mut() {
        writeln!(w, "{}\t{}", word, trans.join("\t"))?;
        n += 1;
    }
    w.commit()?;
    Ok(n)
}

/// 校对义项插入到对应词的翻译列表最前(ECDICT 义项去重续后),整体截到 `cap`。
/// 校对表键为全大写词形(WHO),运行时键为小写(who),归并时统一小写。
fn merge_with_acronyms(
    mut keys: BTreeMap<String, Vec<String>>,
    acronyms: &BTreeMap<String, Vec<String>>,
    cap: usize,
) -> BTreeMap<String, Vec<String>> {
    for (word, trans) in acronyms {
        let lower = word.to_lowercase();
        let entry = keys.entry(lower).or_default();
        let mut merged = trans.clone();
        for t in entry.drain(..) {
            if !merged.contains(&t) {
                merged.push(t);
            }
        }
        merged.truncate(cap);
        *entry = merged;
    }
    keys
}

/// 解析缩略语校对表(格式见 acronyms.tsv 头注释);非法行静默跳过。
pub fn load_acronyms(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split('\t');
        let (Some(word), Some(t0)) = (it.next(), it.next()) else {
            continue;
        };
        let word = word.trim();
        if word.is_empty() || !word.chars().all(|c| c.is_ascii_uppercase()) {
            continue;
        }
        let mut trans: Vec<String> = vec![t0.trim().to_string()];
        trans.extend(it.map(|t| t.trim().to_string()));
        trans.retain(|t| !t.is_empty());
        if !trans.is_empty() {
            m.insert(word.to_string(), trans);
        }
    }
    m
}

/// 一段释义 → 清洗后的义项行列表(按行拆分、剥标签、保序去重)。
pub fn translation_lines(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in raw.split('\n') {
        if let Some(t) = clean_translation(line) {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// 清洗单条义项:剥词性标签(n./v./abbr./pron. …)与领域标签([计] 等,
/// 可叠 2~3 层,如 "[计] abbr. xxx");清洗后不含 CJK 的行丢弃(纯英文/音标噪声)。
///
/// 词性标签判定:连续 ≤6 个 ASCII 字母后跟 '.',且 '.' 后是空格或 CJK
/// ——兼容 "n. 电脑" 与 "n.电脑" 两种写法,同时避免误剥 "U.S. 美国"。
pub fn clean_translation(line: &str) -> Option<String> {
    let mut s = line.trim();
    for _ in 0..4 {
        s = s.trim();
        if s.starts_with('[') {
            if let Some(end) = s.find(']') {
                s = &s[end + 1..];
                continue;
            }
            return None; // '[[' 歧义残行,丢弃
        }
        if let Some(dot) = s.find('.') {
            let tag = &s[..dot];
            if !tag.is_empty()
                && tag.len() <= 6
                && tag.chars().all(|c| c.is_ascii_alphabetic())
                && s[dot + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c == ' ' || is_cjk_char(c))
            {
                s = &s[dot + 1..];
                continue;
            }
        }
        break;
    }
    let s = s.trim();
    if s.is_empty() || !s.chars().any(is_cjk_char) {
        return None;
    }
    Some(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 清洗_剥词性与领域标签() {
        assert_eq!(clean_translation("n. 电脑, 电子计算机").as_deref(), Some("电脑, 电子计算机"));
        assert_eq!(clean_translation("[计] 中央处理器").as_deref(), Some("中央处理器"));
        assert_eq!(clean_translation("pron. 谁").as_deref(), Some("谁"));
        assert_eq!(clean_translation("abbr. 世界卫生组织").as_deref(), Some("世界卫生组织"));
        assert_eq!(clean_translation("[计] abbr. 中央处理器").as_deref(), Some("中央处理器"));
        assert_eq!(clean_translation("n.电脑").as_deref(), Some("电脑"));
        // 叠层标签 + 去空格
        assert_eq!(clean_translation("  [网络]  n. 流量 ").as_deref(), Some("流量"));
    }

    #[test]
    fn 清洗_保留误剥防御与噪声过滤() {
        // 词形缩写不误剥('.' 后非空格非 CJK)
        assert_eq!(clean_translation("U.S. type").is_none(), true); // 无中文 → 丢弃
        // 无 CJK 的行丢弃
        assert_eq!(clean_translation("n. abbr. for something"), None);
        assert_eq!(clean_translation("   "), None);
        // 纯中文正常保留
        assert_eq!(clean_translation("国内生产总值").as_deref(), Some("国内生产总值"));
    }

    #[test]
    fn 多行释义_拆分清洗去重() {
        let raw = "中央处理器\n[计] 中央处理器\nn. 芯片\n cpu schedule";
        assert_eq!(translation_lines(raw), ["中央处理器", "芯片"]);
    }

    #[test]
    fn 校对表解析_非法行跳过() {
        let text = "# 注释\nWHO\t世界卫生组织\nlower\t小写\t跳过\nBAD\n\nCPU\t中央处理器\t芯片\n";
        let m = load_acronyms(text);
        assert_eq!(m.get("WHO").map(|v| v.as_slice()), Some(&["世界卫生组织".to_string()][..]));
        assert_eq!(m.get("CPU").map(|v| v.as_slice()), Some(&["中央处理器".to_string(), "芯片".to_string()][..]));
        assert!(!m.contains_key("lower"), "小写键必须跳过");
        assert!(!m.contains_key("BAD"), "无译法行跳过");
    }

    #[test]
    fn 合并_校对义项恒排最前且截断() {
        let mut keys = BTreeMap::new();
        keys.insert(
            "who".to_string(),
            vec!["谁".to_string(), "谁呀".to_string(), "什么人".to_string()],
        );
        keys.insert("zzz".to_string(), vec!["无校对".to_string()]);
        let acr = load_acronyms("WHO\t世界卫生组织\t谁\n");
        // 复用 write_en_trans 的合并逻辑(拆出纯函数便于断言)
        let merged = merge_with_acronyms(keys, &acr, 3);
        assert_eq!(
            merged.get("who").map(|v| v.as_slice()),
            Some(&["世界卫生组织".to_string(), "谁".to_string(), "谁呀".to_string()][..]),
            "校对义项在前,ECDICT 义项去重续后,超限截断"
        );
        assert_eq!(merged.get("zzz").map(|v| v.as_slice()), Some(&["无校对".to_string()][..]));
    }

    #[test]
    fn 从内存库生成_大写词形与小写词形归并() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE stardict (word TEXT, translation TEXT);
             INSERT INTO stardict VALUES ('CPU', '中央处理器');
             INSERT INTO stardict VALUES ('cpu', 'n. 中央处理器\n[计] 中央处理器');
             INSERT INTO stardict VALUES ('WHO', 'pron. 谁');
             INSERT INTO stardict VALUES ('go', 'vi. 去\nvt. 走');
             INSERT INTO stardict VALUES ('Mixed', 'n. 混合词形不收录');
             INSERT INTO stardict VALUES ('unlisted', 'n. 词表外小写词不收录');",
        )
        .unwrap();
        let mut universe = HashSet::new();
        universe.insert("go".to_string());
        let dst = std::env::temp_dir().join(format!("en_trans-test-{}.tsv", std::process::id()));
        let n = entrans_from_conn(&conn, &universe, &dst).unwrap();
        let content = std::fs::read_to_string(&dst).unwrap();
        let _ = std::fs::remove_file(&dst);
        let lines = content.lines().collect::<Vec<_>>();
        assert!(lines.is_sorted(), "输出按词升序: {content}");
        let find = |w: &str| lines.iter().find(|l| l.starts_with(&format!("{w}\t"))).map(|l| *l);
        // cpu:'CPU'/'cpu' 两种词形义项归并去重(资源表仅一条校对义项)
        assert_eq!(find("cpu"), Some("cpu\t中央处理器"));
        assert_eq!(
            find("who"),
            Some("who\t世界卫生组织\t谁"),
            "WHO 缺词由校对表兜底,who 的 ECDICT 义项去重续后"
        );
        assert_eq!(find("go"), Some("go\t去\t走"));
        assert!(find("mixed").is_none() && find("unlisted").is_none());
        assert_eq!(n, lines.len());
    }
}
