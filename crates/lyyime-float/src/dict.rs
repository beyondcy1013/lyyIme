//! lyyIme 悬浮窗词典: 从 ibus-table SQLite 码表构建内存前缀索引。
//!
//! 移植自 floatapp/dictload.py(已删除): 支持 ibus-table 1.17 schema
//! phrases(id, tabkeys, phrase, freq, user_freq)。极点86/海峰86 均可加载。
//! 前缀索引一次性建好, 查询 O(1); 用户词学习按 freq + 10_000_000×次数 加权,
//! 精确码命中加 1<<62 恒置顶(与 Python 版语义一致); 精确层内四码全码单字
//! 再优先于同码词组(与 core 引擎排序合同一致), 词频/学习加权不跨此界。
//! 另加载 core 词库的 pinyin_char.tsv 作为单字真实语料频次(码表 freq 对大量
//! 字是默认值, 排序噪声大): 精确层单字按 语料内频次 > 语料外(沉底) 排序。

use anyhow::{Context, Result};
use rusqlite::{OpenFlags, Connection};
use std::collections::{HashMap, HashSet};

pub struct WubiDict {
    /// code -> [(freq, phrase)] 频率降序
    exact: HashMap<String, Vec<(i64, String)>>,
    /// 任意前缀 -> [(freq, phrase)] 频率降序
    prefix: HashMap<String, Vec<(i64, String)>>,
    /// 字 → 真实语料频次(数据目录缺失时为空表 → 全部按码表频)。
    char_corpus: HashMap<char, i64>,
    /// 字 → GB2312 分档(1=一级常用,2=二级次常用;表外=生僻沉底)。
    /// 拼音语料对生僻字是填充值,分档表才是"常用字"的可靠依据。
    char_tier: HashMap<char, u8>,
}

impl WubiDict {
    pub fn load(db_path: &str) -> Result<WubiDict> {
        Self::load_with_data(db_path, None)
    }

    /// `data_dir` 为 core 词库目录(pinyin_char.tsv 语料 + char_tier.tsv 分档);
    /// 缺失/损坏的文件静默降级(无语料/无分档),不阻塞码表加载。
    pub fn load_with_data(db_path: &str, data_dir: Option<&str>) -> Result<WubiDict> {
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
        let (char_corpus, char_tier) = match data_dir {
            Some(dir) => (
                load_corpus(&format!("{dir}/pinyin_char.tsv")).unwrap_or_default(),
                load_tier(&format!("{dir}/char_tier.tsv")).unwrap_or_default(),
            ),
            None => (HashMap::new(), HashMap::new()),
        };
        Ok(WubiDict { exact, prefix, char_corpus, char_tier })
    }

    /// 查候选: 精确匹配优先, 层内单字按 GB2312 分档(一级 > 二级 > 词组 > 生僻),
    /// 档内按真实语料频次(语料外沉底), 其后按频率; 去重; 最多 limit 条。
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
        // (score, 组序, 组内序键, phrase): 组序降序 一级单字3 > 二级单字2 >
        // 词组/前缀1 > 生僻单字0; 组内序键 = 语料频次(生僻/无语料 0)。
        // 无分档表时退化为 单字2 > 词组/前缀1(旧行为)。
        let mut merged: Vec<(i64, i32, i64, String)> = Vec::new();
        push_merged(self.exact.get(&code), true, self, user_freq, &mut seen, &mut merged);
        push_merged(self.prefix.get(&code), false, self, user_freq, &mut seen, &mut merged);
        merged.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then(b.2.cmp(&a.2))
                .then(b.0.cmp(&a.0))
        });
        merged.truncate(limit);
        merged.into_iter().map(|(s, _, _, p)| (p, s)).collect()
    }
}

/// 把一组(已按频率降序的)候选并入 merged, 去重并计分。
/// 计分: freq + 10_000_000×学习次数 + 精确码奖励 1<<62。
fn push_merged<'a>(
    list: Option<&'a Vec<(i64, String)>>,
    is_exact: bool,
    d: &WubiDict,
    user_freq: &HashMap<String, i64>,
    seen: &mut HashSet<&'a str>,
    merged: &mut Vec<(i64, i32, i64, String)>,
) {
    if let Some(list) = list {
        for (freq, phrase) in list {
            if seen.insert(phrase.as_str()) {
                let score = freq
                    + user_freq.get(phrase).copied().unwrap_or(0) * 10_000_000
                    + if is_exact { 1i64 << 62 } else { 0 };
                let mut chs = phrase.chars();
                let ch = chs.next();
                let single = ch.is_some() && chs.next().is_none();
                let (group, corpus_key) = if !is_exact || !single {
                    (1, 0)
                } else if d.char_tier.is_empty() {
                    // 无分档表: 单字整体一组, 组内按语料频(无语料 0, 表频兜底)
                    (
                        2,
                        ch.and_then(|c| d.char_corpus.get(&c)).map(|f| f + 1).unwrap_or(0),
                    )
                } else {
                    match ch.and_then(|c| d.char_tier.get(&c)) {
                        Some(1) => (
                            3,
                            ch.and_then(|c| d.char_corpus.get(&c)).copied().unwrap_or(0),
                        ),
                        Some(_) => (
                            2,
                            ch.and_then(|c| d.char_corpus.get(&c)).copied().unwrap_or(0),
                        ),
                        None => (0, 0), // 生僻/繁体/扩展字: 沉到词组后
                    }
                };
                merged.push((score, group, corpus_key, phrase.clone()));
            }
        }
    }
}

