//! 系统副作用抽象:进程探测、命令执行、kill、detached 拉起。
//! 单测用 [`FakeSystem`] 桩,生产用 [`RealSystem`]。

use std::process::Command;

#[cfg(test)]
use std::collections::BTreeMap;

/// 一次外部命令的输出
#[derive(Debug, Clone)]
pub struct RunOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    pub fn ok(stdout: &str) -> Self {
        RunOutput { success: true, stdout: stdout.to_string(), stderr: String::new() }
    }
    pub fn failed(stderr: &str) -> Self {
        RunOutput { success: false, stdout: String::new(), stderr: stderr.to_string() }
    }
}

/// 一个进程条目(ps 探测结果)
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcInfo {
    pub pid: u32,
    pub uid: u32,
    pub user: String,
    pub args: String,
}

/// 系统操作抽象(所有可能产生副作用的系统调用都收敛在此,便于测试桩替换)
pub trait SystemOps {
    fn env(&self, key: &str) -> Option<String>;
    fn current_uid(&self) -> u32;
    /// 返回 args 中包含 name_hint 的进程(尽力而为,排除 ps/grep 自身)
    fn find_processes(&self, name_hint: &str) -> Vec<ProcInfo>;
    fn command_exists(&self, prog: &str) -> bool;
    /// 执行外部命令(同步,捕获输出)
    fn run(&self, prog: &str, args: &[&str]) -> std::io::Result<RunOutput>;
    /// 向进程发 SIGTERM(仅允许对过滤过 uid 的 pid 调用)
    fn kill(&self, pid: u32) -> std::io::Result<()>;
    /// 以脱离会话的方式拉起守护进程(不阻塞、不随调用者退出被杀)
    fn spawn_detached(&self, prog: &str, args: &[&str]) -> std::io::Result<()>;
}

/// 生产实现
pub struct RealSystem;

impl SystemOps for RealSystem {
    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn current_uid(&self) -> u32 {
        use std::os::unix::fs::MetadataExt;
        match std::fs::metadata("/proc/self") {
            Ok(m) => m.uid(),
            Err(_) => self
                .run("id", &["-u"])
                .ok()
                .and_then(|o| o.stdout.trim().parse().ok())
                .unwrap_or(0),
        }
    }

    fn find_processes(&self, name_hint: &str) -> Vec<ProcInfo> {
        let out = match self.run("ps", &["-eo", "pid=,uid=,user=,args="]) {
            Ok(o) if o.success => o.stdout,
            _ => return vec![],
        };
        out.lines()
            .filter_map(parse_ps_line)
            // 严格匹配:首 token 的文件名 == hint(如 "ibus-daemon -drx")。
            // 不能用子串包含,否则 shell 包装脚本参数里恰含 "ibus-daemon"
            // 字样的进程(包括调用方自身)会被误杀。
            .filter(|p| basename_of(p.args.split_whitespace().next().unwrap_or("")) == name_hint)
            .filter(|p| p.pid != std::process::id())
            .collect()
    }

    fn command_exists(&self, prog: &str) -> bool {
        // prog 全部来自本 crate 内部常量,无注入风险
        let script = format!("command -v -- {prog}");
        matches!(
            self.run("sh", &["-c", &script]),
            Ok(o) if o.success && !o.stdout.trim().is_empty()
        )
    }

    fn run(&self, prog: &str, args: &[&str]) -> std::io::Result<RunOutput> {
        let out = Command::new(prog).args(args).output()?;
        Ok(RunOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn kill(&self, pid: u32) -> std::io::Result<()> {
        let out = Command::new("kill").arg(pid.to_string()).output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "kill {pid} 失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }

    fn spawn_detached(&self, prog: &str, args: &[&str]) -> std::io::Result<()> {
        // sh 后台化后立即退出;守护进程被 init 收养,不残留 zombie
        let mut parts: Vec<String> = vec![prog.to_string()];
        parts.extend(args.iter().map(|s| s.to_string()));
        let script = format!("nohup {} >/dev/null 2>&1 &", parts.join(" "));
        let status = Command::new("sh").arg("-c").arg(&script).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("拉起 {prog} 失败")))
        }
    }
}

/// 路径末段("usr/bin/ibus-daemon" → "ibus-daemon")
fn basename_of(p: &str) -> &str {
    match p.rsplit_once('/') {
        Some((_, b)) => b,
        None => p,
    }
}

fn parse_ps_line(line: &str) -> Option<ProcInfo> {
    let mut it = line.split_whitespace();
    let pid: u32 = it.next()?.parse().ok()?;
    let uid: u32 = it.next()?.parse().ok()?;
    let user = it.next()?.to_string();
    let args = it.collect::<Vec<_>>().join(" ");
    Some(ProcInfo { pid, uid, user, args })
}

