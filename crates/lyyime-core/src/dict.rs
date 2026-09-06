//! 码表/词库加载与内存索引(HashMap + 前缀索引)。
//!
//! 数据为 dicttool 产物 TSV(格式见 ARCHITECTURE.md §4),UTF-8,`#` 开头与空行忽略:
//!
//! | 文件 | 列 |
//! |---|---|
//! | wubi.tsv | code \t word \t freq |
//! | pinyin_char.tsv | pinyin \t char \t freq |
//! | pinyin_phrase.tsv | word \t pinyin(空格分隔音节) \t freq |
//! | english.tsv | word \t freq |
//! | suggestion.tsv | word \t freq(通用词频,排序兜底) |
//! | meta.json | 元信息(仅校验用途,损坏忽略) |
//!
//! 降级策略:单个文件缺失/个别行格式坏 → 跳过,只降级对应通道,不影响其它通道;
//! 数据目录不存在 → 得到全空索引(Engine 仍可用)。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// 五笔词条:编码 + 词 + 词频。
#[derive(Debug)]
pub(crate) struct WubiEntry {
    pub code: String,
    pub word: String,
    pub freq: u64,
}

/// 拼音词组词条。
#[derive(Debug)]
pub(crate) struct PhraseEntry {
    pub word: String,
    /// 无声调音节序列,如 ["ni", "hao"]。
    pub sylls: Vec<String>,
    /// 简拼串(各音节首字母连接),如 "nh"。
    pub jian: String,
    pub freq: u64,
}

/// 内存索引:五个通道各自独立,缺哪个通道对应查询为空。
pub(crate) struct DictIndex {
    // ---- 五笔通道 ----
    pub wubi: Vec<WubiEntry>,
    /// 完全同码:code → 词条下标。
    pub wubi_exact: HashMap<String, Vec<u32>>,
    /// 前缀索引:任意前缀 → 词条下标(渐进候选)。
    pub wubi_prefix: HashMap<String, Vec<u32>>,
    /// 反查:词 → 五笔编码(拼音候选注释统一为五笔码;同词多码取最长全码)。
    pub wubi_rev: HashMap<String, String>,
    pub wubi_max: u64,

    // ---- 拼音通道 ----
    /// 全拼 → 单字列表。
    pub py_chars: HashMap<String, Vec<(char, u64)>>,
    /// 音节表(取自 pinyin_char.tsv 去重,合同 §5.3)。
    pub syllables: HashSet<String>,
    /// 音节表的全部字符前缀(判定"末音节不完整"片段是否可能有字)。
    pub syllable_prefixes: HashSet<String>,
    pub py_char_max: u64,

    pub phrases: Vec<PhraseEntry>,
    /// 全拼音节串(空格连接)精确命中 → 词组下标。
    pub py_exact: HashMap<String, Vec<u32>>,
    /// 音节串的音节级前缀(不含全串)→ 词组下标,供缺尾音节查询。
    pub py_boundary: HashMap<String, Vec<u32>>,
    /// 简拼串精确命中 → 词组下标。
    pub jian_exact: HashMap<String, Vec<u32>>,
    /// 简拼串字符级前缀(不含全串)→ 词组下标,供渐进简拼。
    pub jian_boundary: HashMap<String, Vec<u32>>,
    pub py_phrase_max: u64,

    // ---- 英文通道 ----
    pub english: Vec<(String, u64)>,
    /// 前缀 → 词条下标。
    pub en_prefix: HashMap<String, Vec<u32>>,
    /// 词 → 词频名次(1 起),用于 en_freq_top_n 判定。
    pub en_rank: HashMap<String, u32>,
    pub en_max: u64,

    // ---- 兜底词频 ----
    /// 词 → 通用词频(排序同分兜底,合同 §5.5"排序兜底")。
    pub suggestion: HashMap<String, u64>,

    /// 成功加载出数据的通道数(0 = 全空引擎)。
    pub loaded_channels: usize,
}

/// 每码保留的词条数上限(合同 §5.2:取每码频次前 9)。
const WUBI_PER_CODE: usize = 9;
/// 前缀桶词条上限(控制渐进候选池规模)。
const WUBI_BUCKET: usize = 32;
/// 每个音节保留的单字数上限。
const PY_CHAR_PER_SYLL: usize = 16;
/// 英文前缀桶词条上限。
const EN_BUCKET: usize = 32;

