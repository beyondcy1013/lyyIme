//! lyyIme 悬浮窗配置与数据文件:
//!   ~/.config/lyyime/config.json      dict / send / position
//!   ~/.config/lyyime/user_freq.json   选词学习(phrase -> 次数)
//!   ~/.local/share/lyyime/float.pid   单实例 pidfile
//! 与已删除的 floatapp(Python 版)格式完全兼容, 用户数据无缝沿用。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".config/lyyime")
}

pub fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".local/share/lyyime")
}

pub fn phrase_json_path() -> PathBuf {
    config_dir().join("phrase.json")
}

pub fn pid_file() -> PathBuf {
    data_dir().join("float.pid")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    /// jidian86 | haifeng86
    #[serde(default = "default_dict")]
    pub dict: String,
    /// type(直输) | paste(粘贴)
    #[serde(default = "default_send")]
    pub send: String,
    #[serde(default)]
    pub position: Option<[i32; 2]>,
}

fn default_dict() -> String {
    "jidian86".into()
}
fn default_send() -> String {
    "type".into()
}

impl Default for Config {
    fn default() -> Self {
        Config { dict: default_dict(), send: default_send(), position: None }
    }
}

impl Config {
    pub fn load() -> Config {
        let path = config_dir().join("config.json");
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let dir = config_dir();
        fs::create_dir_all(&dir)?;
        let path = dir.join("config.json");
        fs::write(&path, serde_json::to_string(self)?)
            .with_context(|| format!("写配置失败: {}", path.display()))?;
        Ok(())
    }
}

pub fn load_user_freq() -> HashMap<String, i64> {
    let path = config_dir().join("user_freq.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_user_freq(uf: &HashMap<String, i64>) {
    let dir = config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(s) = serde_json::to_string(uf) {
        let _ = fs::write(dir.join("user_freq.json"), s);
    }
}

/// 读 pidfile 并确认进程存活; 残留/非法内容视为不存在。
pub fn pidfile_alive() -> Option<i32> {
    let s = fs::read_to_string(pid_file()).ok()?;
    let pid: i32 = s.trim().parse().ok()?;
    if pid <= 0 {
        return None;
    }
    // kill(pid, 0): 存活探测
    unsafe {
        if libc::kill(pid, 0) == 0 {
            Some(pid)
        } else {
            None
        }
    }
}

pub fn write_pidfile() -> Result<()> {
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    fs::write(pid_file(), format!("{}\n", std::process::id()))?;
    Ok(())
}

pub fn remove_pidfile() {
    let _ = fs::remove_file(pid_file());
}
