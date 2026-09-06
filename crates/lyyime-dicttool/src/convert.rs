//! `convert`:读取本机 ibus-table sqlite(默认海峰86)→ 生成 data/runtime 下的 TSV。
//!
//! 产物格式见 docs/ARCHITECTURE.md §4:
//! - wubi.tsv        `code\tword\tfreq`   按 code 升序、同 code 内 freq 降序
//! - pinyin_char.tsv `pinyin\tchar\tfreq` 声调归一化,按 pinyin 升序、freq 降序
//! - suggestion.tsv  `word\tfreq`         freq 降序
//! - goucima.tsv     `zi\tgoucima`        按 zi 码点升序
//! - meta.json       版本/来源/行数/构建时间
//!
//! 实现要点:
//! - 全程流式:排序/去重下推到 SQLite(GROUP BY MAX + ORDER BY),Rust 侧 O(1) 额外内存;
//! - 幂等:输出先写 .tmp 再原子改名,同输入重跑产物字节一致(meta.built_at 除外);
//! - 脏数据容错:NULL/空值/不可见控制字符行被跳过或剥离,统计后打印;
//! - 海峰隐藏词条(词尾「.」标记的生僻字,见 convert_wubi)不进入 wubi.tsv。

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::json;

use crate::util::{strip_control, AtomicWriter, Meta, Progress};

pub const DEFAULT_WUBI_DB: &str = "/usr/share/ibus-table/tables/wubi-haifeng86.db";

pub struct ConvertOpts {
    pub wubi_db: PathBuf,
    pub out: PathBuf,
}

pub fn run(opts: ConvertOpts) -> Result<()> {
    let db_path = opts.wubi_db.clone();
    let out = opts.out.clone();
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("以只读方式打开码表库 {:?} 失败", db_path))?;

    std::fs::create_dir_all(&out).with_context(|| format!("创建输出目录 {:?} 失败", out))?;

    // 从 ime 表取码表元信息(serial_number 可用于溯源)
    let serial: String = conn
        .query_row(
            "SELECT val FROM ime WHERE attr='serial_number'",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| "unknown".into());
    let sources = vec![json!({
        "name": db_path.file_name().and_then(|s| s.to_str()).unwrap_or("wubi.db"),
        "path": db_path.display().to_string(),
        "serial_number": serial,
    })];
    convert_all(&conn, &out, sources).map(|_| ())
}

/// 转换入口(测试可直接传内存库连接):生成 4 个 TSV + meta.json,返回各文件行数。
pub fn convert_all(
    conn: &Connection,
    out: &Path,
    mut sources: Vec<serde_json::Value>,
) -> Result<(u64, u64, u64, u64)> {
    eprintln!("[convert] 输出目录:{:?}", out);
    let wubi = convert_wubi(conn, &out)?;
    let pinyin_char = convert_pinyin_char(conn, &out)?;
    let suggestion = convert_suggestion(conn, &out)?;
    let goucima = convert_goucima(conn, &out)?;

    // meta.json:若已有(比如 fetch 先跑过)则保留其 sources 并补齐行数
    let mut meta = Meta::load(&out.join("meta.json"))?.unwrap_or_else(|| Meta::new(Vec::new()));
    for s in sources.drain(..) {
        if !meta.sources.iter().any(|e| e == &s) {
            meta.sources.push(s);
        }
    }
    meta.rows.insert("wubi.tsv".into(), wubi);
    meta.rows.insert("pinyin_char.tsv".into(), pinyin_char);
    meta.rows.insert("suggestion.tsv".into(), suggestion);
    meta.rows.insert("goucima.tsv".into(), goucima);
    // fetch 产物若已存在,把行数一并登记
    for name in ["pinyin_phrase.tsv", "english.tsv"] {
        let p = out.join(name);
        if p.exists() {
            let n = crate::util::count_lines(&p)?;
            meta.rows.insert(name.into(), n);
        }
    }
    meta.built_at = crate::util::now_iso();
    meta.save(&out.join("meta.json"))?;

    eprintln!(
        "[convert] 完成:wubi={} pinyin_char={} suggestion={} goucima={} -> {:?}",
        wubi, pinyin_char, suggestion, goucima, out
    );
    Ok((wubi, pinyin_char, suggestion, goucima))
}

/// phrases 表 → wubi.tsv。
///
/// 表结构实测(id, tabkeys, phrase, freq, user_freq):tabkeys 只有纯字母,无分隔符;
/// 存在少量全大写 tabkeys(1228 行,海峰“直接上屏”记法,如 'AAWI'),统一 lower 归一。
/// 海峰源库用「词尾加 .」标记隐藏词条(CJK 扩展区生僻字/兼容字,freq≤100,约 4.7 万行,
/// ibus-table 不在正常候选展示),此类字多数无字体可渲染,保留会破坏四码唯一等判定,过滤。
/// 同 (code, word) 重复行取 MAX(freq);排序下推 SQLite,流式写出。
fn convert_wubi(conn: &Connection, out: &Path) -> Result<u64> {
    let mut stmt = conn.prepare(
        "SELECT lower(tabkeys) AS code, phrase, MAX(freq) AS f
         FROM phrases
         WHERE tabkeys IS NOT NULL AND phrase IS NOT NULL AND freq IS NOT NULL
         GROUP BY lower(tabkeys), phrase
         ORDER BY code ASC, f DESC, phrase ASC",
    )?;
    let mut rows = stmt.query([])?;
    let mut w = AtomicWriter::create(&out.join("wubi.tsv"))?;
    let mut prog = Progress::new("wubi", 20_000);
    let mut skipped = 0u64;
    while let Some(r) = rows.next()? {
        let code: String = r.get(0)?;
        let word_raw: String = r.get(1)?;
        let freq: i64 = r.get(2)?;
        // 五笔码只允许 a–z;其它(脏数据)跳过
        if code.is_empty() || !code.chars().all(|c| c.is_ascii_lowercase()) {
            skipped += 1;
            continue;
        }
        // 排除不可见控制字符(保留原词,不做全半角规范化);剥离后为空或含制表符则跳过
        let word = strip_control(&word_raw);
        if word.is_empty() || word.contains('\t') || word.contains('\n') {
            skipped += 1;
            continue;
        }
        // 海峰「词尾加 .」隐藏词条(生僻字标记),见函数头注释
        if word.ends_with('.') {
            skipped += 1;
            continue;
        }
        writeln!(w, "{}\t{}\t{}", code, word, freq)?;
        prog.tick();
    }
    let n = prog.finish();
    w.commit()?;
    if skipped > 0 {
        eprintln!("[wubi] 跳过脏数据 {} 行", skipped);
    }
    Ok(n)
}