impl DictIndex {
    /// 从目录加载全部数据;目录不存在时返回全空索引。
    pub(crate) fn load(dir: &Path) -> Self {
        let mut d = DictIndex {
            wubi: Vec::new(),
            wubi_exact: HashMap::new(),
            wubi_prefix: HashMap::new(),
            wubi_rev: HashMap::new(),
            wubi_max: 0,
            py_chars: HashMap::new(),
            syllables: HashSet::new(),
            syllable_prefixes: HashSet::new(),
            py_char_max: 0,
            phrases: Vec::new(),
            py_exact: HashMap::new(),
            py_boundary: HashMap::new(),
            jian_exact: HashMap::new(),
            jian_boundary: HashMap::new(),
            py_phrase_max: 0,
            english: Vec::new(),
            en_prefix: HashMap::new(),
            en_rank: HashMap::new(),
            en_max: 0,
            suggestion: HashMap::new(),
            loaded_channels: 0,
        };
        d.load_wubi(&dir.join("wubi.tsv"));
        d.load_pinyin_char(&dir.join("pinyin_char.tsv"));
        d.load_pinyin_phrase(&dir.join("pinyin_phrase.tsv"));
        d.load_english(&dir.join("english.tsv"));
        d.load_suggestion(&dir.join("suggestion.tsv"));
        // meta.json 仅作元信息校验,损坏/缺失直接忽略(不影响任何通道)。
        if let Ok(text) = fs::read_to_string(dir.join("meta.json")) {
            let _ = serde_json::from_str::<serde_json::Value>(&text);
        }
        d
    }

    /// 是否加载到了任何数据(对应 `Engine::is_loaded`)。
    pub(crate) fn is_empty(&self) -> bool {
        self.loaded_channels == 0
    }

    /// 单字是否在五笔码表(造词编码前置校验用)。
    pub(crate) fn has_wubi_code(&self, ch: char) -> bool {
        self.wubi_rev.contains_key(&ch.to_string())
    }

    /// 按五笔86 词组取码规则推算 `word` 的词组编码(合同 §12;取码规则为
    /// 五笔86 标准词组编码,与极点五笔/海峰86 码表一致):
    /// - 单字:全码;
    /// - 二字词:各取全码前 2 码(如 你好 = wqiy+ vbg → wqvb);
    /// - 三字词:前两字各第 1 码 + 末字前 2 码;
    /// - 四字词:每字第 1 码;
    /// - ≥5 字:第 1、2、3 字与末字各第 1 码。
    /// 任一参与取码的字不在码表 → None(造词给出人话失败提示)。
    pub(crate) fn wubi_word_code(&self, word: &str) -> Option<String> {
        let chars: Vec<char> = word.chars().collect();
        let code_of = |ch: char| -> Option<String> { self.wubi_rev.get(&ch.to_string()).cloned() };
        let full: Vec<String> = chars.iter().map(|c| code_of(*c)).collect::<Option<Vec<_>>>()?;
        let take = |i: usize, n: usize| -> Option<String> {
            full.get(i)?.get(..n).map(|s| s.to_string())
        };
        let out = match chars.len() {
            0 => return None,
            1 => full[0].clone(),
            2 => format!("{}{}", take(0, 2)?, take(1, 2)?),
            3 => format!("{}{}{}", take(0, 1)?, take(1, 1)?, take(2, 2)?),
            4 => format!("{}{}{}{}", take(0, 1)?, take(1, 1)?, take(2, 1)?, take(3, 1)?),
            n => format!(
                "{}{}{}{}",
                take(0, 1)?,
                take(1, 1)?,
                take(2, 1)?,
                take(n - 1, 1)?
            ),
        };
        (0 < out.len() && out.len() <= 4).then_some(out)
    }

    /// 把用户造词并入内存五笔索引(词可被其编码直接打出):
    /// 追加词条 → 精确码桶置顶 → 前缀桶按频重排 → 反查表。
    /// `freq` 为排序用词频(用户词取当前码表最大词频,保证同码首位)。
    pub(crate) fn insert_user_word(&mut self, word: &str, code: &str, freq: u64) {
        if word.is_empty() || code.is_empty() || freq == 0 {
            return;
        }
        let idx = self.wubi.len() as u32;
        self.wubi.push(WubiEntry {
            code: code.to_string(),
            word: word.to_string(),
            freq,
        });
        self.wubi_exact.entry(code.to_string()).or_default().insert(0, idx);
        for plen in 1..=code.len() {
            self.wubi_prefix
                .entry(code[..plen].to_string())
                .or_default()
                .push(idx);
        }
        if let Some(bucket) = self.wubi_prefix.get_mut(code) {
            bucket.sort_by(|&a, &b| {
                self.wubi[b as usize]
                    .freq
                    .cmp(&self.wubi[a as usize].freq)
                    .then_with(|| self.wubi[a as usize].word.cmp(&self.wubi[b as usize].word))
            });
        }
        if freq > self.wubi_max {
            self.wubi_max = freq;
        }
        self.wubi_rev.insert(word.to_string(), code.to_string());
    }

