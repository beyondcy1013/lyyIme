//! 候选右键操作持久层(合同 §15):固定首位 / 删除词组 / 中→英反查。
//!
//! 与用户词典/造词库同目录(`~/.local/share/lyyime/`,随 `user_dict` 迁移),
//! 均为"用户数据区"小文件,原子写(临时文件 + rename),坏行静默跳过:
//! - `pinned.tsv`  `code \t word`  固定首位:该编码候选中此词恒居第一;
//! - `blocked.tsv` `word`(每行一条)  删除词组:屏蔽词在任何编码下都不再
//!   出现——内置词条不改词典文件,只屏蔽显示;用户造词另从
//!   `user_words.tsv` 移除(见 `Engine`/`user_words.rs`)。
//!
//! 词典目录(`data_dir`)下:
//! - `zh_en.tsv`  `zh \t en1 \t en2 …`(dicttool `zhen` 由 ECDICT 反生成),
//!   中→英反查表,供"反查英文";缺失时该操作降级为提示,不影响输入。
//!
//! Mode B(xim,经 FFI)与 Mode C(float,直接依赖本 crate)共用本模块,
//! 保证两种前端的行为与数据文件完全一致。

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// 可写性预检(与 `user_words::prepare` 同语义):建目录(幂等)+ 写模式探测
/// 打开(不创建文件,NotFound 视为可写)。plan 阶段调用,重试安全。
fn writable_probe(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() && std::fs::create_dir_all(dir).is_err() {
            return false;
        }
    }
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

/// 全量原子落盘:`fill` 产出文本 → `<path>.tmp` → rename 替换。
fn write_atomic(path: &Path, fill: impl FnOnce(&mut String)) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| {
                Error::new(format!("无法创建数据目录 {}: {e}", dir.display()))
            })?;
        }
    }
    let mut text = String::new();
    fill(&mut text);
    let tmp = path.with_extension("tsv.tmp");
    std::fs::write(&tmp, text.as_bytes())
        .map_err(|e| Error::new(format!("写入 {} 失败: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| Error::new(format!("落盘 {} 失败: {e}", path.display())))
}

/// `pinned.tsv` 固定首位表:`code \t word`,一码一词(重复行取最后一条)。
#[derive(Debug, Default)]
pub struct PinTable {
    path: PathBuf,
    map: HashMap<String, String>,
}

impl PinTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `path` 装载(覆盖旧数据);文件缺失 = 空表(降级不致命)。
    pub fn load(&mut self, path: &Path) {
        self.path = path.to_path_buf();
        self.map.clear();
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split('\t');
            let (Some(code), Some(word)) = (it.next(), it.next()) else {
                continue;
            };
            let code = code.trim().to_lowercase();
            let word = word.trim();
            if code.is_empty()
                || word.is_empty()
                || !code.chars().all(|c| c.is_ascii_lowercase())
            {
                continue;
            }
            self.map.insert(code, word.to_string());
        }
    }

    /// 该编码当前的固定词。
    pub fn get(&self, code: &str) -> Option<&str> {
        self.map.get(code).map(String::as_str)
    }

    /// 该编码的固定词是否恰为 `word`(菜单"取消固定"判定用)。
    pub fn is_pinned(&self, code: &str, word: &str) -> bool {
        self.get(code) == Some(word)
    }

    /// 全表引用(plan 阶段克隆覆盖用,表通常很小)。
    pub fn map(&self) -> &HashMap<String, String> {
        &self.map
    }

    /// 固定 `code → word`(同码再固定覆盖旧值)并即时落盘。
    pub fn pin(&mut self, code: &str, word: &str) -> Result<(), Error> {
        self.map.insert(code.to_string(), word.to_string());
        self.save()
    }

    /// 解除 `code` 的固定(未固定也视为成功)并落盘。
    pub fn unpin(&mut self, code: &str) -> Result<(), Error> {
        self.map.remove(code);
        self.save()
    }

    /// 删除词组时顺带解除该词在各编码上的固定;无改动不落盘。
    pub fn drop_word(&mut self, word: &str) {
        let before = self.map.len();
        self.map.retain(|_, w| w != word);
        if self.map.len() != before {
            if let Err(e) = self.save() {
                eprintln!("lyyime-core: 解除固定落盘失败: {e}");
            }
        }
    }

    /// 落盘可写性预检(plan 阶段;详见 [`writable_probe`])。
    pub fn prepare(&self) -> bool {
        writable_probe(&self.path)
    }

    /// 路径展示(人话提示用)。
    pub fn path_display(&self) -> String {
        self.path.display().to_string()
    }

    fn save(&self) -> Result<(), Error> {
        write_atomic(&self.path, |out| {
            // code 升序稳定输出,便于人工查看/编辑。
            let mut rows: Vec<(&String, &String)> = self.map.iter().collect();
            rows.sort();
            for (code, word) in rows {
                out.push_str(code);
                out.push('\t');
                out.push_str(word);
                out.push('\n');
            }
        })
    }
}

