//! lyyIme ibus 引擎(Mode A)入口 —— Rust/zbus 版。
//!
//! 分层(与原 python 版一致,python 实现已删除):
//!   logic.rs  按键逻辑核心(keysym → LKey → lyyime-core 直连 → 效果流),可独立单测
//!   service.rs zbus 胶水(Factory/Engine 接口、效果流 → IBus 信号、/AI 后台调用)
//!   ibus_wire.rs IBus 对象 GVariant 构建(签名对照真机抓包)
//!
//! 行为合同:docs/ARCHITECTURE.md §2/§3/§6/§7、§11(/AI)。

mod candidate_ui;
mod ibus_wire;
mod keyboard;
mod keysym;
mod logic;
mod logger;
mod service;

use lyyime_core::Config;
use std::path::{Path, PathBuf};

/// 词典目录回退链(与原 lyyime_ffi.py / lyyime-xim resolve_dict_dir 一致):
/// $LYYIME_DATA_DIR → 用户级 data/(含 meta.json 才认)→ 系统安装位 → 用户兜底。
fn resolve_data_dir() -> PathBuf {
    if let Ok(env) = std::env::var("LYYIME_DATA_DIR") {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let user = PathBuf::from(&home).join(".local/share/lyyime");
    let mut candidates = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            candidates.push(PathBuf::from(xdg).join("lyyime/data"));
        }
    }
    candidates.push(user.join("data"));
    candidates.push(PathBuf::from("/usr/local/share/lyyime/data"));
    for cand in candidates {
        if cand.join("meta.json").is_file() {
            return cand;
        }
    }
    user
}

/// 解析 ibus 私有总线地址(组件必须连 ibus 总线而非 session bus!):
/// 1) 环境变量 IBUS_ADDRESS;2) ~/.config/ibus/bus/<machine>-unix-<display>
/// 地址文件中的 IBUS_ADDRESS= 行(优先匹配当前 DISPLAY 编号,退回任意一份)。
fn resolve_ibus_address() -> Option<String> {
    if let Ok(a) = std::env::var("IBUS_ADDRESS") {
        if !a.is_empty() {
            return Some(a);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{home}/.config"));
    let dir = PathBuf::from(base).join("ibus/bus");
    let display_no = std::env::var("DISPLAY")
        .ok()
        .and_then(|d| d.split(':').next_back().and_then(|n| n.split('.').next().map(str::to_string)));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().contains("-unix-"))
        .collect();
    // 当前 DISPLAY 对应的地址文件排最前
    if let Some(no) = &display_no {
        files.sort_by_key(|p| {
            let name = p.to_string_lossy().into_owned();
            (if name.ends_with(&format!("-unix-{no}")) { 0 } else { 1 }, name.clone())
        });
    }
    for f in files {
        if let Ok(text) = std::fs::read_to_string(&f) {
            for line in text.lines() {
                if let Some(a) = line.strip_prefix("IBUS_ADDRESS=") {
                    if !a.is_empty() {
                        return Some(a.to_string());
                    }
                }
            }
        }
    }
    None
}

fn config_toml_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{home}/.config", home = std::env::var("HOME").unwrap_or_else(|_| "/root".into())));
    PathBuf::from(base).join("lyyime/config.toml")
}
pub(crate) const MENU_BOOL_KEYS: &[&str] = &[
    "pinyin_only",
    "chinese_punct",
    "learning",
    "next_word_prediction",
    "quick_actions_enabled",
];

pub(crate) fn read_config_bool(key: &str) -> Option<bool> {
    read_config_bool_at(&config_toml_path(), key)
}

pub(crate) fn read_config_bool_at(path: &Path, key: &str) -> Option<bool> {
    let text = std::fs::read_to_string(path).ok()?;
    let data = toml::from_str::<toml::Value>(&text).ok()?;
    data.get(key).and_then(|v| v.as_bool())
}

pub(crate) fn menu_bool_default(key: &str) -> bool {
    let d = Config::default();
    match key {
        "pinyin_only" => d.pinyin_only,
        "chinese_punct" => d.cn_punct,
        "learning" => d.learning,
        "next_word_prediction" => d.next_word_prediction,
        "quick_actions_enabled" => d.quick_actions_enabled,
        _ => false,
    }
}

