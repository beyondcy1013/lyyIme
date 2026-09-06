//! 真屏输入链路探测:编排「自建窗口 + 显式聚焦 + xdotool 注入 + 缓冲断言」,
//! 复刻 lyyime-ops SKILL §4 的手工流程。安全边界:绝不向用户焦点窗口注入按键,
//! 只对自建的 `lyyime-probe` 窗口做 `xdotool windowfocus` 后注入(铁律 5)。

use crate::paths::Paths;
use crate::session::detect_session_env;
use crate::system::SystemOps;

#[derive(Debug, Clone)]
pub struct ProbeOptions {
    /// 目标显示器(缺省:会话 GUI 进程的 DISPLAY,再退回调用方环境)
    pub display: Option<String>,
    /// 目标会话总线(缺省:会话 GUI 进程的 DBUS_SESSION_BUS_ADDRESS)
    pub bus: Option<String>,
    /// 注入的字母串(默认 "nihao")
    pub text: String,
    /// 缓冲中应出现的上屏结果(默认 "你好")
    pub expect: String,
    pub timeout_secs: u64,
    /// 缓冲文件(缺省:/tmp/lyyime-doctor-probe-<pid>.txt;测试注入)
    pub buffer: Option<std::path::PathBuf>,
    /// ime-probe.py 脚本路径(缺省:项目 scripts/ 或 /usr/local/share/lyyime/)
    pub script: Option<std::path::PathBuf>,
    /// 测试钩子:窗口就绪等待与轮询间隔(毫秒)
    pub settle_ms: u64,
    pub poll_ms: u64,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        ProbeOptions {
            display: None,
            bus: None,
            text: "nihao".into(),
            expect: "你好".into(),
            timeout_secs: 20,
            buffer: None,
            script: None,
            settle_ms: 2000,
            poll_ms: 300,
        }
    }
}

/// 一步探测结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// 探测报告(全部步骤 + 总结论)
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeReport {
    pub steps: Vec<ProbeStep>,
    pub passed: bool,
}

