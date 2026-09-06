//! lyyIme ibus 引擎(Mode A)入口 —— Rust/zbus 版。
//!
//! 分层(与原 python 版一致,python 实现已删除):
//!   logic.rs  按键逻辑核心(keysym → LKey → lyyime-core 直连 → 效果流),可独立单测
//!   service.rs zbus 胶水(Factory/Engine 接口、效果流 → IBus 信号、/AI 后台调用)
//!   ibus_wire.rs IBus 对象 GVariant 构建(签名对照真机抓包)
//!
//! 行为合同:docs/ARCHITECTURE.md §2/§3/§6/§7、§11(/AI)。

mod ibus_wire;
mod keysym;
mod logic;
mod logger;
mod service;

use lyyime_core::Config;
use std::path::PathBuf;

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

/// 读共用 config.toml 的 page_size(1–9)与 commit_on_extra_after_four。
fn read_core_config() -> (usize, bool) {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{home}/.config", home = std::env::var("HOME").unwrap_or_else(|_| "/root".into())));
    let path = PathBuf::from(base).join("lyyime/config.toml");
    let mut page_size = 5usize;
    let mut commit_after_four = false;
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(data) = toml::from_str::<toml::Value>(&text) {
            if let Some(v) = data.get("page_size").and_then(|v| v.as_integer()) {
                page_size = v as usize;
            }
            if let Some(v) = data.get("commit_on_extra_after_four").and_then(|v| v.as_bool()) {
                commit_after_four = v;
            }
        }
    }
    (page_size.clamp(1, 9), commit_after_four)
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
