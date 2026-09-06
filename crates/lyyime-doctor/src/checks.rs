//! `check`:10 项环境诊断(ARCHITECTURE.md §9)。

use crate::model::{CheckResult, Status};
use crate::paths::Paths;
use crate::session::{detect_session_env, SessionEnv};
use crate::system::{ProcInfo, SystemOps};

#[allow(dead_code)] // 供测试断言顺序
pub(crate) const CHECK_IDS: [&str; 10] = [
    "env",
    "gui-env",
    "session-bus",
    "daemon",
    "engine-register",
    "autostart",
    "immodule",
    "data",
    "logs",
    "locale",
];

pub(crate) fn run_checks(p: &Paths, sys: &dyn SystemOps) -> Vec<CheckResult> {
    vec![
        check_env(sys),
        check_gui_env(sys),
        check_session_bus(p, sys),
        check_daemon(p, sys),
        check_engine_register(p, sys),
        check_autostart(p),
        check_immodule(p),
        check_data(p),
        check_logs(p),
        check_locale(sys),
    ]
}

// ---------------------------------------------------------------------------
// 框架判定辅助
// ---------------------------------------------------------------------------

/// 探测本机会话"应使用"的输入法框架:fcitx5 进程/环境优先,否则 ibus。
pub(crate) fn detected_framework(sys: &dyn SystemOps) -> &'static str {
    if !sys.find_processes("fcitx5").is_empty() {
        return "fcitx5";
    }
    for k in ["XMODIFIERS", "GTK_IM_MODULE", "QT_IM_MODULE"] {
        if let Some(v) = sys.env(k) {
            if v.to_lowercase().contains("fcitx") {
                return "fcitx5";
            }
        }
    }
    "ibus"
}

/// 归一化框架名:"@im=ibus"→"ibus","fcitx"→"fcitx5"
pub(crate) fn normalize_framework(v: &str) -> String {
    let t = v.trim().trim_start_matches("@im=").trim().to_lowercase();
    match t.as_str() {
        "fcitx" | "fcitx5" => "fcitx5".into(),
        "ibus" => "ibus".into(),
        other => other.to_string(),
    }
}

fn hint_fix_env(sys: &dyn SystemOps) -> String {
    format!(
        "lyyime-doctor fix --issue env  (按框架 {} 写入 ~/.xprofile 与 ~/.config/lyyime/env.sh,需重新登录或 source 生效)",
        detected_framework(sys)
    )
}

// ---------------------------------------------------------------------------
// 1. env:三件套
// ---------------------------------------------------------------------------