// ---------------------------------------------------------------------------
// 测试桩
// ---------------------------------------------------------------------------
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// 记录全部操作并可编程返回结果的测试桩。ops 用 Rc 共享,便于 Box 之后断言。
    pub(crate) struct FakeSystem {
        pub envs: BTreeMap<String, String>,
        pub uid: u32,
        pub procs: Vec<ProcInfo>,
        pub exists: Vec<String>,
        pub results: BTreeMap<String, RunOutput>,
        pub ops: Rc<RefCell<Vec<String>>>,
        pub fail_spawn: bool,
    }

    impl FakeSystem {
        pub fn new(uid: u32) -> Self {
            FakeSystem {
                envs: BTreeMap::new(),
                uid,
                procs: vec![],
                exists: vec![],
                results: BTreeMap::new(),
                ops: Rc::new(RefCell::new(vec![])),
                fail_spawn: false,
            }
        }
        pub fn with_env(mut self, k: &str, v: &str) -> Self {
            self.envs.insert(k.to_string(), v.to_string());
            self
        }
        pub fn with_proc(mut self, pid: u32, uid: u32, user: &str, args: &str) -> Self {
            self.procs.push(ProcInfo {
                pid,
                uid,
                user: user.to_string(),
                args: args.to_string(),
            });
            self
        }
        pub fn with_cmd(mut self, prog: &str, args: &[&str], out: RunOutput) -> Self {
            self.results.insert(cmd_key(prog, args), out);
            self
        }
        pub fn with_exists(mut self, prog: &str) -> Self {
            self.exists.push(prog.to_string());
            self
        }
        pub fn ops_snapshot(&self) -> Vec<String> {
            self.ops.borrow().clone()
        }
    }

    fn cmd_key(prog: &str, args: &[&str]) -> String {
        if args.is_empty() {
            prog.to_string()
        } else {
            format!("{prog} {}", args.join(" "))
        }
    }

    impl SystemOps for FakeSystem {
        fn env(&self, key: &str) -> Option<String> {
            self.envs.get(key).filter(|s| !s.is_empty()).cloned()
        }
        fn current_uid(&self) -> u32 {
            self.uid
        }
        fn find_processes(&self, hint: &str) -> Vec<ProcInfo> {
            self.procs
                .iter()
                .filter(|p| {
                    let first = p.args.split_whitespace().next().unwrap_or("");
                    let base = first.rsplit_once('/').map(|(_, b)| b).unwrap_or(first);
                    base == hint
                })
                .cloned()
                .collect()
        }
        fn command_exists(&self, prog: &str) -> bool {
            self.exists.iter().any(|e| e == prog)
        }
        fn run(&self, prog: &str, args: &[&str]) -> std::io::Result<RunOutput> {
            self.ops
                .borrow_mut()
                .push(format!("run {}", cmd_key(prog, args)));
            Ok(self
                .results
                .get(&cmd_key(prog, args))
                .cloned()
                .unwrap_or(RunOutput { success: true, stdout: String::new(), stderr: String::new() }))
        }
        fn kill(&self, pid: u32) -> std::io::Result<()> {
            self.ops.borrow_mut().push(format!("kill {pid}"));
            Ok(())
        }
        fn spawn_detached(&self, prog: &str, args: &[&str]) -> std::io::Result<()> {
            self.ops
                .borrow_mut()
                .push(format!("spawn {}", cmd_key(prog, args)));
            if self.fail_spawn {
                Err(std::io::Error::other("stub spawn failure"))
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_line_parsing() {
        let p = parse_ps_line("  1630041       0 root  ibus-daemon -drx").unwrap();
        assert_eq!(p.pid, 1630041);
        assert_eq!(p.uid, 0);
        assert_eq!(p.user, "root");
        assert_eq!(p.args, "ibus-daemon -drx");
        assert!(parse_ps_line("not-a-pid x").is_none());
    }

    #[test]
    fn wrapper_scripts_mentioning_daemon_are_not_matched() {
        // 防回归:调用方 shell 的参数里含 "ibus-daemon" 字样时绝不能算作守护进程,
        // 否则 restart-ibus 会误杀调用方自己。
        let s = RealSystem;
        let procs = s.find_processes("ibus-daemon");
        assert!(
            procs.iter().all(|p| {
                let first = p.args.split_whitespace().next().unwrap_or("");
                let base = first.rsplit_once('/').map(|(_, b)| b).unwrap_or(first);
                base == "ibus-daemon"
            }),
            "{procs:?}"
        );
    }

    #[test]
    fn real_system_env_reads_process_env() {
        let s = RealSystem;
        // PATH 一定存在
        assert!(s.env("PATH").is_some());
        assert!(s.env("LYYIME_NO_SUCH_VAR__").is_none());
    }
}
