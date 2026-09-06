//! lyyime-doctor CLI:
//!   check                       诊断(表格 / --json)
//!   fix --issue <id> | --all [--dry-run]
//!   ime-list [--json]
//!   ime-add <id> [--yes]        默认 dry-run 预览
//!   ime-remove <id> [--force] [--yes]
//!   ime-default <id> [--yes]
//! 依赖仅 std + serde/serde_json/anyhow(项目规则:少引依赖)。

use lyyime_doctor::{CheckReport, Doctor, ImeManager, ImeOpResult, Paths, ProbeOptions, Status};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = r#"lyyime-doctor —— Linux 中文输入法诊断/修复/管理

用法:
  lyyime-doctor check [--json]
      诊断输入法环境:env / gui-env / session-bus / daemon / engine-register /
      autostart / immodule / data / logs / locale
      (gui-env 读真实 GUI 进程 /proc/<pid>/environ;session-bus 核对 IBus 是否
       注册在桌面会话真实使用的总线上——「总线接错」「环境断层」两大故障的直接定位项)

  lyyime-doctor fix (--issue <id>)... | --all [--dry-run]
      修复动作 id:env / modeb-env / autostart / engine-register / restart-ibus / clean-cache / reinstall-dict
      也可给检查项 id(env/gui-env/daemon/session-bus/engine-register/autostart/immodule/data),自动映射到对应修复
      restart-ibus 会按会话真实环境(取自 GUI 进程)接到正确总线后拉起 ibus-daemon
      默认实际执行;--dry-run 只打印将做什么

  lyyime-doctor probe [--display :N] [--bus <addr>] [--text nihao] [--expect 你好]
                      [--timeout 秒] [--script 路径]
      真屏输入链路端到端验证:自建 lyyime-probe 窗口 + 显式聚焦(不碰用户焦点)
      + xdotool 注入字母,断言缓冲出现上屏结果;通过=环境→总线→daemon→引擎→上屏全通

  lyyime-doctor ime-list [--json]
      枚举本机输入法(ibus 引擎 / fcitx5 / lyyime / 框架)

  lyyime-doctor ime-add <id> [--yes]
      安装输入法(内置目录:pinyin / wubi-haifeng / wubi-jidian / rime / fcitx5 / ibus-table)
      默认 dry-run 预览将执行的命令;--yes 才实际执行(需 root)

  lyyime-doctor ime-remove <id> [--force] [--yes]
      卸载/注销输入法;保护:拒绝移除默认 lyyime、会话唯一中文输入法(--force 强制)
      默认 dry-run;--yes 才实际执行(卸包需 root)

  lyyime-doctor ime-default <id> [--yes]
      把引擎设为默认(写入 ibus preload-engines / engines-order 首位;gsettings 不可用时降级为手动指引)

  lyyime-doctor --help | --version
"#;

fn main() {
    // `lyyime-doctor ... | head` 场景:stdout 管道关闭时以默认行为安静退出,
    // 避免 Rust 对 EPIPE panic(常规 Unix CLI 语义)。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    // 输出被 head/grep -q 提前关闭管道时静默退出(对齐常见 CLI 行为),其余 panic 正常报错
    let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&args)))
        .unwrap_or_else(|p| {
            let msg = p
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            if msg.contains("Broken pipe") {
                141 // 128 + SIGPIPE
            } else {
                eprintln!("lyyime-doctor 内部错误:{msg}");
                101
            }
        });
    std::process::exit(code);
}

fn run(args: &[String]) -> i32 {
    let flags = |name: &str| args.iter().any(|a| a == name);
    let json = flags("--json");
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            print!("{USAGE}");
            0
        }
        Some("--version") | Some("-V") | Some("version") => {
            println!("lyyime-doctor {VERSION}");
            0
        }
        Some("check") => cmd_check(json),
        Some("fix") => cmd_fix(&args[1..]),
        Some("probe") => cmd_probe(&args[1..]),
        Some("ime-list") => cmd_ime_list(json),
        Some("ime-add") => cmd_ime_op(&args[1..], ImeOp::Add),
        Some("ime-remove") => cmd_ime_op(&args[1..], ImeOp::Remove),
        Some("ime-default") => cmd_ime_op(&args[1..], ImeOp::Default),
        Some(other) => {
            eprintln!("未知子命令:{other}\n");
            print!("{USAGE}");
            2
        }
    }
}

// ---------------------------------------------------------------------------
// check
// ---------------------------------------------------------------------------