fn check_env(sys: &dyn SystemOps) -> CheckResult {
    let id = "env";
    let title = "环境变量三件套(GTK/Qt/X11)";
    let raw = [
        ("XMODIFIERS", sys.env("XMODIFIERS")),
        ("GTK_IM_MODULE", sys.env("GTK_IM_MODULE")),
        ("QT_IM_MODULE", sys.env("QT_IM_MODULE")),
    ];
    let missing: Vec<&str> = raw.iter().filter(|(_, v)| v.is_none()).map(|(k, _)| *k).collect();
    if missing.len() == 3 {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: "XMODIFIERS / GTK_IM_MODULE / QT_IM_MODULE 均未设置,应用将不知道该连哪个输入法框架".into(),
            fix_hint: Some(hint_fix_env(sys)),
        };
    }
    if !missing.is_empty() {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!(
                "部分未设置: {};已设置: {}",
                missing.join(", "),
                raw.iter()
                    .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={v}")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            fix_hint: Some(hint_fix_env(sys)),
        };
    }
    // Mode B profile:GTK 走 xim 直连 lyyime-xim(合同 §8),属合法工作态;
    // Qt 侧保持 ibus,故 QT_IM_MODULE=ibus 不算不一致。
    let xmod = raw.iter().find(|(k, _)| *k == "XMODIFIERS").and_then(|(_, v)| v.clone());
    let gtk = raw.iter().find(|(k, _)| *k == "GTK_IM_MODULE").and_then(|(_, v)| v.clone());
    if let (Some(x), Some(g)) = (xmod, gtk) {
        if x.contains("lyyime") && g.trim() == "xim" {
            return CheckResult {
                id: id.into(),
                title: title.into(),
                status: Status::Ok,
                detail: "Mode B 独立外挂 profile(XMODIFIERS=@im=lyyime,GTK_IM_MODULE=xim);Qt 应用仍走 ibus;切回 Mode A: lyyime-doctor fix --issue env".into(),
                fix_hint: None,
            };
        }
    }
    let frameworks: Vec<String> = raw
        .iter()
        .filter_map(|(_, v)| v.as_deref().map(normalize_framework))
        .collect();
    let mut uniq = frameworks.clone();
    uniq.sort();
    uniq.dedup();
    let detail = format!(
        "{}",
        raw.iter()
            .map(|(k, v)| format!("{k}={}", v.clone().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if uniq.len() > 1 {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!("指向不同框架({detail}),GTK/Qt/X11 应用会连到不同输入法框架"),
            fix_hint: Some(hint_fix_env(sys)),
        };
    }
    // 格式提醒:XMODIFIERS 惯例为 @im=<框架>
    let xfmt = raw[0].1.as_deref().unwrap_or("");
    let extra = if !xfmt.starts_with("@im=") {
        format!(";建议 XMODIFIERS 使用 @im={} 形式", uniq[0])
    } else {
        String::new()
    };
    CheckResult {
        id: id.into(),
        title: title.into(),
        status: Status::Ok,
        detail: format!("统一指向「{}」({detail}{extra})", uniq[0]),
        fix_hint: None,
    }
}

// ---------------------------------------------------------------------------
// 1b. gui-env:图形会话进程的真实环境(经验:以 GUI 应用 pid 为准,不看 shell)
// ---------------------------------------------------------------------------

/// 三件套归类。Ok((框架 token, 描述))= 该进程的应用会正确连到输入法;Err(原因)= 有断层。
/// token 用于跨进程一致性比对:ibus / fcitx5 / lyyime(Mode B)。
fn classify_three(
    gtk: Option<&str>,
    qt: Option<&str>,
    xmod: Option<&str>,
) -> Result<(String, String), String> {
    let missing: Vec<&str> = [
        ("GTK_IM_MODULE", gtk),
        ("QT_IM_MODULE", qt),
        ("XMODIFIERS", xmod),
    ]
    .iter()
    .filter(|(_, v)| v.is_none())
    .map(|(k, _)| *k)
    .collect();
    if !missing.is_empty() {
        return Err(format!("缺少 {}", missing.join("、")));
    }
    let (g, q, x) = (gtk.unwrap().trim(), qt.unwrap().trim(), xmod.unwrap().trim());
    // Mode B(独立外挂 lyyime-xim):GTK 走 xim 直连,XMODIFIERS=@im=lyyime,Qt 仍走 ibus
    if x.contains("lyyime") && g == "xim" {
        return Ok((
            "lyyime".into(),
            "Mode B 外挂 profile(GTK=xim, XMOD=@im=lyyime, Qt=ibus)".into(),
        ));
    }
    let mut toks: Vec<String> = [g, q, x].iter().map(|v| normalize_framework(v)).collect();
    toks.sort();
    toks.dedup();
    if toks.len() == 1 {
        Ok((toks[0].clone(), format!("统一指向「{}」", toks[0])))
    } else {
        Err(format!(
            "指向不同框架(GTK={g}, QT={q}, XMOD={x})",
        ))
    }
}

fn check_gui_env(sys: &dyn SystemOps) -> CheckResult {
    let id = "gui-env";
    let title = "图形会话进程真实环境(/proc/<pid>/environ 三件套)";
    let sess = detect_session_env(sys);
    if sess.sampled.is_empty() {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: "未发现桌面会话进程(xfce4-session/panel 等),无法采样真实应用环境(未登录图形界面时属正常)".into(),
            fix_hint: None,
        };
    }
    let mut ok_lines = vec![];
    let mut tokens: Vec<String> = vec![];
    let mut bad = vec![];
    for s in &sess.sampled {
        let label = format!("pid={}({})", s.pid, s.name);
        match classify_three(
            s.getenv("GTK_IM_MODULE"),
            s.getenv("QT_IM_MODULE"),
            s.getenv("XMODIFIERS"),
        ) {
            Ok((token, desc)) => {
                ok_lines.push(format!("{label} {desc}"));
                tokens.push(token);
            }
            Err(reason) => bad.push(format!("{label} {reason}")),
        }
    }
    if !bad.is_empty() {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!(
                "{}(正常项:{});环境断层:会话内新启动的应用会原样继承这份环境,GTK 3.24 实测不认 XSettings 的 im-module,三件套只能靠环境变量传入(docs/RESEARCH.md §4)",
                bad.join("; "),
                if ok_lines.is_empty() { String::new() } else { ok_lines.join("; ") }
            ),
            fix_hint: Some(
                "lyyime-doctor fix --issue env(写入持久配置后注销重登生效;免登急修:source ~/.config/lyyime/env.sh 后重启目标应用)".into(),
            ),
        };
    }
    // 跨进程一致性:各会话进程自身没问题、但彼此指向不同框架 → 两批应用会连到不同 IM
    let mut uniq = tokens.clone();
    uniq.sort();
    uniq.dedup();
    if uniq.len() > 1 {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!(
                "会话内不同进程指向不同框架({});不同批启动的应用会连到不同输入法框架",
                uniq.join(" vs ")
            ),
            fix_hint: Some(
                "lyyime-doctor fix --issue env(统一写入后注销重登生效)".into(),
            ),
        };
    }
    CheckResult {
        id: id.into(),
        title: title.into(),
        status: Status::Ok,
        detail: ok_lines.join("; "),
        fix_hint: None,
    }
}

// ---------------------------------------------------------------------------
// 1c. session-bus:IBus 是否注册在桌面会话真实使用的总线上
// ---------------------------------------------------------------------------

/// (总线上的注册名, 框架进程名)
fn framework_bus_names(fw: &str) -> (&'static str, &'static str) {
    if fw == "fcitx5" {
        ("org.fcitx.Fcitx5", "fcitx5")
    } else {
        ("org.freedesktop.IBus", "ibus-daemon")
    }
}

/// 读取本 uid 框架进程各自连接的总线地址(来自其 /proc/<pid>/environ)
fn daemon_bus_addresses(sys: &dyn SystemOps, daemons: &[ProcInfo]) -> Vec<Option<String>> {
    daemons
        .iter()
        .map(|d| {
            sys.read_proc_environ(d.pid)
                .iter()
                .find(|(k, _)| k == "DBUS_SESSION_BUS_ADDRESS")
                .map(|(_, v)| v.clone())
        })
        .collect()
}