/// `blocked.tsv` 屏蔽词表(右键"删除词组"的数据层):每行一个词面,
/// 命中的词在任何编码下都不再作为候选出现。
#[derive(Debug, Default)]
pub struct BlockList {
    path: PathBuf,
    words: HashSet<String>,
}

impl BlockList {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 `path` 装载(覆盖旧数据);文件缺失 = 空表。
    pub fn load(&mut self, path: &Path) {
        self.path = path.to_path_buf();
        self.words.clear();
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines().map(str::trim) {
            if !line.is_empty() && !line.starts_with('#') {
                self.words.insert(line.to_string());
            }
        }
    }

    pub fn contains(&self, word: &str) -> bool {
        self.words.contains(word)
    }

    /// 全表引用(plan 阶段克隆覆盖用)。
    pub fn words(&self) -> &HashSet<String> {
        &self.words
    }

    /// 加入屏蔽并即时落盘(重复加入幂等——仍重写一次保证文件存在)。
    pub fn add(&mut self, word: &str) -> Result<(), Error> {
        self.words.insert(word.to_string());
        self.save()
    }

    /// 解除屏蔽(重新造词时自动调用);不在表中返回 Ok(false) 且不落盘。
    pub fn remove(&mut self, word: &str) -> Result<bool, Error> {
        if !self.words.remove(word) {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// 落盘可写性预检(plan 阶段;详见 [`writable_probe`])。
    pub fn prepare(&self) -> bool {
        writable_probe(&self.path)
    }

    /// 路径展示(人话提示用)。
    pub fn path_display(&self) -> String {
        self.path.display().to_string()
    }

    fn save(&self) -> Result<(), Error> {
        write_atomic(&self.path, |out| {
            let mut rows: Vec<&String> = self.words.iter().collect();
            rows.sort();
            for w in rows {
                out.push_str(w);
                out.push('\n');
            }
        })
    }
}

/// `zh_en.tsv` 中→英反查表:`zh \t en1 \t en2 …`(zh 升序,en 按词频/星级
/// 排序)。文件位于词典目录(dicttool `zhen` 产物),加载失败得空表——
/// 反查操作降级为"无结果"提示,不影响输入主链路。
#[derive(Debug, Default)]
pub struct ZhEn {
    map: HashMap<String, Vec<String>>,
}

impl ZhEn {
    /// 从 `path` 装载;文件缺失/不可读 → 空表。
    pub fn load(path: &Path) -> Self {
        let mut z = ZhEn::default();
        let Ok(f) = std::fs::File::open(path) else {
            return z;
        };
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.split('\t');
            let Some(zh) = it.next() else {
                continue;
            };
            if zh.is_empty() {
                continue;
            }
            let ens: Vec<String> = it
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if !ens.is_empty() {
                z.map.insert(zh.to_string(), ens);
            }
        }
        z
    }

    /// 查中文词的英文反查结果(无则空切片)。
    pub fn lookup(&self, word: &str) -> &[String] {
        self.map.get(word).map(Vec::as_slice).unwrap_or(&[])
    }

    /// 表内中文词条数(诊断用)。
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// 候选列表后处理(非 Engine 路径共用,Mode C 悬浮窗):
/// 先滤掉屏蔽词;当前编码 `code` 的固定词若仍在列表中,移到首位并返回其
/// 原下标(宿主打"固"标记用)。返回 None 表示无固定词命中。
///
/// `text_of` 为元素取词面访问器(Engine 内部在 RawCand/Candidate 上另行
/// 内联同等逻辑,本函数服务于 `Vec<(String, String)>` 形态的宿主列表)。
pub fn apply_pin_block<T>(
    list: &mut Vec<T>,
    code: &str,
    pins: &PinTable,
    blocked: &BlockList,
    text_of: impl Fn(&T) -> &str,
) -> Option<usize> {
    if !blocked.words.is_empty() {
        list.retain(|t| !blocked.words.contains(text_of(t)));
    }
    let word = pins.get(code)?;
    let pos = list.iter().position(|t| text_of(t) == word)?;
    let item = list.remove(pos);
    list.insert(0, item);
    Some(pos)
}

// ---------------------------------------------------------------------------
// §15 自定义查询(右键菜单第 4 项):宿主侧动作 —— 按用户配置的网址模板把
// 候选词代入 {q} 字段,经 xdg-open 拉起浏览器;不动引擎状态,与 §14 快速
// 功能键"宿主执行命令"同型。三端共用同一份替换/编码合同。
// ---------------------------------------------------------------------------

/// 自定义查询配置(config.toml 顶层 `custom_query_label`/`custom_query_url`,
/// 统一设置窗管理;`url` 为空 = 菜单不显示此项)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomQuery {
    /// 菜单显示名(空 → 「自定义查询」)。
    pub label: String,
    /// 网址模板;`{q}` 占位符替换为百分号编码后的候选词。
    pub url: String,
}

impl CustomQuery {
    /// 菜单显示名(空标签回退默认名)。
    pub fn menu_label(&self) -> &str {
        let l = self.label.trim();
        if l.is_empty() {
            "自定义查询"
        } else {
            l
        }
    }

