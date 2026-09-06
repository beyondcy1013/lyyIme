//! lyyIme 自定义短语簿: 自定义编码 → 任意长度文本(可多条)。
//!
//! 交互习惯对齐搜狗/万能五笔"自定义短语"(借鉴其精确码置顶习惯):
//! 输入精确码, 短语排候选最前。文本不限长度、可含换行(长短语上屏时
//! float UI 会自动改用粘贴模式)。
//!
//! 存储 ~/.config/lyyime/phrase.json, 格式(编码按插入序, 保持用户自定义顺序):
//!     {"yf": ["北京市海淀区…"], "dx": ["a@example.com", "b@example.com"]}
//! 编码限小写字母 1–12 个(数字键是选词键, 不允许进编码)。
//!
//! 文本支持动态变量, 候选展示与上屏时实时展开(expand_text, 可注入 now 便于测试):
//!     $date(格式)  当前日期, 格式缺省 yyyy-MM-dd, 如 $date(yyyy年M月d日) → 2026年9月6日
//!     $time(格式)  当前时间, 格式缺省 HH:mm
//!     $week        星期几, 如 星期日
//!     $$           字面 $
//!     格式 token(办公软件习惯, 补零用双写): yyyy yy MM M dd d HH H mm m ss s;
//!     E = 星期几的汉字(如 六), 可写 "星期E"。未识别的 $… 原样保留, 不报错。
//!
//! 动态日期/时间触发(不影响自定义短语原有行为): 候选里打出"日期/时间"这个词
//! (五笔等任意能命中它的码)且该词居首、或候选总数很少时, 紧随其后插入展开后的
//! 动态日期/时间候选; 编码恰为整拼 riqi/shijian 时(悬浮窗只装五笔码表)也直接触发。
//!
//! 本模块不依赖 GUI, cargo test 直接覆盖。

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// 编码合法性: 1–12 个小写字母。
pub fn is_valid_code(code: &str) -> bool {
    (1..=12).contains(&code.len())
        && code.bytes().all(|b| b.is_ascii_lowercase())
}

/// 文明历日期时间(本地时区由调用方折算;测试可手工构造)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0=周一 … 6=周日
    pub weekday: usize,
}

impl Civil {
    /// 从 Unix 时间戳构造(供 now_civil 使用)。
    pub fn from_unix(secs: i64) -> Civil {
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Civil {
            year,
            month,
            day,
            hour: (sod / 3600) as u32,
            minute: (sod % 3600 / 60) as u32,
            second: (sod % 60) as u32,
            weekday: ((days + 3).rem_euclid(7)) as usize,
        }
    }
}

/// 当前本地时间(简化: 本机时区固定 CST(+8), 与输入法部署环境一致;
/// 若系统为 UTC, 日期时间整体偏移不影响 weekday 语义演示——由 TZ 环境决定)。
pub fn now_civil() -> Civil {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 + tz_offset_secs())
        .unwrap_or(0);
    Civil::from_unix(secs)
}

/// 本机时区偏移秒数。读取 /etc/localtime 链接名(如 CST-8)太脆,
/// 直接用 libc::localtime(单线程窗口内调用, 不共享 tm 结构)。
fn tz_offset_secs() -> i64 {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let tm = libc::localtime(&t);
        if tm.is_null() {
            return 0;
        }
        (*tm).tm_gmtoff as i64
    }
}

/// Howard Hinnant civil_from_days: days since 1970-01-01 → (y, m, d)。
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

pub const WEEK_CN: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
pub const DEFAULT_DATE_FMT: &str = "yyyy-MM-dd";
pub const DEFAULT_TIME_FMT: &str = "HH:mm";

// ---- 动态日期/时间触发词(新增能力, 自定义短语行为不变) ----
/// 触发词 → 动态候选模板(经 expand_text 按当前时刻展开)。
pub const DYNAMIC_WORDS: [(&str, [&str; 2]); 2] = [
    ("日期", ["$date(yyyy年M月d日)", "$date(yyyy-MM-dd)"]),
    ("时间", ["$time(HH:mm)", "$time(H时mm分)"]),
];
/// 触发词不是首选时, 候选总数 ≤ 该值仍触发("可选少了");
/// 6 词 + 2 动态 ≤ 每页 9, 动态项保证首页可见。
pub const DYNAMIC_FEW: usize = 6;

