//! 用户词学习与 write-behind 落盘(合同 §4/§5.6)。
//!
//! - 内存累计:`word → (freq_extra, last_used_epoch)`,每次上屏候选 +1;
//! - 落盘:每 64 次 commit 或显式 [`Learner::save`] / Engine 销毁时批量写入,
//!   临时文件 + rename 原子替换,避免异常退出残留半截文件;
//! - 格式:`word\tfreq_extra\tlast_used_epoch`(TSV,UTF-8);
//! - 容错:文件缺失/个别行坏 → 忽略,不影响引擎。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Error;

/// write-behind 阈值:累计 64 次上屏自动落盘一次(合同 §4)。
pub(crate) const SAVE_EVERY: u64 = 64;

pub(crate) struct Learner {
    enabled: bool,
    path: PathBuf,
    map: HashMap<String, (u64, u64)>,
    /// 自上次成功落盘以来未保存的 commit 次数。
    unsaved: u64,
    /// 内存中有未落盘的修改。
    dirty: bool,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Learner {
    pub(crate) fn new(enabled: bool, path: PathBuf) -> Self {
        Self { enabled, path, map: HashMap::new(), unsaved: 0, dirty: false }
    }

    /// 从 user.tsv 装载历史词频;文件缺失/损坏行静默忽略。
    pub(crate) fn load(&mut self) {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return;
        };
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split('\t');
            let (Some(word), Some(extra)) = (it.next(), it.next()) else { continue };
            let Ok(extra) = extra.parse::<u64>() else { continue };
            let epoch = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            if word.is_empty() {
                continue;
            }
            self.map.insert(word.to_string(), (extra, epoch));
        }
    }

    /// 记一次上屏;达到阈值自动 write-behind。
    pub(crate) fn record(&mut self, word: &str) {
        if !self.enabled {
            return;
        }
        let epoch = now_epoch();
        let entry = self.map.entry(word.to_string()).or_insert((0, epoch));
        entry.0 += 1;
        entry.1 = epoch;
        self.dirty = true;
        self.unsaved += 1;
        if self.unsaved >= SAVE_EVERY {
            // 自动落盘失败不打断输入,留待下次 flush 再试(错误随 flush 上抛)。
            let _ = self.save();
        }
    }

    /// 查询某词的学习次数(参与 ×1.5 加成)。
    pub(crate) fn get(&self, word: &str) -> Option<u64> {
        self.map.get(word).map(|(extra, _)| *extra)
    }

    /// 把内存累计写入磁盘(原子替换);无修改时空操作。
    pub(crate) fn save(&mut self) -> Result<(), Error> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| Error::new(format!("无法创建用户词典目录 {}: {e}", dir.display())))?;
            }
        }
        let mut text = String::new();
        for (word, (extra, epoch)) in &self.map {
            text.push_str(word);
            text.push('\t');
            text.push_str(&extra.to_string());
            text.push('\t');
            text.push_str(&epoch.to_string());
            text.push('\n');
        }
        // 临时文件 + rename,异常退出不残留半截 user.tsv。
        let tmp = self.path.with_extension("tsv.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| Error::new(format!("写入用户词典 {} 失败: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| Error::new(format!("落盘用户词典 {} 失败: {e}", self.path.display())))?;
        self.dirty = false;
        self.unsaved = 0;
        Ok(())
    }

    /// 开关学习;打开时若尚未装载数据则立即加载。
    pub(crate) fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if on && self.map.is_empty() {
            self.load();
        }
    }

    /// 覆盖用户词典路径并重新装载(切目录后旧数据不再参与)。
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        if path == self.path {
            return;
        }
        self.path = path;
        self.map.clear();
        self.unsaved = 0;
        self.dirty = false;
        if self.enabled {
            self.load();
        }
    }
}