fn check_session_bus(_p: &Paths, sys: &dyn SystemOps) -> CheckResult {
    let id = "session-bus";
    let title = "会话总线归属(IBus 注册在桌面会话真实使用的总线上)";
    let fw = detected_framework(sys);
    let (bus_name, daemon_name) = framework_bus_names(fw);
    let sess: SessionEnv = detect_session_env(sys);
    let Some(bus) = sess.dbus_address.clone() else {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: "未探测到图形会话总线(无桌面会话进程),无法核对注册位置".into(),
            fix_hint: None,
        };
    };
    let uid = sys.current_uid();
    let daemons: Vec<ProcInfo> = sys
        .find_processes(daemon_name)
        .into_iter()
        .filter(|x| x.uid == uid)
        .collect();
    let daemon_buses = daemon_bus_addresses(sys, &daemons);
    let on_session = daemon_buses.iter().any(|b| b.as_deref() == Some(bus.as_str()));
    let multi_note = if sess.all_dbus_addresses.len() > 1 {
        format!(
            "(注意:采样到 {} 条会话总线 [{}],存在多屏/多会话分裂)",
            sess.all_dbus_addresses.len(),
            sess.all_dbus_addresses.join(" | ")
        )
    } else {
        String::new()
    };

    // 地面真值:busctl 在会话总线上列名字;不可用(None)则退回按 daemon 进程环境比对
    let busctl_seen: Option<bool> = match sys.run("busctl", &[&format!("--address={bus}"), "list"]) {
        Ok(o) if o.success => Some(o.stdout.contains(bus_name)),
        _ => None,
    };

    match (busctl_seen, daemons.is_empty()) {
        (Some(true), _) => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: format!("{fw}({bus_name})已注册在会话总线 {bus}{multi_note}"),
            fix_hint: None,
        },
        (Some(false), true) => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!(
                "会话总线({bus})上没有 {bus_name} 注册,也没有运行中的 {daemon_name}——图形会话将没有输入法可用"
            ),
            fix_hint: Some("lyyime-doctor fix --issue restart-ibus".into()),
        },
        (Some(false), false) if !on_session => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!(
                "「总线接错」:{daemon_name} 在运行,但连的是别的总线({});会话总线({bus})上没有 {bus_name} 注册,桌面应用全部连不上输入法(本机最常见故障,见 docs/RESEARCH.md §4)",
                daemon_buses
                    .iter()
                    .map(|b| b.as_deref().unwrap_or("(未知)"))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            fix_hint: Some(format!("lyyime-doctor fix --issue restart-ibus(会自动按会话环境接到 {bus})")),
        },
        (Some(false), false) => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: format!(
                "{daemon_name} 自称连着会话总线,但 busctl 未在总线 {bus} 上见到 {bus_name}(可能刚启动或会话策略限制);建议重启后复检"
            ),
            fix_hint: Some("lyyime-doctor fix --issue restart-ibus".into()),
        },
        (None, true) => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!("未发现运行中的 {daemon_name}(busctl 不可用,无法进一步核对注册)"),
            fix_hint: Some("lyyime-doctor fix --issue restart-ibus".into()),
        },
        (None, false) if on_session => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: format!(
                "{daemon_name} 连接的地址与会话总线一致({bus})(busctl 不可用,按 daemon 进程环境比对){multi_note}"
            ),
            fix_hint: None,
        },
        (None, false) => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!(
                "「总线接错」(busctl 不可用,按 daemon 进程环境比对):{daemon_name} 连的是({});会话总线是({bus})",
                daemon_buses
                    .iter()
                    .map(|b| b.as_deref().unwrap_or("(未知)"))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            fix_hint: Some("lyyime-doctor fix --issue restart-ibus(会自动接到会话总线)".into()),
        },
    }
}

// ---------------------------------------------------------------------------
// 2. daemon:框架进程
// ---------------------------------------------------------------------------

fn check_daemon(p: &Paths, sys: &dyn SystemOps) -> CheckResult {
    let _ = p;
    let id = "daemon";
    let title = "输入法框架进程";
    let fw = detected_framework(sys);
    let hint_name = if fw == "fcitx5" { "fcitx5" } else { "ibus-daemon" };
    let procs = sys.find_processes(hint_name);
    let uid = sys.current_uid();
    let mine: Vec<_> = procs.iter().filter(|x| x.uid == uid).collect();
    if !mine.is_empty() {
        let desc = mine
            .iter()
            .map(|x| format!("pid={} user={}", x.pid, x.user))
            .collect::<Vec<_>>()
            .join(", ");
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: format!("{hint_name} 运行中({desc})"),
            fix_hint: None,
        };
    }
    if !procs.is_empty() {
        let desc = procs
            .iter()
            .map(|x| format!("pid={} user={}", x.pid, x.user))
            .collect::<Vec<_>>()
            .join(", ");
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: format!(
                "{hint_name} 仅运行于其他用户({desc});当前 uid={uid} 的会话没有自己的框架进程"
            ),
            fix_hint: Some("lyyime-doctor fix --issue restart-ibus".into()),
        };
    }
    CheckResult {
        id: id.into(),
        title: title.into(),
        status: Status::Fail,
        detail: format!("未发现运行中的 {hint_name};图形会话将没有输入法可用"),
        fix_hint: Some("lyyime-doctor fix --issue restart-ibus".into()),
    }
}

// ---------------------------------------------------------------------------
// 3. engine-register:组件 XML / 引擎脚本 / ibus 缓存
// ---------------------------------------------------------------------------

fn check_engine_register(p: &Paths, sys: &dyn SystemOps) -> CheckResult {
    let _ = sys;
    let id = "engine-register";
    let title = "lyyime 引擎注册(组件 XML/引擎脚本/ibus 缓存)";
    let xml_sys = p.system_component_xml();
    let xml_usr = p.user_component_dir().join("lyyime.xml");
    let py_sys = p.system_engine_dir().join("lyyime.py");
    let py_usr = p.user_engine_dir().join("lyyime.py");

    let xml = [xml_sys.as_path(), xml_usr.as_path()]
        .into_iter()
        .find(|f| f.is_file());
    match xml {
        None => {
            return CheckResult {
                id: id.into(),
                title: title.into(),
                status: Status::Fail,
                detail: format!(
                    "组件 XML 未安装(检查了 {} 与 {});ibus 无法发现 lyyime 引擎",
                    xml_sys.display(),
                    xml_usr.display()
                ),
                fix_hint: Some("lyyime-doctor fix --issue engine-register".into()),
            };
        }
        Some(x) => {
            if !(py_sys.is_file() || py_usr.is_file()) {
                return CheckResult {
                    id: id.into(),
                    title: title.into(),
                    status: Status::Fail,
                    detail: format!(
                        "组件 XML 在 {},但引擎脚本 lyyime.py 缺失(检查了 {} 与 {})",
                        x.display(),
                        py_sys.display(),
                        py_usr.display()
                    ),
                    fix_hint: Some("lyyime-doctor fix --issue engine-register".into()),
                };
            }
        }
    }
    // 尽力而为:在 ibus 缓存目录的二进制文件里找 "lyyime" 字样;失败降级 warn
    match scan_ibus_cache(p) {
        CacheScan::Found => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: "组件 XML、引擎脚本就位,且 ibus 缓存已包含 lyyime".into(),
            fix_hint: None,
        },
        CacheScan::NotFound => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: "组件已安装,但 ibus 缓存中未见 lyyime(需要刷新缓存或重启 ibus)".into(),
            fix_hint: Some("lyyime-doctor fix --issue engine-register(会执行 ibus write-cache)".into()),
        },
        CacheScan::Unavailable(msg) => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: format!("组件已安装,但无法确认 ibus 缓存({msg})"),
            fix_hint: Some("lyyime-doctor fix --issue engine-register".into()),
        },
    }
}

