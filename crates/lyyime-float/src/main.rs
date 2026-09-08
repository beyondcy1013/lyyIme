//! lyyIme 悬浮窗独立输入(Rust/GTK3)。
//!
//! 不依赖 ibus/fcitx、不切换系统输入法: 在悬浮窗里打五笔86(极点/海峰码表),
//! 空格/数字选词, 选中的文本自动发往"最近使用的应用窗口"(直输/粘贴两种模式)。
//! 中文标点直上(空缓冲直上; 有缓冲先上首选/原码再上标点, 引号开合交替);
//! 空缓冲退格/删除直通目标窗口删字。
//! 「模拟内置」模式(inline.rs): 候选小窗贴目标应用光标显示——pyatspi 助手
//! 轮询光标的窗口相对坐标, X11 提供窗口根位置合成绝对坐标; 不支持无障碍的
//! 应用(Electron 未开 ACCESSIBILITY、终端)退化为指针位置锚点。
//! 支持自定义短语(精确码 → 任意长度文本, 动态变量 $date/$time/$week)。
//!
//! 用法:
//!   lyyime-float              主程序(悬浮窗)
//!   lyyime-float --pad        简易打字板(复制中转用)
//!   lyyime-float --smoke-phrases  短语管理对话框冒烟(e2e 测试用)

mod config;
mod dict;
mod inline;
mod phrases;
mod sender;
mod ui;
mod undo;
mod xtrack;

fn main() {
    // 悬浮窗自己的按键必须直达 Entry, 绝不能被桌面上的输入法服务二次组词:
    // 桌面会话里 ibus-daemon(带 lyyime 引擎)/lyyime-xim 都活着, GTK 默认 IM
    // 模块若绑上它们, 打进悬浮窗的键会被当成编码吃掉并提交回输入框自己,
    // 表现为"标点/空格失灵, 要按回车才上屏"。gtk_init 前钉死为无 IM 模块。
    std::env::set_var("GTK_IM_MODULE", "gtk-im-context-none");
    std::env::set_var("XMODIFIERS", "@im=none");
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