pub(crate) fn run_probe(sys: &dyn SystemOps, p: &Paths, opts: &ProbeOptions) -> ProbeReport {
    let mut steps: Vec<ProbeStep> = vec![];

    // 1. 组装探测进程环境:以会话 GUI 进程的真实环境为准(铁律 3)
    let sess = detect_session_env(sys);
    let display = opts
        .display
        .clone()
        .or_else(|| sess.display.clone())
        .or_else(|| sys.env("DISPLAY"));
    let bus = opts
        .bus
        .clone()
        .or_else(|| sess.dbus_address.clone())
        .or_else(|| sys.env("DBUS_SESSION_BUS_ADDRESS"));
    // IM 三件套:优先取会话进程的真实值(那才是应用真正继承的);没有再退回调用方
    let sample = sess.sampled.first();
    let im_of = |key: &str| -> Option<String> {
        sample
            .and_then(|s| s.getenv(key))
            .map(str::to_string)
            .or_else(|| sys.env(key))
    };
    let gtk = im_of("GTK_IM_MODULE");
    let qt = im_of("QT_IM_MODULE");
    let xmod = im_of("XMODIFIERS");
    let mut envs: Vec<(String, String)> = vec![];
    if let Some(d) = &display {
        envs.push(("DISPLAY".into(), d.clone()));
    }
    if let Some(b) = &bus {
        envs.push(("DBUS_SESSION_BUS_ADDRESS".into(), b.clone()));
    }
    for (k, v) in [
        ("GTK_IM_MODULE", &gtk),
        ("QT_IM_MODULE", &qt),
        ("XMODIFIERS", &xmod),
    ] {
        if let Some(v) = v {
            envs.push((k.into(), v.clone()));
        }
    }
    {
        let missing_im = [&gtk, &qt, &xmod].iter().any(|v| v.is_none());
        let detail = envs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        steps.push(ProbeStep {
            name: "env".into(),
            ok: !missing_im && display.is_some() && bus.is_some(),
            detail: if missing_im {
                format!("{detail};警告:未取到 IM 三件套,探测窗口将不启用输入法,结果必为字母直通(先跑 `fix --issue env`)")
            } else {
                detail
            },
        });
    }

    // 2. 探测脚本
    let script = opts
        .script
        .clone()
        .or_else(|| {
            p.project_dir
                .as_ref()
                .map(|d| d.join("scripts/ime-probe.py"))
                .filter(|f| f.is_file())
        })
        .or_else(|| {
            let f = p.prefix_share.join("lyyime/ime-probe.py");
            f.is_file().then_some(f)
        });
    let Some(script) = script else {
        steps.push(ProbeStep {
            name: "script".into(),
            ok: false,
            detail: "找不到 scripts/ime-probe.py(项目根或 /usr/local/share/lyyime/);可用 --script 指定".into(),
        });
        return ProbeReport { steps, passed: false };
    };
    steps.push(ProbeStep {
        name: "script".into(),
        ok: true,
        detail: script.display().to_string(),
    });

    // 3. 缓冲文件
    let buffer = opts.buffer.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("lyyime-doctor-probe-{}.txt", std::process::id()))
    });
    let _ = std::fs::remove_file(&buffer);
    if let Some(d) = buffer.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&buffer, "");

    // 4. 后台拉起探测窗口
    let timeout_s = opts.timeout_secs.to_string();
    let script_str = script.to_string_lossy().into_owned();
    let buffer_str = buffer.to_string_lossy().into_owned();
    let child_pid: Option<u32> =
        match sys.spawn_background(&envs, "python3", &[&script_str, &buffer_str, &timeout_s]) {
            Ok(pid) => {
                steps.push(ProbeStep {
                    name: "spawn".into(),
                    ok: true,
                    detail: format!("探测窗口已拉起(pid={pid},超时 {timeout_s}s)"),
                });
                Some(pid)
            }
            Err(e) => {
                steps.push(ProbeStep {
                    name: "spawn".into(),
                    ok: false,
                    detail: format!("拉起 python3 ime-probe.py 失败:{e}"),
                });
                return ProbeReport { steps, passed: false };
            }
        };

    let cleanup = |sys: &dyn SystemOps, pid: Option<u32>| {
        if let Some(pid) = pid {
            let _ = sys.kill(pid);
        }
    };

    std::thread::sleep(std::time::Duration::from_millis(opts.settle_ms));

    // 5. 找到自建窗口并显式聚焦(绝不碰用户焦点窗口)
    let wid: Option<String> =
        match sys.run("xdotool", &["search", "--name", "^lyyime-probe$"]) {
            Ok(o) if o.success => o.stdout.split_whitespace().next().map(str::to_string),
            _ => None,
        };
    let Some(wid) = wid else {
        steps.push(ProbeStep {
            name: "window".into(),
            ok: false,
            detail: format!(
                "探测窗口未出现(xdotool search 无结果;确认 DISPLAY={} 可连且窗口未被关闭)",
                display.as_deref().unwrap_or("(未设置)")
            ),
        });
        cleanup(sys, child_pid);
        return ProbeReport { steps, passed: false };
    };
    steps.push(ProbeStep { name: "window".into(), ok: true, detail: format!("wid={wid}") });

    let focus_ok = sys
        .run("xdotool", &["windowfocus", &wid])
        .map(|o| o.success)
        .unwrap_or(false);
    steps.push(ProbeStep {
        name: "focus".into(),
        ok: focus_ok,
        detail: format!("xdotool windowfocus {wid}"),
    });

    // 6. 注入按键 + 空格上屏(对已聚焦的自建窗口)
    let type_args = ["type", "--delay", "90", "--", opts.text.as_str()];
    let type_ok = sys.run("xdotool", &type_args).map(|o| o.success).unwrap_or(false);
    let key_ok = sys
        .run("xdotool", &["key", "space"])
        .map(|o| o.success)
        .unwrap_or(false);
    steps.push(ProbeStep {
        name: "type".into(),
        ok: type_ok && key_ok,
        detail: format!("已注入「{}」+ 空格(仅注入到自建窗口)", opts.text),
    });

    // 7. 轮询缓冲断言
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(opts.timeout_secs.max(1));
    let poll = std::time::Duration::from_millis(opts.poll_ms.max(1));
    let mut content = String::new();
    let mut hit = false;
    while std::time::Instant::now() < deadline {
        if let Ok(c) = std::fs::read_to_string(&buffer) {
            content = c;
            if content.contains(opts.expect.as_str()) {
                hit = true;
                break;
            }
        }
        std::thread::sleep(poll);
    }
    if hit {
        steps.push(ProbeStep {
            name: "assert".into(),
            ok: true,
            detail: format!(
                "缓冲出现「{}」——链路(环境→总线→daemon→引擎→上屏)全通",
                opts.expect
            ),
        });
    } else {
        let mut detail =
            format!("超时未在缓冲见到「{}」;缓冲实际内容:{content:?}", opts.expect);
        // 干扰现象速判(会话实录):全角字母 = 引擎被切到全角模式,Shift+Space 切回
        let fullwidth: String = opts.text.chars().map(fullwidth_char).collect();
        if !fullwidth.is_empty() && content.contains(fullwidth.as_str()) {
            detail
                .push_str(";缓冲恰为全角字母——引擎处于全角模式,按 Shift+Space 切回即可,非链路故障");
        } else if content.contains(opts.text.as_str()) {
            detail.push_str(
                ";缓冲为原样字母(raw 直通)——IM 未接管:核对 gui-env(应用环境变量)与 session-bus(IBus 注册总线)两项",
            );
        }
        steps.push(ProbeStep { name: "assert".into(), ok: false, detail });
    }
    cleanup(sys, child_pid);
    ProbeReport { steps, passed: hit }
}

