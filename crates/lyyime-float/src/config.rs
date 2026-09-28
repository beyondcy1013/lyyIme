//! lyyIme 悬浮窗配置与数据文件:
//!   ~/.config/lyyime/config.json      dict / send / position / keep_target_focus …
//!   ~/.config/lyyime/config.toml      统一配置(stats_* 统计设置真源, 设置窗管理)
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
    /// type(直输) | paste(粘贴) | inline(模拟内置: 候选贴目标光标显示)
    #[serde(default = "default_send")]
    pub send: String,
    #[serde(default)]
    pub position: Option<[i32; 2]>,
    /// 输入停顿时在状态行显示今日输入统计(总开关)。
    /// 真源在统一配置 config.toml 顶层 stats_*(设置窗统一管理),本文件不再
    /// 持久化;启动时由 load_stats 覆盖(Skip 序列化,旧键留在文件里无害)
    #[serde(skip, default = "default_true")]
    pub stats_enabled: bool,
    /// 停顿多少秒后开始显示(3–300);真源 config.toml,同上
    #[serde(skip, default = "default_stats_pause")]
    pub stats_pause_secs: u32,
    /// 计入速度的最长停顿秒数;超过的空隙(思考/离开)不计入活跃时长(5–600)
    #[serde(skip, default = "default_stats_idle")]
    pub stats_idle_exclude_secs: u32,
    /// 不抢焦点模式: 悬浮窗不夺取 X 焦点(accept_focus=false), 用键盘抓取收键,
    /// 目标文本框的光标全程保持; 上屏时解抓→注入→重抓
    #[serde(default)]
    pub keep_target_focus: bool,
}

fn default_dict() -> String {
    "jidian86".into()
}
fn default_send() -> String {
    "type".into()
}
fn default_true() -> bool {
    true
}
fn default_stats_pause() -> u32 {
    10
}
fn default_stats_idle() -> u32 {
    30
}