fn toml_key_matches(raw: &str, key: &str) -> bool {
    let k = raw.trim();
    if k == key {
        return true;
    }
    let b = k.as_bytes();
    k.len() >= 2
        && ((b[0] == b'"' && b[k.len() - 1] == b'"') || (b[0] == b'\'' && b[k.len() - 1] == b'\''))
        && &k[1..k.len() - 1] == key
}

fn toplevel_multiline_string(text: &str) -> bool {
    let mut term = None;
    for line in text.split_inclusive('\n') {
        if let Some(t) = term {
            if line.contains(t) {
                term = None;
            }
            continue;
        }
        let t = line.trim_start();
        if t.starts_with('[') || t.starts_with('#') || t.is_empty() {
            continue;
        }
        if t.matches("\"\"\"").count() % 2 == 1 {
            term = Some("\"\"\"");
        } else if t.matches("'''").count() % 2 == 1 {
            term = Some("'''");
        }
        if term.is_some() {
            return true;
        }
    }
    false
}

fn update_config_bool_via_toml(text: &str, key: &str, value: bool) -> Option<String> {
    let mut tbl = toml::from_str::<toml::value::Table>(text).ok()?;
    tbl.insert(key.to_string(), toml::Value::Boolean(value));
    toml::to_string(&toml::Value::Table(tbl)).ok()
}

pub(crate) fn update_config_bool_in_text(text: &str, key: &str, value: bool) -> Option<String> {
    if !MENU_BOOL_KEYS.contains(&key) {
        return None;
    }
    if !text.is_empty() && toml::from_str::<toml::Value>(text).is_err() {
        return None;
    }
    if toplevel_multiline_string(text) {
        return update_config_bool_via_toml(text, key, value);
    }
    let lit = if value { "true" } else { "false" };
    let mut out = String::with_capacity(text.len() + 64);
    let mut done = false;
    let mut inserted = false;
    let mut section_depth = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let is_section = trimmed.starts_with('[');
        if !done && !is_section && !section_depth {
            if let Some(eq) = trimmed.find('=') {
                if toml_key_matches(&trimmed[..eq], key) && !trimmed.starts_with('#') {
                    let eol = if line.ends_with('\n') { "\n" } else { "" };
                    let indent = &line[..line.len() - trimmed.len()];
                    let tail = trimmed[eq + 1..].trim_end();
                    let mut in_str = false;
                    let mut comment_at = None;
                    for (i, ch) in tail.char_indices() {
                        match ch {
                            '"' => in_str = !in_str,
                            '#' if !in_str => {
                                comment_at = Some(i);
                                break;
                            }
                            _ => {}
                        }
                    }
                    if let Some(pos) = comment_at {
                        out.push_str(&format!("{indent}{key} = {lit} {}{eol}", &tail[pos..]));
                    } else {
                        out.push_str(&format!("{indent}{key} = {lit}{eol}"));
                    }
                    done = true;
                    continue;
                }
            }
        }
        if is_section {
            section_depth = true;
            if !done && !inserted {
                out.push_str(&format!("{key} = {lit}\n\n"));
                inserted = true;
                done = true;
            }
        }
        out.push_str(line);
    }
    if !done {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{key} = {lit}\n"));
    }
    if toml::from_str::<toml::Value>(&out).is_err() {
        return None;
    }
    Some(out)
}

