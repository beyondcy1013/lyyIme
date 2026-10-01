//! 上屏后联想(本地静态联想):中文词上屏后,把「已上屏文本的尾部」当作词条
//! 前缀在本地词典里反查可续接的尾巴,经候选条供继续选择(空格/数字/点选)。
//!
//! - 纯本地:BTreeMap 排序词表,来源 = 五笔词条 + 拼音词组 + suggestion 词表,
//!   建引擎时一次合并;重复词条取最大提供频次,不逐键扫全词典;
//! - 匹配 = 最长上下文优先:上下文越长命中率越低,但尾巴越准;
//!   同一尾巴被多个上下文命中时保留最长上下文的一条(全局去重);
//! - 每档按频次降序、同频按尾巴文本升序(确定性排序,与词条装载顺序无关);
//! - 屏蔽表(blocked)同时过滤整条词与尾巴本身(右键"删除词组"后不再联想);
//! - 无任何网络/AI 调用;索引不落盘用户输入历史。

use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;

use crate::dict::DictIndex;
use crate::types::{CandKind, Candidate};
use crate::wordops::BlockList;

/// 联想上下文保留的最近上屏 CJK 字数(上屏历史只参与最长匹配窗口)。
pub(crate) const MAX_CTX_CHARS: usize = 6;

/// 联想候选总量上限(与组合候选同量级,防极端词库把条列撑爆)。
pub(crate) const MAX_CANDS: usize = 50;

/// 入索引条件:≥2 字且全部为 CJK(单字/含西文/标点不参与联想)。
fn indexable(word: &str) -> bool {
    let mut n = 0usize;
    for c in word.chars() {
        if !is_cjk(c) {
            return false;
        }
        n += 1;
    }
    n >= 2
}

fn is_cjk(c: char) -> bool {
    matches!(u32::from(c),
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF)
}

/// 联想索引:词条(全 CJK,≥2 字)→ 词频。BTreeMap 保证前缀 range 查询有序。
pub(crate) struct PredictionIndex {
    words: BTreeMap<String, u64>,
}

impl PredictionIndex {
    /// 建索引:合并五笔词条 word 列、拼音词组键、suggestion 词表键;
    /// 重复词条跨来源取最大提供频次。
    pub(crate) fn new(dict: &DictIndex) -> Self {
        let mut idx = Self {
            words: BTreeMap::new(),
        };
        for e in &dict.wubi {
            idx.insert(&e.word, e.freq);
        }
        for p in &dict.phrases {
            idx.insert(&p.word, p.freq);
        }
        for (word, freq) in &dict.suggestion {
            idx.insert(word, *freq);
        }
        idx
    }

    /// 收进一个词(用户造词/词库增量即时生效):≥2 字且全 CJK 才入索引;
    /// 重复词取最大频次。
    pub(crate) fn insert(&mut self, word: &str, freq: u64) {
        if !indexable(word) {
            return;
        }
        match self.words.entry(word.to_string()) {
            std::collections::btree_map::Entry::Occupied(mut e) => {
                if freq > *e.get() {
                    *e.get_mut() = freq;
                }
            }
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(freq);
            }
        }
    }

    /// 以最近上屏上下文反查可续接尾巴。
    ///
    /// 上下文取尾部 1..=MAX_CTX_CHARS 字逐档查(最长档先查),词条前缀
    /// 命中后剩余部分即尾巴;空尾巴(词条与上下文恰好等长)跳过,尾巴
    /// 全局去重——最长上下文档先占先赢。整条词或尾巴在 `blocked` 内跳过。
    /// 每档内按频次降序、同频按尾巴文本升序;总量封顶 `limit`(至多
    /// [`MAX_CANDS`])。
    pub(crate) fn candidates(
        &self,
        context: &str,
        blocked: &BlockList,
        limit: usize,
    ) -> Vec<Candidate> {
        let cap = limit.min(MAX_CANDS);
        if cap == 0 || context.is_empty() {
            return Vec::new();
        }
        let ctx: Vec<char> = context.chars().collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Candidate> = Vec::new();
        for len in (1..=ctx.len().min(MAX_CTX_CHARS)).rev() {
            let prefix: String = ctx[ctx.len() - len..].iter().collect();
            let mut bucket: Vec<(String, u64)> = Vec::new();
            // 前缀 range:只扫以该上下文结尾为前缀的词条,不触全表。
            for (word, freq) in self
                .words
                .range::<String, _>((Bound::Included(prefix.clone()), Bound::Unbounded))
                .take_while(|(w, _)| w.starts_with(prefix.as_str()))
            {
                let suffix = &word[prefix.len()..];
                if suffix.is_empty() || seen.contains(suffix) {
                    continue;
                }
                if blocked.contains(word) || blocked.contains(suffix) {
                    continue;
                }
                seen.insert(suffix.to_string());
                bucket.push((suffix.to_string(), *freq));
            }
            bucket.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            for (suffix, freq) in bucket {
                if out.len() >= cap {
                    return out;
                }
                out.push(Candidate {
                    text: suffix,
                    comment: String::new(),
                    score: freq as f32,
                    kind: CandKind::Wubi,
                    consumed: 0,
                });
            }
        }
        out
    }
}