fn cmd_check(json: bool) -> i32 {
    let doctor = Doctor::new(Paths::detect());
    let results = doctor.run_checks();
    if json {
        let report = CheckReport::new(results);
        println!("{}", serde_json::to_string_pretty(&report).expect("序列化失败"));
        return 0;
    }
    println!("lyyime-doctor v{VERSION} — 输入法环境诊断");
    println!("{}", "─".repeat(72));
    let id_w = results.iter().map(|r| r.id.len()).max().unwrap_or(2).max(2);
    for r in &results {
        let tag = match r.status {
            Status::Ok => "[ OK ]",
            Status::Warn => "[WARN]",
            Status::Fail => "[FAIL]",
            Status::Fixed => "[FIXD]",
        };
        println!("{tag} {:<id_w$}  {}", r.id, r.title);
        println!("      {}", r.detail.replace('\n', "\n      "));
        if let Some(h) = &r.fix_hint {
            println!("      ↳ 修复: {h}");
        }
    }
    let report = CheckReport::new(results);
    println!(
        "{}",
        "─".repeat(72)
    );
    println!(
        "汇总: {} ok / {} warn / {} fail(共 {} 项)",
        report.summary.ok, report.summary.warn, report.summary.fail,
        report.summary.ok + report.summary.warn + report.summary.fail
    );
    0
}

// ---------------------------------------------------------------------------
// fix
// ---------------------------------------------------------------------------

fn cmd_fix(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("用法: lyyime-doctor fix (--issue <id>)... | --all [--dry-run]\n       可用修复 id:env modeb-env autostart engine-register restart-ibus clean-cache reinstall-dict");
        return 2;
    }
    let dry = args.iter().any(|a| a == "--dry-run");
    let all = args.iter().any(|a| a == "--all");
    let mut issues: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--issue" {
            match args.get(i + 1) {
                Some(v) => {
                    issues.push(v.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--issue 需要一个 id 参数");
                    return 2;
                }
            }
        } else if let Some(v) = args[i].strip_prefix("--issue=") {
            issues.push(v.to_string());
            i += 1;
        } else {
            i += 1;
        }
    }
    if !all && issues.is_empty() {
        eprintln!("需要 --issue <id> 或 --all");
        return 2;
    }

    let doctor = Doctor::new(Paths::detect());
    let actions = if all {
        doctor.plan_fixes_all()
    } else {
        // 对未知 id 给出提示
        for id in &issues {
            if !fixes_known(id) {
                eprintln!("提示:「{id}」不是修复 id 也不是可映射的检查项 id,已跳过");
            }
        }
        doctor.plan_fixes(&issues)
    };
    if actions.is_empty() {
        println!("没有可执行的修复动作(所有检查项正常,或给定问题无自动修复)。");
        println!("可先运行:lyyime-doctor check");
        return 0;
    }
    println!(
        "共 {} 个修复动作{}:",
        actions.len(),
        if dry { "(DRY-RUN 预览)" } else { "" }
    );
    let mut failed = 0;
    for a in &actions {
        println!("\n>> [{}] {}", a.id, a.description);
        match doctor.apply(a, dry) {
            Ok(out) => {
                if out.is_failure() {
                    failed += 1;
                }
                println!("   [{}] {}", out.tag(), out.message());
            }
            Err(e) => {
                failed += 1;
                println!("   [FAILED] {e:#}");
            }
        }
    }
    println!();
    if failed > 0 {
        println!("完成:{failed} 个动作失败,请检查上方输出。");
        1
    } else {
        println!("完成:全部动作{}成功。", if dry { "预览" } else { "" });
        0
    }
}

fn fixes_known(id: &str) -> bool {
    matches!(
        id,
        "env" | "modeb-env" | "daemon" | "gui-env" | "session-bus"
            | "engine-register"
            | "autostart"
            | "immodule"
            | "data"
            | "logs"
            | "locale"
            | "restart-ibus"
            | "clean-cache"
            | "reinstall-dict"
    )
}

// ---------------------------------------------------------------------------
// probe
// ---------------------------------------------------------------------------

