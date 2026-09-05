//! 路径集合:全部可注入,测试时用 `Paths::for_home(tempdir)` 收敛到临时目录,
//! 禁止单测触碰真实 HOME。

use std::path::{Path, PathBuf};

/// 诊断/修复涉及的全部路径。字段公开,便于测试与 GUI 注入。
#[derive(Debug, Clone)]
pub struct Paths {
    /// 用户 HOME(所有用户级路径的根)
    pub home: PathBuf,
    /// lyyIme 项目根(找 ibus-engine/ 组件源、data/runtime)。可为 None。
    pub project_dir: Option<PathBuf>,
    /// 安装前缀(系统级组件 XML / 引擎脚本所在;/usr/local/share)
    pub prefix_share: PathBuf,
    /// 系统级 ibus 组件目录(/usr/share/ibus/component)
    pub ibus_component_dir: PathBuf,
    /// ibus-table 码表目录(/usr/share/ibus-table/tables)
    pub ibus_table_db_dir: PathBuf,
    pub gtk3_im_dir: PathBuf,
    pub gtk3_im_cache: PathBuf,
    pub gtk2_im_dir: PathBuf,
    pub gtk2_im_cache: PathBuf,
    pub qt5_im_dir: Option<PathBuf>,
    pub qt6_im_dir: Option<PathBuf>,
    /// 海峰86 码表(dicttool reinstall-dict 用)
    pub wubi_db: PathBuf,
    /// dicttool 可执行文件显式覆盖(None=自动探测)
    pub dicttool: Option<PathBuf>,
}

impl Default for Paths {
    fn default() -> Self {
        Self::detect()
    }
}