/// 整拼直触: 悬浮窗只装五笔码表打不出拼音词, 编码恰为整拼时直接识别。
fn dynamic_code_word(code: &str) -> Option<&'static str> {
    match code.trim().to_lowercase().as_str() {
        "riqi" => Some("日期"),
        "shijian" => Some("时间"),
        _ => None,
    }
}

/// 动态变量: $$ 转义优先; $date(格式)/$time(格式) 括号内不含括号;
/// $week 后不接字母数字(否则视为普通文本)。
pub fn expand_text(text: &str, now: Civil) -> String {
    if !text.contains('$') {
        return text.to_string();
    }
    let b: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != '$' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        // b[i] == '$'
        if i + 1 < b.len() && b[i + 1] == '$' {
            out.push('$');
            i += 2;
            continue;
        }
        // $date(…)/$time(…)
        let mut matched = false;
        for kind in ["date", "time"] {
            let kw: Vec<char> = kind.chars().collect();
            if b[i + 1..].starts_with(&kw)
                && i + 1 + kw.len() < b.len()
                && b[i + 1 + kw.len()] == '('
            {
                // 内容不含括号; 必须找到闭合 ')'
                let start = i + 1 + kw.len() + 1;
                let mut j = start;
                let mut close = None;
                while j < b.len() {
                    if b[j] == '(' {
                        break; // 非法(内容含括号) → 不匹配
                    }
                    if b[j] == ')' {
                        close = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(close) = close {
                    let fmt: String = b[start..close].iter().collect();
                    let fmt = if fmt.is_empty() {
                        if kind == "date" {
                            DEFAULT_DATE_FMT
                        } else {
                            DEFAULT_TIME_FMT
                        }
                    } else {
                        fmt.as_str()
                    };
                    out.push_str(&expand_fmt(fmt, now));
                    i = close + 1;
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        // $week(后不接字母数字)
        const WEEK: [char; 4] = ['w', 'e', 'e', 'k'];
        if b[i + 1..].starts_with(&WEEK) {
            let after = i + 1 + WEEK.len();
            let ok = after >= b.len() || !b[after].is_ascii_alphanumeric();
            if ok {
                out.push_str("星期");
                out.push_str(WEEK_CN[now.weekday]);
                i = after;
                continue;
            }
        }
        // 未识别 → 原样输出 '$', 继续扫描(后续字符里若还有 $ 会另行处理)
        out.push('$');
        i += 1;
    }
    out
}

/// 日期/时间格式串 → 文本; 非 token 字符(汉字、分隔符等)原样保留。
/// token 长者优先(yyyy 先于 yy, MM 先于 M)。
pub fn expand_fmt(fmt: &str, now: Civil) -> String {
    let tokens: [&str; 13] = [
        "yyyy", "MM", "dd", "HH", "mm", "ss", "yy", "M", "d", "H", "m", "s", "E",
    ];
    let mut out = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        let mut hit = None;
        for t in tokens.iter() {
            if rest.starts_with(t) {
                hit = Some(*t);
                break;
            }
        }
        match hit {
            Some(t) => {
                out.push_str(&match t {
                    "yyyy" => format!("{:04}", now.year),
                    "yy" => format!("{:02}", now.year.rem_euclid(100)),
                    "MM" => format!("{:02}", now.month),
                    "M" => now.month.to_string(),
                    "dd" => format!("{:02}", now.day),
                    "d" => now.day.to_string(),
                    "HH" => format!("{:02}", now.hour),
                    "H" => now.hour.to_string(),
                    "mm" => format!("{:02}", now.minute),
                    "m" => now.minute.to_string(),
                    "ss" => format!("{:02}", now.second),
                    "s" => now.second.to_string(),
                    _ => WEEK_CN[now.weekday].to_string(), // E
                });
                i += t.chars().count();
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// phrase.json 的内存镜像。GUI 线程独占访问; Mutex 保护 items 以便
/// 保存快照与查找的瞬间一致性。
#[derive(Default)]
pub struct PhraseBook {
    pub path: String,
    pub(crate) items: Mutex<Vec<(String, Vec<String>)>>, // code -> texts, 插入序
}

impl PhraseBook {
    pub fn open(path: impl AsRef<Path>) -> PhraseBook {
        let mut book = PhraseBook {
            path: path.as_ref().to_string_lossy().into_owned(),
            items: Mutex::new(Vec::new()),
        };
        book.reload();
        book
    }

    /// 读盘并清洗: 非法编码/空文本/非列表值丢弃; 编码两侧空格宽容剥离。
    pub fn reload(&mut self) {
        let data: Vec<(String, Vec<String>)> = fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| {
                let obj = v.as_object()?.clone();
                let mut out = Vec::new();
                for (k, texts) in obj {
                    let code = k.trim().to_lowercase();
                    if !is_valid_code(&code) {
                        continue;
                    }
                    // 非列表值的条目按"无文本"丢弃, 不影响其它条目
                    let texts: Vec<String> = texts
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str())
                                .map(|t| t.to_string())
                                .filter(|t| !t.trim().is_empty())
                                .collect()
                        })
                        .unwrap_or_default();
                    if !texts.is_empty() {
                        out.push((code, texts));
                    }
                }
                Some(out)
            })
            .unwrap_or_default();
        if let Ok(mut items) = self.items.lock() {
            *items = data;
        }
    }

    /// 原子写盘(tmp + rename), 断电不损文件。
    pub fn save(&self) -> std::io::Result<()> {
        let items = self.items.lock().map_err(|_| std::io::Error::other("锁中毒"))?;
        let mut body = String::from("{");
        for (idx, (code, texts)) in items.iter().enumerate() {
            if idx > 0 {
                body.push(',');
            }
            body.push('\n');
            body.push_str(&format!(" {}: {}", jstr(code), jarr(texts)));
        }
        body.push_str("\n}\n");
        let path = Path::new(&self.path);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = format!("{}.tmp", self.path);
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// 精确码 → 文本列表(按定义序); 无则空。
    pub fn lookup(&self, code: &str) -> Vec<String> {
        let code = code.trim().to_lowercase();
        let items = match self.items.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        items
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    /// 全部条目快照(管理列表用)。
    pub fn snapshot(&self) -> Vec<(String, Vec<String>)> {
        self.items.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 追加一条; 编码已存在时追加到该码文本列表尾部。
    pub fn add(&mut self, code: &str, text: &str) -> Result<(), String> {
        let code = code.trim().to_lowercase();
        let text = text.trim_end_matches('\n');
        if !is_valid_code(&code) {
            return Err("编码只能是 1–12 个小写字母".into());
        }
        if text.trim().is_empty() {
            return Err("短语内容不能为空".into());
        }
        let mut items = self.items.lock().map_err(|_| "锁中毒".to_string())?;
        match items.iter_mut().find(|(c, _)| *c == code) {
            Some((_, v)) => v.push(text.to_string()),
            None => items.push((code, vec![text.to_string()])),
        }
        Ok(())
    }

    /// 原地编辑一条: 编码/文本都可改。编码变化时移动到新码列表尾部。
    pub fn update(
        &mut self,
        old_code: &str,
        old_text: &str,
        new_code: &str,
        new_text: &str,
    ) -> Result<(), String> {
        let old_key = old_code.trim().to_lowercase();
        let new_code = new_code.trim().to_lowercase();
        let new_text = new_text.trim_end_matches('\n');
        if !is_valid_code(&new_code) {
            return Err("编码只能是 1–12 个小写字母".into());
        }
        if new_text.trim().is_empty() {
            return Err("短语内容不能为空".into());
        }
        let mut items = self.items.lock().map_err(|_| "锁中毒".to_string())?;
        let pos = items
            .iter()
            .position(|(c, _)| *c == old_key)
            .ok_or_else(|| "该短语已被修改或删除, 请刷新后重试".to_string())?;
        let idx = items[pos].1.iter().position(|t| t == old_text).ok_or_else(
            || "该短语已被修改或删除, 请刷新后重试".to_string(),
        )?;
        if new_code == old_key {
            items[pos].1[idx] = new_text.to_string();
        } else {
            items[pos].1.remove(idx);
            let moved = new_text.to_string();
            if items[pos].1.is_empty() {
                items.remove(pos);
            }
            match items.iter_mut().find(|(c, _)| *c == new_code) {
                Some((_, v)) => v.push(moved),
                None => items.push((new_code, vec![moved])),
            }
        }
        Ok(())
    }

    /// 删除一条; 该码最后一条删除后连同编码一起移除。
    pub fn remove(&mut self, code: &str, text: &str) -> Result<(), String> {
        let key = code.trim().to_lowercase();
        let mut items = self.items.lock().map_err(|_| "锁中毒".to_string())?;
        let pos = items
            .iter()
            .position(|(c, _)| *c == key)
            .ok_or_else(|| "该短语已被修改或删除, 请刷新后重试".to_string())?;
        let idx = items[pos].1.iter().position(|t| t == text).ok_or_else(
            || "该短语已被修改或删除, 请刷新后重试".to_string(),
        )?;
        items[pos].1.remove(idx);
        if items[pos].1.is_empty() {
            items.remove(pos);
        }
        Ok(())
    }
}

/// JSON 字符串字面量(含引号)。
fn jstr(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// JSON 字符串数组。
fn jarr(v: &[String]) -> String {
    let parts: Vec<String> = v.iter().map(|s| jstr(s)).collect();
    format!("[{}]", parts.join(", "))
}

/// "日期/时间"触发动态候选: 触发词居首, 或候选总数 ≤ DYNAMIC_FEW(可选少了)时,
/// 紧随触发词插入展开后的动态日期/时间(自定义短语原有行为不受影响)。
/// 触发途径有二: 候选中出现触发词本身(五笔等任意能打出它的码), 或编码恰为
/// 整拼 riqi/shijian(悬浮窗只装五笔码表打不出拼音词, 直接识别, 追加到尾部)。
pub fn insert_dynamic(
    mut cands: Vec<(String, String)>,
    code: &str,
    now: Civil,
) -> Vec<(String, String)> {
    let mut fired: Vec<&str> = Vec::new();
    for (word, templates) in &DYNAMIC_WORDS {
        if let Some(pos) = cands.iter().position(|(t, _)| t == *word) {
            // pos==0 触发词居首; cands.len()<=DYNAMIC_FEW 可选少了
            if pos == 0 || cands.len() <= DYNAMIC_FEW {
                let dyn_c: Vec<(String, String)> = templates
                    .iter()
                    .map(|t| (expand_text(t, now), "动态".to_string()))
                    .collect();
                let at = pos + 1;
                cands.splice(at..at, dyn_c);
                fired.push(word);
            }
        }
    }
    if let Some(word) = dynamic_code_word(code) {
        if !fired.contains(&word) {
            let dyn_c: Vec<(String, String)> = DYNAMIC_WORDS
                .iter()
                .find(|(w, _)| *w == word)
                .map(|(_, ts)| {
                    ts.iter()
                        .map(|t| (expand_text(t, now), "动态".to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let at = cands
                .iter()
                .position(|(t, _)| t == word)
                .map(|p| p + 1)
                .unwrap_or(cands.len());
            cands.splice(at..at, dyn_c);
        }
    }
    cands
}

/// floatapp 候选合并规则: 自定义短语(精确码命中, 按定义序, 先展开动态变量)
/// 排最前, 其后接词典候选, 重复文本去重; 最后叠加"日期/时间"触发词的动态候选。
/// 返回 [(text, tag)] — tag 为 "自定义"/"动态" 或词典分值十进制串。
pub fn merged_candidates(
    code: &str,
    book: &PhraseBook,
    dict_lookup: impl Fn(&str, usize) -> Vec<(String, i64)>,
    limit: usize,
    now: Option<Civil>,
) -> Vec<(String, String)> {
    let code = code.trim().to_lowercase();
    if code.is_empty() {
        return Vec::new();
    }
    let now = now.unwrap_or_else(now_civil);
    let custom: Vec<(String, String)> = book
        .lookup(&code)
        .into_iter()
        .map(|t| (expand_text(&t, now), "自定义".to_string()))
        .collect();
    let seen: std::collections::HashSet<&str> =
        custom.iter().map(|(t, _)| t.as_str()).collect();
    let mut rest: Vec<(String, String)> = dict_lookup(&code, limit)
        .into_iter()
        .filter(|(t, _)| !seen.contains(t.as_str()))
        .take(limit.saturating_sub(custom.len()))
        .map(|(t, s)| (t, s.to_string()))
        .collect();
    let mut out = custom;
    out.append(&mut rest);
    insert_dynamic(out, &code, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_book(name: &str) -> (tempdir::TempDir, PhraseBook) {
        let dir = tempdir::TempDir::new();
        let book = PhraseBook::open(dir.path().join(name));
        (dir, book)
    }

    /// 极简 tempdir(单测用, 进程退出由 OS 回收)。
    mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};
        pub struct TempDir(PathBuf);
        static N: AtomicU32 = AtomicU32::new(0);
        impl TempDir {
            pub fn new() -> TempDir {
                let p = std::env::temp_dir().join(format!(
                    "lyyime-float-test-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::SeqCst)
                ));
                std::fs::create_dir_all(&p).unwrap();
                TempDir(p)
            }
            pub fn path(&self) -> PathBuf {
                self.0.clone()
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    // ---------- PhraseBook ----------

    #[test]
    fn add_save_load_roundtrip() {
        let (_d, mut book) = temp_book("phrase.json");
        book.add("yf", "北京市海淀区中关村大街1号").unwrap();
        book.add("dx", "a@example.com").unwrap();
        book.add("dx", "b@example.com").unwrap(); // 同码第二条
        book.save().unwrap();
        let book2 = PhraseBook::open(&book.path);
        assert_eq!(book2.lookup("yf"), vec!["北京市海淀区中关村大街1号"]);
        assert_eq!(book2.lookup("dx"), vec!["a@example.com", "b@example.com"]);
    }

    #[test]
    fn lookup_exact_only_and_case() {
        let (_d, mut book) = temp_book("phrase.json");
        book.add("yf", "短语甲").unwrap();
        assert_eq!(book.lookup("YF"), vec!["短语甲"]); // 大小写归一
        assert!(book.lookup("y").is_empty()); // 无前缀匹配
        assert!(book.lookup("yfx").is_empty());
    }

    #[test]
    fn add_validation() {
        let (_d, mut book) = temp_book("phrase.json");
        assert!(book.add("A1", "含数字编码").is_err()); // 数字是选词键
        assert!(book.add("abcdefghijklmnop", "超长编码").is_err());
        assert!(book.add("ok", "   ").is_err()); // 空内容
        assert!(book.snapshot().is_empty());
    }

    #[test]
    fn update_in_place_and_recode() {
        let (_d, mut book) = temp_book("phrase.json");
        book.add("yf", "旧文本").unwrap();
        book.update("yf", "旧文本", "yf", "新文本").unwrap();
        assert_eq!(book.lookup("yf"), vec!["新文本"]);
        book.update("yf", "新文本", "dz", "换码文本").unwrap();
        assert!(book.lookup("yf").is_empty());
        assert_eq!(book.lookup("dz"), vec!["换码文本"]);
    }

    #[test]
    fn remove_and_last_item_cleanup() {
        let (_d, mut book) = temp_book("phrase.json");
        book.add("yf", "甲").unwrap();
        book.add("yf", "乙").unwrap();
        book.remove("yf", "甲").unwrap();
        assert_eq!(book.lookup("yf"), vec!["乙"]);
        book.remove("yf", "乙").unwrap();
        assert!(book.snapshot().iter().all(|(c, _)| c != "yf")); // 空码不残留
        assert!(book.remove("yf", "乙").is_err()); // 重复删除报错
    }

    #[test]
    fn corrupt_and_missing_file() {
        let (dir, _b) = temp_book("phrase.json");
        let path = dir.path().join("phrase.json");
        std::fs::write(&path, "{not json").unwrap();
        let mut book = PhraseBook::open(&path); // 不 panic
        assert!(book.snapshot().is_empty());
        book.add("ok", "还能写回").unwrap();
        book.save().unwrap(); // 覆盖修复
        assert_eq!(PhraseBook::open(&path).lookup("ok"), vec!["还能写回"]);
    }

    #[test]
    fn load_filters_dirty_entries() {
        let (dir, _b) = temp_book("phrase.json");
        let path = dir.path().join("phrase.json");
        std::fs::write(
            &path,
            r#"{"A1": ["脏编码"], "ok": ["", "干净"], " junk ": ["含空格码"], "good": "不是列表"}"#,
        )
        .unwrap();
        let book = PhraseBook::open(&path);
        let snap = book.snapshot();
        // 非法编码/空文本/非列表值被清洗; 编码两侧空格宽容剥离
        assert!(snap.iter().any(|(c, v)| c == "ok" && v == &["干净"]));
        assert!(snap.iter().any(|(c, v)| c == "junk" && v == &["含空格码"]));
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn multiline_long_text() {
        let (_d, mut book) = temp_book("phrase.json");
        let long_text = format!("第一行地址\n第二行电话 13800000000\n{}", "很".repeat(200));
        book.add("dz", &long_text).unwrap();
        book.save().unwrap();
        assert_eq!(PhraseBook::open(&book.path).lookup("dz"), vec![long_text]);
    }

    // ---------- expand_text ----------

    // 2026-09-06 15:07:09 是星期日
    fn now() -> Civil {
        Civil { year: 2026, month: 9, day: 6, hour: 15, minute: 7, second: 9, weekday: 6 }
    }

    #[test]
    fn date_formats() {
        let n = now();
        assert_eq!(expand_text("$date(yyyy-MM-dd)", n), "2026-09-06");
        assert_eq!(expand_text("$date(yyyy年M月d日)", n), "2026年9月6日");
        assert_eq!(expand_text("$date(yy/M/d)", n), "26/9/6");
        assert_eq!(expand_text("$date()", n), "2026-09-06"); // 缺省格式
        assert_eq!(expand_text("$date(E)", n), "日");
        assert_eq!(expand_text("$date(星期E)", n), "星期日");
    }

    #[test]
    fn time_week_and_escape() {
        let n = now();
        assert_eq!(expand_text("$time(HH:mm)", n), "15:07");
        assert_eq!(expand_text("$time()", n), "15:07"); // 缺省格式
        assert_eq!(expand_text("$time(H时m分s秒)", n), "15时7分9秒");
        assert_eq!(expand_text("$week", n), "星期日");
        assert_eq!(expand_text("$$date(yyyy)", n), "$date(yyyy)"); // $$ 转义
        assert_eq!(expand_text("$week的安排", n), "星期日的安排"); // 后接汉字照常展开
    }

    #[test]
    fn unknown_variable_kept_verbatim() {
        let n = now();
        // 未识别的 $…、括号不闭合 → 原样保留, 不报错(用户数据容错)
        assert_eq!(expand_text("$mail@example", n), "$mail@example");
        assert_eq!(expand_text("$date(yyyy-MM-dd", n), "$date(yyyy-MM-dd");
        assert_eq!(expand_text("$weekly", n), "$weekly"); // $week 后接字母不展开
        assert_eq!(expand_text("尾随的$", n), "尾随的$");
        assert_eq!(expand_text("纯文本无变量", n), "纯文本无变量");
    }

    #[test]
    fn zero_padding() {
        let n = Civil { year: 2026, month: 1, day: 2, hour: 3, minute: 4, second: 5, weekday: 0 };
        assert_eq!(expand_text("$date(yyyy年MM月dd日)", n), "2026年01月02日");
        assert_eq!(expand_text("$time(HH:mm:ss)", n), "03:04:05");
    }

    #[test]
    fn mixed_static_and_dynamic() {
        let n = now();
        assert_eq!(
            expand_text("今天 $date(yyyy/M/d), 现在 $time(HH:mm)", n),
            "今天 2026/9/6, 现在 15:07"
        );
    }

    #[test]
    fn civil_from_unix_matches_known() {
        // 2026-09-06 15:07:09 UTC = 1788707229(星期日)
        let c = Civil::from_unix(1_788_707_229);
        assert_eq!(c.year, 2026);
        assert_eq!(c.month, 9);
        assert_eq!(c.day, 6);
        assert_eq!(c.hour, 15);
        assert_eq!(c.minute, 7);
        assert_eq!(c.second, 9);
        assert_eq!(WEEK_CN[c.weekday], "日");
    }

    // ---------- merged_candidates ----------

    fn dict_lookup(_code: &str, limit: usize) -> Vec<(String, i64)> {
        vec![
            ("你好".into(), 999),
            ("你".into(), 100),
            ("候".into(), 90),
        ]
        .into_iter()
        .take(limit)
        .collect()
    }

    #[test]
    fn merge_custom_first_and_dedup() {
        let (_d, mut book) = temp_book("phrase.json");
        book.add("wqvb", "你好").unwrap();
        book.add("wqvb", "自定义你好").unwrap();
        let got = merged_candidates("wqvb", &book, dict_lookup, 45, Some(now()));
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        // 短语在前, 词典重复的"你好"去掉
        assert_eq!(texts, vec!["你好", "自定义你好", "你", "候"]);
        assert_eq!(got[0].1, "自定义");
        assert_eq!(got[2].1, "100"); // 词典候选带原分值
    }

    #[test]
    fn merge_empty_code_and_no_custom() {
        let (_d, book) = temp_book("phrase.json");
        assert!(merged_candidates("", &book, dict_lookup, 45, None).is_empty());
        let got = merged_candidates(
            "zzz",
            &book,
            |c, _| if c == "zzz" { vec![("词".into(), 1)] } else { vec![] },
            45,
            None,
        );
        assert_eq!(got, vec![("词".to_string(), "1".to_string())]);
    }

    #[test]
    fn merge_limit_respects_custom() {
        let (_d, mut book) = temp_book("phrase.json");
        for i in 0..3 {
            book.add("aaa", &format!("短{i}")).unwrap();
        }
        let got = merged_candidates(
            "aaa",
            &book,
            |_, _| (0..10).map(|_| ("d".to_string(), 1)).collect(),
            5,
            None,
        );
        assert_eq!(got.len(), 5);
        let texts: Vec<&str> = got.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(&texts[..3], &["短0", "短1", "短2"]); // 短语不超限挤掉
    }

    #[test]
    fn merge_expands_dynamic() {
        let (_d, mut book) = temp_book("phrase.json");
        book.add("rq", "$date(yyyy/M/d)").unwrap();
        book.add("rq", "固定短语").unwrap();
        let got = merged_candidates("rq", &book, |_, _| Vec::new(), 45, Some(now()));
        assert_eq!(
            got,
            vec![
                ("2026/9/6".to_string(), "自定义".to_string()),
                ("固定短语".to_string(), "自定义".to_string())
            ]
        );
    }

    // ---------- 动态日期/时间触发词 ----------

    fn texts_of(got: &[(String, String)]) -> Vec<&str> {
        got.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn dynamic_top_word_triggers_date() {
        let (_d, book) = temp_book("phrase.json");
        // "日期"是候选首选(如五笔 jjad 精确命中)
        let got = merged_candidates(
            "jjad",
            &book,
            |_, _| vec![("日期".into(), 999), ("日".into(), 100)],
            45,
            Some(now()),
        );
        assert_eq!(texts_of(&got), vec!["日期", "2026年9月6日", "2026-09-06", "日"]);
        assert_eq!(got[1].1, "动态");
    }

    #[test]
    fn dynamic_time_word_triggers() {
        let (_d, book) = temp_book("phrase.json");
        let got = merged_candidates(
            "jfuj",
            &book,
            |_, _| vec![("时间".into(), 900), ("时".into(), 50)],
            45,
            Some(now()),
        );
        assert_eq!(texts_of(&got), vec!["时间", "15:07", "15时07分", "时"]);
    }

    #[test]
    fn dynamic_not_first_but_few_still_triggers() {
        let (_d, book) = temp_book("phrase.json");
        // "日期"排第 3, 但候选很少(≤ DYNAMIC_FEW)→ 仍触发
        let got = merged_candidates(
            "xxxx",
            &book,
            |_, _| {
                vec![
                    ("甲".into(), 9),
                    ("乙".into(), 8),
                    ("日期".into(), 7),
                    ("丙".into(), 6),
                ]
            },
            45,
            Some(now()),
        );
        assert_eq!(
            texts_of(&got),
            vec!["甲", "乙", "日期", "2026年9月6日", "2026-09-06", "丙"]
        );
    }

    #[test]
    fn dynamic_buried_in_many_no_trigger() {
        let (_d, book) = temp_book("phrase.json");
        // "日期"深埋、候选又多(> DYNAMIC_FEW)→ 不打扰
        let mut cands: Vec<(String, i64)> =
            (0..10).map(|i| (format!("词{i}"), i as i64)).collect();
        cands.insert(3, ("日期".into(), 5));
        let got =
            merged_candidates("xxxx", &book, |_, _| cands.clone(), 45, Some(now()));
        assert!(got.iter().all(|(_, tag)| tag != "动态"));
    }

    #[test]
    fn dynamic_few_without_trigger_word_nothing_added() {
        let (_d, book) = temp_book("phrase.json");
        // 候选少但没有触发词 → 不加动态
        let got = merged_candidates(
            "wqvb",
            &book,
            |_, _| vec![("你好".into(), 999), ("你".into(), 100)],
            45,
            Some(now()),
        );
        assert_eq!(texts_of(&got), vec!["你好", "你"]);
    }

    #[test]
    fn dynamic_pinyin_code_direct_trigger() {
        let (_d, book) = temp_book("phrase.json");
        // 悬浮窗只装五笔码表: 整拼 riqi 打不出词, 动态日期兜底出现
        let got =
            merged_candidates("riqi", &book, |_, _| Vec::new(), 45, Some(now()));
        assert_eq!(
            got,
            vec![
                ("2026年9月6日".to_string(), "动态".to_string()),
                ("2026-09-06".to_string(), "动态".to_string()),
            ]
        );
        let got = merged_candidates(
            "shijian",
            &book,
            |_, _| vec![("间".into(), 1)],
            45,
            Some(now()),
        );
        assert_eq!(texts_of(&got), vec!["间", "15:07", "15时07分"]);
    }

    #[test]
    fn dynamic_custom_phrase_feature_untouched() {
        let (_d, mut book) = temp_book("phrase.json");
        book.add("rq", "$date(yyyy/M/d)").unwrap();
        let got = merged_candidates(
            "rq",
            &book,
            |_, _| vec![("日期".into(), 1)],
            45,
            Some(now()),
        );
        // 自定义短语仍排最前并展开; 词典里的"日期"照常触发, 动态候选紧随其后
        assert_eq!(
            got,
            vec![
                ("2026/9/6".to_string(), "自定义".to_string()),
                ("日期".to_string(), "1".to_string()),
                ("2026年9月6日".to_string(), "动态".to_string()),
                ("2026-09-06".to_string(), "动态".to_string()),
            ]
        );
    }
}
