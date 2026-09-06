//! 极简文件日志:~/.local/share/lyyime/logs/ibus.log(追加)。
//! LYYIME_DEBUG=1 或 -v 时 DEBUG 级同时输出 stderr。模式借鉴 ibus-table
//! 的日志落盘习惯(不实现按天轮转,单文件 + 启动分割线足够排障)。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

static VERBOSE: OnceLock<bool> = OnceLock::new();
static LOG_FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();

pub fn log_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    format!("{home}/.local/share/lyyime/logs")
}

pub fn init(verbose: bool) {
    let _ = VERBOSE.set(verbose);
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{dir}/ibus.log"))
        .ok()
        .map(Mutex::new);
    let _ = LOG_FILE.set(file);
    info(&format!(
        "===== ibus-engine-lyyime 启动(pid={}) =====",
        std::process::id()
    ));
}

/// 本地时间 HH:MM:SS(UTC 偏移经 libc::localtime,输入法单线程写日志)。
fn timestamp() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let tm = libc::localtime(&t);
        if tm.is_null() {
            return String::new();
        }
        format!(
            "{:02}:{:02}:{:02}",
            (*tm).tm_hour, (*tm).tm_min, (*tm).tm_sec
        )
    }
}

fn write(level: &str, msg: &str) {
    let line = format!("{} {} {}\n", timestamp(), level, msg);
    if VERBOSE.get().copied().unwrap_or(false) {
        let _ = std::io::stderr().write_all(line.as_bytes());
    }
    if let Some(f) = LOG_FILE.get().and_then(|opt| opt.as_ref()) {
        if let Ok(mut f) = f.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

pub fn debug(msg: &str) {
    if VERBOSE.get().copied().unwrap_or(false) {
        write("DEBUG", msg);
    }
}
