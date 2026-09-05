//! lyyime-doctor —— Linux 中文输入法环境诊断/修复 + 输入法管理。
//!
//! 分层:
//! - [`Doctor`]:check(结构化诊断)/ fix(幂等修复,支持 dry-run)
//! - [`ImeManager`]:ime-list / ime-add / ime-remove / ime-default(ARCHITECTURE.md §9.1)
//! - [`Paths`] + [`SystemOps`]:路径与系统副作用全部可注入,单测只碰 tempdir
//!
//! GUI(lyyime-app 工具菜单)直接使用本 lib;`lyyime-doctor` bin 是其 CLI 形态。

pub mod catalog;
pub mod checks;
pub mod fixes;
pub mod ime;
pub mod model;
pub mod paths;
pub mod system;

pub use ime::ImeManager;
pub use model::{
    ApplyOutcome, CheckReport, CheckResult, FixAction, ImeEntry, ImeKind, ImeOpResult, Status,
    Summary,
};
pub use paths::Paths;
pub use system::{ProcInfo, RealSystem, RunOutput, SystemOps};

/// 诊断 + 修复入口(GUI 与 CLI 共用)
pub struct Doctor {
    p: Paths,
    sys: Box<dyn SystemOps>,
}

impl Doctor {
    /// 生产构造
    pub fn new(paths: Paths) -> Self {
        Self::with_system(paths, Box::new(RealSystem))
    }

    /// 测试/GUI 注入系统桩
    pub fn with_system(paths: Paths, sys: Box<dyn SystemOps>) -> Self {
        Doctor { p: paths, sys }
    }

    pub fn paths(&self) -> &Paths {
        &self.p
    }

    /// 运行全部 8 项检查(顺序:id, daemon, engine-register, autostart, immodule, data, logs, locale)
    pub fn run_checks(&self) -> Vec<CheckResult> {
        checks::run_checks(&self.p, self.sys.as_ref())
    }

    /// 按显式 id 列表规划修复动作;id 可以是检查项 id(env/daemon/…)或修复 id(restart-ibus/…)。
    pub fn plan_fixes(&self, ids: &[String]) -> Vec<FixAction> {
        fixes::plan_fixes(&self.p, self.sys.as_ref(), ids)
    }

    /// 按当前检查结果规划(--all):仅对 warn/fail 且有对应修复的项生成动作。
    pub fn plan_fixes_all(&self) -> Vec<FixAction> {
        fixes::plan_fixes_from_results(&self.run_checks())
    }

    /// 执行修复动作;`dry_run=true` 只描述将做什么,不落盘、不杀进程。
    pub fn apply(&self, action: &FixAction, dry_run: bool) -> anyhow::Result<ApplyOutcome> {
        fixes::apply(&self.p, self.sys.as_ref(), action, dry_run)
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! 无 tempfile 依赖的最小临时目录助手(单测专用,绝不指向真实 HOME)。

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    pub struct TempDir {
        pub path: PathBuf,
    }

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let n = SEQ.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "lyyime-doctor-test-{}-{tag}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("创建临时目录失败");
            TempDir { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// 创建父目录并写入文件
    pub fn temp_write(path: &Path, content: &str) {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).expect("创建父目录失败");
        }
        std::fs::write(path, content).expect("写文件失败");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fake::FakeSystem;
    use crate::testutil::TempDir;

    #[test]
    fn doctor_end_to_end_with_stubs() {
        let t = TempDir::new("doctor");
        let p = Paths::for_home(&t.path);
        let sys = FakeSystem::new(0)
            .with_env("XMODIFIERS", "@im=ibus")
            .with_env("GTK_IM_MODULE", "ibus")
            .with_env("QT_IM_MODULE", "ibus")
            .with_proc(42, 0, "root", "ibus-daemon -drx");
        let d = Doctor::with_system(p, Box::new(sys));
        let results = d.run_checks();
        assert_eq!(results.len(), 8);
        let expected = [
            (Status::Ok, "env"),
            (Status::Ok, "daemon"),
            (Status::Fail, "engine-register"), // 未安装组件
            (Status::Fail, "autostart"),       // 无 autostart 目录
            (Status::Fail, "immodule"),        // 无 GTK3 目录
            (Status::Warn, "data"),            // 无 meta.json
            (Status::Ok, "logs"),
            (Status::Fail, "locale"),          // 未设 LANG
        ];
        for (r, (st, id)) in results.iter().zip(expected) {
            assert_eq!(r.id, id);
            assert_eq!(r.status, st, "{id}: {:?} / {}", r.status, r.detail);
        }

        // plan + dry-run apply
        let ids = vec!["env".to_string()];
        let actions = d.plan_fixes(&ids);
        assert_eq!(actions.len(), 1);
        let out = d.apply(&actions[0], true).unwrap();
        assert!(matches!(out, ApplyOutcome::DryRun(_)));
        assert!(!d.paths().env_sh().exists());
    }

    #[test]
    fn doctor_json_report_serializes() {
        let t = TempDir::new("doctorjson");
        let d = Doctor::with_system(Paths::for_home(&t.path), Box::new(FakeSystem::new(0)));
        let report = CheckReport::new(d.run_checks());
        let s = serde_json::to_string(&report).unwrap();
        assert!(s.contains("\"checks\""));
        assert!(s.contains("\"status\":\"fail\""));
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["checks"].as_array().unwrap().len(), 8);
    }
}
