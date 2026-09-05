//! `ImeManager`:输入法枚举/安装/卸载/设默认(ARCHITECTURE.md §9.1)。
//!
//! 安全模型:所有写操作默认 dry-run(仅打印命令);实际执行必须 `--yes`;
//! 包操作仅 root;remove 有保护规则;全程写 ~/.local/share/lyyime/logs/ime-manager.log。

use crate::catalog;
use crate::model::{ImeEntry, ImeKind, ImeOpResult};
use crate::paths::Paths;
use crate::system::SystemOps;
use anyhow::anyhow;

pub struct ImeManager {
    p: Paths,
    sys: Box<dyn SystemOps>,
}

/// ibus 配置读写通道(gsettings 优先,dconf 兜底)
struct IbusConfig {
    use_gsettings: bool,
    /// gsettings schema(org.freedesktop.ibus.general / desktop.ibus.general)
    schema: String,
    /// dconf 基路径
    base: String,
}

impl IbusConfig {
    fn describe(&self) -> String {
        if self.use_gsettings {
            format!("gsettings {schema}", schema = self.schema)
        } else {
            format!("dconf {base}", base = self.base)
        }
    }

    fn get_list(&self, sys: &dyn SystemOps, key: &str) -> Option<Vec<String>> {
        let out = if self.use_gsettings {
            sys.run("gsettings", &["get", &self.schema, key]).ok()?
        } else {
            sys.run("dconf", &["read", &format!("{}/{}", self.base, key)]).ok()?
        };
        if out.success && out.stdout.contains('[') {
            Some(parse_gvariant_list(&out.stdout))
        } else {
            None
        }
    }

    fn set_list(&self, sys: &dyn SystemOps, key: &str, list: &[String]) -> Result<(), String> {
        let val = fmt_gvariant_list(list);
        let r = if self.use_gsettings {
            sys.run("gsettings", &["set", &self.schema, key, &val])
        } else {
            sys.run("dconf", &["write", &format!("{}/{}", self.base, key), &val])
        };
        match r {
            Ok(o) if o.success => Ok(()),
            Ok(o) => Err(format!("{} 失败:{}", self.describe(), o.stderr.trim())),
            Err(e) => Err(format!("{} 执行失败:{e}", self.describe())),
        }
    }
}

/// 结构化操作(渲染成人读命令 + 实际执行共用一份定义,避免两处漂移)
enum Op {
    Dnf { remove: bool, pkgs: Vec<String> },
    GsSet { cfg: IbusConfig, key: String, list: Vec<String> },
    WriteCache,
    RmFile(String),
}

impl Op {
    fn render(&self) -> String {
        match self {
            Op::Dnf { remove, pkgs } => format!(
                "dnf {} -y {}",
                if *remove { "remove" } else { "install" },
                pkgs.join(" ")
            ),
            Op::GsSet { cfg, key, list } => {
                if cfg.use_gsettings {
                    format!("gsettings set {} {} {}", cfg.schema, key, fmt_gvariant_list(list))
                } else {
                    format!("dconf write {}/{} {}", cfg.base, key, fmt_gvariant_list(list))
                }
            }
            Op::WriteCache => "ibus write-cache".into(),
            Op::RmFile(f) => format!("rm -f {f}"),
        }
    }
}

impl ImeManager {
    pub fn new(p: Paths) -> Self {
        Self::with_system(p, Box::new(crate::system::RealSystem))
    }
    pub fn with_system(p: Paths, sys: Box<dyn SystemOps>) -> Self {
        ImeManager { p, sys }
    }