impl Default for Config {
    fn default() -> Self {
        Config {
            dict: default_dict(),
            send: default_send(),
            position: None,
            stats_enabled: default_true(),
            stats_pause_secs: default_stats_pause(),
            stats_idle_exclude_secs: default_stats_idle(),
            keep_target_focus: false,
        }
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

    /// 用统一配置(config.toml 顶层 stats_* 键)覆盖统计三项。
    /// create() 在 Config::load 之后调用;旧 config.json 遗留键的迁移回退在
    /// load_stats 内部完成。
    pub fn apply_stats(&mut self, s: StatsCfg) {
        self.stats_enabled = s.enabled;
        self.stats_pause_secs = s.pause_secs;
        self.stats_idle_exclude_secs = s.idle_exclude_secs;
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

/// 输入统计设置(全局:数据与配置都跨输入模式共用)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatsCfg {
    pub enabled: bool,
    pub pause_secs: u32,
    pub idle_exclude_secs: u32,
}

impl Default for StatsCfg {
    fn default() -> Self {
        StatsCfg { enabled: true, pause_secs: 10, idle_exclude_secs: 30 }
    }
}

#[derive(serde::Deserialize, Default)]
struct StatsToml {
    stats_enabled: Option<bool>,
    stats_pause_secs: Option<u32>,
    stats_idle_exclude_secs: Option<u32>,
}

#[derive(serde::Deserialize, Default)]
struct LegacyJsonStats {
    stats_enabled: Option<bool>,
    stats_pause_secs: Option<u32>,
    stats_idle_exclude_secs: Option<u32>,
}

/// 真源:统一设置窗写 config.toml 顶层 stats_*;config.toml 一个键都没有时
/// 回退读旧 config.json 遗留键(历史迁移),都没有用内置默认。
/// 纯函数便于单测;钳制与设置窗一致(pause 3..300, idle 5..600)。
pub fn load_stats_from(
    toml_text: Option<&str>,
    json_text: Option<&str>,
) -> StatsCfg {
    let mut out = StatsCfg::default();
    if let Some(t) = toml_text {
        if let Ok(p) = toml::from_str::<StatsToml>(t) {
            let mut hit = false;
            if let Some(v) = p.stats_enabled {
                out.enabled = v;
                hit = true;
            }
            if let Some(v) = p.stats_pause_secs {
                out.pause_secs = v.clamp(3, 300);
                hit = true;
            }
            if let Some(v) = p.stats_idle_exclude_secs {
                out.idle_exclude_secs = v.clamp(5, 600);
                hit = true;
            }
            if hit {
                return out;
            }
        }
    }
    if let Some(j) = json_text {
        if let Ok(p) = serde_json::from_str::<LegacyJsonStats>(j) {
            if let Some(v) = p.stats_enabled {
                out.enabled = v;
            }
            if let Some(v) = p.stats_pause_secs {
                out.pause_secs = v.clamp(3, 300);
            }
            if let Some(v) = p.stats_idle_exclude_secs {
                out.idle_exclude_secs = v.clamp(5, 600);
            }
        }
    }
    out
}

/// 读统一配置的统计设置(配置缺失/字段缺失逐级回退,见 load_stats_from)。
pub fn load_stats() -> StatsCfg {
    let dir = config_dir();
    let toml_text = fs::read_to_string(dir.join("config.toml")).ok();
    let json_text = fs::read_to_string(dir.join("config.json")).ok();
    load_stats_from(toml_text.as_deref(), json_text.as_deref())
}

/// §15 自定义查询(config.toml 顶层 custom_query_label/custom_query_url,
/// 统一设置窗管理):每次右键弹菜单时读取(设置保存后即时生效);
/// url 空白 → None(菜单不显示此项)。
pub fn load_custom_query() -> Option<lyyime_core::wordops::CustomQuery> {
    let text = fs::read_to_string(config_dir().join("config.toml")).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    let cq = lyyime_core::wordops::CustomQuery {
        label: v
            .get("custom_query_label")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        url: v
            .get("custom_query_url")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
    };
    cq.configured().then_some(cq)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml键优先且缺省键取默认() {
        let s = load_stats_from(
            Some("page_size = 7\nstats_enabled = false\n"),
            Some(r#"{"stats_enabled": false, "stats_pause_secs": 42}"#),
        );
        assert!(!s.enabled);
        assert_eq!(s.pause_secs, 10, "toml 无该键取默认, 不回退旧 json");
        assert_eq!(s.idle_exclude_secs, 30);
    }

    #[test]
    fn 无toml统计键时回退旧config_json() {
        let s = load_stats_from(
            Some("page_size = 7\n[ai]\nenabled = true\n"),
            Some(r#"{"dict":"jidian86","stats_enabled":false,"stats_pause_secs":42,"stats_idle_exclude_secs":77}"#),
        );
        assert!(!s.enabled);
        assert_eq!(s.pause_secs, 42);
        assert_eq!(s.idle_exclude_secs, 77);
    }

    #[test]
    fn 都缺失用默认且钳制越界() {
        assert_eq!(load_stats_from(None, None), StatsCfg::default());
        let s = load_stats_from(
            Some("stats_pause_secs = 9999\nstats_idle_exclude_secs = 1\n"),
            None,
        );
        assert_eq!(s.pause_secs, 300);
        assert_eq!(s.idle_exclude_secs, 5);
    }

    #[test]
    fn skip序列化不再写旧键且能解析遗留文件() {
        // 旧文件带 stats 键: serde 默认忽略未知字段, 解析不报错
        let cfg: Config = serde_json::from_str(
            r#"{"dict":"haifeng86","send":"paste","stats_enabled":false,"stats_pause_secs":42}"#,
        )
        .expect("遗留 config.json 应可解析");
        assert_eq!(cfg.dict, "haifeng86");
        // 新保存不再写 stats 键(Skip)
        let text = serde_json::to_string(&cfg).unwrap();
        assert!(!text.contains("stats_"), "config.json 不应再持久化统计键: {text}");
    }
}
