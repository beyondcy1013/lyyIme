//! dicttool CLI 入口:参数手工解析(能用 std 解决的不引第三方 clap)。

use std::path::PathBuf;

use anyhow::{bail, Result};

use lyyime_dicttool::{convert, fetch, query, tier, verify};

const USAGE: &str = "用法:
  dicttool convert [--wubi-db <path>] [--out <dir>]      ibus-table sqlite -> wubi/pinyin_char/suggestion/goucima TSV
  dicttool fetch   [--out <dir>] [--cache <dir>]         下载拼音词组与英文词频 -> pinyin_phrase/english TSV
  dicttool verify  [<dir>]                               行数/格式/排序/抽样断言
  dicttool tier    [--out <dir>]                         生成 GB2312 单字分档表 char_tier.tsv
  dicttool query   wubi|pinyin|en <query> [--out <dir>]  对 TSV 做前缀查询,打印前 9 个候选

默认:wubi-db=/usr/share/ibus-table/tables/wubi-haifeng86.db,out=data/runtime,cache=dicts/raw";

/// 解析结果:剩余位置参数 + 各路径选项。
struct Parsed {
    positionals: Vec<String>,
    out: PathBuf,
    cache: PathBuf,
    wubi_db: PathBuf,
}

/// 摘除 `--out/--cache/--wubi-db <value>` 选项(可出现在任意位置),其余为位置参数。
fn parse_args(args: Vec<String>) -> Result<Parsed> {
    let mut positionals = Vec::new();
    let (mut out, mut cache, mut wubi_db): (Option<String>, Option<String>, Option<String>) =
        (None, None, None);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if matches!(a, "--out" | "--cache" | "--wubi-db") {
            if i + 1 >= args.len() {
                bail!("{} 缺少取值", a);
            }
            let val = args[i + 1].clone();
            match a {
                "--out" => out = Some(val),
                "--cache" => cache = Some(val),
                "--wubi-db" => wubi_db = Some(val),
                _ => unreachable!(),
            }
            i += 2;
        } else if a.starts_with("--") {
            bail!("未知选项 {}", a);
        } else {
            positionals.push(args[i].clone());
            i += 1;
        }
    }
    Ok(Parsed {
        positionals,
        out: PathBuf::from(out.unwrap_or_else(|| "data/runtime".into())),
        cache: PathBuf::from(cache.unwrap_or_else(|| "dicts/raw".into())),
        wubi_db: PathBuf::from(wubi_db.unwrap_or_else(|| convert::DEFAULT_WUBI_DB.into())),
    })
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        println!("{}", USAGE);
        return std::process::ExitCode::SUCCESS;
    }
    match dispatch(args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dicttool 错误: {:#}", e);
            std::process::ExitCode::FAILURE
        }
    }
}

fn dispatch(args: Vec<String>) -> Result<()> {
    let mut it = args.into_iter();
    let cmd = it.next().unwrap_or_default();
    let parsed = parse_args(it.collect())?;
    match cmd.as_str() {
        "convert" => {
            if !parsed.positionals.is_empty() {
                bail!("convert 不接受位置参数:{:?}", parsed.positionals);
            }
            convert::run(convert::ConvertOpts { wubi_db: parsed.wubi_db, out: parsed.out })
        }
        "fetch" => {
            if !parsed.positionals.is_empty() {
                bail!("fetch 不接受位置参数:{:?}", parsed.positionals);
            }
            fetch::run(fetch::FetchOpts { out: parsed.out, cache: parsed.cache })
        }
        "tier" => {
            if !parsed.positionals.is_empty() {
                bail!("tier 不接受位置参数:{:?}", parsed.positionals);
            }
            tier::run(&parsed.out)
        }
        "verify" => {
            let mut pos = parsed.positionals;
            let dir = match pos.len() {
                0 => parsed.out,
                1 => PathBuf::from(pos.remove(0)),
                _ => bail!("verify 只接受一个目录参数"),
            };
            verify::run(dir)
        }
        "query" => {
            let mut pos = parsed.positionals;
            if pos.len() != 2 {
                bail!("query 需要 <类型 wubi|pinyin|en> <查询串> 两个参数");
            }
            let q = pos.pop().unwrap();
            let kind = pos.pop().unwrap();
            query::run(&kind, &q, &parsed.out)
        }
        _ => bail!("未知子命令 {:?}\n{}", cmd, USAGE),
    }
}
