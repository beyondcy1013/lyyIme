//! 会话环境探测。经验来源:RESEARCH.md §4 / lyyime-ops SKILL「环境铁律」:
//! - 诊断必须以真实 GUI 会话进程的 `/proc/<pid>/environ` 为准,不看调用方 shell;
//! - XFCE 常由 `dbus-run-session -- startxfce4` 启动,持有私有会话总线
//!   (`/tmp/dbus-XXXX`),不要想当然用 `/run/user/<uid>/bus`;
//! - ibus-daemon 只有注册在这条会话总线上,桌面应用才连得上。

use crate::system::SystemOps;

/// 桌面会话"领导者/常驻"进程:它们的环境决定会话内所有新应用继承到什么。
pub(crate) const SESSION_LEADER_HINTS: &[&str] = &[
    "xfce4-session",
    "xfce4-panel",
    "gnome-shell",
    "plasmashell",
    "mate-session",
    "cinnamon-session",
    "lxsession",
];

/// 一个被采样的会话进程
#[derive(Debug, Clone)]
pub(crate) struct SampledProc {
    pub pid: u32,
    /// 进程名(首 token 的 basename)
    pub name: String,
    pub env: Vec<(String, String)>,
}

impl SampledProc {
    pub fn getenv(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// 图形会话环境归纳结果
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionEnv {
    pub sampled: Vec<SampledProc>,
    /// 会话总线地址(GUI 进程多数派;None = 没采到)
    pub dbus_address: Option<String>,
    pub display: Option<String>,
    /// 采样到的全部总线地址(去重;>1 条说明存在多屏/多会话分裂)
    pub all_dbus_addresses: Vec<String>,
}

/// 采样当前 uid 的桌面会话进程环境并归纳;无图形会话时 `sampled` 为空。
pub(crate) fn detect_session_env(sys: &dyn SystemOps) -> SessionEnv {
    let uid = sys.current_uid();
    let mut sampled: Vec<SampledProc> = vec![];
    for hint in SESSION_LEADER_HINTS {
        for p in sys.find_processes(hint) {
            if p.uid != uid {
                continue;
            }
            let first = p.args.split_whitespace().next().unwrap_or(hint);
            let name = first.rsplit_once('/').map(|(_, b)| b).unwrap_or(first);
            sampled.push(SampledProc {
                pid: p.pid,
                name: name.to_string(),
                env: sys.read_proc_environ(p.pid),
            });
        }
    }
    let bus_vals: Vec<String> = sampled
        .iter()
        .filter_map(|s| s.getenv("DBUS_SESSION_BUS_ADDRESS"))
        .map(str::to_string)
        .collect();
    let mut all: Vec<String> = bus_vals.clone();
    all.sort();
    all.dedup();
    let display_vals: Vec<String> = sampled
        .iter()
        .filter_map(|s| s.getenv("DISPLAY"))
        .map(str::to_string)
        .collect();
    SessionEnv {
        dbus_address: majority(bus_vals),
        display: majority(display_vals),
        all_dbus_addresses: all,
        sampled,
    }
}

/// 取出现最多的值(并列取字典序最小,保证确定性)
fn majority(vals: Vec<String>) -> Option<String> {
    let mut counts: Vec<(String, usize)> = vec![];
    for v in vals {
        match counts.iter_mut().find(|(k, _)| *k == v) {
            Some((_, c)) => *c += 1,
            None => counts.push((v, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts.into_iter().next().map(|(k, _)| k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fake::FakeSystem;

    #[test]
    fn detects_session_bus_and_display_from_leader_procs() {
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session --display :11.0")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/dbus-AAA")
            .with_proc_environ(10, "DISPLAY", ":11.0")
            .with_proc(11, 0, "root", "xfce4-panel")
            .with_proc_environ(11, "DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/dbus-AAA")
            .with_proc_environ(11, "DISPLAY", ":11.0");
        let env = detect_session_env(&sys);
        assert_eq!(env.sampled.len(), 2);
        assert_eq!(env.dbus_address.as_deref(), Some("unix:path=/tmp/dbus-AAA"));
        assert_eq!(env.display.as_deref(), Some(":11.0"));
        assert_eq!(env.all_dbus_addresses, vec!["unix:path=/tmp/dbus-AAA".to_string()]);
    }

    #[test]
    fn skips_other_uid_and_collects_all_addresses() {
        let sys = FakeSystem::new(0)
            .with_proc(10, 0, "root", "xfce4-session")
            .with_proc_environ(10, "DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/dbus-BBB")
            .with_proc(20, 1001, "beyondcy", "xfce4-session")
            .with_proc_environ(20, "DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/dbus-OTHER");
        let env = detect_session_env(&sys);
        assert_eq!(env.sampled.len(), 1);
        assert_eq!(env.dbus_address.as_deref(), Some("unix:path=/tmp/dbus-BBB"));
    }

    #[test]
    fn empty_when_no_desktop_procs() {
        let env = detect_session_env(&FakeSystem::new(0));
        assert!(env.sampled.is_empty());
        assert!(env.dbus_address.is_none());
        assert!(env.display.is_none());
    }

    #[test]
    fn majority_is_deterministic() {
        assert_eq!(majority(vec![]), None);
        assert_eq!(majority(vec!["a".into(), "b".into(), "a".into()]).as_deref(), Some("a"));
        assert_eq!(majority(vec!["b".into(), "a".into()]).as_deref(), Some("a"));
    }
}