impl Paths {
    /// 按本机真实环境推导(生产入口)。
    pub fn detect() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/root"));
        let qt5 = PathBuf::from("/usr/lib64/qt5/plugins/platforminputcontexts");
        let qt6 = PathBuf::from("/usr/lib64/qt6/plugins/platforminputcontexts");
        Paths {
            gtk3_im_dir: PathBuf::from("/usr/lib64/gtk-3.0/3.0.0/immodules"),
            gtk3_im_cache: PathBuf::from("/usr/lib64/gtk-3.0/3.0.0/immodules.cache"),
            gtk2_im_dir: PathBuf::from("/usr/lib64/gtk-2.0/2.10.0/immodules"),
            gtk2_im_cache: PathBuf::from("/usr/lib64/gtk-2.0/2.10.0/immodules.cache"),
            qt5_im_dir: qt5.exists().then_some(qt5),
            qt6_im_dir: qt6.exists().then_some(qt6),
            wubi_db: PathBuf::from("/usr/share/ibus-table/tables/wubi-haifeng86.db"),
            dicttool: None,
            prefix_share: PathBuf::from("/usr/local/share"),
            ibus_component_dir: PathBuf::from("/usr/share/ibus/component"),
            ibus_table_db_dir: PathBuf::from("/usr/share/ibus-table/tables"),
            project_dir: Self::detect_project_dir(),
            home,
        }
    }

    fn detect_project_dir() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("LYYIME_PROJECT_DIR") {
            return Some(PathBuf::from(p));
        }
        // 开发机兜底:以编译期 crate 路径向上找 workspace 根(存在 Cargo.toml 才采信)
        let cand = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../");
        let cand = cand.canonicalize().ok()?;
        cand.join("Cargo.toml").is_file().then_some(cand)
    }

    /// 测试构造:所有路径收敛到一个目录之下,绝不触碰真实 HOME。
    pub fn for_home(home: &Path) -> Self {
        let h = home.to_path_buf();
        Paths {
            gtk3_im_dir: h.join("gtk3/immodules"),
            gtk3_im_cache: h.join("gtk3/immodules.cache"),
            gtk2_im_dir: h.join("gtk2/immodules"),
            gtk2_im_cache: h.join("gtk2/immodules.cache"),
            qt5_im_dir: None,
            qt6_im_dir: None,
            wubi_db: h.join("usr/share/ibus-table/tables/wubi-haifeng86.db"),
            dicttool: None,
            prefix_share: h.join("prefix/share"),
            ibus_component_dir: h.join("usr/share/ibus/component"),
            ibus_table_db_dir: h.join("usr/share/ibus-table/tables"),
            project_dir: None,
            home: h,
        }
    }

    // ---- 派生路径(全部由 home / prefix_share / project_dir 推出)----

    pub fn user_data_dir(&self) -> PathBuf {
        self.home.join(".local/share/lyyime")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.user_data_dir().join("logs")
    }
    pub fn ime_manager_log(&self) -> PathBuf {
        self.logs_dir().join("ime-manager.log")
    }
    pub fn user_tsv(&self) -> PathBuf {
        self.user_data_dir().join("user.tsv")
    }
    pub fn user_config_dir(&self) -> PathBuf {
        self.home.join(".config/lyyime")
    }
    pub fn env_sh(&self) -> PathBuf {
        self.user_config_dir().join("env.sh")
    }
    pub fn xprofile(&self) -> PathBuf {
        self.home.join(".xprofile")
    }
    pub fn autostart_dir(&self) -> PathBuf {
        self.home.join(".config/autostart")
    }
    pub fn ibus_cache_dir(&self) -> PathBuf {
        self.home.join(".cache/ibus/bus")
    }
    /// 用户级 ibus 组件目录(免 root 注册引擎)
    pub fn user_component_dir(&self) -> PathBuf {
        self.home.join(".local/share/ibus/component")
    }
    /// 用户级 lyyime 引擎脚本目录
    pub fn user_engine_dir(&self) -> PathBuf {
        self.home.join(".local/share/lyyime/ibus/engine")
    }
    pub fn system_component_xml(&self) -> PathBuf {
        self.prefix_share.join("ibus/component/lyyime.xml")
    }
    pub fn system_engine_dir(&self) -> PathBuf {
        self.prefix_share.join("lyyime/ibus/engine")
    }
    /// 项目内组件源(M4 产出)
    pub fn project_component_xml(&self) -> Option<PathBuf> {
        self.project_dir
            .as_ref()
            .map(|p| p.join("ibus-engine/component/lyyime.xml"))
    }
    pub fn project_engine_py(&self) -> Option<PathBuf> {
        self.project_dir
            .as_ref()
            .map(|p| p.join("ibus-engine/engine/lyyime.py"))
    }
    /// dicttool 产物目录(项目 data/runtime)
    pub fn runtime_dir(&self) -> Option<PathBuf> {
        self.project_dir.as_ref().map(|p| p.join("data/runtime"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn for_home_keeps_everything_under_tempdir() {
        let t = TempDir::new("paths");
        let p = Paths::for_home(&t.path);
        // 全部派生路径必须在 tempdir 之内
        for derived in [
            p.user_data_dir(),
            p.logs_dir(),
            p.ime_manager_log(),
            p.user_tsv(),
            p.env_sh(),
            p.xprofile(),
            p.autostart_dir(),
            p.ibus_cache_dir(),
            p.user_component_dir(),
            p.user_engine_dir(),
            p.system_component_xml(),
            p.system_engine_dir(),
        ] {
            assert!(derived.starts_with(&t.path), "{derived:?} 不在 tempdir 内");
        }
        assert!(p.project_dir.is_none());
        assert!(p.runtime_dir().is_none());
    }

    #[test]
    fn project_dir_env_override() {
        let t = TempDir::new("proj");
        // 不直接改进程环境(并发测试不安全),仅验证 for_home 与 runtime_dir 逻辑
        let mut p = Paths::for_home(&t.path);
        p.project_dir = Some(t.path.join("proj"));
        assert_eq!(p.runtime_dir(), Some(t.path.join("proj/data/runtime")));
        assert_eq!(
            p.project_component_xml(),
            Some(t.path.join("proj/ibus-engine/component/lyyime.xml"))
        );
    }
}