fn cmd_probe(args: &[String]) -> i32 {
    let val = |flag: &str| -> Option<String> {
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == flag {
                return it.next().cloned();
            }
            if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
                return Some(v.to_string());
            }
        }
        None
    };
    let opts = ProbeOptions {
        display: val("--display"),
        bus: val("--bus"),
        text: val("--text").unwrap_or_else(|| "nihao".into()),
        expect: val("--expect").unwrap_or_else(|| "你好".into()),
        timeout_secs: val("--timeout").and_then(|v| v.parse().ok()).unwrap_or(20),
        script: val("--script").map(std::path::PathBuf::from),
        ..Default::default()
    };
    let doctor = Doctor::new(Paths::detect());
    let report = doctor.run_probe(&opts);
    let tag = |ok: bool| if ok { "[ OK ]" } else { "[FAIL]" };
    for s in &report.steps {
        println!("{} {:<8} {}", tag(s.ok), s.name, s.detail.replace('\n', "\n             "));
    }
    println!("{}", "─".repeat(72));
    if report.passed {
        println!("probe:PASS —— 真屏输入链路全通(环境→总线→daemon→引擎→上屏)");
        0
    } else {
        println!("probe:FAIL —— 见上方步骤;可运行 `lyyime-doctor check` 定位具体环节,`lyyime-doctor fix --all` 一键修复");
        1
    }
}

// ---------------------------------------------------------------------------
// ime-*
// ---------------------------------------------------------------------------

fn cmd_ime_list(json: bool) -> i32 {
    let m = ImeManager::new(Paths::detect());
    match m.ime_list() {
        Ok(list) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&list).expect("序列化失败"));
                return 0;
            }
            println!(
                "{:<24} {:<12} {:<4} {:<4} {:<4} {}",
                "ID", "类型", "已装", "活动", "默认", "名称 / 包"
            );
            for e in &list {
                let yn = |b: bool| if b { "是" } else { "否" };
                let pkg = if e.package.is_empty() { "-" } else { e.package.as_str() };
                println!(
                    "{:<24} {:<12} {:<4} {:<4} {:<4} {} [{}]",
                    e.id,
                    e.kind.as_str(),
                    yn(e.installed),
                    yn(e.active),
                    yn(e.is_default),
                    e.name,
                    pkg
                );
            }
            println!("\n内置目录 id(ime-add/ime-remove 可用):{}", lyyime_doctor::catalog::ids().join(" / "));
            0
        }
        Err(e) => {
            eprintln!("ime-list 失败:{e:#}");
            1
        }
    }
}

enum ImeOp {
    Add,
    Remove,
    Default,
}

impl ImeOp {
    fn name(&self) -> &'static str {
        match self {
            ImeOp::Add => "ime-add",
            ImeOp::Remove => "ime-remove",
            ImeOp::Default => "ime-default",
        }
    }
}

fn cmd_ime_op(args: &[String], op: ImeOp) -> i32 {
    let yes = args.iter().any(|a| a == "--yes");
    let dry_flag = args.iter().any(|a| a == "--dry-run");
    let force = args.iter().any(|a| a == "--force");
    let json = args.iter().any(|a| a == "--json");
    let target = args.iter().find(|a| !a.starts_with("--"));
    let Some(target) = target else {
        eprintln!("用法: lyyime-doctor {} <id> [--yes] [--dry-run]{}", op.name(), if matches!(op, ImeOp::Remove) { " [--force]" } else { "" });
        return 2;
    };
    // 合同:默认 dry-run;--yes 才实际执行(--dry-run 优先)
    let execute = yes && !dry_flag;

    let m = ImeManager::new(Paths::detect());
    let result = match op {
        ImeOp::Add => m.ime_add(target, execute),
        ImeOp::Remove => m.ime_remove(target, force, execute),
        ImeOp::Default => m.ime_default(target, execute),
    };
    match result {
        Ok(r) => {
            print_ime_result(&r, json);
            0
        }
        Err(e) => {
            eprintln!("{} 失败:{e:#}", op.name());
            1
        }
    }
}

fn print_ime_result(r: &ImeOpResult, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(r).expect("序列化失败"));
        return;
    }
    if r.executed {
        println!("== ime-{} {} 已执行 ==", r.action, r.target);
    } else {
        println!(
            "== ime-{} {} (DRY-RUN 预览,未执行;加 --yes 实际执行) ==",
            r.action, r.target
        );
    }
    if !r.commands.is_empty() {
        println!("命令:");
        for (i, c) in r.commands.iter().enumerate() {
            println!("  {}. {c}", i + 1);
        }
    }
    for w in &r.warnings {
        println!("警告: {w}");
    }
    if let Some(h) = &r.manual_hint {
        println!("手动指引: {h}");
    }
    if r.executed && !r.before.is_empty() {
        let after = r.after.clone().unwrap_or_default();
        let added: Vec<&String> = after.iter().filter(|x| !r.before.contains(x)).collect();
        let removed: Vec<&String> = r.before.iter().filter(|x| !after.contains(x)).collect();
        println!(
            "差量(ime-list 前后): 新增 {:?} / 移除 {:?}",
            added, removed
        );
    }
    if let Some(l) = &r.log_path {
        println!("日志: {l}");
    }
}
