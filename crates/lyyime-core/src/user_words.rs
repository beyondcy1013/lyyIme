//! 用户造词库(合同 §12):`~/.local/share/lyyime/user_words.tsv`。
//!
//! - 格式:`word \t code \t count`(TSV,UTF-8;count = 造词/使用累计次数);
//! - 启动时整表并入五笔内存索引(`DictIndex::insert_user_word`),造的词
//!   立即可用其编码打出;排序词频取当前码表最大词频,保证同码首位;
//! - 造词即时落盘(临时文件 + rename 原子替换),与 learner 的 write-behind
//!   相互独立——造词是低频显式动作,失败必须让人看见,不做静默延迟。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dict::DictIndex;
use crate::error::Error;

/// 造词条目数上限(防异常膨胀;超限后仍可输入但不再新增,给提示)。
pub(crate) const MAX_USER_WORDS: usize = 100_000;

pub(crate) struct UserWords {
    path: PathBuf,
    /// word → (code, count)。
    words: HashMap<String, (String, u64)>,
    dirty: bool,
}

impl UserWords {
    /// 空库(路径由 [`UserWords::set_path_and_load`] 给出)。
    pub(crate) fn new() -> Self {
        Self {
            path: PathBuf::new(),
            words: HashMap::new(),
            dirty: false,
        }
    }

    /// 从 `path` 装载并逐条并入 `dict`;文件缺失/坏行静默跳过(降级不致命)。
    pub(crate) fn load(&mut self, dict: &mut DictIndex, path: &Path) {
        self.path = path.to_path_buf();
        self.words.clear();
        self.dirty = false;
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split('\t');
            let (Some(word), Some(code)) = (it.next(), it.next()) else {
                continue;
            };
            let count = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1);
            let code = code.to_lowercase();
            if word.is_empty()
                || code.is_empty()
                || !code.is_ascii()
                || code.chars().any(|c| !c.is_ascii_lowercase())
            {
                continue;
            }
            self.words
                .insert(word.to_string(), (code.clone(), count.max(1)));
            let rank_freq = dict.wubi_max.max(1);
            dict.insert_user_word(word, &code, rank_freq);
        }
    }

    /// 记录一条新造词:并入 `dict` 内存索引并即时落盘。
    /// 返回人话错误(目录不可写等);调用方以 Notice 呈现给用户。
    pub(crate) fn add(&mut self, dict: &mut DictIndex, word: &str, code: &str) -> Result<(), Error> {
        if self.words.len() >= MAX_USER_WORDS && !self.words.contains_key(word) {
            return Err(Error::new(format!(
                "造词库已满({} 条),请清理 {} 后重试",
                MAX_USER_WORDS,
                self.path.display()
            )));
        }
        let known = self.words.contains_key(word);
        let slot = self
            .words
            .entry(word.to_string())
            .or_insert((code.to_string(), 0));
        slot.1 += 1;
        // 重复造同一个词只累计次数,词库索引里已有,不重复插入。
        if !known {
            let rank_freq = dict.wubi_max.max(1);
            dict.insert_user_word(word, code, rank_freq);
        }
        self.dirty = true;
        self.save()
    }

    /// 移除一条词(右键"删除词组"的造词侧,合同 §15):词表剔除并即时落盘;
    /// 不在表中返回 Ok(false) 且不落盘。词典内存索引不反删——命中词条由
    /// 屏蔽表(`blocked.tsv`)统一挡住,避免与内置词重名纠缠。
    pub(crate) fn remove(&mut self, word: &str) -> Result<bool, Error> {
        if self.words.remove(word).is_none() {
            return Ok(false);
        }
        self.dirty = true;
        self.save()?;
        Ok(true)
    }

    /// 可写性预检:建目录(幂等)+ 以写模式探测打开(不创建文件)。
    /// 在 plan(纯读取)阶段调用,重试安全;返回 false 时造词给人话失败提示。
    pub(crate) fn prepare(&self) -> bool {
        if self.path.as_os_str().is_empty() {
            return false;
        }
        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() && std::fs::create_dir_all(dir).is_err() {
                return false;
            }
        }
        match std::fs::OpenOptions::new().write(true).open(&self.path) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        }
    }

    /// 造词库路径(人话提示用)。
    pub(crate) fn path_display(&self) -> String {
        self.path.display().to_string()
    }

    /// 全量落盘(临时文件 + rename 原子替换);无修改时空操作。
    fn save(&mut self) -> Result<(), Error> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    Error::new(format!("无法创建造词库目录 {}: {e}", dir.display()))
                })?;
            }
        }
        // 按 count 降序稳定输出,便于人工查看/清理。
        let mut rows: Vec<(&String, &(String, u64))> = self.words.iter().collect();
        rows.sort_by(|a, b| {
            b.1 .1
                .cmp(&a.1 .1)
                .then_with(|| a.0.cmp(b.0))
        });
        let mut text = String::new();
        for (word, (code, count)) in rows {
            text.push_str(word);
            text.push('\t');
            text.push_str(code);
            text.push('\t');
            text.push_str(&count.to_string());
            text.push('\n');
        }
        let tmp = self.path.with_extension("tsv.tmp");
        std::fs::write(&tmp, text.as_bytes())
            .map_err(|e| Error::new(format!("写入造词库 {} 失败: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| Error::new(format!("落盘造词库 {} 失败: {e}", self.path.display())))?;
        self.dirty = false;
        Ok(())
    }

    /// 覆盖造词库路径并重新装载(路径来自 user_dict 同目录推导)。
    /// 返回 true = 路径变了、词表已重新装载(词典索引随之变化)。
    pub(crate) fn set_path_and_load(&mut self, dict: &mut DictIndex, path: PathBuf) -> bool {
        if path == self.path {
            return false;
        }
        self.load(dict, &path);
        true
    }
}