    fn load_wubi(&mut self, path: &Path) {
        let mut by_code: HashMap<String, Vec<(String, u64)>> = HashMap::new();
        // 反查暂存:word → (最优码, 该码词频)。
        let mut rev: HashMap<String, (String, u64)> = HashMap::new();
        for cols in read_rows(path, 3) {
            let code = cols[0].to_lowercase();
            let word = cols[1].clone();
            let Ok(freq) = cols[2].parse::<u64>() else {
                continue;
            };
            if code.is_empty() || word.is_empty() || !code.is_ascii() {
                continue;
            }
            by_code
                .entry(code.clone())
                .or_default()
                .push((word.clone(), freq));
            // 同词多码取最长码(全码优先于简码),同长取频高,再同取字典序小,保证确定。
            match rev.get_mut(&word) {
                Some(slot) => {
                    let better = code.len() > slot.0.len()
                        || (code.len() == slot.0.len()
                            && (freq > slot.1 || (freq == slot.1 && code < slot.0)));
                    if better {
                        *slot = (code.clone(), freq);
                    }
                }
                None => {
                    rev.insert(word, (code, freq));
                }
            }
        }
        if by_code.is_empty() {
            return;
        }
        for (code, mut words) in by_code {
            // 每码按频次降序,取前 9(合同 §5.2);同频按词序稳定。
            words.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            words.truncate(WUBI_PER_CODE);
            let base = self.wubi.len() as u32;
            for (word, freq) in words {
                self.wubi.push(WubiEntry {
                    code: code.clone(),
                    word,
                    freq,
                });
            }
            let idxs: Vec<u32> = (base..self.wubi.len() as u32).collect();
            self.wubi_exact.insert(code.clone(), idxs);
        }
        // 前缀索引:每条词条按其编码的全部前缀入桶。
        for i in 0..self.wubi.len() {
            let code = self.wubi[i].code.clone();
            for plen in 1..=code.len() {
                self.wubi_prefix
                    .entry(code[..plen].to_string())
                    .or_default()
                    .push(i as u32);
            }
        }
        for bucket in self.wubi_prefix.values_mut() {
            bucket.sort_by(|&a, &b| {
                self.wubi[b as usize]
                    .freq
                    .cmp(&self.wubi[a as usize].freq)
                    .then_with(|| self.wubi[a as usize].word.cmp(&self.wubi[b as usize].word))
            });
            bucket.truncate(WUBI_BUCKET);
        }
        self.wubi_max = self.wubi.iter().map(|e| e.freq).max().unwrap_or(0);
        self.wubi_rev = rev
            .into_iter()
            .map(|(word, (code, _))| (word, code))
            .collect();
        self.loaded_channels += 1;
    }

    fn load_pinyin_char(&mut self, path: &Path) {
        let mut map: HashMap<String, Vec<(char, u64)>> = HashMap::new();
        for cols in read_rows(path, 3) {
            let py = cols[0].to_lowercase();
            let Some(ch) = cols[1].chars().next() else {
                continue;
            };
            let Ok(freq) = cols[2].parse::<u64>() else {
                continue;
            };
            if py.is_empty() || !py.is_ascii() || ch.is_whitespace() {
                continue;
            }
            map.entry(py).or_default().push((ch, freq));
        }
        if map.is_empty() {
            return;
        }
        for (syll, list) in map {
            // 音节表取自 pinyin_char 去重(合同 §5.3),并建字符前缀表供缺尾判定。
            for plen in 1..=syll.len() {
                self.syllable_prefixes.insert(syll[..plen].to_string());
            }
            self.syllables.insert(syll.clone());
            let mut list = list;
            list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            list.truncate(PY_CHAR_PER_SYLL);
            self.py_chars.insert(syll, list);
        }
        self.py_char_max = self
            .py_chars
            .values()
            .flat_map(|v| v.iter().map(|(_, f)| *f))
            .max()
            .unwrap_or(0);
        self.loaded_channels += 1;
    }