enum CacheScan {
    Found,
    NotFound,
    Unavailable(String),
}

fn scan_ibus_cache(p: &Paths) -> CacheScan {
    let dir = p.ibus_cache_dir();
    if !dir.is_dir() {
        return CacheScan::Unavailable(format!("缓存目录 {} 不存在", dir.display()));
    }
    let mut scanned = 0usize;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let fp = e.path();
            if !fp.is_file() {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&fp) {
                scanned += 1;
                if bytes.windows(6).any(|w| w == b"lyyime") {
                    return CacheScan::Found;
                }
            }
        }
    }
    if scanned == 0 {
        CacheScan::Unavailable("缓存目录为空或不可读".into())
    } else {
        CacheScan::NotFound
    }
}

// ---------------------------------------------------------------------------
// 4. autostart
// ---------------------------------------------------------------------------

fn check_autostart(p: &Paths) -> CheckResult {
    let id = "autostart";
    let title = "开机自启配置(~/.config/autostart)";
    let dir = p.autostart_dir();
    if !dir.is_dir() {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!("{} 不存在,输入法不会随桌面自启", dir.display()),
            fix_hint: Some("lyyime-doctor fix --issue autostart".into()),
        };
    }
    let mut lyyime_on = vec![];
    let mut framework_on = vec![];
    let mut disabled = vec![];
    let mut entries = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let fp = e.path();
            if fp.extension().map(|x| x == "desktop").unwrap_or(false) {
                entries.push(fp);
            }
        }
    }
    entries.sort();
    for fp in &entries {
        let content = std::fs::read_to_string(fp).unwrap_or_default();
        let name = fp.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let lower = content.to_lowercase();
        let relevant = ["ibus", "fcitx", "lyyime"].iter().any(|k| lower.contains(k));
        if !relevant {
            continue;
        }
        let hidden = desktop_entry_flag(&content, "Hidden");
        let enabled = desktop_entry_flag(&content, "X-GNOME-Autostart-enabled");
        if hidden == Some(true) || enabled == Some(false) {
            disabled.push(name);
        } else if lower.contains("lyyime") {
            lyyime_on.push(name);
        } else {
            framework_on.push(name);
        }
    }
    if !lyyime_on.is_empty() {
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: format!("lyyime 自启已配置({})", lyyime_on.join(", ")),
            fix_hint: None,
        }
    } else if !framework_on.is_empty() {
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: format!(
                "输入法框架有自启({}),但 lyyime 未配置自启",
                framework_on.join(", ")
            ),
            fix_hint: Some("lyyime-doctor fix --issue autostart".into()),
        }
    } else {
        let extra = if disabled.is_empty() {
            String::new()
        } else {
            format!("(另有已停用项:{})", disabled.join(", "))
        };
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!("未发现启用中的输入法自启项{extra}"),
            fix_hint: Some("lyyime-doctor fix --issue autostart".into()),
        }
    }
}

