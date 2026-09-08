//! 打分与排序 —— 实现修订版合同 docs/ARCHITECTURE.md §5:**层级主导的词典序**。
//!
//! 背景(真库验收发现的跨尺度问题):拼音单字(ibus 尺度 ~1e9)、词组(jieba 尺度 ~1e3)、
//! 五笔(~1e3)、英文(~2e4)频率尺度相差 4 个数量级以上,单一乘性/加性公式都会被大频值碾压。
//! 因此排序改为按 (tier, specificity, freq_norm) **词典序**:
//!
//! - `tier`:整数层级,层间不可跨越(硬约束:五笔精确含简码 > 拼音完整切分 > 拼音不完整尾 > 简拼);
//! - `specificity`:同层排序因子——"完整音节偏好",匹配消耗的音节越多越具体越靠前
//!   (如词组全拼 > 末音节单字;完整片段 > 更短片段);
//! - `freq_norm`:同文件内 log10 归一 ×10 ∈ [0,10),只在同层内起作用;
//!   用户词乘 ×1.5,允许层内溢出(层隔离由词典序保证,不会跨层)。
//!
//! `Candidate.score`(展示值)= tier + norm,仅供 UI 参考;**排序以上述词典序为准**。
//!
//! 交互与排序习惯借鉴:ibus-table(码表频次序)、搜狗/万能五笔(中英混打、简码优先)、
//! fcitx5/libime(拼音切分思路,见 pinyin.rs)。

/// 五笔完全同码(code == buffer,含简码;简码奖励已并入本层;
/// 层内 spec 单字按 GB2312 分档:一级 3.0 > 二级 2.5 > 词组 1.0 > 表外生僻 0.5,
/// 档内按真实语料频次归一,语料外/生僻档按表频;无分档表时单字 2.0)。
pub(crate) const TIER_WUBI_EXACT: f32 = 60.0;
/// 英文:无中文命中时的前缀候选(合同修订 §5.4/§5.C)。
pub(crate) const TIER_ENGLISH_NO_CN: f32 = 55.0;
/// 拼音:完整音节切分全命中(词组/单字)。
pub(crate) const TIER_PINYIN_FULL: f32 = 50.0;
/// 五笔:前缀渐进候选。
pub(crate) const TIER_WUBI_PREFIX: f32 = 40.0;
/// 拼音:仅末音节不完整(如 xia+n)。
pub(crate) const TIER_PINYIN_PARTIAL: f32 = 35.0;
/// 英文:有中文命中时的完整高频词(top mixed_auto_commit_top_n,合同修订 §5.C)。
pub(crate) const TIER_ENGLISH_WITH_CN: f32 = 30.0;
/// 拼音:简拼(每音节首字母,≥2 键)。
pub(crate) const TIER_PINYIN_ABBREV: f32 = 20.0;

/// 用户词(学习过)层内加成乘子(合同 §5.6:×1.5)。
pub(crate) const USER_BOOST: f32 = 1.5;

/// 同文件词频归一 ×10:`log10(1+freq) / log10(1+maxf_file) × 10 ∈ [0,10]`。
///
/// 跨源尺度隔离靠层级完成,同层内必然同源同尺度;文件为空(maxf=0)时返回满值 10。
pub(crate) fn norm10(freq: u64, maxf: u64) -> f32 {
    if maxf == 0 {
        return 10.0;
    }
    let f = (1 + freq) as f64;
    let m = (1 + maxf) as f64;
    (f.log10() / m.log10()) as f32 * 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 归一化落在_0_10_区间() {
        assert!((norm10(0, 100) - 0.0).abs() < 1e-5);
        assert!((norm10(100, 100) - 10.0).abs() < 1e-5);
        let mid = norm10(10, 100);
        assert!(mid > 0.0 && mid < 10.0);
    }

    #[test]
    fn 空文件归一化为满值() {
        assert_eq!(norm10(0, 0), 10.0);
        assert_eq!(norm10(999, 0), 10.0);
    }

    #[test]
    fn 跨尺度样本_单字与词组分属不同文件互不影响() {
        // 拼音单字 1.78e9(自身文件满值)与词组 725(自身文件内偏中)各自归一,
        // 层级才是主导,数值仅供参考。
        let char_norm = norm10(1_780_000_000, 1_780_000_000);
        let phrase_norm = norm10(725, 260_000);
        assert!((char_norm - 10.0).abs() < 1e-4);
        assert!(phrase_norm > 0.0 && phrase_norm < 10.0);
        assert!(TIER_PINYIN_FULL + 0.5 < TIER_WUBI_EXACT);
        assert!(TIER_PINYIN_ABBREV + 15.0 < TIER_PINYIN_FULL);
    }
}