    // ------------------------------------------------------------------
    // ime-list
    // ------------------------------------------------------------------
    pub fn ime_list(&self) -> anyhow::Result<Vec<ImeEntry>> {
        let pkgs = self.installed_packages();
        let cfg = self.load_ibus_config();
        let preload: Vec<String> = cfg
            .as_ref()
            .and_then(|c| c.get_list(self.sys.as_ref(), "preload-engines"))
            .unwrap_or_default();
        let is_def = |id: &str| preload.first().map(|f| f == id).unwrap_or(false);
        let act = |id: &str| preload.iter().any(|e| e == id);

        let mut map: std::collections::BTreeMap<String, ImeEntry> = std::collections::BTreeMap::new();

        // ① lyyime 自身(双模式算一个条目)
        let lyy_installed = [
            self.p.system_component_xml(),
            self.p.user_component_dir().join("lyyime.xml"),
        ]
        .iter()
        .any(|f| f.is_file());
        map.insert(
            "lyyime".into(),
            ImeEntry {
                id: "lyyime".into(),
                name: "lyyIme 五笔/拼音混合(自研,双模式)".into(),
                kind: ImeKind::Lyyime,
                installed: lyy_installed,
                active: act("lyyime"),
                is_default: is_def("lyyime"),
                package: "(自研,无系统包)".into(),
                language: "zh_CN".into(),
            },
        );

        // ② ibus 组件 XML → 引擎(用户级优先于系统级)
        let component_dirs = [
            self.p.user_component_dir(),
            self.p.prefix_share.join("ibus/component"),
            self.p.ibus_component_dir.clone(),
        ];
        for dir in &component_dirs {
            let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|f| f.extension().map(|x| x == "xml").unwrap_or(false))
                        .collect()
                })
                .unwrap_or_default();
            files.sort();
            for f in files {
                let content = std::fs::read_to_string(&f).unwrap_or_default();
                for eng in parse_component_engines(&content) {
                    if eng.name.is_empty() || map.contains_key(&eng.name) {
                        continue;
                    }
                    let package =
                        catalog::package_for_engine(&eng.name).unwrap_or("").to_string();
                    let (active, is_default) = (act(&eng.name), is_def(&eng.name));
                    let display_name = if eng.longname.is_empty() {
                        if eng.description.is_empty() { eng.name.clone() } else { eng.description }
                    } else {
                        eng.longname
                    };
                    map.insert(
                        eng.name.clone(),
                        ImeEntry {
                            name: display_name,
                            id: eng.name,
                            kind: ImeKind::IbusEngine,
                            installed: true,
                            active,
                            is_default,
                            package,
                            language: eng.language,
                        },
                    );
                }
            }
        }

        // ③ ibus-table 动态引擎(component XML 里只有 <engines exec=…/>,需枚举 .db)
        if let Ok(rd) = std::fs::read_dir(&self.p.ibus_table_db_dir) {
            let mut dbs: Vec<std::path::PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|f| f.extension().map(|x| x == "db").unwrap_or(false))
                .collect();
            dbs.sort();
            for db in dbs {
                let stem = db.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if stem.is_empty() {
                    continue;
                }
                let id = format!("table:{stem}");
                if map.contains_key(&id) {
                    continue;
                }
                let package = catalog::package_for_engine(&id)
                    .unwrap_or("ibus-table-chinese")
                    .to_string();
                map.insert(
                    id.clone(),
                    ImeEntry {
                        name: format!("{stem}(ibus 码表)"),
                        id,
                        kind: ImeKind::IbusEngine,
                        installed: true,
                        active: act(&format!("table:{stem}")),
                        is_default: is_def(&format!("table:{stem}")),
                        package,
                        language: "zh_CN".into(),
                    },
                );
            }
        }

        // ④ fcitx5(按包探测)
        let fcitx_pkgs: Vec<String> =
            pkgs.iter().filter(|x| x.starts_with("fcitx5")).cloned().collect();
        let fcitx_active = !self.sys.find_processes("fcitx5").is_empty()
            || self
                .sys
                .env("GTK_IM_MODULE")
                .map(|v| v.contains("fcitx"))
                .unwrap_or(false);
        map.insert(
            "fcitx5".into(),
            ImeEntry {
                id: "fcitx5".into(),
                name: "Fcitx 5 框架".into(),
                kind: ImeKind::Fcitx5,
                installed: !fcitx_pkgs.is_empty(),
                active: fcitx_active,
                is_default: false,
                package: fcitx_pkgs.join(", "),
                language: "zh_CN".into(),
            },
        );

        // ⑤ ibus 框架本体
        let ibus_up = !self.sys.find_processes("ibus-daemon").is_empty();
        map.insert(
            "ibus".into(),
            ImeEntry {
                id: "ibus".into(),
                name: "IBus 输入法框架".into(),
                kind: ImeKind::Other,
                installed: pkgs.iter().any(|x| x == "ibus"),
                active: ibus_up,
                is_default: false,
                package: "ibus".into(),
                language: String::new(),
            },
        );

        let mut v: Vec<ImeEntry> = map.into_values().collect();
        v.sort_by(|a, b| a.kind.rank().cmp(&b.kind.rank()).then_with(|| a.id.cmp(&b.id)));
        Ok(v)
    }

    // ------------------------------------------------------------------
    // ime-add
    // ------------------------------------------------------------------
    pub fn ime_add(&self, id: &str, yes: bool) -> anyhow::Result<ImeOpResult> {
        let cat = catalog::find(id).ok_or_else(|| {
            anyhow!(
                "内置清单中没有输入法「{}」。可用:{}",
                id,
                catalog::ids().join(", ")
            )
        })?;
        let mut warnings: Vec<String> = vec![];
        let mut ops: Vec<Op> = vec![Op::Dnf { remove: false, pkgs: cat.packages.iter().map(|s| s.to_string()).collect() }];
        let mut plan: Option<(IbusConfig, Vec<String>)> = None;
        if !cat.engines.is_empty() {
            if let Some(cfg) = self.load_ibus_config() {
                let mut nl: Vec<String> = cat.engines.iter().map(|s| s.to_string()).collect();
                if let Some(cur) = cfg.get_list(self.sys.as_ref(), "preload-engines") {
                    for e in cur {
                        if !nl.contains(&e) {
                            nl.push(e);
                        }
                    }
                }
                ops.push(Op::GsSet { cfg: clone_cfg(&cfg), key: "preload-engines".into(), list: nl.clone() });
                ops.push(Op::GsSet { cfg: clone_cfg(&cfg), key: "engines-order".into(), list: nl.clone() });
                plan = Some((cfg, nl));
            } else {
                warnings.push(
                    "未检测到可用的 gsettings/dconf;安装后请在图形会话内运行 ibus-setup 手动添加引擎"
                        .into(),
                );
            }
        }
        ops.push(Op::WriteCache);
        if cat.kind == ImeKind::Fcitx5 {
            warnings.push(
                "fcitx5 与 ibus 共用环境变量;安装后请运行 `lyyime-doctor fix --issue env` 并注销重登".into(),
            );
        }
        let commands: Vec<String> = ops.iter().map(|o| o.render()).collect();
        if !yes {
            let log_path = self.log_op("add", id, false, &commands, &warnings);
            return Ok(preview("add", id, commands, warnings, log_path));
        }
        if self.sys.current_uid() != 0 {
            anyhow::bail!("安装软件包需要 root 权限;请用 sudo 重跑,或去掉 --yes 先预览命令");
        }
        let before = self.list_ids()?;
        let install_pkgs: Vec<&str> = cat.packages.to_vec();
        let mut args: Vec<&str> = vec!["install", "-y"];
        args.extend(install_pkgs.iter().copied());
        let out = self.sys.run("dnf", &args)?;
        if !out.success {
            anyhow::bail!("dnf install 失败:\n{}", truncate(&out.stderr, 800));
        }
        if let Some((cfg, nl)) = &plan {
            if let Err(e) = cfg.set_list(self.sys.as_ref(), "preload-engines", nl) {
                warnings.push(e);
            }
            if let Err(e) = cfg.set_list(self.sys.as_ref(), "engines-order", nl) {
                warnings.push(e);
            }
        }
        self.write_cache(&mut warnings);
        warnings.push("如托盘未出现新引擎,请重启 ibus:lyyime-doctor fix --issue restart-ibus".into());
        let after = self.list_ids().ok();
        let log_path = self.log_op("add", id, true, &commands, &warnings);
        Ok(ImeOpResult {
            action: "add".into(),
            target: id.into(),
            executed: true,
            commands,
            before,
            after,
            warnings,
            manual_hint: None,
            log_path,
        })
    }

    // ------------------------------------------------------------------
    // ime-remove
    // ------------------------------------------------------------------
    pub fn ime_remove(&self, id: &str, force: bool, yes: bool) -> anyhow::Result<ImeOpResult> {
        let entries = self.ime_list()?;
        let target = entries.iter().find(|e| e.id == id).ok_or_else(|| {
            anyhow!(
                "未发现输入法「{}」。当前:{};内置目录:{}",
                id,
                entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", "),
                catalog::ids().join(", ")
            )
        })?;
        let t = target.clone();

        // 保护规则 1:lyyime 仍是默认引擎
        if t.kind == ImeKind::Lyyime && t.is_default && !force {
            anyhow::bail!(
                "「lyyime」当前是默认输入引擎,拒绝移除;请先 `ime-default <其它引擎>` 切换,或加 --force"
            );
        }
        // 保护规则 2:会话中唯一可用的中文输入法
        let chinese_active: Vec<&ImeEntry> = entries
            .iter()
            .filter(|e| e.active && e.kind != ImeKind::Other && is_chinese_entry(e))
            .collect();
        if t.kind != ImeKind::Other
            && is_chinese_entry(&t)
            && t.active
            && chinese_active.len() <= 1
            && !force
        {
            anyhow::bail!(
                "「{}」是会话中唯一可用的中文输入法,移除后将无法输入中文;确认请加 --force",
                t.id
            );
        }

        let mut warnings: Vec<String> = vec![];
        let mut ops: Vec<Op> = vec![];
        let pkgs = self.installed_packages();
        match t.kind {
            ImeKind::Lyyime => {
                let ux = self.p.user_component_dir().join("lyyime.xml");
                ops.push(Op::RmFile(ux.display().to_string()));
                ops.push(Op::WriteCache);
            }
            ImeKind::Fcitx5 => {
                let mine: Vec<String> =
                    pkgs.iter().filter(|x| x.starts_with("fcitx5")).cloned().collect();
                if mine.is_empty() {
                    warnings.push("未发现 fcitx5 系统包,仅需从预载列表注销".into());
                } else {
                    ops.push(Op::Dnf { remove: true, pkgs: mine });
                }
                warnings.push("卸载 fcitx5 后请确认环境变量已指回 ibus,并注销重登".into());
            }
            ImeKind::IbusEngine => {
                let cat_pkgs: Vec<&str> =
                    catalog::find(&t.id).map(|c| c.packages.to_vec()).unwrap_or_default();
                let installed: Vec<&str> = cat_pkgs
                    .into_iter()
                    .filter(|x| pkgs.iter().any(|p| p == x))
                    .collect();
                if installed.is_empty() {
                    warnings.push(format!("未找到「{}」对应的已安装包,仅从预载列表注销", t.id));
                } else {
                    ops.push(Op::Dnf { remove: true, pkgs: installed.iter().map(|s| s.to_string()).collect() });
                }
            }
            ImeKind::Other => {
                warnings.push("框架组件不做卸载,仅从预载列表注销".into());
            }
        }
        // 从 ibus 预载列表移除
        if let Some(cfg) = self.load_ibus_config() {
            if let Some(cur) = cfg.get_list(self.sys.as_ref(), "preload-engines") {
                if cur.iter().any(|x| x == &t.id) {
                    let nl: Vec<String> = cur.into_iter().filter(|x| x != &t.id).collect();
                    ops.push(Op::GsSet { cfg: clone_cfg(&cfg), key: "preload-engines".into(), list: nl.clone() });
                    ops.push(Op::GsSet { cfg: clone_cfg(&cfg), key: "engines-order".into(), list: nl });
                }
            }
        }
        let commands: Vec<String> = ops.iter().map(|o| o.render()).collect();
        if !yes {
            let log_path = self.log_op("remove", id, false, &commands, &warnings);
            return Ok(preview("remove", id, commands, warnings, log_path));
        }
        let before = self.list_ids()?;
        self.exec_ops(&ops, &mut warnings)?;
        let after = self.list_ids().ok();
        let log_path = self.log_op("remove", id, true, &commands, &warnings);
        Ok(ImeOpResult {
            action: "remove".into(),
            target: id.into(),
            executed: true,
            commands,
            before,
            after,
            warnings,
            manual_hint: None,
            log_path,
        })
    }

    // ------------------------------------------------------------------
    // ime-default
    // ------------------------------------------------------------------
    pub fn ime_default(&self, id: &str, yes: bool) -> anyhow::Result<ImeOpResult> {
        let engine = match catalog::find(id) {
            Some(c) if !c.engines.is_empty() => c.engines[0].to_string(),
            _ => id.to_string(),
        };
        let mut warnings: Vec<String> = vec![];
        let mut manual_hint: Option<String> = None;

        if engine == "fcitx5" || id == "fcitx5" {
            manual_hint = Some(
                "fcitx5 不走 ibus 预载列表:请编辑 ~/.config/fcitx5/profile 把目标输入法放到 Group 第一项,并注销重登"
                    .into(),
            );
            let log_path = self.log_op("default", id, false, &[], &warnings);
            return Ok(ImeOpResult {
                action: "default".into(),
                target: id.into(),
                executed: false,
                commands: vec![],
                before: vec![],
                after: None,
                warnings,
                manual_hint,
                log_path,
            });
        }

        // 引擎存在性提醒
        match self.ime_list()?.into_iter().find(|e| e.id == engine) {
            Some(e) if !e.installed => {
                warnings.push(format!("「{engine}」当前未安装;建议先 `ime-add {}`", id));
            }
            None => {
                warnings.push(format!(
                    "系统中未发现引擎「{engine}」;仍会写入预载列表(未安装则不生效)"
                ));
            }
            _ => {}
        }

        let Some(cfg) = self.load_ibus_config() else {
            warnings.push("gsettings/dconf 均不可用,降级为手动指引".into());
            manual_hint = Some(format!(
                "在图形会话内执行:gsettings set org.freedesktop.ibus.general preload-engines \"{example}\";或在 ibus-setup 的输入法列表中把「{engine}」移到首位",
                example = fmt_gvariant_list(&[engine.clone()])
            ));
            let log_path = self.log_op("default", id, false, &[], &warnings);
            return Ok(ImeOpResult {
                action: "default".into(),
                target: id.into(),
                executed: false,
                commands: vec![],
                before: vec![],
                after: None,
                warnings,
                manual_hint,
                log_path,
            });
        };

        let mut nl: Vec<String> = vec![engine.clone()];
        if let Some(cur) = cfg.get_list(self.sys.as_ref(), "preload-engines") {
            for e in cur {
                if e != engine && !nl.contains(&e) {
                    nl.push(e);
                }
            }
        }
        let ops = vec![
            Op::GsSet { cfg: clone_cfg(&cfg), key: "preload-engines".into(), list: nl.clone() },
            Op::GsSet { cfg: clone_cfg(&cfg), key: "engines-order".into(), list: nl.clone() },
            Op::WriteCache,
        ];
        let commands: Vec<String> = ops.iter().map(|o| o.render()).collect();
        if !yes {
            let log_path = self.log_op("default", id, false, &commands, &warnings);
            return Ok(preview("default", id, commands, warnings, log_path));
        }
        let before = self.list_ids()?;
        let mut set_failed = false;
        if let Err(e) = cfg.set_list(self.sys.as_ref(), "preload-engines", &nl) {
            warnings.push(e);
            set_failed = true;
        }
        if let Err(e) = cfg.set_list(self.sys.as_ref(), "engines-order", &nl) {
            warnings.push(e);
            set_failed = true;
        }
        self.write_cache(&mut warnings);
        if set_failed {
            manual_hint = Some(format!(
                "写入失败通常因为当前终端不在图形会话(D-Bus)内;请在桌面会话的终端里执行:{};或运行 ibus-setup",
                commands[..2].join(" && ")
            ));
        }
        let after = self.list_ids().ok();
        let log_path = self.log_op("default", id, true, &commands, &warnings);
        Ok(ImeOpResult {
            action: "default".into(),
            target: id.into(),
            executed: true,
            commands,
            before,
            after,
            warnings,
            manual_hint,
            log_path,
        })
    }

    // ------------------------------------------------------------------
    // 内部工具
    // ------------------------------------------------------------------

    fn sys_ref(&self) -> &dyn SystemOps {
        self.sys.as_ref()
    }

    fn list_ids(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.ime_list()?.into_iter().map(|e| e.id).collect())
    }

    fn installed_packages(&self) -> Vec<String> {
        match self.sys.run("rpm", &["-qa", "--qf", "%{NAME}\n"]) {
            Ok(o) if o.success => o.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
            _ => vec![],
        }
    }

    /// 探测可用的 ibus 配置通道:gsettings(schema 自动识别)→ dconf
    fn load_ibus_config(&self) -> Option<IbusConfig> {
        let sys = self.sys_ref();
        if sys.command_exists("gsettings") {
            for schema in ["org.freedesktop.ibus.general", "desktop.ibus.general"] {
                let cfg = IbusConfig { use_gsettings: true, schema: schema.into(), base: String::new() };
                if cfg.get_list(sys, "preload-engines").is_some() {
                    return Some(cfg);
                }
            }
        }
        if sys.command_exists("dconf") {
            let cfg = IbusConfig { use_gsettings: false, schema: String::new(), base: "/desktop/ibus/general".into() };
            if cfg.get_list(sys, "preload-engines").is_some() {
                return Some(cfg);
            }
        }
        None
    }

    fn write_cache(&self, warnings: &mut Vec<String>) {
        if self.sys.command_exists("ibus") {
            match self.sys.run("ibus", &["write-cache"]) {
                Ok(o) if o.success => {}
                Ok(o) => warnings.push(format!("ibus write-cache 失败:{}", o.stderr.trim())),
                Err(e) => warnings.push(format!("ibus write-cache 执行失败:{e}")),
            }
        } else {
            warnings.push("未找到 ibus 命令,跳过 write-cache".into());
        }
    }

    fn exec_ops(&self, ops: &[Op], warnings: &mut Vec<String>) -> anyhow::Result<()> {
        for op in ops {
            match op {
                Op::Dnf { remove, pkgs } => {
                    if self.sys.current_uid() != 0 {
                        anyhow::bail!("包操作需要 root 权限;请用 sudo 重跑,或去掉 --yes 先预览命令");
                    }
                    let mut args: Vec<&str> =
                        vec![if *remove { "remove" } else { "install" }, "-y"];
                    args.extend(pkgs.iter().map(|s| s.as_str()));
                    let out = self.sys.run("dnf", &args)?;
                    if !out.success {
                        anyhow::bail!("dnf 失败:\n{}", truncate(&out.stderr, 800));
                    }
                }
                Op::GsSet { cfg, key, list } => {
                    if let Err(e) = cfg.set_list(self.sys_ref(), key, list) {
                        warnings.push(e);
                    }
                }
                Op::WriteCache => self.write_cache(warnings),
                Op::RmFile(f) => {
                    if let Err(e) = std::fs::remove_file(f) {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            warnings.push(format!("删除 {f} 失败:{e}"));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn log_op(
        &self,
        action: &str,
        target: &str,
        executed: bool,
        commands: &[String],
        warnings: &[String],
    ) -> Option<String> {
        let log = self.p.ime_manager_log();
        if let Some(d) = log.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&log).ok()?;
        let _ = writeln!(f, "---- {} ime-{action} {target} executed={executed} ----", timestamp_now());
        for c in commands {
            let _ = writeln!(f, "  CMD: {c}");
        }
        for w in warnings {
            let _ = writeln!(f, "  WARN: {w}");
        }
        Some(log.display().to_string())
    }
}

// ---------------------------------------------------------------------------
// 纯函数助手
// ---------------------------------------------------------------------------

fn clone_cfg(c: &IbusConfig) -> IbusConfig {
    IbusConfig { use_gsettings: c.use_gsettings, schema: c.schema.clone(), base: c.base.clone() }
}

fn preview(action: &str, target: &str, commands: Vec<String>, warnings: Vec<String>, log_path: Option<String>) -> ImeOpResult {
    ImeOpResult {
        action: action.into(),
        target: target.into(),
        executed: false,
        commands,
        before: vec![],
        after: None,
        warnings,
        manual_hint: None,
        log_path,
    }
}

fn is_chinese_entry(e: &ImeEntry) -> bool {
    e.language.starts_with("zh")
        || catalog::find(&e.id).map(|c| c.chinese).unwrap_or(false)
}

/// "['a', 'b']" / "@as []" → ["a","b"]
pub(crate) fn parse_gvariant_list(s: &str) -> Vec<String> {
    let s = s.trim();
    let (a, b) = match (s.find('['), s.rfind(']')) {
        (Some(a), Some(b)) if b > a => (a, b),
        _ => return vec![],
    };
    s[a + 1..b]
        .split(',')
        .map(|t| {
            t.trim()
                .trim_matches(|c| c == '\'' || c == '"')
                .to_string()
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// ["a","b"] → "['a', 'b']"
pub(crate) fn fmt_gvariant_list(items: &[String]) -> String {
    let inner: Vec<String> =
        items.iter().map(|s| format!("'{}'", s.replace('\'', ""))).collect();
    format!("[{}]", inner.join(", "))
}

fn truncate(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

/// UTC 时间戳(无 chrono 依赖,Hinnant civil 算法)
pub(crate) fn timestamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02} UTC")
}

// ---------------------------------------------------------------------------
// component XML 解析(手写,无 xml 依赖)
// ---------------------------------------------------------------------------

/// 从 component XML 提取引擎列表
pub(crate) struct EngineXml {
    pub name: String,
    pub longname: String,
    pub description: String,
    pub language: String,
}

pub(crate) fn parse_component_engines(content: &str) -> Vec<EngineXml> {
    let mut out = vec![];
    let mut rest = content;
    while let Some(pos) = rest.find("<engine>") {
        let after = &rest[pos..];
        let Some(end) = after.find("</engine>") else { break };
        let block = &after[..end];
        out.push(EngineXml {
            name: xml_tag(block, "name").unwrap_or_default(),
            longname: xml_tag(block, "longname").unwrap_or_default(),
            description: xml_tag(block, "description").unwrap_or_default(),
            language: xml_tag(block, "language").unwrap_or_default(),
        });
        rest = &after[end + "</engine>".len()..];
    }
    out
}

fn xml_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = block.find(&open)? + open.len();
    let e = s + block[s..].find(&close)?;
    Some(unescape_xml(block[s..e].trim()))
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let Some(semi) = tail.find(';') else {
            out.push('&');
            rest = &rest[pos + 1..];
            continue;
        };
        let ent = &tail[1..semi];
        let decoded: Option<char> = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => {
                u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)
            }
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[pos + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// 测试(全 stub + tempdir,不真装包)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fake::FakeSystem;
    use crate::system::RunOutput;
    use crate::testutil::{temp_write, TempDir};

    /// 本机同构 stub:rpm 包列表 + org.freedesktop.ibus.general 预载列表
    fn real_like_sys(uid: u32, preload: &str) -> FakeSystem {
        FakeSystem::new(uid)
            .with_cmd(
                "rpm",
                &["-qa", "--qf", "%{NAME}\n"],
                RunOutput::ok(
                    "ibus\nibus-libpinyin\nibus-table\nibus-table-chinese\nibus-table-chinese-wubi-haifeng\nibus-table-chinese-wubi-jidian\n",
                ),
            )
            .with_cmd(
                "gsettings",
                &["get", "org.freedesktop.ibus.general", "preload-engines"],
                RunOutput::ok(preload),
            )
            .with_exists("gsettings")
            .with_exists("ibus")
            .with_proc(100, uid, "root", "ibus-daemon -drx")
    }

    fn setup_machine(t: &TempDir) -> Paths {
        let mut p = Paths::for_home(&t.path);
        p.project_dir = None;
        // 系统 component XML(libpinyin 两个引擎)
        temp_write(
            &p.ibus_component_dir.join("libpinyin.xml"),
            "<?xml version='1.0'?>\n<component>\n<engines>\n<engine>\n<name>libpinyin</name>\n<language>zh_CN</language>\n<longname>Intelligent Pinyin</longname>\n<description>Intelligent Pinyin input method</description>\n<symbol>&#x62FC;</symbol>\n</engine>\n<engine>\n<name>libbopomofo</name>\n<language>zh_TW</language>\n<longname>Bopomofo</longname>\n</engine>\n</engines>\n</component>",
        );
        // simple.xml 同构:xkb 引擎
        temp_write(
            &p.ibus_component_dir.join("simple.xml"),
            "<component><engines><engine><name>xkb:us::eng</name><language>en</language><longname>English (US)</longname></engine></engines></component>",
        );
        // ibus-table 动态引擎
        temp_write(&p.ibus_table_db_dir.join("wubi-haifeng86.db"), "sqlite");
        temp_write(&p.ibus_table_db_dir.join("wubi-jidian86.db"), "sqlite");
        p
    }

    #[test]
    fn gvariant_list_roundtrip() {
        assert_eq!(parse_gvariant_list("['a', 'b']"), vec!["a", "b"]);
        assert_eq!(parse_gvariant_list("@as []"), Vec::<String>::new());
        assert_eq!(parse_gvariant_list("['xkb:us::eng']"), vec!["xkb:us::eng"]);
        let items = vec!["table:wubi-haifeng86".to_string(), "libpinyin".to_string()];
        assert_eq!(parse_gvariant_list(&fmt_gvariant_list(&items)), items);
    }

    #[test]
    fn xml_parse_engines_and_entities() {
        let content = "<component><engines><engine><name>a</name><longname>A IM</longname><language>zh_CN</language><symbol>&#x62FC;</symbol></engine><engine><name>b</name><description>B &amp; C</description></engine></engines></component>";
        let engines = parse_component_engines(content);
        assert_eq!(engines.len(), 2);
        assert_eq!(engines[0].name, "a");
        assert_eq!(engines[0].longname, "A IM");
        assert_eq!(engines[0].language, "zh_CN");
        assert_eq!(engines[1].description, "B & C");
    }

    #[test]
    fn ime_list_enumerates_engines_tables_packages_and_default() {
        let t = TempDir::new("ime1");
        let p = setup_machine(&t);
        let sys = real_like_sys(0, "['xkb:us::eng', 'table:wubi-jidian86', 'table:wubi-haifeng86']");
        let m = ImeManager::with_system(p, Box::new(sys));
        let list = m.ime_list().unwrap();
        let get = |id: &str| list.iter().find(|e| e.id == id).cloned().unwrap_or_else(|| panic!("缺少 {id}: {list:?}"));

        let haifeng = get("table:wubi-haifeng86");
        assert_eq!(haifeng.kind, ImeKind::IbusEngine);
        assert!(haifeng.installed && haifeng.active && !haifeng.is_default);
        assert_eq!(haifeng.package, "ibus-table-chinese-wubi-haifeng");

        let lp = get("libpinyin");
        assert_eq!(lp.name, "Intelligent Pinyin");
        assert!(lp.installed && !lp.active);
        assert_eq!(lp.package, "ibus-libpinyin");

        let xkb = get("xkb:us::eng");
        assert!(xkb.is_default, "preload 首位应为默认");

        let lyy = get("lyyime");
        assert_eq!(lyy.kind, ImeKind::Lyyime);
        assert!(!lyy.installed, "未装组件时应为 false");

        let fw = get("fcitx5");
        assert_eq!(fw.kind, ImeKind::Fcitx5);
        assert!(!fw.installed);

        let ibus = get("ibus");
        assert_eq!(ibus.kind, ImeKind::Other);
        assert!(ibus.installed && ibus.active);

        // 排序:lyyime 在最前
        assert_eq!(list[0].id, "lyyime");
    }

    #[test]
    fn ime_add_dry_run_prints_commands_without_side_effects() {
        let t = TempDir::new("ime2");
        let p = setup_machine(&t);
        let sys = real_like_sys(0, "['xkb:us::eng']");
        let m = ImeManager::with_system(p.clone(), Box::new(sys));
        let r = m.ime_add("rime", false).unwrap();
        assert!(!r.executed);
        assert_eq!(r.commands[0], "dnf install -y ibus-rime");
        assert!(r.commands.iter().any(|c| c.contains("gsettings set org.freedesktop.ibus.general preload-engines ['rime', 'xkb:us::eng']")));
        assert!(r.commands.iter().any(|c| c == "ibus write-cache"));
        assert!(p.ime_manager_log().is_file(), "dry-run 也要留审计日志");
    }

    #[test]
    fn ime_add_unknown_id_and_non_root_are_rejected() {
        let t = TempDir::new("ime3");
        let p = setup_machine(&t);
        let m = ImeManager::with_system(p.clone(), Box::new(real_like_sys(0, "[]")));
        assert!(m.ime_add("sogou", false).is_err());

        let m2 = ImeManager::with_system(p, Box::new(real_like_sys(1000, "[]")));
        let err = m2.ime_add("rime", true).unwrap_err().to_string();
        assert!(err.contains("root"), "{err}");
    }

    #[test]
    fn ime_remove_protects_default_lyyime() {
        let t = TempDir::new("ime4");
        let p = setup_machine(&t);
        temp_write(&p.user_component_dir().join("lyyime.xml"), "<component/>");
        let xml = p.user_component_dir().join("lyyime.xml");
        let m = ImeManager::with_system(p, Box::new(real_like_sys(0, "['lyyime']")));
        let err = m.ime_remove("lyyime", false, true).unwrap_err().to_string();
        assert!(err.contains("默认输入引擎"), "{err}");
        // --force 放行(此处无包,只是注销+删 XML)
        let r = m.ime_remove("lyyime", true, true).unwrap();
        assert!(r.executed);
        assert!(!xml.exists());
    }

    #[test]
    fn ime_remove_protects_last_chinese_ime() {
        let t = TempDir::new("ime5");
        let p = setup_machine(&t);
        // 预载里只有 libpinyin 一个中文输入法
        let sys = real_like_sys(0, "['libpinyin', 'xkb:us::eng']");
        let m = ImeManager::with_system(p, Box::new(sys));
        let err = m.ime_remove("libpinyin", false, true).unwrap_err().to_string();
        assert!(err.contains("唯一可用的中文输入法"), "{err}");
        // xkb(非中文)不受保护
        assert!(m.ime_remove("xkb:us::eng", false, false).is_ok());
    }

    #[test]
    fn ime_remove_dry_run_and_protection_pass_normal_case() {
        let t = TempDir::new("ime6");
        let p = setup_machine(&t);
        // 两个中文输入法都在预载里,移除其一不受“唯一”保护
        let m = ImeManager::with_system(p, Box::new(real_like_sys(0, "['table:wubi-jidian86', 'table:wubi-haifeng86']")));
        let r = m.ime_remove("table:wubi-jidian86", false, false).unwrap();
        assert!(!r.executed);
        assert!(r.commands.iter().any(|c| c == "dnf remove -y ibus-table-chinese-wubi-jidian"));
        assert!(r.commands.iter().any(|c| c.contains("preload-engines ['table:wubi-haifeng86']")));
    }

    #[test]
    fn ime_remove_unknown_id_lists_available() {
        let t = TempDir::new("ime7");
        let p = setup_machine(&t);
        let m = ImeManager::with_system(p, Box::new(real_like_sys(0, "[]")));
        let err = m.ime_remove("nope", false, false).unwrap_err().to_string();
        assert!(err.contains("未发现输入法") && err.contains("table:wubi-haifeng86"));
    }

    #[test]
    fn ime_default_degrades_without_gsettings() {
        let t = TempDir::new("ime8");
        let p = setup_machine(&t);
        let sys = FakeSystem::new(0) // 无 gsettings/dconf
            .with_cmd("rpm", &["-qa", "--qf", "%{NAME}\n"], RunOutput::ok("ibus\n"));
        let m = ImeManager::with_system(p, Box::new(sys));
        let r = m.ime_default("table:wubi-haifeng86", false).unwrap();
        assert!(!r.executed);
        assert!(r.manual_hint.as_deref().unwrap().contains("gsettings set"));
        assert!(!r.warnings.is_empty());
    }

    #[test]
    fn ime_default_dry_run_puts_engine_first() {
        let t = TempDir::new("ime9");
        let p = setup_machine(&t);
        let m = ImeManager::with_system(p, Box::new(real_like_sys(0, "['xkb:us::eng', 'table:wubi-haifeng86']")));
        let r = m.ime_default("table:wubi-haifeng86", false).unwrap();
        assert!(!r.executed);
        assert!(r.commands[0].contains("preload-engines ['table:wubi-haifeng86', 'xkb:us::eng']"));
        assert!(r.commands[1].contains("engines-order ['table:wubi-haifeng86', 'xkb:us::eng']"));
    }

    #[test]
    fn ime_default_fcitx5_gives_manual_hint() {
        let t = TempDir::new("ime10");
        let p = setup_machine(&t);
        let m = ImeManager::with_system(p, Box::new(real_like_sys(0, "[]")));
        let r = m.ime_default("fcitx5", false).unwrap();
        assert!(r.manual_hint.as_deref().unwrap().contains("fcitx5/profile"));
    }

    #[test]
    fn timestamp_is_sane() {
        let ts = timestamp_now();
        assert_eq!(ts.len(), 23, "{ts}");
        assert!(ts.ends_with("UTC"));
        assert_eq!(&ts[4..5], "-");
    }
}