/// 读 .desktop 里 bool 型键(Hidden / X-GNOME-Autostart-enabled)
fn desktop_entry_flag(content: &str, key: &str) -> Option<bool> {
    for line in content.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(key) {
            if let Some(v) = rest.strip_prefix('=') {
                let v = v.trim().to_lowercase();
                return Some(matches!(v.as_str(), "true" | "1" | "yes"));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 5. immodule:GTK/Qt 输入法模块
// ---------------------------------------------------------------------------

fn check_immodule(p: &Paths) -> CheckResult {
    let id = "immodule";
    let title = "GTK/Qt 输入法模块";
    let mut problems: Vec<String> = vec![];
    let mut notes: Vec<String> = vec![];

    // GTK3(关键)
    if p.gtk3_im_dir.is_dir() {
        let has_ibus = p.gtk3_im_dir.join("im-ibus.so").is_file();
        let has_fcitx = p.gtk3_im_dir.join("im-fcitx.so").is_file();
        if !(has_ibus || has_fcitx) {
            problems.push(format!("{} 中未发现 im-ibus.so / im-fcitx.so", p.gtk3_im_dir.display()));
        }
    } else {
        problems.push(format!("GTK3 immodule 目录不存在: {}", p.gtk3_im_dir.display()));
    }
    if p.gtk3_im_cache.is_file() {
        let c = std::fs::read_to_string(&p.gtk3_im_cache).unwrap_or_default();
        if !(c.contains("ibus") || c.contains("fcitx")) {
            problems.push(format!(
                "immodules 缓存({})未包含 ibus/fcitx 模块,需重建",
                p.gtk3_im_cache.display()
            ));
        }
    } else {
        problems.push(format!("GTK3 immodules 缓存不存在: {}", p.gtk3_im_cache.display()));
    }
    let tool = ["gtk-query-immodules-3.0", "gtk-query-immodules-3.0-64"]
        .iter()
        .find(|t| which(t));
    match tool {
        Some(t) => notes.push(format!("重建工具可用:{t}")),
        None => notes.push("未找到 gtk-query-immodules-3.0(重建工具),如需重建请安装 gtk3-devel".into()),
    }

    // GTK2(尽力而为,缺失不算问题)
    if p.gtk2_im_dir.is_dir() {
        if p.gtk2_im_dir.join("im-ibus.so").is_file() {
            notes.push("GTK2 im-ibus.so 存在".into());
        } else {
            notes.push("GTK2 immodule 目录存在但无 im-ibus.so(仅影响 GTK2 老应用)".into());
        }
    } else {
        notes.push("GTK2 immodule 目录不存在(跳过,不影响 GTK3 应用)".into());
    }

    // Qt(尽力而为)
    for (label, dir) in [("Qt5", &p.qt5_im_dir), ("Qt6", &p.qt6_im_dir)] {
        match dir {
            None => notes.push(format!("{label} platforminputcontexts 目录不存在(未安装 {label},跳过)")),
            Some(d) => {
                let ok = d.join("libibusplatforminputcontextplugin.so").is_file()
                    || d.join("libfcitx5platforminputcontextplugin.so").is_file();
                if ok {
                    notes.push(format!("{label} 输入法插件存在"));
                } else {
                    problems.push(format!("{label} 插件目录 {} 中无 ibus/fcitx 输入法插件", d.display()));
                }
            }
        }
    }

    if problems.is_empty() {
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: format!("GTK3 immodule 与缓存正常;{}", notes.join(";")),
            fix_hint: None,
        }
    } else {
        let hard = problems.iter().any(|x| x.contains("GTK3"));
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: if hard { Status::Fail } else { Status::Warn },
            detail: format!("{}(备注:{})", problems.join(";"), notes.join(";")),
            fix_hint: Some("lyyime-doctor fix --issue clean-cache(重建 immodules 缓存)".into()),
        }
    }
}

fn which(prog: &str) -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|d| d.join(prog))
        .any(|f| f.is_file())
}

// ---------------------------------------------------------------------------
// 6. data:lyyime 数据完整性
// ---------------------------------------------------------------------------

fn check_data(p: &Paths) -> CheckResult {
    let id = "data";
    let title = "lyyime 数据完整性(user.tsv / meta.json)";
    let mut notes = vec![];
    let mut fail: Option<String> = None;

    // user.tsv:存在才校验,缺失不算错(首次使用时生成)
    let tsv = p.user_tsv();
    if tsv.is_file() {
        let content = std::fs::read_to_string(&tsv).unwrap_or_default();
        let bad = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| l.split('\t').count() < 3)
            .count();
        if bad > 0 {
            fail = Some(format!("user.tsv 有 {bad} 行格式非法(应为 word\\tfreq\\tlast_used)"));
        } else {
            let n = content.lines().filter(|l| !l.trim().is_empty()).count();
            notes.push(format!("user.tsv 正常({n} 条用户词)"));
        }
    } else {
        notes.push("user.tsv 尚未生成(首次输入后自动创建)".into());
    }

    // meta.json:项目 data/runtime 优先,用户级 runtime 兜底
    let meta_candidates = [
        p.runtime_dir().map(|d| d.join("meta.json")),
        Some(p.user_data_dir().join("runtime/meta.json")),
    ];
    let meta = meta_candidates.into_iter().flatten().find(|f| f.is_file());
    match meta {
        None => {
            let fail_part = match fail {
                Some(f) => format!(";{f}"),
                None => String::new(),
            };
            return CheckResult {
                id: id.into(),
                title: title.into(),
                status: Status::Warn,
                detail: format!("词典 runtime 数据缺失(meta.json 不存在);{}{}", notes.join(";"), fail_part),
                fix_hint: Some("lyyime-doctor fix --issue reinstall-dict".into()),
            };
        }
        Some(fp) => {
            let parsed: Result<serde_json::Value, _> =
                serde_json::from_str(&std::fs::read_to_string(&fp).unwrap_or_default());
            match parsed {
                Ok(v) => {
                    let version = v.get("version").cloned().unwrap_or(serde_json::Value::Null);
                    match version {
                        serde_json::Value::Number(n) => {
                            notes.push(format!("meta.json 版本 {n}({})", fp.display()));
                        }
                        _ => {
                            return CheckResult {
                                id: id.into(),
                                title: title.into(),
                                status: Status::Warn,
                                detail: format!("meta.json 缺少 version 字段({})", fp.display()),
                                fix_hint: Some("lyyime-doctor fix --issue reinstall-dict".into()),
                            };
                        }
                    }
                }
                Err(e) => {
                    return CheckResult {
                        id: id.into(),
                        title: title.into(),
                        status: Status::Fail,
                        detail: format!("meta.json 解析失败({e})"),
                        fix_hint: Some("lyyime-doctor fix --issue reinstall-dict".into()),
                    };
                }
            }
        }
    }

    match fail {
        Some(f) => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: f,
            fix_hint: None,
        },
        None => CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: notes.join(";"),
            fix_hint: None,
        },
    }
}

// ---------------------------------------------------------------------------
// 7. logs:日志尾部错误摘录
// ---------------------------------------------------------------------------