    /// 是否已配置(菜单项可见条件:url 非空白)。
    pub fn configured(&self) -> bool {
        !self.url.trim().is_empty()
    }
}

/// RFC 3986 unreserved 字符原样保留,其余 UTF-8 字节百分号编码
/// (查询字段值编码;中文词 → %EXX%EXX%EXX)。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_'
            | b'~' => out.push(b as char),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0xF) as usize] as char);
            }
        }
    }
    out
}

/// 网址模板 + 词 → 最终网址:模板中全部 `{q}` 出现处替换为
/// percent-encode 后的词;模板没有 `{q}` 时原样返回(直接打开模板页)。
/// `word`/`template` 空白输入由调用方先行判空(configured()/词非空)。
pub fn custom_query_url(template: &str, word: &str) -> String {
    if template.contains("{q}") {
        template.replace("{q}", &percent_encode(word))
    } else {
        template.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lyyime-wordops-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn pin_roundtrip() {
        let dir = tmp_dir();
        let path = dir.join("pinned.tsv");
        let mut pins = PinTable::new();
        pins.load(&path);
        pins.pin("wqvb", "你好").unwrap();
        pins.pin("aa", "工工").unwrap();
        assert_eq!(pins.get("wqvb"), Some("你好"));
        // 重载后仍在(持久化)
        let mut pins2 = PinTable::new();
        pins2.load(&path);
        assert_eq!(pins2.get("wqvb"), Some("你好"));
        assert!(pins2.is_pinned("wqvb", "你好"));
        assert!(!pins2.is_pinned("wqvb", "你号"));
        // 取消固定
        pins2.unpin("wqvb").unwrap();
        assert!(pins2.get("wqvb").is_none());
        let mut pins3 = PinTable::new();
        pins3.load(&path);
        assert!(pins3.get("wqvb").is_none());
        assert_eq!(pins3.get("aa"), Some("工工"));
        // drop_word 清掉跨码固定
        pins3.pin("bbbb", "你好").unwrap();
        pins3.drop_word("你好");
        assert_eq!(pins3.get("bbbb"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocked_roundtrip() {
        let dir = tmp_dir();
        let path = dir.join("blocked.tsv");
        let mut bl = BlockList::new();
        bl.load(&path);
        bl.add("你号").unwrap();
        bl.add("拟好").unwrap();
        bl.add("你号").unwrap(); // 幂等
        let mut bl2 = BlockList::new();
        bl2.load(&path);
        assert!(bl2.contains("你号"));
        assert!(bl2.contains("拟好"));
        assert!(!bl2.contains("你好"));
        assert!(bl2.remove("拟好").unwrap());
        assert!(!bl2.remove("不存在").unwrap());
        let mut bl3 = BlockList::new();
        bl3.load(&path);
        assert!(bl3.contains("你号"));
        assert!(!bl3.contains("拟好"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zhen_load_lookup() {
        let dir = tmp_dir();
        let path = dir.join("zh_en.tsv");
        std::fs::write(&path, "你好\thello\thi\n电脑\tcomputer\n").unwrap();
        let z = ZhEn::load(&path);
        assert_eq!(z.len(), 2);
        assert_eq!(z.lookup("你好"), &["hello".to_string(), "hi".to_string()]);
        assert!(z.lookup("不存在").is_empty());
        // 缺失文件 → 空表
        let z2 = ZhEn::load(&dir.join("nope.tsv"));
        assert!(z2.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_pin_block_orders() {
        let dir = tmp_dir();
        let mut pins = PinTable::new();
        pins.load(&dir.join("pinned.tsv"));
        let mut blocked = BlockList::new();
        blocked.load(&dir.join("blocked.tsv"));
        let mut list = vec!["工".to_string(), "你号".to_string(), "你好".to_string()];
        // 无固定无屏蔽:不动
        assert_eq!(apply_pin_block(&mut list, "wq", &pins, &blocked, |s| s.as_str()), None);
        assert_eq!(list, vec!["工", "你号", "你好"]);
        // 屏蔽词过滤
        blocked.add("你号").unwrap();
        apply_pin_block(&mut list, "wq", &pins, &blocked, |s| s.as_str());
        assert_eq!(list, vec!["工", "你好"]);
        // 固定词置顶(返回原下标)
        pins.pin("wq", "你好").unwrap();
        let mut list2 = vec!["甲".to_string(), "乙".to_string(), "你好".to_string()];
        let pos = apply_pin_block(&mut list2, "wq", &pins, &blocked, |s| s.as_str());
        assert_eq!(pos, Some(2));
        assert_eq!(list2[0], "你好");
        // 固定词已被屏蔽 → 不置顶
        blocked.add("你好").unwrap();
        let mut list3 = vec!["甲".to_string(), "你好".to_string(), "乙".to_string()];
        let pos = apply_pin_block(&mut list3, "wq", &pins, &blocked, |s| s.as_str());
        assert_eq!(pos, None);
        assert_eq!(list3, vec!["甲", "乙"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_query_url_substitute() {
        // {q} 替换为 UTF-8 百分号编码词;多处占位全替换
        assert_eq!(
            custom_query_url("https://baike.baidu.com/item/{q}", "你好"),
            "https://baike.baidu.com/item/%E4%BD%A0%E5%A5%BD"
        );
        assert_eq!(
            custom_query_url("https://x/{q}?p={q}", "a b&c"),
            "https://x/a%20b%26c?p=a%20b%26c"
        );
        // 无 {q}:原样返回;unreserved 字符不编码
        assert_eq!(
            custom_query_url("https://example.test/", "a-z_0.9~"),
            "https://example.test/"
        );
        // 空标签回退默认菜单名;url 空白视为未配置
        let cq = CustomQuery { label: "  ".into(), url: "https://x/{q}".into() };
        assert_eq!(cq.menu_label(), "自定义查询");
        assert!(cq.configured());
        assert!(!CustomQuery::default().configured());
        let cq2 = CustomQuery { label: "查词典".into(), url: String::new() };
        assert_eq!(cq2.menu_label(), "查词典");
        assert!(!cq2.configured());
    }
}
