//! 输入统计:今日上屏字数与活跃打字时长(悬浮窗"停顿显示今日输入"的数据层)。
//!
//! 数据文件:`~/.local/share/lyyime/stats/YYYY-MM-DD.tsv`(本地日期),
//! 每行 `epoch_ms\tchars` —— 一次上屏一条。Mode A(ibus 引擎)与 Mode C
//! (悬浮窗)在不同进程各自追加同一目录:单行 O_APPEND 写入保证不交错
//! (行长远小于 PIPE_BUF),按天分文件免去清理与跨进程当日聚合竞争。
//!
//! 查询口径(与主流打字统计的"有效时长"一致):
//! - 字数 = 当天全部 chars 之和(非空白字符,汉字/字母/标点均计);
//! - 活跃时长 = 相邻两次上屏间隔 ≤ idle_exclude_secs 的时间累计,
//!   超过阈值的空隙(离开、思考)不计入;
//! - 速度 = 字数 ÷ 活跃时长(字/分钟)。
//!
//! 记录与汇总均为纯函数,时间由调用方传入,可离线测试;任何文件错误
//! 都静默降级——统计是旁路功能,绝不影响输入主链路。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 默认统计目录(`~/.local/share/lyyime/stats`;无 HOME 环境返回 None)。
pub fn default_dir() -> Option<PathBuf> {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Some(PathBuf::from(h).join(".local/share/lyyime/stats")),
        _ => None,
    }
}

/// epoch 毫秒 → 本地日期 `yyyy-mm-dd`(libc localtime_r,不引时间库)。
pub fn local_date(now_ms: u64) -> String {
    let secs = (now_ms / 1000) as i64;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t: libc::time_t = secs as libc::time_t;
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return "1970-01-01".to_string();
    }
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

/// 记录一次上屏:`chars` 为本次上屏的非空白字符数,`now_ms` 为毫秒时间戳。
/// chars 为 0(空上屏)不记;创建目录/写文件失败静默忽略。
pub fn record(dir: &Path, chars: u64, now_ms: u64) {
    if chars == 0 {
        return;
    }
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.tsv", local_date(now_ms)));
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{now_ms}\t{chars}");
    }
}

/// 单日统计结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DaySummary {
    /// 当天上屏总字数(非空白字符)。
    pub chars: u64,
    /// 活跃打字时长(秒,四舍五入;已排除超过阈值的空隙)。
    pub active_secs: u64,
    /// 上屏次数。
    pub events: u64,
}

impl DaySummary {
    /// 平均速度(字/分钟,四舍五入);活跃时长不足(仅一次上屏或不足 1 秒)时 None。
    pub fn speed_per_min(&self) -> Option<u64> {
        if self.active_secs == 0 {
            return None;
        }
        Some((self.chars * 60 + self.active_secs / 2) / self.active_secs)
    }
}

/// 聚合单个当日文件;文件缺失/损坏行静默跳过。
pub fn summarize_file(path: &Path, idle_exclude_secs: u64) -> DaySummary {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return DaySummary::default(),
    };
    let mut events: Vec<(u64, u64)> = text
        .lines()
        .filter_map(|line| {
            let mut it = line.split('\t');
            let ms = it.next()?.trim().parse::<u64>().ok()?;
            let chars = it.next()?.trim().parse::<u64>().ok()?;
            Some((ms, chars))
        })
        .collect();
    events.sort_unstable_by_key(|e| e.0);

    let mut s = DaySummary { events: events.len() as u64, ..DaySummary::default() };
    let exclude_ms = idle_exclude_secs.saturating_mul(1000);
    let mut active_ms: u64 = 0;
    let mut prev: Option<u64> = None;
    for (ms, chars) in events {
        s.chars += chars;
        if let Some(p) = prev {
            let gap = ms.saturating_sub(p);
            if gap <= exclude_ms {
                active_ms += gap;
            }
        }
        prev = Some(ms);
    }
    s.active_secs = (active_ms + 500) / 1000;
    s
}

/// 汇总 `now_ms` 所在本地日的统计。
pub fn today_summary(dir: &Path, now_ms: u64, idle_exclude_secs: u64) -> DaySummary {
    summarize_file(&dir.join(format!("{}.tsv", local_date(now_ms))), idle_exclude_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立临时目录(不写真实 HOME;用进程 id + 计数避免并发冲突)。
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static SEQ: AtomicU32 = AtomicU32::new(0);
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!(
                "lyyime-stats-test-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn 记录与汇总往返() {
        let td = TempDir::new();
        record(&td.0, 4, 1_000_000);
        record(&td.0, 2, 1_000_500);
        record(&td.0, 10, 1_001_000);
        let s = today_summary(&td.0, 1_001_000, 30);
        assert_eq!(s.chars, 16);
        assert_eq!(s.events, 3);
        // 活跃时长 = 0.5s + 0.5s = 1s(间隔均 ≤30s)
        assert_eq!(s.active_secs, 1);
        assert_eq!(s.speed_per_min(), Some((16 * 60 + 0) / 1));
    }

    #[test]
    fn 排除超过阈值的空闲段() {
        let td = TempDir::new();
        record(&td.0, 5, 0);
        record(&td.0, 5, 1_000); // gap 1s 计入
        record(&td.0, 5, 100_000); // gap 99s(>30s)排除
        record(&td.0, 5, 101_000); // gap 1s 计入
        let s = today_summary(&td.0, 101_000, 30);
        assert_eq!(s.chars, 20);
        assert_eq!(s.active_secs, 2);
        assert_eq!(s.speed_per_min(), Some((20 * 60 + 1) / 2)); // 601 字/分
    }

    #[test]
    fn 文件缺失与坏行容错() {
        let td = TempDir::new();
        assert_eq!(today_summary(&td.0, 0, 30), DaySummary::default());
        let f = td.0.join(format!("{}.tsv", local_date(0)));
        std::fs::write(&f, "not a line\n100\t3\n\nbad\t\n200\t4\n").unwrap();
        let s = summarize_file(&f, 30);
        assert_eq!(s.chars, 7);
        assert_eq!(s.events, 2);
        // 唯一间隔 100s(>30s)被排除 → 活跃 0 → 无有效速度
        assert_eq!(s.speed_per_min(), None);
    }

    #[test]
    fn 单次上屏无速度() {
        let td = TempDir::new();
        record(&td.0, 8, 50_000);
        let s = today_summary(&td.0, 50_000, 30);
        assert_eq!(s.chars, 8);
        assert_eq!(s.active_secs, 0);
        assert_eq!(s.speed_per_min(), None);
    }

    #[test]
    fn 今日_只统计当前日期文件() {
        let td = TempDir::new();
        let now = 1_700_000_000_000u64; // 任意时刻
        let today = local_date(now);
        // "昨天":同目录另一个日期文件,内容再多也不计入
        let other = td.0.join("2000-01-01.tsv");
        std::fs::write(&other, format!("{now}\t999\n")).unwrap();
        record(&td.0, 6, now);
        let s = today_summary(&td.0, now, 30);
        assert_eq!(s.chars, 6);
        assert!(td.0.join(format!("{today}.tsv")).exists());
    }

    #[test]
    fn 本地日期格式合法() {
        let d = local_date(0);
        assert_eq!(d.len(), 10);
        let bytes = d.as_bytes();
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert!(d.parse::<i64>().is_err() || d.starts_with("19") || d.starts_with("20"));
    }

    #[test]
    fn 空上屏不记录() {
        let td = TempDir::new();
        record(&td.0, 0, 1_000);
        assert_eq!(today_summary(&td.0, 1_000, 30), DaySummary::default());
    }
}
