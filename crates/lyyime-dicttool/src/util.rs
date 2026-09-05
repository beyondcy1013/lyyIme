//! 公共工具:进度输出、原子写、拼音归一化、CJK 判定、meta.json。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 每隔 N 行向 stderr 打印一次进度的计数器(流式转换用,内存 O(1))。
pub struct Progress {
    name: String,
    every: u64,
    count: u64,
}

impl Progress {
    pub fn new(name: &str, every: u64) -> Self {
        Self { name: name.to_string(), every, count: 0 }
    }
    pub fn tick(&mut self) {
        self.count += 1;
        if self.count % self.every == 0 {
            eprintln!("[{}] {:>9} 行 ...", self.name, self.count);
        }
    }
    pub fn finish(&self) -> u64 {
        eprintln!("[{}] 共 {} 行", self.name, self.count);
        self.count
    }
}

/// 原子写文件:先写 `<path>.tmp`,提交时改名,保证产物不出现半截文件(幂等重跑安全)。
pub struct AtomicWriter {
    writer: BufWriter<File>,
    tmp: PathBuf,
    dst: PathBuf,
}

impl AtomicWriter {
    pub fn create(dst: &Path) -> Result<Self> {
        let tmp = dst.with_extension("tsv.tmp");
        let writer = BufWriter::new(File::create(&tmp).with_context(|| format!("创建临时文件 {:?}", tmp))?);
        Ok(Self { writer, tmp, dst: dst.to_path_buf() })
    }
    pub fn commit(mut self) -> Result<()> {
        self.writer.flush()?;
        drop(self.writer);
        std::fs::rename(&self.tmp, &self.dst).with_context(|| format!("改名 {:?} -> {:?}", self.tmp, self.dst))?;
        Ok(())
    }
}

impl Write for AtomicWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

/// 去掉不可见控制字符(C0/C1,含 \t \r \n)。词本身保留原样,不做全半角规范化。
pub fn strip_control(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// ibus-table 拼音归一化 → 无声调小写。
///
/// 本机 wubi-haifeng86.db 的 pinyin 列带声调后缀符号(实测):
///   `!`=阴平(zhong!→zhōng) `@`=阳平(de@→dé) `#`=上声(ai#→ǎi) `$`=去声(zhong$→zhòng)
///   `%`=轻声(de%→de,的/得/地)。
/// 轻声处理:`%` 与其它声调符号一样直接剥掉,不保留任何轻声标记(de% → de)。
/// 另做防御:若遇到数字声调后缀(如 zhong1 / xi1)同样剥掉,仅保留 a–z。
pub fn normalize_pinyin(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_suffix(['!', '@', '#', '$', '%']) {
        s = rest;
    }
    loop {
        match s.chars().last() {
            Some(c) if c.is_ascii_digit() => s = &s[..s.len() - c.len_utf8()],
            _ => break,
        }
    }
    let out: String = s.chars().filter(|c| c.is_ascii_lowercase()).collect();
    if out.is_empty() { None } else { Some(out) }
}

/// 带调拼音音节(mozillazg/phrase-pinyin-data 风格,如 `zhōng`、`lǜ`)→ 无声调小写。
/// `ü` 归一为 `v`,与本机 ibus 码表 lv/nv 约定一致。非法字符导致返回 None。
pub fn toneless_syllable(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.trim().chars() {
        // 先小写化(带调字母也有大写形式)
        let lower = c.to_lowercase().next().unwrap_or(c);
        let plain = match lower {
            'ā' | 'á' | 'ǎ' | 'à' | 'a' => 'a',
            'ē' | 'é' | 'ě' | 'è' | 'ê' | 'e' => 'e',
            'ī' | 'í' | 'ǐ' | 'ì' | 'i' => 'i',
            'ō' | 'ó' | 'ǒ' | 'ò' | 'o' => 'o',
            'ū' | 'ú' | 'ǔ' | 'ù' | 'u' => 'u',
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ü' => 'v',
            'ń' | 'ň' | 'ǹ' | 'n' => 'n',
            'ḿ' | 'm' => 'm',
            c if c.is_ascii_lowercase() => c,
            _ => return None, // 数字、符号、其它文字 → 该音节不可用
        };
        out.push(plain);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// CJK 统一表意文字判断(CJK 基本区 + 扩展 A)。fetch 阶段过滤词组用。
pub fn is_cjk_char(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF)
}

pub fn is_cjk_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_cjk_char)
}

/// Unix 时间戳 → ISO8601(UTC)。手写 civil_from_days,避免引入 chrono。
pub fn epoch_to_iso(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60
    )
}

pub fn now_iso() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    epoch_to_iso(secs)
}

/// data/runtime/meta.json 的结构(ARCHITECTURE.md §4 / 任务规定的字段)。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Meta {
    pub version: u32,
    pub sources: Vec<serde_json::Value>,
    pub rows: BTreeMap<String, u64>,
    pub built_at: String,
}

impl Meta {
    pub fn new(sources: Vec<serde_json::Value>) -> Self {
        Self { version: 1, sources, rows: BTreeMap::new(), built_at: now_iso() }
    }

    pub fn load(path: &Path) -> Result<Option<Meta>> {
        match File::open(path) {
            Ok(f) => {
                let meta: Meta = serde_json::from_reader(f).context("解析 meta.json 失败")?;
                Ok(Some(meta))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("读取 {:?} 失败", path)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).context("序列化 meta.json 失败")?;
        let mut w = AtomicWriter::create(path)?;
        writeln!(w, "{}", text)?;
        w.commit()
    }
}

/// 统计文本文件行数(verify / meta 用)。
pub fn count_lines(path: &Path) -> Result<u64> {
    let f = File::open(path).with_context(|| format!("打开 {:?} 失败", path))?;
    let mut n = 0u64;
    for line in BufReader::new(f).lines() {
        line?;
        n += 1;
    }
    Ok(n)
}