pub(crate) fn update_config_bool_at(path: &Path, key: &str, value: bool) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let out = update_config_bool_in_text(&text, key, value).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("配置解析失败或键不受支持,未回写:{}", path.display()),
        )
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    let mode: u32 = {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(m) => m.permissions().mode() & 0o777,
            Err(_) => 0o600,
        }
    };
    use std::io::Write;
    let mut file = None;
    let mut tmp = path.to_path_buf();
    for attempt in 0..16u32 {
        let mut name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(format!(".tmp.{}.{}", std::process::id(), attempt));
        let cand = path.with_file_name(name);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
        }
        match opts.open(&cand) {
            Ok(f) => {
                file = Some(f);
                tmp = cand;
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let Some(mut f) = file else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "无法分配临时文件",
        ));
    };
    let res = (|| -> std::io::Result<()> {
        f.write_all(out.as_bytes())?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        }
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

pub(crate) fn update_config_bool(key: &str, value: bool) -> std::io::Result<()> {
    update_config_bool_at(&config_toml_path(), key, value)
}
/// 读共用 config.toml(缺项/损坏一律回退默认,引擎永不因配置启动失败):
/// page_size(1–9)、commit_on_extra_after_four、commit_unique_four、
/// phrase_hint、coin_hotkey、shot_hotkey(§12/§13)与快速功能键
/// quick_actions_enabled/[[quick_actions]](§14,EngineLogic/service 用)。
fn read_core_config() -> Config {
    let path = config_toml_path();
    let mut cfg = Config::default();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(data) = toml::from_str::<toml::Value>(&text) {
            if let Some(v) = data.get("page_size").and_then(|v| v.as_integer()) {
                cfg.page_size = v.clamp(1, 10) as usize;
            }
            if let Some(v) = data.get("pinyin_only").and_then(|v| v.as_bool()) {
                cfg.pinyin_only = v;
            }
            if let Some(v) = data.get("mixed_english").and_then(|v| v.as_bool()) {
                cfg.mixed_en = v;
            }
            if let Some(v) = data.get("learning").and_then(|v| v.as_bool()) {
                cfg.learning = v;
            }
            if let Some(v) = data
                .get("chinese_punct")
                .and_then(|v| v.as_bool())
                .or_else(|| data.get("cn_punct").and_then(|v| v.as_bool()))
            {
                cfg.cn_punct = v;
            }
            // 四码顶屏:规范键 commit_on_extra_after_four,兼容 XIM 设置窗
            // 写盘键名 commit_after_four(同一份 config.toml 跨模式共用)。
            if let Some(v) = data
                .get("commit_on_extra_after_four")
                .and_then(|v| v.as_bool())
                .or_else(|| data.get("commit_after_four").and_then(|v| v.as_bool()))
            {
                cfg.commit_on_extra_after_four = v;
            }
            if let Some(v) = data.get("commit_first_at_four").and_then(|v| v.as_bool()) {
                cfg.commit_first_at_four = v;
            }
            if let Some(v) = data.get("commit_unique_four").and_then(|v| v.as_bool()) {
                cfg.commit_unique_four = v;
            }
            if let Some(v) = data.get("phrase_hint").and_then(|v| v.as_bool()) {
                cfg.phrase_hint = v;
            }
            if let Some(v) = data.get("next_word_prediction").and_then(|v| v.as_bool()) {
                cfg.next_word_prediction = v;
            }
            if let Some(v) = data.get("coin_hotkey").and_then(|v| v.as_str()) {
                if !v.trim().is_empty() {
                    cfg.coin_hotkey = v.trim().to_string();
                }
            }
            if let Some(v) = data.get("shot_hotkey").and_then(|v| v.as_str()) {
                if !v.trim().is_empty() {
                    cfg.shot_hotkey = v.trim().to_string();
                }
            }
            // 快速功能键(§14):写出的表替换默认表;非法条目跳过,不致命
            if let Some(v) = data.get("quick_actions_enabled").and_then(|v| v.as_bool()) {
                cfg.quick_actions_enabled = v;
            }
            if let Some(arr) = data.get("quick_actions").and_then(|v| v.as_array()) {
                let list: Vec<lyyime_core::QuickAction> = arr
                    .iter()
                    .filter_map(|t| {
                        let trigger = t.get("trigger")?.as_str()?.to_string();
                        let label = t.get("label")?.as_str()?.to_string();
                        let command = t.get("command")?.as_str()?.to_string();
                        lyyime_core::QuickAction::trigger_valid(&trigger)
                            .then_some(lyyime_core::QuickAction {
                                trigger,
                                label,
                                command,
                            })
                    })
                    .take(lyyime_core::QUICK_ACTIONS_MAX)
                    .collect();
                cfg.quick_actions = list;
            }
            apply_menu_trigger_toml(&data, &mut cfg);
        }
    }
    // 热键冲突自动升级(合同 §13):加载即自愈,截屏热键让位并留痕日志
    for note in lyyime_core::hotkey::resolve_config_hotkeys(&mut cfg) {
        crate::logger::warn(&note);
    }
    cfg
}

/// 菜单触发三键(menu_trigger_enabled/_key/_disabled)套用进 Config;
/// 缺项保留默认,key 越界回退默认 7(与 core Config 校验同口径)。
fn apply_menu_trigger_toml(data: &toml::Value, cfg: &mut Config) {
    if let Some(v) = data.get("menu_trigger_enabled").and_then(|v| v.as_bool()) {
        cfg.menu_trigger_enabled = v;
    }
    if let Some(v) = data.get("menu_trigger_key").and_then(|v| v.as_integer()) {
        cfg.menu_trigger_key = if (1..=12).contains(&v) {
            v as usize
        } else {
            Config::default().menu_trigger_key
        };
    }
    if let Some(v) = data.get("menu_trigger_disabled").and_then(|v| v.as_str()) {
        cfg.menu_trigger_disabled = v.to_string();
    }
}