    fn load_pinyin_phrase(&mut self, path: &Path) {
        let mut count = 0usize;
        for cols in read_rows(path, 3) {
            let word = cols[0].clone();
            let py = cols[1].to_lowercase();
            let Ok(freq) = cols[2].parse::<u64>() else {
                continue;
            };
            let sylls: Vec<String> = py.split_whitespace().map(str::to_string).collect();
            if word.is_empty() || sylls.is_empty() {
                continue;
            }
            if sylls.iter().any(|s| s.is_empty() || !s.is_ascii()) {
                continue;
            }
            let jian: String = sylls.iter().filter_map(|s| s.chars().next()).collect();
            let idx = self.phrases.len() as u32;
            self.phrases.push(PhraseEntry {
                word,
                sylls,
                jian,
                freq,
            });
            let joined = self.phrases[idx as usize].sylls.join(" ");
            self.py_exact.entry(joined).or_default().push(idx);
            // 音节级前缀(不含全串):缺尾音节查询用。
            let sylls = self.phrases[idx as usize].sylls.clone();
            for j in 1..sylls.len() {
                self.py_boundary
                    .entry(sylls[..j].join(" "))
                    .or_default()
                    .push(idx);
            }
            let jian = self.phrases[idx as usize].jian.clone();
            self.jian_exact.entry(jian.clone()).or_default().push(idx);
            for j in 1..jian.len() {
                self.jian_boundary
                    .entry(jian[..j].to_string())
                    .or_default()
                    .push(idx);
            }
            count += 1;
        }
        if count == 0 {
            return;
        }
        self.py_phrase_max = self.phrases.iter().map(|p| p.freq).max().unwrap_or(0);
        self.loaded_channels += 1;
    }

    fn load_english(&mut self, path: &Path) {
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for cols in read_rows(path, 2) {
            let word = cols[0].to_lowercase();
            let Ok(freq) = cols[1].parse::<u64>() else {
                continue;
            };
            if word.is_empty() || !word.is_ascii() || !seen.insert(word.clone()) {
                continue;
            }
            rows.push((word, freq));
        }
        if rows.is_empty() {
            return;
        }
        // 词频名次(1 起):按频次降序,同频按词序稳定 —— 供 en_freq_top_n 判定。
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            rows[b]
                .1
                .cmp(&rows[a].1)
                .then_with(|| rows[a].0.cmp(&rows[b].0))
        });
        for (rank, &i) in order.iter().enumerate() {
            self.en_rank.insert(rows[i].0.clone(), (rank + 1) as u32);
        }
        for i in 0..rows.len() {
            let word = rows[i].0.clone();
            for plen in 1..=word.len() {
                self.en_prefix
                    .entry(word[..plen].to_string())
                    .or_default()
                    .push(i as u32);
            }
        }
        for bucket in self.en_prefix.values_mut() {
            bucket.sort_by(|&a, &b| {
                rows[b as usize]
                    .1
                    .cmp(&rows[a as usize].1)
                    .then_with(|| rows[a as usize].0.cmp(&rows[b as usize].0))
            });
            bucket.truncate(EN_BUCKET);
        }
        self.english = rows;
        self.en_max = self.english.iter().map(|(_, f)| *f).max().unwrap_or(0);
        self.loaded_channels += 1;
    }

    fn load_suggestion(&mut self, path: &Path) {
        let mut count = 0usize;
        for cols in read_rows(path, 2) {
            let word = cols[0].clone();
            let Ok(freq) = cols[1].parse::<u64>() else {
                continue;
            };
            if word.is_empty() {
                continue;
            }
            let slot = self.suggestion.entry(word).or_insert(0);
            if freq > *slot {
                *slot = freq;
            }
            count += 1;
        }
        if count > 0 {
            self.loaded_channels += 1;
        }
    }
}

/// 读取 TSV 行,按 \t 切列;文件缺失返回空;列数不足/空行/`#` 注释行跳过。
fn read_rows(path: &Path, min_cols: usize) -> Vec<Vec<String>> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.split('\t')
                .map(|s| s.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|cols| cols.len() >= min_cols)
        .collect()
}

/// 供 engine 取兜底词频。
pub(crate) fn suggestion_of(dict: &DictIndex, word: &str) -> u64 {
    dict.suggestion.get(word).copied().unwrap_or(0)
}
