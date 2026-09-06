//! lyyIme 悬浮窗词典: 从 ibus-table SQLite 码表构建内存前缀索引。
//!
//! 移植自 floatapp/dictload.py(已删除): 支持 ibus-table 1.17 schema
//! phrases(id, tabkeys, phrase, freq, user_freq)。极点86/海峰86 均可加载。
//! 前缀索引一次性建好, 查询 O(1); 用户词学习按 freq + 10_000_000×次数 加权,
//! 精确码命中加 1<<62 恒置顶(与 Python 版语义一致)。

use anyhow::{Context, Result};
use rusqlite::{OpenFlags, Connection};
use std::collections::{HashMap, HashSet};

pub struct WubiDict {
    /// code -> [(freq, phrase)] 频率降序
    exact: HashMap<String, Vec<(i64, String)>>,
    /// 任意前缀 -> [(freq, phrase)] 频率降序
    prefix: HashMap<String, Vec<(i64, String)>>,
}

impl WubiDict {
    pub fn load(db_path: &str) -> Result<WubiDict> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .with_context(|| format!("打开码表失败: {db_path}"))?;
        let mut stmt =
            conn.prepare("SELECT tabkeys, phrase, freq FROM phrases")?;
        let mut rows = stmt.query([])?;
        let mut exact: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        let mut prefix: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        while let Some(row) = rows.next()? {
            let code: String = row.get(0)?;
            let phrase: String = row.get(1)?;
            let freq: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
            let code = code.to_lowercase();
            if code.is_empty() {
                continue;
            }
            let bytes = code.as_bytes();
            for i in 1..=bytes.len() {
                // 按字节切前缀: 码表编码均为 ASCII 字母, 不会劈开 UTF-8
                let p = std::str::from_utf8(&bytes[..i]).unwrap();
                prefix.entry(p.to_string()).or_default().push((freq, phrase.clone()));
            }
            exact.entry(code).or_default().push((freq, phrase));
        }
        for v in exact.values_mut() {
            v.sort_by(|a, b| b.0.cmp(&a.0));
        }
        for v in prefix.values_mut() {
            v.sort_by(|a, b| b.0.cmp(&a.0));
        }
        Ok(WubiDict { exact, prefix })
    }

    /// 查候选: 精确匹配优先, 其后按频率; 去重; 最多 limit 条。
    /// 返回 [(phrase, score)] — score 已并入用户学习加权(供合并层排序显示)。
    pub fn lookup(
        &self,
        code: &str,
        limit: usize,
        user_freq: &HashMap<String, i64>,
    ) -> Vec<(String, i64)> {
        let code = code.trim().to_lowercase();
        if code.is_empty() {
            return Vec::new();
        }
        let mut seen: HashSet<&str> = HashSet::new();
        let mut merged: Vec<(i64, String)> = Vec::new();
        push_merged(self.exact.get(&code), true, user_freq, &mut seen, &mut merged);
        push_merged(self.prefix.get(&code), false, user_freq, &mut seen, &mut merged);
        merged.sort_by(|a, b| b.0.cmp(&a.0));
        merged.truncate(limit);
        merged.into_iter().map(|(s, p)| (p, s)).collect()
    }
}

/// 把一组(已按频率降序的)候选并入 merged, 去重并计分。
/// 计分: freq + 10_000_000×学习次数 + 精确码奖励 1<<62。
fn push_merged<'a>(
    list: Option<&'a Vec<(i64, String)>>,
    is_exact: bool,
    user_freq: &HashMap<String, i64>,
    seen: &mut HashSet<&'a str>,
    merged: &mut Vec<(i64, String)>,
) {
    if let Some(list) = list {
        for (freq, phrase) in list {
            if seen.insert(phrase.as_str()) {
                let score = freq
                    + user_freq.get(phrase).copied().unwrap_or(0) * 10_000_000
                    + if is_exact { 1i64 << 62 } else { 0 };
                merged.push((score, phrase.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_merges_exact_prefix_with_learning_weight() {
        let mut d = WubiDict {
            exact: HashMap::new(),
            prefix: HashMap::new(),
        };
        d.exact.insert(
            "wq".into(),
            vec![(10, "你".into()), (5, "尼".into())],
        );
        d.prefix.insert(
            "wq".into(),
            vec![(3, "你".into()), (2, "拟".into()), (9, "呢".into())],
        );
        let mut uf = HashMap::new();
        uf.insert("尼".to_string(), 2); // 学习加权应把"尼"抬到无加权"你"前

        let got = d.lookup("wq", 45, &HashMap::new());
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, vec!["你", "尼", "呢", "拟"]); // 精确优先, 前缀按频

        let got = d.lookup("wq", 45, &uf);
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        // "尼"带 2 次学习: 5 + 2e7 > 10 → 但它属精确组, 仍在"你"(无学习)之前?
        // 你=10, 尼=5+2e7 → 尼第一
        assert_eq!(texts, vec!["尼", "你", "呢", "拟"]);
    }

    #[test]
    fn empty_and_missing_codes() {
        let d = WubiDict { exact: HashMap::new(), prefix: HashMap::new() };
        assert!(d.lookup("", 45, &HashMap::new()).is_empty());
        assert!(d.lookup("zzzz", 45, &HashMap::new()).is_empty());
    }
}