/// §15 自定义查询(config.toml 顶层 custom_query_label/custom_query_url):
/// 返回 None = 未配置(操作行不显示第 4 项);url 空白同效。
/// 与 read_core_config 同一文件,焦点进入/建引擎时调用(设置保存即生效)。
pub(crate) fn read_custom_query() -> Option<lyyime_core::wordops::CustomQuery> {
    let path = config_toml_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let data = toml::from_str::<toml::Value>(&text).ok()?;
    let cq = lyyime_core::wordops::CustomQuery {
        label: data
            .get("custom_query_label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        url: data
            .get("custom_query_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    };
    cq.configured().then_some(cq)
}

pub(crate) fn ui_style() -> (String, i32) {
    let data = std::fs::read_to_string(config_toml_path())
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok());
    let get = |k: &str| data.as_ref().and_then(|d| d.get(k));
    let skin = get("skin")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("system")
        .to_string();
    let font_size = get("font_size")
        .and_then(|v| v.as_integer())
        .unwrap_or(14)
        .clamp(10, 28) as i32;
    (skin, font_size)
}

/// 图标目录:二进制同级的 ../icons(与 python 版 ../icons 解析一致;装机时
/// install.sh 把引擎放 ibus/engine/、图标放 ibus/icons/)。
fn default_icon() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(dir) = exe {
        let p = dir.join("../icons/lyyime.svg");
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
        let p2 = dir.join("../../ibus/icons/lyyime.svg");
        if p2.is_file() {
            return p2.to_string_lossy().into_owned();
        }
    }
    "lyyime".into()
}

fn icon_dir_for_engine() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(dir) = exe {
        let p = dir.join("../icons");
        if p.is_dir() {
            return p.to_string_lossy().into_owned();
        }
    }
    "/usr/local/share/lyyime/ibus/icons".into()
}

/// 对输入法进程实施内存锁定，防止在高 I/O 和内存压力下被换出到 Swap 造成按键卡顿
fn lock_process_memory() {
    #[cfg(target_os = "linux")]
    unsafe {
        let rlim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        let _ = libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim);

        if libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) == 0 {
            logger::info("mlockall 内存锁定成功：已防止进程换出到 Swap");
        } else {
            let err = std::io::Error::last_os_error();
            logger::warn(&format!(
                "mlockall 内存锁定未生效({err}): 输入法在内存压力下可能被置换到 Swap"
            ));
        }
    }
}