/// 解析 core 词库 pinyin_char.tsv(`pinyin\tchar\tfreq`), 同字取最大频次;
/// 文件缺失返回 None(调用方降级为无语料), 个别坏行跳过。
fn load_corpus(path: &str) -> Option<HashMap<char, i64>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut m: HashMap<char, i64> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split('\t');
        let Some(py) = it.next() else { continue };
        if py.trim().is_empty() {
            continue;
        }
        let Some(ch) = it.next().and_then(|s| s.trim().chars().next()) else {
            continue;
        };
        let Some(freq) = it.next().and_then(|s| s.trim().parse::<i64>().ok()) else {
            continue;
        };
        let slot = m.entry(ch).or_insert(0);
        if freq > *slot {
            *slot = freq;
        }
    }
    Some(m)
}

/// 解析 GB2312 分档表(`char\ttier`,dicttool tier 产物);
/// 文件缺失返回 None(调用方降级为无分档), 非 1/2 的档位行跳过。
fn load_tier(path: &str) -> Option<HashMap<char, u8>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut m: HashMap<char, u8> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split('\t');
        let Some(ch) = it.next().and_then(|s| s.trim().chars().next()) else {
            continue;
        };
        if let Ok(tier @ 1..=2) = it.next().unwrap_or("").trim().parse::<u8>() {
            m.insert(ch, tier);
        }
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_merges_exact_prefix_with_learning_weight() {
        let mut d = WubiDict {
            exact: HashMap::new(),
            prefix: HashMap::new(),
            char_corpus: HashMap::new(),
            char_tier: HashMap::new(),
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
        let d = WubiDict {
            exact: HashMap::new(),
            prefix: HashMap::new(),
            char_corpus: HashMap::new(),
            char_tier: HashMap::new(),
        };
        assert!(d.lookup("", 45, &HashMap::new()).is_empty());
        assert!(d.lookup("zzzz", 45, &HashMap::new()).is_empty());
    }

    #[test]
    fn lookup_四码单字优先于同码词组() {
        let mut d = WubiDict {
            exact: HashMap::new(),
            prefix: HashMap::new(),
            char_corpus: HashMap::new(),
            char_tier: HashMap::new(),
        };
        // 词组频次更高且被学习加权, 单字仍须居首(与 core 排序合同一致)。
        d.exact.insert(
            "gcft".into(),
            vec![(1200, "死难者".into()), (900, "致".into())],
        );
        d.prefix.insert("gcft".into(), vec![(500, "死无对证".into())]);
        let mut uf = HashMap::new();
        uf.insert("死难者".to_string(), 3);

        let got = d.lookup("gcft", 45, &HashMap::new());
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, vec!["致", "死难者", "死无对证"]);

        let got = d.lookup("gcft", 45, &uf);
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, vec!["致", "死难者", "死无对证"], "学习加权不跨单字界");
    }

    #[test]
    fn lookup_分档表驱动_生僻单字沉到词组后() {
        let mut d = WubiDict {
            exact: HashMap::new(),
            prefix: HashMap::new(),
            char_corpus: HashMap::new(),
            char_tier: HashMap::new(),
        };
        // 镜像真库 thgj: 牏(表频默认 1000, 语料填充值)不在 GB2312,
        // 词组「处理」须排其前; 一级字「引」压过词组「引子」。
        d.char_tier.insert('引', 1);
        d.char_corpus.insert('引', 900_000_000);
        d.exact.insert(
            "thgj".into(),
            vec![(1000, "牏".into()), (95, "㸟".into()), (500, "处理".into())],
        );
        d.exact.insert(
            "xxyy".into(),
            vec![(1000, "引".into()), (1500, "引子".into())],
        );
        let texts: Vec<String> = ["thgj", "xxyy"]
            .iter()
            .flat_map(|c| d.lookup(c, 45, &HashMap::new()))
            .map(|(t, _)| t)
            .collect();
        assert_eq!(texts, vec!["处理", "牏", "㸟", "引", "引子"]);
    }

    #[test]
    fn lookup_单字按语料频次排_语料外沉底() {
        let mut d = WubiDict {
            exact: HashMap::new(),
            prefix: HashMap::new(),
            char_corpus: HashMap::new(),
            char_tier: HashMap::new(),
        };
        // 码表频把生僻字排前: 靷(2000) > 引(1000); 语料反转: 引 9e8 >> 靷 5e3;
        // 齾 无语料 → 沉底; 学习加权也压不过语料序。
        d.char_corpus.insert('引', 900_000_000);
        d.char_corpus.insert('靷', 5_000);
        d.exact.insert(
            "xxyy".into(),
            vec![(2000, "靷".into()), (1000, "引".into()), (3000, "齾".into())],
        );
        let mut uf = HashMap::new();
        uf.insert("靷".to_string(), 5);

        let got = d.lookup("xxyy", 45, &HashMap::new());
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, vec!["引", "靷", "齾"]);

        let got = d.lookup("xxyy", 45, &uf);
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, vec!["引", "靷", "齾"], "学习加权不越语料序");
    }
}