/// pinyin 表 → pinyin_char.tsv。
///
/// 声调归一化在 SQL 里完成(剥后缀符号,轻声 `%` 同样剥掉,见 util::normalize_pinyin 注释),
/// 使同一音节的不同声调行在流式输出中天然相邻,便于 GROUP BY 去重。
fn convert_pinyin_char(conn: &Connection, out: &Path) -> Result<u64> {
    let norm = "CASE WHEN substr(pinyin,-1) IN ('!','#','$','%','@')
                 THEN substr(pinyin,1,length(pinyin)-1) ELSE pinyin END";
    let sql = format!(
        "SELECT {norm} AS py, zi, MAX(freq) AS f
         FROM pinyin
         WHERE pinyin IS NOT NULL AND zi IS NOT NULL AND freq IS NOT NULL
         GROUP BY {norm}, zi
         ORDER BY py ASC, f DESC, zi ASC",
        norm = norm
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut w = AtomicWriter::create(&out.join("pinyin_char.tsv"))?;
    let mut prog = Progress::new("pinyin_char", 10_000);
    let mut skipped = 0u64;
    while let Some(r) = rows.next()? {
        let py_raw: String = r.get(0)?;
        let zi_raw: String = r.get(1)?;
        let freq: i64 = r.get(2)?;
        // 防御性归一化(再剥数字声调),非法音节跳过
        let Some(py) = crate::util::normalize_pinyin(&py_raw) else {
            skipped += 1;
            continue;
        };
        let zi = strip_control(&zi_raw);
        // pinyin 表是单字表:zi 必须恰好一个字符
        if zi.chars().count() != 1 {
            skipped += 1;
            continue;
        }
        writeln!(w, "{}\t{}\t{}", py, zi, freq)?;
        prog.tick();
    }
    let n = prog.finish();
    w.commit()?;
    if skipped > 0 {
        eprintln!("[pinyin_char] 跳过脏数据 {} 行", skipped);
    }
    Ok(n)
}

/// suggestion 表 → suggestion.tsv(通用词频,排序兜底)。同词取 MAX(freq)。
fn convert_suggestion(conn: &Connection, out: &Path) -> Result<u64> {
    let mut stmt = conn.prepare(
        "SELECT phrase, MAX(freq) AS f
         FROM suggestion
         WHERE phrase IS NOT NULL AND freq IS NOT NULL
         GROUP BY phrase
         ORDER BY f DESC, phrase ASC",
    )?;
    let mut rows = stmt.query([])?;
    let mut w = AtomicWriter::create(&out.join("suggestion.tsv"))?;
    let mut prog = Progress::new("suggestion", 20_000);
    let mut skipped = 0u64;
    while let Some(r) = rows.next()? {
        let word_raw: String = r.get(0)?;
        let freq: i64 = r.get(1)?;
        let word = strip_control(&word_raw);
        if word.is_empty() || word.contains('\t') || word.contains('\n') {
            skipped += 1;
            continue;
        }
        writeln!(w, "{}\t{}", word, freq)?;
        prog.tick();
    }
    let n = prog.finish();
    w.commit()?;
    if skipped > 0 {
        eprintln!("[suggestion] 跳过脏数据 {} 行", skipped);
    }
    Ok(n)
}

/// goucima 表 → goucima.tsv(单字构词码,自造词用,v1 引擎可不消费)。
fn convert_goucima(conn: &Connection, out: &Path) -> Result<u64> {
    let mut stmt = conn.prepare(
        "SELECT zi, goucima FROM goucima
         WHERE zi IS NOT NULL AND goucima IS NOT NULL
         ORDER BY zi ASC",
    )?;
    let mut rows = stmt.query([])?;
    let mut w = AtomicWriter::create(&out.join("goucima.tsv"))?;
    let mut prog = Progress::new("goucima", 10_000);
    let mut skipped = 0u64;
    while let Some(r) = rows.next()? {
        let zi_raw: String = r.get(0)?;
        let code_raw: String = r.get(1)?;
        let zi = strip_control(&zi_raw);
        let code = strip_control(&code_raw);
        if zi.is_empty() || !code.chars().all(|c| c.is_ascii_lowercase()) {
            skipped += 1;
            continue;
        }
        writeln!(w, "{}\t{}", zi, code)?;
        prog.tick();
    }
    let n = prog.finish();
    w.commit()?;
    if skipped > 0 {
        eprintln!("[goucima] 跳过脏数据 {} 行", skipped);
    }
    Ok(n)
}