fn main() {
    let mut exec_by_ibus = false;
    let mut verbose = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--ibus" | "-i" => exec_by_ibus = true,
            "-v" | "--verbose" => verbose = true,
            "--version" => {
                println!("lyyime ibus engine {}", ibus_wire::VERSION);
                return;
            }
            "-h" | "--help" => {
                println!("用法: ibus-engine-lyyime [--ibus|-i] [-v] [--version]\n  --ibus/-i  由 ibus-daemon 拉起时自动附加,手动运行勿用");
                return;
            }
            other => {
                eprintln!("未知参数:{other}");
                std::process::exit(2);
            }
        }
    }
    logger::init(verbose || std::env::var("LYYIME_DEBUG").as_deref() == Ok("1"));
    keyboard::init_threads(); // 须早于一切 Xlib 使用(GTK 候选窗/caps 解锁)
    lock_process_memory();

    let icon_dir = icon_dir_for_engine();
    let ibus_addr = resolve_ibus_address();
    // zbus 4 自带后台执行器;连接注册完成后主线程挂起即可持续服务
    let result = zbus::block_on(service::run(exec_by_ibus, icon_dir, ibus_addr));
    if let Err(e) = result {
        let msg = format!(
            "lyyime: 无法连接 ibus-daemon,请确认 ibus 正在运行(可执行 ibus-daemon -drx 后重试)。\n{e:#}"
        );
        eprintln!("{msg}");
        logger::error(&msg);
        std::process::exit(1);
    }
    // 常驻:服务不退出(ibus 断开时 zbus 连接会关,由信号发送失败兜底退出)
    zbus::block_on(std::future::pending::<()>());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let d = std::env::temp_dir().join(format!(
            "lyyime-cfg-{}-{nanos}-{n}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d.join("config.toml")
    }

    #[test]
    fn 布尔更新_无关段落注释与未知键字节保留() {
        let src = "learning = true # keep me\nunknown_key = \"zzz\"\n# tail comment\n\n[ai]\napi_key = \"sk-SECRET\"\nmodel = \"sentinel\"\n\n[[quick_actions]]\ntrigger = \"jietu\"\nlabel = \"截图\"\ncommand = \"@shot\"\nlearning = true\n";
        let out = update_config_bool_in_text(src, "learning", false).unwrap();
        let expect = "learning = false # keep me\nunknown_key = \"zzz\"\n# tail comment\n\n[ai]\napi_key = \"sk-SECRET\"\nmodel = \"sentinel\"\n\n[[quick_actions]]\ntrigger = \"jietu\"\nlabel = \"截图\"\ncommand = \"@shot\"\nlearning = true\n";
        assert_eq!(out, expect, "应只替换顶层 learning,其余字节不动");
    }

    #[test]
    fn 布尔更新_顶层多行字符串走整表序列化() {
        let src = "note = '''\nlearning = true\n[ai]\n'''\nmode = \"cn\"\nlearning = true\n";
        let out = update_config_bool_in_text(src, "learning", false).unwrap();
        let data = toml::from_str::<toml::Value>(&out).unwrap();
        assert_eq!(data["learning"].as_bool(), Some(false));
        assert_eq!(
            data["note"].as_str().unwrap(),
            "learning = true\n[ai]\n",
            "哨兵字符串内容不得被改写:{out}"
        );
        assert_eq!(data["mode"].as_str(), Some("cn"));
    }

    #[test]
    fn 布尔更新_缺键插到首个段前() {
        let out = update_config_bool_in_text("[ai]\nmodel = \"x\"\n", "pinyin_only", true).unwrap();
        let pos = out.find("pinyin_only = true").unwrap();
        assert!(pos < out.find("[ai]").unwrap(), "{out}");
        let out = update_config_bool_in_text("", "learning", false).unwrap();
        assert_eq!(out.trim(), "learning = false");
    }

    #[test]
    fn 布尔更新_引号键与非法键() {
        let out = update_config_bool_in_text("\"pinyin_only\" = true\n", "pinyin_only", false).unwrap();
        assert!(out.contains("pinyin_only = false"), "{out}");
        let out = update_config_bool_in_text("'learning' = true\n", "learning", false).unwrap();
        assert!(out.contains("learning = false"), "{out}");
        assert!(update_config_bool_in_text("", "api_key", true).is_none());
    }

    #[test]
    fn 布尔更新_损坏toml拒绝回写() {
        assert!(update_config_bool_in_text("learning = \n[broken", "learning", false).is_none());
        assert!(update_config_bool_in_text("learning = yes\n", "learning", false).is_none());
    }

    #[test]
    #[cfg(unix)]
    fn 持久化_权限私有且原子替换() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp("perm");
        std::fs::write(&path, "learning = true\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        update_config_bool_at(&path, "learning", false).unwrap();
        let m = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "原权限应保留,实为 {m:o}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("learning = false"));

        let fresh = tmp("fresh");
        update_config_bool_at(&fresh, "pinyin_only", true).unwrap();
        let m = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "新文件应为 0600,实为 {m:o}");
    }

    #[test]
    fn 持久化_读取错误与损坏输入均不改文件() {
        let path = tmp("bad");
        std::fs::write(&path, "learning = yes\n").unwrap();
        assert!(update_config_bool_at(&path, "learning", false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "learning = yes\n");

        let dir = std::env::temp_dir().join(format!("lyyime-cfg-{}-dir", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = update_config_bool_at(&dir, "learning", false);
        assert!(r.is_err(), "目录路径应报读取错误");
    }

    #[test]
    fn 默认值表_与Config一致() {
        let d = Config::default();
        assert_eq!(menu_bool_default("learning"), d.learning);
        assert_eq!(menu_bool_default("chinese_punct"), d.cn_punct);
        assert_eq!(menu_bool_default("pinyin_only"), d.pinyin_only);
        assert_eq!(menu_bool_default("quick_actions_enabled"), d.quick_actions_enabled);
        assert!(menu_bool_default("learning"), "learning 默认应为 true");
    }
}
