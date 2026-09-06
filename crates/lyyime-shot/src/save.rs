//! 截屏产物落地:保存 PNG(图片目录)、复制剪贴板、桌面通知。
//!
//! 目录回退链:`--dir` → `$LYYIME_SHOT_DIR` → `~/图片` → `~/Pictures` → `~`;
//! 文件名 `lyyIme_YYYYMMDD_HHMMSS.png`(秒级冲突时追加 `_1`/`_2` 递增)。
//! 时间取本地时区(libc localtime_r,遵守 TZ),无第三方时间库;
//! libc 失败时退回 UTC 推算(Howard Hinnant civil_from_days,公有领域)。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, Result};

/// 保存 PNG,返回实际路径。
pub fn save_png(pixbuf: &gtk::gdk_pixbuf::Pixbuf, dir_override: Option<&str>) -> Result<PathBuf> {
    let dir = resolve_dir(dir_override);
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow!("无法创建保存目录 {}: {e}", dir.display()))?;
    let path = unique_path(&dir);
    pixbuf
        .savev(&path, "png", &[])
        .map_err(|e| anyhow!("写入 {} 失败:{e}", path.display()))?;
    Ok(path)
}

/// 目录回退链(显式参数优先,逐级探测存在性)。
fn resolve_dir(override_dir: Option<&str>) -> PathBuf {
    if let Some(d) = override_dir {
        if !d.trim().is_empty() {
            return PathBuf::from(shellexpand_home(d));
        }
    }
    if let Ok(d) = std::env::var("LYYIME_SHOT_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    for name in ["图片", "Pictures"] {
        let p = Path::new(&home).join(name);
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from(&home)
}

/// 展开路径开头的 `~`(仅 `~` 与 `~/…` 两种形态)。
fn shellexpand_home(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return p.to_string();
    }
    if p == "~" {
        return home;
    }
    p.strip_prefix("~/")
        .map(|rest| Path::new(&home).join(rest).to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

/// 目标文件名;同秒已存在则 `_1`、`_2` 递增。
fn unique_path(dir: &Path) -> PathBuf {
    let stem = timestamp_stem(now_secs());
    let mut p = dir.join(format!("{stem}.png"));
    for n in 1..100 {
        if !p.exists() {
            return p;
        }
        p = dir.join(format!("{stem}_{n}.png"));
    }
    p // 极端情况:100 个同名,直接覆盖最后一个(理论不可达)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `lyyIme_YYYYMMDD_HHMMSS`(本地时区;libc 失败退回 UTC 近似)。
fn timestamp_stem(secs: i64) -> String {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::localtime_r(&t, &mut tm) };
    if !ok.is_null() {
        return fmt_stem(
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec,
        );
    }
    // UTC 兜底(Howard Hinnant civil_from_days,公有领域)
    let (y, mo, d) = civil_from_days(secs.div_euclid(86400));
    let sod = secs.rem_euclid(86400);
    fmt_stem(
        y as i32,
        mo as i32,
        d as i32,
        (sod / 3600) as i32,
        (sod % 3600 / 60) as i32,
        (sod % 60) as i32,
    )
}

fn fmt_stem(y: i32, mo: i32, d: i32, h: i32, mi: i32, s: i32) -> String {
    format!("lyyIme_{y:04}{mo:02}{d:02}_{h:02}{mi:02}{s:02}")
}

/// UNIX 天数 → (年, 月, 日)(UTC;兜底用)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 复制到剪贴板(GTK CLIPBOARD;set_image 后 store 让 X 剪贴板持久一份)。
pub fn copy_to_clipboard(pixbuf: &gtk::gdk_pixbuf::Pixbuf) {
    let clipboard = gtk::Clipboard::get(&gdk::Atom::intern("CLIPBOARD"));
    clipboard.set_image(pixbuf);
    clipboard.store();
}

/// 桌面通知(notify-send;缺失/失败静默 —— 通知是锦上添花,不该报错打扰)。
pub fn notify(path: &Path, size: &str) {
    let _ = Command::new("notify-send")
        .args([
            "-a",
            "lyyIme",
            "-i",
            "camera-photo",
            "截屏已保存",
            &format!("{}({size}),已复制到剪贴板", path.display()),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 时间戳文件名() {
        // 固定日期:2026-09-06 00:00:00 UTC = 1788652800
        assert_eq!(fmt_stem(2026, 9, 6, 0, 0, 0), "lyyIme_20260906_000000");
        assert_eq!(fmt_stem(2026, 9, 6, 1, 1, 1), "lyyIme_20260906_010101");
        // localtime_r:遵守 TZ 环境变量(此处显式设 UTC 验证换算;
        // tzset 用 extern 直连 libc,libc crate 未封装该符号)
        std::env::set_var("TZ", "UTC0");
        unsafe extern "C" {
            fn tzset();
        }
        unsafe { tzset() };
        assert_eq!(timestamp_stem(1_788_652_800), "lyyIme_20260906_000000");
        assert_eq!(timestamp_stem(1_788_652_800 + 3661), "lyyIme_20260906_010101");
        assert_eq!(timestamp_stem(0), "lyyIme_19700101_000000");
    }

    #[test]
    fn 目录回退链() {
        // 显式参数最高优先;显式空串则落到环境/默认链
        assert_eq!(resolve_dir(Some("/tmp/x")), PathBuf::from("/tmp/x"));
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let pics = Path::new(&home).join("图片");
        let expect = if pics.is_dir() {
            pics
        } else if Path::new(&home).join("Pictures").is_dir() {
            Path::new(&home).join("Pictures")
        } else {
            PathBuf::from(&home)
        };
        assert_eq!(resolve_dir(None), expect);
    }
}