fn check_logs(p: &Paths) -> CheckResult {
    let id = "logs";
    let title = "日志错误扫描(~/.local/share/lyyime/logs)";
    let dir = p.logs_dir();
    if !dir.is_dir() {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: format!("暂无日志目录({}),尚无可检查的日志", dir.display()),
            fix_hint: None,
        };
    }
    let mut files = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let fp = e.path();
            if fp.extension().map(|x| x == "log").unwrap_or(false) && fp.is_file() {
                files.push(fp);
            }
        }
    }
    files.sort();
    let mut excerpts: Vec<String> = vec![];
    let mut total_errors = 0usize;
    for fp in &files {
        let content = std::fs::read_to_string(fp).unwrap_or_default();
        let tail: Vec<&str> = content.lines().rev().take(20).collect();
        for line in tail.into_iter().rev() {
            let hit = ["ERROR", "CRIT", "FATAL", "Traceback"]
                .iter()
                .any(|k| line.contains(k));
            if hit {
                total_errors += 1;
                if excerpts.len() < 3 {
                    let name = fp.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let trimmed: String = line.trim().chars().take(160).collect();
                    excerpts.push(format!("{name}: {trimmed}"));
                }
            }
        }
    }
    if total_errors > 0 {
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Warn,
            detail: format!(
                "最近日志有 {total_errors} 条错误;摘录:{}{}",
                excerpts.join(" | "),
                if total_errors > excerpts.len() { " …" } else { "" }
            ),
            fix_hint: None,
        }
    } else {
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Ok,
            detail: format!("日志无错误(共 {} 个文件)", files.len()),
            fix_hint: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 8. locale
// ---------------------------------------------------------------------------

fn check_locale(sys: &dyn SystemOps) -> CheckResult {
    let id = "locale";
    let title = "locale 编码(需 UTF-8)";
    let lang = sys.env("LANG");
    let lc_ctype = sys.env("LC_CTYPE");
    let lc_all = sys.env("LC_ALL");
    if lang.is_none() && lc_ctype.is_none() && lc_all.is_none() {
        return CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: "LANG / LC_CTYPE / LC_ALL 均未设置".into(),
            fix_hint: Some("在 ~/.xprofile 中设置 LANG=zh_CN.UTF-8(或 en_US.UTF-8)".into()),
        };
    }
    let effective = lc_all.clone().or(lang.clone()).or(lc_ctype.clone()).unwrap_or_default();
    let is_utf8 = effective.to_uppercase().contains("UTF-8") || effective.to_uppercase().contains("UTF8");
    let detail = format!(
        "LANG={} LC_CTYPE={} LC_ALL={} → 生效编码 {}",
        lang.unwrap_or_else(|| "(未设置)".into()),
        lc_ctype.unwrap_or_else(|| "(未设置)".into()),
        lc_all.unwrap_or_else(|| "(未设置)".into()),
        effective
    );
    if is_utf8 {
        CheckResult { id: id.into(), title: title.into(), status: Status::Ok, detail, fix_hint: None }
    } else {
        CheckResult {
            id: id.into(),
            title: title.into(),
            status: Status::Fail,
            detail: format!("{detail};非 UTF-8 locale 会导致候选词/上屏乱码"),
            fix_hint: Some("设置 LANG=zh_CN.UTF-8 后重新登录".into()),
        }
    }
}

