//! 拼音音节切分:自写极简 DP 枚举器(不引第三方分词库)。
//!
//! 切分思路参考 fcitx5/libime 的音节切分器,但这里按合同 §5.3 采用
//! "枚举全部合法切分" 的做法:同一串字母(如 `xian`)同时按
//! `xian` 与 `xi'an` 两种切分查询,天然解决歧义问题。
//!
//! 缓冲区只可能是连续小写 ASCII 字母(engine 已保证),因此按字节切片安全。

use std::collections::HashSet;

/// 单个拼音音节的最大长度(如 zhuang = 6)。
pub(crate) const MAX_SYLLABLE_LEN: usize = 6;

/// 枚举 `buf` 的所有完整音节切分,返回每条切分的"音节长度序列"。
///
/// 最多输出 `cap` 条以防组合爆炸(缓冲 ≤12 字母,实际远到不了)。
pub(crate) fn segmentations(buf: &str, syllables: &HashSet<String>, cap: usize) -> Vec<Vec<usize>> {
    let bytes = buf.as_bytes();
    let mut out = Vec::new();
    let mut path = Vec::new();
    dfs(bytes, 0, syllables, &mut path, &mut out, cap);
    out
}

fn dfs(
    bytes: &[u8],
    pos: usize,
    syllables: &HashSet<String>,
    path: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
    cap: usize,
) {
    if out.len() >= cap {
        return;
    }
    if pos == bytes.len() {
        out.push(path.clone());
        return;
    }
    for len in 1..=MAX_SYLLABLE_LEN {
        let end = pos + len;
        if end > bytes.len() {
            break;
        }
        // 位置 pos..end 均为 ASCII 小写字母,from_utf8 不会失败
        let slice = std::str::from_utf8(&bytes[pos..end]).expect("缓冲必为 ASCII");
        if syllables.contains(slice) {
            path.push(len);
            dfs(bytes, end, syllables, path, out, cap);
            path.pop();
            if out.len() >= cap {
                return;
            }
        }
    }
}

/// 从位置 0 出发、由若干完整音节可达的全部切分终点(含 buf 末尾,不含 0)。
///
/// 用于"末音节不完整"查询:对每个终点 k,前缀 buf[..k] 已可完整切分,
/// 剩余 buf[k..] 视为不完整的最后一个音节。
pub(crate) fn prefix_ends(buf: &str, syllables: &HashSet<String>) -> Vec<usize> {
    let bytes = buf.as_bytes();
    let mut ends: Vec<usize> = Vec::new();
    let mut visited = vec![false; bytes.len() + 1];
    let mut stack = vec![0usize];
    while let Some(pos) = stack.pop() {
        for len in 1..=MAX_SYLLABLE_LEN {
            let end = pos + len;
            if end > bytes.len() {
                break;
            }
            if visited[end] {
                continue;
            }
            let slice = std::str::from_utf8(&bytes[pos..end]).expect("缓冲必为 ASCII");
            if syllables.contains(slice) {
                visited[end] = true;
                ends.push(end);
                stack.push(end);
            }
        }
    }
    ends.sort_unstable();
    ends
}

/// 缓冲内容在拼音语法上是否"还能继续":词库缺词(无候选)不等于非法。
/// buf 本身是合法音节前缀,或某个可达切点之后剩余部分仍是音节前缀,
/// 都算可续;否则属死码,交给既有死码保护直通。
/// (判定相对当前音节表:测试夹具里没有 q 起头的音节,`jieq` 的 q
/// 在该夹具下是不可续后缀;真实词库有 qi*/qia* 等,`jieq` 仍可续。)
pub(crate) fn can_continue(
    buf: &str,
    syllables: &HashSet<String>,
    prefixes: &HashSet<String>,
) -> bool {
    !buf.is_empty()
        && (prefixes.contains(buf)
            || prefix_ends(buf, syllables)
                .into_iter()
                .any(|k| k == buf.len() || prefixes.contains(&buf[k..])))
}

/// 按切分的音节长度序列,取回各音节字符串(空格连接形如 `ni hao`)。
pub(crate) fn seg_joined(buf: &str, lens: &[usize]) -> String {
    let mut parts = Vec::with_capacity(lens.len());
    let mut pos = 0;
    for len in lens {
        parts.push(&buf[pos..pos + len]);
        pos += len;
    }
    parts.join(" ")
}

/// 按切分的音节长度序列,取最后一个音节(切片)。
/// 引擎已不再需要"末音节"视图(多音节末字候选改为前缀消费路径),
/// 仅单元测试保留校验。
#[cfg(test)]
pub(crate) fn seg_last<'a>(buf: &'a str, lens: &[usize]) -> &'a str {
    let last = lens.last().copied().unwrap_or(0);
    let start = buf.len() - last;
    &buf[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 切分歧义枚举_xian() {
        let syl = table(&["xian", "xi", "an"]);
        let mut segs = segmentations("xian", &syl, 64);
        segs.sort();
        assert_eq!(segs, vec![vec![2, 2], vec![4]]); // xi'an 与 xian
    }

    #[test]
    fn 完整切分_nihao() {
        let syl = table(&["ni", "hao", "ha", "o"]);
        let mut segs = segmentations("nihao", &syl, 64);
        segs.sort();
        // ni/hao 与 ni/ha/o 两种
        assert!(segs.contains(&vec![2, 3]));
        assert!(segs.contains(&vec![2, 2, 1]));
    }

    #[test]
    fn 前缀终点收集() {
        let syl = table(&["ni", "hao"]);
        let ends = prefix_ends("nihaoma", &syl);
        assert_eq!(ends, vec![2, 5]); // ni | ni hao
    }

    #[test]
    fn 切片辅助函数() {
        assert_eq!(seg_joined("nihao", &[2, 3]), "ni hao");
        assert_eq!(seg_last("nihao", &[2, 3]), "hao");
    }
}
