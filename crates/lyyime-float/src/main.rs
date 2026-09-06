//! lyyIme 悬浮窗独立输入(Rust/GTK3)。
//!
//! 不依赖 ibus/fcitx、不切换系统输入法: 在悬浮窗里打五笔86(极点/海峰码表),
//! 空格/数字选词, 选中的文本自动发往"最近使用的应用窗口"(直输/粘贴两种模式)。
//! 支持自定义短语(精确码 → 任意长度文本, 动态变量 $date/$time/$week)。
//!
//! 用法:
//!   lyyime-float              主程序(悬浮窗)
//!   lyyime-float --pad        简易打字板(复制中转用)
//!   lyyime-float --smoke-phrases  短语管理对话框冒烟(e2e 测试用)

mod config;
mod dict;
mod phrases;
mod sender;
mod ui;
mod xtrack;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--pad") => ui::run_pad(),
        Some("--smoke-stats") => {
            if let Err(e) = ui::run_smoke_stats() {
                eprintln!("冒烟失败: {e}");
                std::process::exit(1);
            }
        }
        Some("--smoke-phrases") => {
            if let Err(e) = ui::run_smoke_phrases() {
                eprintln!("冒烟失败: {e}");
                std::process::exit(1);
            }
        }
        Some(arg) => {
            eprintln!("未知参数: {arg}\n用法: lyyime-float [--pad|--smoke-phrases]");
            std::process::exit(2);
        }
        None => run_main_app(),
    }
}

fn run_main_app() {
    // 单实例: 已有悬浮窗在跑 → SIGUSR1 唤起(显示+聚焦)后自身退出
    if let Some(pid) = config::pidfile_alive() {
        unsafe {
            libc::kill(pid, libc::SIGUSR1);
        }
        eprintln!("lyyime-float 已在运行(pid={pid}),已唤起其悬浮窗。");
        return;
    }
    if let Err(e) = config::write_pidfile() {
        eprintln!("警告: 写 pidfile 失败({e:#}), 单实例守护不生效");
    }
    if let Err(e) = ui::create() {
        eprintln!("{}", xtrack::connect_error_report(e));
        config::remove_pidfile();
        std::process::exit(1);
    }
    gtk::main();
}