// ---------------------------------------------------------------------------
// 测试(全部 tempdir + stub,不触真实 HOME)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fake::FakeSystem;
    use crate::testutil::{temp_write, TempDir};

    fn paths(t: &TempDir) -> Paths {
        Paths::for_home(&t.path)
    }

    #[test]
    fn env_ok_when_consistent() {
        let sys = FakeSystem::new(0)
            .with_env("XMODIFIERS", "@im=ibus")
            .with_env("GTK_IM_MODULE", "ibus")
            .with_env("QT_IM_MODULE", "ibus");
        let r = check_env(&sys);
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
    }

    #[test]
    fn env_ok_for_modeb_profile() {
        let sys = FakeSystem::new(0)
            .with_env("XMODIFIERS", "@im=lyyime")
            .with_env("GTK_IM_MODULE", "xim")
            .with_env("QT_IM_MODULE", "ibus");
        let r = check_env(&sys);
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
        assert!(r.detail.contains("Mode B"));
    }

    #[test]
    fn env_fail_when_all_missing() {
        let sys = FakeSystem::new(0);
        let r = check_env(&sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("均未设置"));
        assert!(r.fix_hint.as_deref().unwrap().contains("fix --issue env"));
    }

    #[test]
    fn env_fail_when_partial() {
        let sys = FakeSystem::new(0).with_env("GTK_IM_MODULE", "ibus");
        let r = check_env(&sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("XMODIFIERS"));
    }

    #[test]
    fn env_fail_when_frameworks_differ() {
        let sys = FakeSystem::new(0)
            .with_env("XMODIFIERS", "@im=fcitx")
            .with_env("GTK_IM_MODULE", "ibus")
            .with_env("QT_IM_MODULE", "ibus");
        let r = check_env(&sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("不同框架"));
    }

    #[test]
    fn daemon_ok_for_same_uid() {
        let sys = FakeSystem::new(0).with_proc(100, 0, "root", "ibus-daemon -drx");
        let r = check_daemon(&paths(&TempDir::new("d1")), &sys);
        assert_eq!(r.status, Status::Ok);
        assert!(r.detail.contains("pid=100"));
    }

    #[test]
    fn daemon_warn_for_other_uid_only() {
        let sys = FakeSystem::new(0).with_proc(200, 1001, "beyondcy", "ibus-daemon -drx");
        let r = check_daemon(&paths(&TempDir::new("d2")), &sys);
        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn daemon_fail_when_none() {
        let sys = FakeSystem::new(0);
        let r = check_daemon(&paths(&TempDir::new("d3")), &sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.fix_hint.as_deref().unwrap().contains("restart-ibus"));
    }

    #[test]
    fn engine_register_fail_when_xml_missing() {
        let t = TempDir::new("er1");
        let sys = FakeSystem::new(0);
        let r = check_engine_register(&paths(&t), &sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("组件 XML 未安装"));
    }

    #[test]
    fn engine_register_ok_with_cache_hit() {
        let t = TempDir::new("er2");
        temp_write(
            &t.path.join(".local/share/ibus/component/lyyime.xml"),
            "<component><engines><engine><name>lyyime</name></engine></engines></component>",
        );
        temp_write(&t.path.join(".local/share/lyyime/ibus/engine/lyyime.py"), "# engine");
        temp_write(&t.path.join(".cache/ibus/bus/registry"), "binary-with-lyyime-inside");
        let r = check_engine_register(&paths(&t), &FakeSystem::new(0));
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
    }

    #[test]
    fn engine_register_fail_when_py_missing() {
        let t = TempDir::new("er3");
        temp_write(&t.path.join(".local/share/ibus/component/lyyime.xml"), "<component/>");
        let r = check_engine_register(&paths(&t), &FakeSystem::new(0));
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("lyyime.py 缺失"));
    }

    #[test]
    fn engine_register_warn_when_cache_stale() {
        let t = TempDir::new("er4");
        temp_write(&t.path.join(".local/share/ibus/component/lyyime.xml"), "<component/>");
        temp_write(&t.path.join(".local/share/lyyime/ibus/engine/lyyime.py"), "# engine");
        temp_write(&t.path.join(".cache/ibus/bus/registry"), "stale-cache-no-engine-here");
        let r = check_engine_register(&paths(&t), &FakeSystem::new(0));
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("缓存"));
    }

    #[test]
    fn autostart_fail_when_no_dir() {
        let t = TempDir::new("as1");
        let r = check_autostart(&paths(&t));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn autostart_ok_with_lyyime_entry() {
        let t = TempDir::new("as2");
        temp_write(
            &t.path.join(".config/autostart/im-lyyime.desktop"),
            "[Desktop Entry]\nType=Application\nName=lyyIme\nExec=lyyime-app\n",
        );
        let r = check_autostart(&paths(&t));
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
    }

    #[test]
    fn autostart_warn_framework_only_and_respect_hidden() {
        let t = TempDir::new("as3");
        temp_write(
            &t.path.join(".config/autostart/ibus.desktop"),
            "[Desktop Entry]\nExec=ibus-daemon -drx\n",
        );
        let r = check_autostart(&paths(&t));
        assert_eq!(r.status, Status::Warn);
        // Hidden=true 视为停用
        let t2 = TempDir::new("as4");
        temp_write(
            &t2.path.join(".config/autostart/ibus.desktop"),
            "[Desktop Entry]\nExec=ibus-daemon -drx\nHidden=true\n",
        );
        let r2 = check_autostart(&paths(&t2));
        assert_eq!(r2.status, Status::Fail, "{:?}", r2.detail);
    }

    #[test]
    fn immodule_ok_when_gtk3_and_cache_present() {
        let t = TempDir::new("im1");
        temp_write(&t.path.join("gtk3/immodules/im-ibus.so"), "so");
        temp_write(&t.path.join("gtk3/immodules.cache"), "im-ibus.so");
        let r = check_immodule(&paths(&t));
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
    }

    #[test]
    fn immodule_fail_when_gtk3_missing() {
        let t = TempDir::new("im2");
        let r = check_immodule(&paths(&t));
        assert_eq!(r.status, Status::Fail);
    }

    #[test]
    fn data_warn_without_meta_json() {
        let t = TempDir::new("dt1");
        let r = check_data(&paths(&t));
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("meta.json"));
    }

    #[test]
    fn data_ok_with_meta_and_clean_user_tsv() {
        let t = TempDir::new("dt2");
        temp_write(
            &t.path.join(".local/share/lyyime/runtime/meta.json"),
            r#"{"version":1,"rows":123}"#,
        );
        temp_write(&t.path.join(".local/share/lyyime/user.tsv"), "你好\t3\t1700000000\n");
        let r = check_data(&paths(&t));
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
    }

    #[test]
    fn data_fail_with_malformed_user_tsv() {
        let t = TempDir::new("dt3");
        temp_write(&t.path.join(".local/share/lyyime/runtime/meta.json"), r#"{"version":1}"#);
        temp_write(&t.path.join(".local/share/lyyime/user.tsv"), "no-tabs-here\n");
        let r = check_data(&paths(&t));
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("非法"));
    }

    #[test]
    fn logs_ok_when_no_dir_and_warn_on_errors() {
        let t = TempDir::new("lg1");
        let r = check_logs(&paths(&t));
        assert_eq!(r.status, Status::Ok);

        let t2 = TempDir::new("lg2");
        temp_write(
            &t2.path.join(".local/share/lyyime/logs/ibus.log"),
            "info line\nERROR: engine crashed\nok line\n",
        );
        let r2 = check_logs(&paths(&t2));
        assert_eq!(r2.status, Status::Warn);
        assert!(r2.detail.contains("ERROR: engine crashed"));

        let t3 = TempDir::new("lg3");
        temp_write(&t3.path.join(".local/share/lyyime/logs/ibus.log"), "clean\n");
        assert_eq!(check_logs(&paths(&t3)).status, Status::Ok);
    }

    #[test]
    fn locale_matrix() {
        assert_eq!(
            check_locale(&FakeSystem::new(0).with_env("LANG", "zh_CN.UTF-8")).status,
            Status::Ok
        );
        assert_eq!(
            check_locale(&FakeSystem::new(0).with_env("LANG", "en_US.UTF-8")).status,
            Status::Ok
        );
        assert_eq!(check_locale(&FakeSystem::new(0).with_env("LANG", "C")).status, Status::Fail);
        assert_eq!(check_locale(&FakeSystem::new(0)).status, Status::Fail);
        // LC_ALL 覆盖 LANG
        assert_eq!(
            check_locale(
                &FakeSystem::new(0)
                    .with_env("LANG", "C")
                    .with_env("LC_ALL", "zh_CN.UTF-8")
            )
            .status,
            Status::Ok
        );
    }

    #[test]
    fn gui_env_warn_without_desktop_procs() {
        let r = check_gui_env(&FakeSystem::new(0));
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("未发现桌面会话进程"));
    }

    #[test]
    fn gui_env_ok_when_leaders_have_full_trio() {
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "GTK_IM_MODULE", "ibus")
            .with_proc_environ(10, "QT_IM_MODULE", "ibus")
            .with_proc_environ(10, "XMODIFIERS", "@im=ibus")
            .with_proc(11, 0, "root", "xfce4-panel")
            .with_proc_environ(11, "GTK_IM_MODULE", "ibus")
            .with_proc_environ(11, "QT_IM_MODULE", "ibus")
            .with_proc_environ(11, "XMODIFIERS", "@im=ibus");
        let r = check_gui_env(&sys);
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
        assert!(r.detail.contains("pid=10"));
    }

    #[test]
    fn gui_env_ok_for_modeb_profile() {
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "GTK_IM_MODULE", "xim")
            .with_proc_environ(10, "QT_IM_MODULE", "ibus")
            .with_proc_environ(10, "XMODIFIERS", "@im=lyyime");
        let r = check_gui_env(&sys);
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
        assert!(r.detail.contains("Mode B"));
    }

    #[test]
    fn gui_env_fail_when_leader_missing_vars() {
        // 会话经验根因 #2:xfce4-session 本身没有三件套 → 全部子应用失效
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DISPLAY", ":11.0");
        let r = check_gui_env(&sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("pid=10(xfce4-session) 缺少"), "{}", r.detail);
        assert!(r.fix_hint.as_deref().unwrap().contains("fix --issue env"));
    }

    #[test]
    fn gui_env_fail_when_leaders_disagree() {
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "GTK_IM_MODULE", "ibus")
            .with_proc_environ(10, "QT_IM_MODULE", "ibus")
            .with_proc_environ(10, "XMODIFIERS", "@im=ibus")
            .with_proc(12, 0, "root", "xfce4-panel")
            .with_proc_environ(12, "GTK_IM_MODULE", "fcitx")
            .with_proc_environ(12, "QT_IM_MODULE", "fcitx")
            .with_proc_environ(12, "XMODIFIERS", "@im=fcitx");
        let r = check_gui_env(&sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("指向不同框架"));
    }

    #[test]
    fn session_bus_ok_when_registered_on_session_bus() {
        let bus = "unix:path=/tmp/dbus-AAA";
        let sys = FakeSystem::new(0)
            .with_env("XMODIFIERS", "@im=ibus")
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", bus)
            .with_proc(20, 0, "root", "ibus-daemon -drx")
            .with_proc_environ(20, "DBUS_SESSION_BUS_ADDRESS", bus)
            .with_cmd(
                "busctl",
                &[&format!("--address={bus}"), "list"],
                crate::system::RunOutput::ok("org.freedesktop.IBus :1.5 root"),
            );
        let r = check_session_bus(&paths(&TempDir::new("sb1")), &sys);
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
    }

    #[test]
    fn session_bus_fail_when_daemon_on_wrong_bus() {
        // 会话经验根因 #1:daemon 活着,但接在 /run/user/0/bus;会话总线是私有 /tmp/dbus-*
        let sess_bus = "unix:path=/tmp/dbus-AAA";
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", sess_bus)
            .with_proc(20, 0, "root", "ibus-daemon -drx")
            .with_proc_environ(20, "DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/0/bus")
            .with_cmd(
                "busctl",
                &[&format!("--address={sess_bus}"), "list"],
                crate::system::RunOutput::ok("org.freedesktop.portal.Desktop :1.2"),
            );
        let r = check_session_bus(&paths(&TempDir::new("sb2")), &sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("总线接错"), "{}", r.detail);
        assert!(r.fix_hint.as_deref().unwrap().contains("restart-ibus"));
    }

    #[test]
    fn session_bus_fail_when_no_daemon_at_all() {
        let sess_bus = "unix:path=/tmp/dbus-AAA";
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", sess_bus)
            .with_cmd(
                "busctl",
                &[&format!("--address={sess_bus}"), "list"],
                crate::system::RunOutput::ok("org.freedesktop.DBus"),
            );
        let r = check_session_bus(&paths(&TempDir::new("sb3")), &sys);
        assert_eq!(r.status, Status::Fail);
        assert!(r.detail.contains("没有 org.freedesktop.IBus 注册"));
    }

    #[test]
    fn session_bus_degrades_to_environ_compare_without_busctl() {
        let sess_bus = "unix:path=/tmp/dbus-AAA";
        // busctl 执行失败(返回 success=false)→ 退化为进程环境比对
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", sess_bus)
            .with_proc(20, 0, "root", "ibus-daemon -drx")
            .with_proc_environ(20, "DBUS_SESSION_BUS_ADDRESS", sess_bus)
            .with_cmd(
                "busctl",
                &[&format!("--address={sess_bus}"), "list"],
                crate::system::RunOutput::failed("could not connect"),
            );
        let r = check_session_bus(&paths(&TempDir::new("sb4")), &sys);
        assert_eq!(r.status, Status::Ok, "{:?}", r.detail);
        assert!(r.detail.contains("busctl 不可用"));

        // 同一退化路径下,daemon 接错总线 → Fail
        let sys2 = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", sess_bus)
            .with_proc(20, 0, "root", "ibus-daemon -drx")
            .with_proc_environ(20, "DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/0/bus")
            .with_cmd(
                "busctl",
                &[&format!("--address={sess_bus}"), "list"],
                crate::system::RunOutput::failed("could not connect"),
            );
        let r2 = check_session_bus(&paths(&TempDir::new("sb5")), &sys2);
        assert_eq!(r2.status, Status::Fail);
    }

    #[test]
    fn session_bus_warn_when_no_session_bus_found() {
        let r = check_session_bus(&paths(&TempDir::new("sb6")), &FakeSystem::new(0));
        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("未探测到图形会话总线"));
    }

    #[test]
    fn all_checks_run_in_order() {
        let t = TempDir::new("all");
        let sys = FakeSystem::new(0);
        let results = run_checks(&paths(&t), &sys);
        let ids: Vec<&str> = results.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, CHECK_IDS.to_vec());
    }
}