/// ASCII → 全角(干扰现象速判用)
fn fullwidth_char(c: char) -> char {
    char::from_u32(c as u32 + 0xFEE0).unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::system::fake::FakeSystem;
    use crate::system::RunOutput;
    use crate::testutil::{temp_write, TempDir};

    /// 组装一个"全链路正常"的假环境:会话进程 + IM 三件套 + xdotool 每步成功
    fn fake_env(t: &TempDir) -> FakeSystem {
        let _ = t;
        let sess_bus = "unix:path=/tmp/dbus-AAA";
        FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", sess_bus)
            .with_proc_environ(10, "DISPLAY", ":11.0")
            .with_proc_environ(10, "GTK_IM_MODULE", "ibus")
            .with_proc_environ(10, "QT_IM_MODULE", "ibus")
            .with_proc_environ(10, "XMODIFIERS", "@im=ibus")
            .with_exists("python3")
            .with_cmd(
                "xdotool",
                &["search", "--name", "^lyyime-probe$"],
                RunOutput::ok("92345\n"),
            )
            .with_cmd("xdotool", &["windowfocus", "92345"], RunOutput::ok(""))
            .with_cmd("xdotool", &["type", "--delay", "90", "--", "nihao"], RunOutput::ok(""))
            .with_cmd("xdotool", &["key", "space"], RunOutput::ok(""))
            .with_bg_pid(777)
    }

    /// 模拟探测窗口的周期写盘:延迟写入 content(run_probe 会先清空缓冲,
    /// 所以必须在 spawn 之后写,正好复刻"引擎上屏后缓冲更新"的真实行为)
    fn buffer_writer(buf: std::path::PathBuf, content: &'static str, delay_ms: u64) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _ = std::fs::write(buf, content);
        });
    }

    #[test]
    fn probe_passes_when_buffer_gets_expected_text() {
        let t = TempDir::new("pr1");
        let buf = t.path.join("probe.txt");
        let script = t.path.join("scripts/ime-probe.py");
        temp_write(&script, "#!/usr/bin/env python3");
        let mut p = Paths::for_home(&t.path);
        p.project_dir = Some(t.path.clone());
        let sys = fake_env(&t);
        buffer_writer(buf.clone(), "你好", 10);
        let opts = ProbeOptions {
            buffer: Some(buf.clone()),
            script: None,
            timeout_secs: 5,
            settle_ms: 0,
            poll_ms: 2,
            ..Default::default()
        };
        let report = run_probe(&sys, &p, &opts);
        assert!(report.passed, "{report:?}");
        let names: Vec<&str> = report.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["env", "script", "spawn", "window", "focus", "type", "assert"]);
        // 环境以会话进程真实值为准
        assert!(sys
            .ops_snapshot()
            .iter()
            .any(|o| o.contains("bg-env") && o.contains("DISPLAY=:11.0") && o.contains("GTK_IM_MODULE=ibus")));
        // 探测进程被收尾 kill
        assert!(sys.ops_snapshot().iter().any(|o| o == "kill 777"));
    }

    #[test]
    fn probe_fails_with_raw_passthrough_hint() {
        let t = TempDir::new("pr2");
        let buf = t.path.join("probe.txt");
        let script = t.path.join("scripts/ime-probe.py");
        temp_write(&script, "x");
        let mut p = Paths::for_home(&t.path);
        p.project_dir = Some(t.path.clone());
        let sys = fake_env(&t);
        buffer_writer(buf.clone(), "nihao", 10); // 字母直通
        let opts = ProbeOptions {
            buffer: Some(buf),
            timeout_secs: 1,
            settle_ms: 0,
            poll_ms: 5,
            ..Default::default()
        };
        let report = run_probe(&sys, &p, &opts);
        assert!(!report.passed);
        let assert_step = report.steps.iter().find(|s| s.name == "assert").unwrap();
        assert!(assert_step.detail.contains("raw 直通"), "{}", assert_step.detail);
        assert!(assert_step.detail.contains("gui-env"), "{}", assert_step.detail);
    }

    #[test]
    fn probe_detects_fullwidth_interference() {
        let t = TempDir::new("pr3");
        let buf = t.path.join("probe.txt");
        let script = t.path.join("scripts/ime-probe.py");
        temp_write(&script, "x");
        let mut p = Paths::for_home(&t.path);
        p.project_dir = Some(t.path.clone());
        let sys = fake_env(&t);
        buffer_writer(buf.clone(), "ｎｉｈａｏ", 10); // 全角字母
        let opts = ProbeOptions {
            buffer: Some(buf),
            timeout_secs: 1,
            settle_ms: 0,
            poll_ms: 5,
            ..Default::default()
        };
        let report = run_probe(&sys, &p, &opts);
        let assert_step = report.steps.iter().find(|s| s.name == "assert").unwrap();
        assert!(
            assert_step.detail.contains("全角模式"),
            "{}",
            assert_step.detail
        );
    }

    #[test]
    fn probe_fails_fast_without_script() {
        let t = TempDir::new("pr4");
        let mut p = Paths::for_home(&t.path);
        p.project_dir = None;
        let sys = FakeSystem::new(0);
        let opts = ProbeOptions { ..Default::default() };
        let report = run_probe(&sys, &p, &opts);
        assert!(!report.passed);
        assert!(report.steps[0].name == "env");
        assert_eq!(report.steps.len(), 2, "找不到脚本应立即返回:{report:?}");
        assert!(sys.ops_snapshot().is_empty(), "未拉起进程就不应有副作用");
    }

    #[test]
    fn probe_fails_when_window_never_appears() {
        let t = TempDir::new("pr5");
        let buf = t.path.join("probe.txt");
        let script = t.path.join("scripts/ime-probe.py");
        temp_write(&script, "x");
        let mut p = Paths::for_home(&t.path);
        p.project_dir = Some(t.path.clone());
        // 不注册 xdotool search 的成功输出 → FakeSystem 默认 success+空 stdout → 找不到窗口
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/dbus-A")
            .with_proc_environ(10, "DISPLAY", ":11.0")
            .with_proc_environ(10, "GTK_IM_MODULE", "ibus")
            .with_proc_environ(10, "QT_IM_MODULE", "ibus")
            .with_proc_environ(10, "XMODIFIERS", "@im=ibus")
            .with_bg_pid(778);
        let opts = ProbeOptions {
            buffer: Some(buf),
            settle_ms: 0,
            poll_ms: 1,
            ..Default::default()
        };
        let report = run_probe(&sys, &p, &opts);
        assert!(!report.passed);
        let win = report.steps.iter().find(|s| s.name == "window").unwrap();
        assert!(!win.ok);
        assert!(sys.ops_snapshot().iter().any(|o| o == "kill 778"), "窗口失败也要收尾探测进程");
    }
}
