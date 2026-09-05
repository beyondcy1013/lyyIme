//! `fix`:幂等修复动作。每个动作先描述、可 `--dry-run` 只打印;
//! 危险边界:绝不写 /etc;kill 仅针对属于当前 $UID 的进程。

use crate::model::{ApplyOutcome, CheckResult, FixAction, Status};
use crate::paths::Paths;
use crate::system::SystemOps;

pub(crate) const FIX_IDS: [&str; 7] = [
    "env",
    "modeb-env",
    "autostart",
    "engine-register",
    "restart-ibus",
    "clean-cache",
    "reinstall-dict",
];

/// 检查项 → 修复动作的映射
fn fix_for_check(check_id: &str) -> Option<&'static str> {
    match check_id {
        "env" | "locale" => Some("env"),
        "daemon" => Some("restart-ibus"),
        "engine-register" => Some("engine-register"),
        "autostart" => Some("autostart"),
        "immodule" => Some("clean-cache"),
        "data" => Some("reinstall-dict"),
        _ => None,
    }
}

fn action(fix_id: &str, issue: &str) -> FixAction {
    let description = match fix_id {
        "env" => "写入 ~/.xprofile 与 ~/.config/lyyime/env.sh(GTK_IM_MODULE/QT_IM_MODULE/XMODIFIERS 三件套,managed block,可重复执行)",
        "modeb-env" => "写入 Mode B 独立外挂会话环境(GTK_IM_MODULE=xim、XMODIFIERS=@im=lyyime;Qt 应用仍走 ibus);切回 ibus 用 fix --issue env",
        "autostart" => "写入 ~/.config/autostart/im-lyyime.desktop(优先拉起 lyyime-app,缺省时拉起 ibus-daemon -drx)",
        "engine-register" => "从项目 ibus-engine/ 拷贝组件 XML 与引擎脚本到系统级(/usr/local/share)与用户级目录,并执行 ibus write-cache",
        "restart-ibus" => "重启当前用户的 ibus-daemon(kill 本 UID 的 ibus-daemon 后以 -drx 重新拉起)",
        "clean-cache" => "清理 ~/.cache/ibus/bus,并用 gtk-query-immodules-3.0 重建 GTK immodule 缓存",
        "reinstall-dict" => "调用 lyyime-dicttool convert 重新生成 data/runtime 词典",
        _ => "",
    };
    FixAction {
        id: fix_id.to_string(),
        issue: issue.to_string(),
        description: description.to_string(),
    }
}

/// 按显式 id 列表规划( id 可为检查项 id 或 fix id;未知 id 跳过,由调用方提示)
pub(crate) fn plan_fixes(_p: &Paths, _sys: &dyn SystemOps, ids: &[String]) -> Vec<FixAction> {
    let mut out: Vec<FixAction> = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        let fix_id = if FIX_IDS.contains(&id.as_str()) {
            id.as_str()
        } else {
            match fix_for_check(id) {
                Some(f) => f,
                None => continue,
            }
        };
        if seen.insert(fix_id.to_string()) {
            out.push(action(fix_id, id));
        }
    }
    out
}

/// 按检查结果规划(--all):只对 warn/fail 项生成修复动作
pub(crate) fn plan_fixes_from_results(results: &[CheckResult]) -> Vec<FixAction> {
    let mut out: Vec<FixAction> = vec![];
    let mut seen = std::collections::BTreeSet::new();
    for c in results {
        if !c.status.is_problem() || c.status == Status::Fixed {
            continue;
        }
        if let Some(f) = fix_for_check(&c.id) {
            if seen.insert(f.to_string()) {
                out.push(action(f, &c.id));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// managed block(env 文件幂等写入)
// ---------------------------------------------------------------------------

pub(crate) const BLOCK_BEGIN: &str = "# >>> lyyime-doctor managed block >>>";
pub(crate) const BLOCK_END: &str = "# <<< lyyime-doctor managed block <<<";
const ENV_VARS: [&str; 3] = ["GTK_IM_MODULE", "QT_IM_MODULE", "XMODIFIERS"];

/// 把 managed block 原子化(upsert)写入已有内容;同时把块外的同名 export 冲突行注释掉。
fn rewrite_with_block(existing: &str, body: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let begin = lines.iter().position(|l| l.trim() == BLOCK_BEGIN);
    let end = lines.iter().position(|l| l.trim() == BLOCK_END);
    let mut out: Vec<String> = vec![];
    let mut in_block = false;
    for (i, line) in lines.iter().enumerate() {
        if i == begin.unwrap_or(usize::MAX) {
            in_block = true;
            continue; // 旧块整体重建
        }
        if i == end.unwrap_or(usize::MAX) {
            in_block = false;
            continue;
        }
        if in_block {
            continue;
        }
        // 块外冲突行:export 三件套 → 注释(幂等:已是注释的不动)
        let mut disabled = false;
        for var in ENV_VARS {
            let pat = format!("export {var}=");
            if line.trim_start().starts_with(&pat) {
                out.push(format!("# lyyime-doctor: 已停用冲突行(原内容: {})", line.trim()));
                disabled = true;
                break;
            }
        }
        if !disabled {
            out.push((*line).to_string());
        }
    }
    // 追加/重建块
    if !out.is_empty() && out.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
        out.push(String::new());
    }
    out.push(BLOCK_BEGIN.to_string());
    for l in body.lines() {
        out.push(l.to_string());
    }
    out.push(BLOCK_END.to_string());
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn env_block_body(fw: &str) -> String {
    format!(
        "export GTK_IM_MODULE={fw}\nexport QT_IM_MODULE={fw}\nexport XMODIFIERS=@im={fw}\n"
    )
}

// ---------------------------------------------------------------------------
// apply 分发
// ---------------------------------------------------------------------------

pub(crate) fn apply(
    p: &Paths,
    sys: &dyn SystemOps,
    a: &FixAction,
    dry_run: bool,
) -> anyhow::Result<ApplyOutcome> {
    match a.id.as_str() {
        "env" => apply_env(p, sys, dry_run),
        "modeb-env" => apply_modeb_env(p, dry_run),
        "autostart" => apply_autostart(p, dry_run),
        "engine-register" => apply_engine_register(p, sys, dry_run),
        "restart-ibus" => apply_restart_ibus(sys, dry_run),
        "clean-cache" => apply_clean_cache(p, sys, dry_run),
        "reinstall-dict" => apply_reinstall_dict(p, sys, dry_run),
        other => Ok(ApplyOutcome::Failed(format!("未知的修复动作 id:{other}"))),
    }
}

/// fix 专用框架判定:env 三件套本身可能就是坏的(这正是要修的东西),
/// 因此以运行中的框架进程为准,无进程时才参考环境变量,默认 ibus。
fn framework_for_fix(sys: &dyn SystemOps) -> &'static str {
    if !sys.find_processes("fcitx5").is_empty() {
        return "fcitx5";
    }
    if !sys.find_processes("ibus-daemon").is_empty() {
        return "ibus";
    }
    for k in ["XMODIFIERS", "GTK_IM_MODULE", "QT_IM_MODULE"] {
        if let Some(v) = sys.env(k) {
            let v = v.to_lowercase();
            // 仅认 fcitx5;fcitx(4)等旧值不盲从,回落默认 ibus
            if v.contains("fcitx5") {
                return "fcitx5";
            }
        }
    }
    "ibus"
}

/// Mode B(独立外挂 lyyime-xim)会话环境:GTK 走内建 xim 模块直连 lyyime,
/// Qt 无 XIM 支持故保持 ibus(合同 §8:Qt 应用由 Mode A 覆盖)。
fn apply_modeb_env(p: &Paths, dry: bool) -> anyhow::Result<ApplyOutcome> {
    let body = "export GTK_IM_MODULE=xim\nexport QT_IM_MODULE=ibus\nexport XMODIFIERS=@im=lyyime\n";
    let env_sh = p.env_sh();
    let xprofile = p.xprofile();
    let env_sh_content = format!(
        "# ~/.config/lyyime/env.sh —— lyyime-doctor 生成;可在 ~/.bashrc 等 source\n{}",
        rewrite_with_block("", &body)
    );
    let existing = std::fs::read_to_string(&xprofile).unwrap_or_default();
    let xprofile_content = rewrite_with_block(&existing, &body);
    if dry {
        return Ok(ApplyOutcome::DryRun(format!(
            "将写入 {} 与 {}(Mode B profile,内容:\n{});需重新登录或执行 `source {}`",
            xprofile.display(),
            env_sh.display(),
            body.trim(),
            env_sh.display()
        )));
    }
    if let Some(d) = env_sh.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&env_sh, &env_sh_content)?;
    if let Some(d) = xprofile.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&xprofile, &xprofile_content)?;
    Ok(ApplyOutcome::Applied(format!(
        "已写入 Mode B 会话环境(GTK_IM_MODULE=xim,XMODIFIERS=@im=lyyime);需重新登录或 `source {}`;切回 Mode A 执行 `lyyime-doctor fix --issue env`",
        env_sh.display()
    )))
}

fn apply_env(p: &Paths, sys: &dyn SystemOps, dry: bool) -> anyhow::Result<ApplyOutcome> {
    let fw = framework_for_fix(sys);
    let body = env_block_body(fw);
    let env_sh = p.env_sh();
    let xprofile = p.xprofile();
    let env_sh_content = format!(
        "# ~/.config/lyyime/env.sh —— lyyime-doctor 生成;可在 ~/.bashrc 等 source\n{}",
        rewrite_with_block("", &body)
    );
    let existing = std::fs::read_to_string(&xprofile).unwrap_or_default();
    let xprofile_content = rewrite_with_block(&existing, &body);
    if dry {
        return Ok(ApplyOutcome::DryRun(format!(
            "将写入 {} 与 {}(框架={},内容:\n{});需重新登录或执行 `source {}`",
            xprofile.display(),
            env_sh.display(),
            fw,
            body.trim(),
            env_sh.display()
        )));
    }
    if let Some(d) = env_sh.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&env_sh, &env_sh_content)?;
    if let Some(d) = xprofile.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&xprofile, &xprofile_content)?;
    Ok(ApplyOutcome::Applied(format!(
        "已写入 {} 与 {}(框架={fw});对当前会话请执行 `source {}`,新会话重新登录后生效",
        xprofile.display(),
        env_sh.display(),
        env_sh.display()
    )))
}

const AUTOSTART_CONTENT: &str = r#"[Desktop Entry]
Type=Application
Name=lyyIme 输入法
Comment=lyyime-doctor 生成:优先拉起 lyyime-app,缺失时确保 ibus-daemon 自启
Exec=/bin/sh -c "command -v lyyime-app >/dev/null 2>&1 && exec lyyime-app; command -v ibus-daemon >/dev/null 2>&1 && exec ibus-daemon -drx"
Terminal=false
X-GNOME-Autostart-enabled=true
Categories=System;Utility;
"#;

fn apply_autostart(p: &Paths, dry: bool) -> anyhow::Result<ApplyOutcome> {
    let dir = p.autostart_dir();
    let target = dir.join("im-lyyime.desktop");
    if target.is_file()
        && std::fs::read_to_string(&target).unwrap_or_default() == AUTOSTART_CONTENT
    {
        return Ok(ApplyOutcome::Skipped(format!(
            "{} 已是期望内容,无需修改",
            target.display()
        )));
    }
    if dry {
        return Ok(ApplyOutcome::DryRun(format!(
            "将写入 {}:\n{}",
            target.display(),
            AUTOSTART_CONTENT.trim_end()
        )));
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&target, AUTOSTART_CONTENT)?;
    Ok(ApplyOutcome::Applied(format!("已写入 {}", target.display())))
}

fn apply_engine_register(
    p: &Paths,
    sys: &dyn SystemOps,
    dry: bool,
) -> anyhow::Result<ApplyOutcome> {
    let mut steps: Vec<String> = vec![];
    let mut notes: Vec<String> = vec![];

    let src_xml = p.project_component_xml().filter(|f| f.is_file());
    let src_py = p.project_engine_py().filter(|f| f.is_file());

    let xml_targets = [p.system_component_xml(), p.user_component_dir().join("lyyime.xml")];
    let py_targets = [
        p.system_engine_dir().join("lyyime.py"),
        p.user_engine_dir().join("lyyime.py"),
    ];

    match (src_xml, src_py) {
        (Some(x), Some(y)) => {
            if dry {
                steps.push(format!("拷贝 {} → {}", x.display(), fmt_targets(&xml_targets)));
                steps.push(format!("拷贝 {} → {}", y.display(), fmt_targets(&py_targets)));
            } else {
                for t in &xml_targets {
                    copy_to(x.clone(), t, &mut notes);
                }
                for t in &py_targets {
                    copy_to(y.clone(), t, &mut notes);
                }
                steps.push(format!("组件 XML/引擎脚本已安装到 {}", fmt_targets(&xml_targets)));
            }
        }
        _ => {
            notes.push(
                "项目 ibus-engine/ 下暂无组件源(M4 引擎尚未产出),跳过文件安装,仅刷新 ibus 缓存"
                    .into(),
            );
        }
    }

    if dry {
        steps.push("执行 ibus write-cache(用户级)".into());
        return Ok(ApplyOutcome::DryRun(format!("{};{}", steps.join("; "), notes.join(";"))));
    }
    if sys.command_exists("ibus") {
        match sys.run("ibus", &["write-cache"]) {
            Ok(o) if o.success => steps.push("ibus write-cache 完成".into()),
            Ok(o) => notes.push(format!(
                "ibus write-cache 失败({}),请手动执行或重启 ibus",
                o.stderr.trim()
            )),
            Err(e) => notes.push(format!("ibus write-cache 执行失败:{e}")),
        }
    } else {
        notes.push("未找到 ibus 命令,跳过缓存刷新;请手动执行 ibus write-cache".into());
    }
    if notes.iter().any(|n| n.contains("失败")) {
        Ok(ApplyOutcome::Failed(format!("{};{}", steps.join("; "), notes.join(";"))))
    } else {
        Ok(ApplyOutcome::Applied(format!("{};{}", steps.join("; "), notes.join(";"))))
    }
}

fn fmt_targets(targets: &[std::path::PathBuf]) -> String {
    targets.iter().map(|t| t.display().to_string()).collect::<Vec<_>>().join(" 与 ")
}

fn copy_to(src: std::path::PathBuf, dst: &std::path::Path, notes: &mut Vec<String>) {
    if let Some(d) = dst.parent() {
        if let Err(e) = std::fs::create_dir_all(d) {
            notes.push(format!("创建目录 {} 失败:{e}", d.display()));
            return;
        }
    }
    match std::fs::copy(&src, dst) {
        Ok(_) => {}
        Err(e) => notes.push(format!(
            "拷贝到 {} 失败({e},可能无权限);可改用用户级注册",
            dst.display()
        )),
    }
}

fn apply_restart_ibus(sys: &dyn SystemOps, dry: bool) -> anyhow::Result<ApplyOutcome> {
    let uid = sys.current_uid();
    let victims: Vec<u32> = sys
        .find_processes("ibus-daemon")
        .into_iter()
        .filter(|x| x.uid == uid)
        .map(|x| x.pid)
        .collect();
    if dry {
        return Ok(ApplyOutcome::DryRun(format!(
            "将 kill 当前用户(uid={uid})的 ibus-daemon: {:?};然后执行 ibus-daemon -drx(仅本 UID,不动他人进程)",
            victims
        )));
    }
    for pid in &victims {
        sys.kill(*pid)?;
    }
    if !sys.command_exists("ibus-daemon") {
        return Ok(ApplyOutcome::Failed(
            "已尝试 kill,但未找到 ibus-daemon 可执行文件,无法重新拉起;请安装 ibus".into(),
        ));
    }
    match sys.spawn_detached("ibus-daemon", &["-drx"]) {
        Ok(()) => Ok(ApplyOutcome::Applied(format!(
            "已停止 {:?} 并以 -drx 重新拉起 ibus-daemon",
            victims
        ))),
        Err(e) => Ok(ApplyOutcome::Failed(format!(
            "ibus-daemon 已停止但重新拉起失败({e});请手动执行 ibus-daemon -drx"
        ))),
    }
}

fn apply_clean_cache(p: &Paths, sys: &dyn SystemOps, dry: bool) -> anyhow::Result<ApplyOutcome> {
    let cache_dir = p.ibus_cache_dir();
    let gtk_cache = p.gtk3_im_cache.clone();
    let tool = ["gtk-query-immodules-3.0-64", "gtk-query-immodules-3.0"]
        .into_iter()
        .find(|t| sys.command_exists(t));
    let mut plan = vec![format!("清空 {}", cache_dir.display())];
    match &tool {
        Some(t) => plan.push(format!("用 {t} 重建 GTK3 immodules 缓存 {}", gtk_cache.display())),
        None => plan.push("未找到 gtk-query-immodules-3.0,跳过 GTK 缓存重建".into()),
    }
    if dry {
        return Ok(ApplyOutcome::DryRun(plan.join("; ")));
    }
    let mut done = vec![];
    if cache_dir.is_dir() {
        for e in std::fs::read_dir(&cache_dir)?.flatten() {
            let fp = e.path();
            if fp.is_dir() {
                let _ = std::fs::remove_dir_all(&fp);
            } else {
                let _ = std::fs::remove_file(&fp);
            }
        }
        done.push(format!("已清空 {}", cache_dir.display()));
    } else {
        done.push(format!("{} 不存在,无需清理", cache_dir.display()));
    }
    if let Some(t) = &tool {
        match sys.run(t, &[]) {
            Ok(o) if o.success && !o.stdout.trim().is_empty() => {
                if let Some(d) = gtk_cache.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match std::fs::write(&gtk_cache, o.stdout.as_bytes()) {
                    Ok(()) => done.push(format!("已重建 {}", gtk_cache.display())),
                    Err(e) => {
                        return Ok(ApplyOutcome::Failed(format!(
                            "GTK 缓存写入 {} 失败:{e}(ibus 缓存已清)",
                            gtk_cache.display()
                        )))
                    }
                }
            }
            Ok(o) => done.push(format!(
                "{t} 输出为空或失败({}),跳过 GTK 缓存重建",
                o.stderr.trim()
            )),
            Err(e) => done.push(format!("{t} 执行失败:{e},跳过 GTK 缓存重建")),
        }
    }
    Ok(ApplyOutcome::Applied(done.join("; ")))
}

fn apply_reinstall_dict(p: &Paths, sys: &dyn SystemOps, dry: bool) -> anyhow::Result<ApplyOutcome> {
    let tool = find_dicttool(p, sys);
    let out_dir = p.runtime_dir();
    let (tool, out_dir) = match (tool, out_dir) {
        (Some(t), Some(o)) => (t, o),
        (None, _) => {
            return Ok(ApplyOutcome::Failed(
                "未找到 lyyime-dicttool;请先在项目根执行 `bash scripts/build.sh` 或设置 Paths.dicttool"
                    .into(),
            ))
        }
        (_, None) => {
            return Ok(ApplyOutcome::Failed(
                "无法确定词典输出目录(未找到项目根);可设置环境变量 LYYIME_PROJECT_DIR".into(),
            ))
        }
    };
    if !p.wubi_db.is_file() {
        return Ok(ApplyOutcome::Failed(format!(
            "码表 {} 不存在,无法重新生成词典",
            p.wubi_db.display()
        )));
    }
    let db = p.wubi_db.display().to_string();
    let out = out_dir.display().to_string();
    let tool_s = tool.display().to_string();
    if dry {
        return Ok(ApplyOutcome::DryRun(format!(
            "将执行 {tool_s} convert --wubi-db {db} --out {out}"
        )));
    }
    let res = sys.run(&tool_s, &["convert", "--wubi-db", &db, "--out", &out])?;
    if res.success {
        Ok(ApplyOutcome::Applied(format!(
            "dicttool 已重新生成 {out}{}",
            if res.stdout.trim().is_empty() {
                String::new()
            } else {
                format!("(输出: {})", res.stdout.trim().chars().take(200).collect::<String>())
            }
        )))
    } else {
        Ok(ApplyOutcome::Failed(format!(
            "dicttool 失败:{}",
            if res.stderr.trim().is_empty() { "未知错误" } else { res.stderr.trim() }
        )))
    }
}

fn find_dicttool(p: &Paths, sys: &dyn SystemOps) -> Option<std::path::PathBuf> {
    if let Some(d) = &p.dicttool {
        if d.is_file() {
            return Some(d.clone());
        }
    }
    if let Some(proj) = &p.project_dir {
        let c = proj.join("target/debug/lyyime-dicttool");
        if c.is_file() {
            return Some(c);
        }
    }
    if sys.command_exists("lyyime-dicttool") {
        return Some(std::path::PathBuf::from("lyyime-dicttool"));
    }
    None
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::fake::FakeSystem;
    use crate::system::RunOutput;
    use crate::testutil::{temp_write, TempDir};

    fn paths(t: &TempDir) -> Paths {
        Paths::for_home(&t.path)
    }

    #[test]
    fn plan_maps_check_ids_and_dedups() {
        let t = TempDir::new("pl1");
        let sys = FakeSystem::new(0);
        let ids: Vec<String> = vec!["env".into(), "daemon".into(), "restart-ibus".into(), "nope".into()];
        let actions = plan_fixes(&paths(&t), &sys, &ids);
        let fix_ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(fix_ids, vec!["env", "restart-ibus"], "restart-ibus 应去重,未知 id 跳过");
    }

    #[test]
    fn plan_all_only_for_problem_results() {
        let mk = |id: &str, s: Status| CheckResult {
            id: id.into(),
            title: id.into(),
            status: s,
            detail: String::new(),
            fix_hint: None,
        };
        let results = vec![
            mk("env", Status::Fail),
            mk("daemon", Status::Ok),
            mk("autostart", Status::Warn),
            mk("logs", Status::Warn),
        ];
        let actions = plan_fixes_from_results(&results);
        let fix_ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(fix_ids, vec!["env", "autostart"], "ok 项与无修复映射项(logs)不产生动作");
    }

    #[test]
    fn fix_modeb_env_writes_xim_profile() {
        let t = TempDir::new("fxmb");
        let p = paths(&t);
        let sys = FakeSystem::new(0);
        let a = FixAction { id: "modeb-env".into(), issue: "modeb-env".into(), description: String::new() };
        let out = apply(&p, &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Applied(_)), "{:?}", out.message());

        let env_sh = std::fs::read_to_string(p.env_sh()).unwrap();
        assert!(env_sh.contains("export XMODIFIERS=@im=lyyime"));
        assert!(env_sh.contains("export GTK_IM_MODULE=xim"));
        assert!(env_sh.contains("export QT_IM_MODULE=ibus"));

        // 切回 Mode A:managed block 被整体重建,不再残留 xim/lyyime
        let a2 = FixAction { id: "env".into(), issue: "env".into(), description: String::new() };
        apply(&p, &sys, &a2, false).unwrap();
        let env_sh2 = std::fs::read_to_string(p.env_sh()).unwrap();
        assert!(env_sh2.contains("export XMODIFIERS=@im=ibus"));
        assert!(!env_sh2.contains("@im=lyyime"));
        assert!(!env_sh2.contains("GTK_IM_MODULE=xim"));
    }

    fn fix_env_writes_both_files_and_is_idempotent() {
        let t = TempDir::new("fx1");
        let p = paths(&t);
        let sys = FakeSystem::new(0); // 无 fcitx5 → ibus
        let a = FixAction { id: "env".into(), issue: "env".into(), description: String::new() };
        let out = apply(&p, &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Applied(_)), "{:?}", out.message());

        let env_sh = std::fs::read_to_string(p.env_sh()).unwrap();
        assert!(env_sh.contains("export XMODIFIERS=@im=ibus"));
        assert!(env_sh.contains("export GTK_IM_MODULE=ibus"));
        assert!(env_sh.contains("export QT_IM_MODULE=ibus"));
        let xp = std::fs::read_to_string(p.xprofile()).unwrap();
        assert!(xp.contains(BLOCK_BEGIN) && xp.contains(BLOCK_END));

        // 幂等:第二次执行内容不变
        let before = std::fs::read_to_string(p.env_sh()).unwrap();
        apply(&p, &sys, &a, false).unwrap();
        let after = std::fs::read_to_string(p.env_sh()).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn fix_env_dry_run_writes_nothing() {
        let t = TempDir::new("fx2");
        let p = paths(&t);
        let sys = FakeSystem::new(0);
        let a = FixAction { id: "env".into(), issue: "env".into(), description: String::new() };
        let out = apply(&p, &sys, &a, true).unwrap();
        assert!(matches!(out, ApplyOutcome::DryRun(_)));
        assert!(!p.env_sh().exists(), "dry-run 不得创建 env.sh");
        assert!(!p.xprofile().exists(), "dry-run 不得创建 xprofile");
    }

    #[test]
    fn fix_env_disables_conflicting_exports() {
        let t = TempDir::new("fx3");
        let p = paths(&t);
        temp_write(&p.xprofile(), "export GTK_IM_MODULE=fcitx\nexport XMODIFIERS=@im=fcitx\n# note\n");
        let sys = FakeSystem::new(0);
        let a = FixAction { id: "env".into(), issue: "env".into(), description: String::new() };
        apply(&p, &sys, &a, false).unwrap();
        let xp = std::fs::read_to_string(p.xprofile()).unwrap();
        assert!(!xp.contains("\nexport GTK_IM_MODULE=fcitx"), "冲突行应被注释:\n{xp}");
        assert!(xp.contains("# lyyime-doctor: 已停用冲突行"));
        assert!(xp.contains("export XMODIFIERS=@im=ibus"));
        assert!(xp.contains("# note"));
        // 再跑一次:不应叠加注释
        apply(&p, &sys, &a, false).unwrap();
        let xp2 = std::fs::read_to_string(p.xprofile()).unwrap();
        assert_eq!(xp, xp2, "重复修复必须幂等");
    }

    #[test]
    fn fix_env_uses_fcitx5_when_fcitx5_running() {
        let t = TempDir::new("fx4");
        let p = paths(&t);
        let sys = FakeSystem::new(0).with_proc(9, 0, "root", "fcitx5");
        let a = FixAction { id: "env".into(), issue: "env".into(), description: String::new() };
        apply(&p, &sys, &a, false).unwrap();
        let env_sh = std::fs::read_to_string(p.env_sh()).unwrap();
        assert!(env_sh.contains("export XMODIFIERS=@im=fcitx5"));
    }

    #[test]
    fn fix_autostart_writes_desktop() {
        let t = TempDir::new("fx5");
        let p = paths(&t);
        let a = FixAction { id: "autostart".into(), issue: "autostart".into(), description: String::new() };
        apply(&p, &FakeSystem::new(0), &a, false).unwrap();
        let f = p.autostart_dir().join("im-lyyime.desktop");
        assert!(f.is_file());
        // 幂等 → Skipped
        let out = apply(&p, &FakeSystem::new(0), &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Skipped(_)));
    }

    #[test]
    fn fix_engine_register_copies_from_project_and_runs_write_cache() {
        let t = TempDir::new("fx6");
        let mut p = paths(&t);
        p.project_dir = Some(t.path.join("proj"));
        temp_write(&t.path.join("proj/ibus-engine/component/lyyime.xml"), "<component/>");
        temp_write(&t.path.join("proj/ibus-engine/engine/lyyime.py"), "#!/usr/bin/env python3");
        let sys = FakeSystem::new(0).with_exists("ibus");
        let a = FixAction { id: "engine-register".into(), issue: "engine-register".into(), description: String::new() };
        let out = apply(&p, &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Applied(_)), "{:?}", out.message());
        assert!(p.user_component_dir().join("lyyime.xml").is_file());
        assert!(p.user_engine_dir().join("lyyime.py").is_file());
        assert!(p.system_component_xml().is_file());
        assert!(sys.ops_snapshot().iter().any(|o| o == "run ibus write-cache"));
    }

    #[test]
    fn fix_engine_register_degrades_without_source() {
        let t = TempDir::new("fx7");
        let p = paths(&t);
        let sys = FakeSystem::new(0).with_exists("ibus");
        let a = FixAction { id: "engine-register".into(), issue: "engine-register".into(), description: String::new() };
        let out = apply(&p, &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Applied(_)));
        assert!(out.message().contains("仅刷新 ibus 缓存"));
        assert!(sys.ops_snapshot().iter().any(|o| o == "run ibus write-cache"));
    }

    #[test]
    fn fix_restart_ibus_kills_only_own_uid() {
        let sys = FakeSystem::new(0)
            .with_proc(100, 0, "root", "ibus-daemon -drx")
            .with_proc(200, 1001, "beyondcy", "ibus-daemon -drx")
            .with_exists("ibus-daemon");
        let a = FixAction { id: "restart-ibus".into(), issue: "daemon".into(), description: String::new() };
        let t = TempDir::new("fx8");
        let out = apply(&Paths::for_home(&t.path), &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Applied(_)));
        let ops = sys.ops_snapshot();
        assert!(ops.contains(&"kill 100".to_string()), "{ops:?}");
        assert!(!ops.contains(&"kill 200".to_string()), "绝不能 kill 其他用户的进程");
        assert!(ops.contains(&"spawn ibus-daemon -drx".to_string()));
    }

    #[test]
    fn fix_restart_ibus_dry_run_lists_victims() {
        let sys = FakeSystem::new(0).with_proc(100, 0, "root", "ibus-daemon -drx");
        let a = FixAction { id: "restart-ibus".into(), issue: "daemon".into(), description: String::new() };
        let t = TempDir::new("fx9");
        let out = apply(&Paths::for_home(&t.path), &sys, &a, true).unwrap();
        assert!(matches!(out, ApplyOutcome::DryRun(_)));
        assert!(sys.ops_snapshot().is_empty(), "dry-run 不得有副作用");
    }

    #[test]
    fn fix_clean_cache_clears_and_rebuilds() {
        let t = TempDir::new("fx10");
        let p = paths(&t);
        temp_write(&p.ibus_cache_dir().join("registry"), "stale");
        temp_write(&p.gtk3_im_cache, "stale-cache");
        let sys = FakeSystem::new(0)
            .with_exists("gtk-query-immodules-3.0-64")
            .with_cmd("gtk-query-immodules-3.0-64", &[], RunOutput::ok("fresh-cache-content"));
        let a = FixAction { id: "clean-cache".into(), issue: "immodule".into(), description: String::new() };
        let gtk_cache = p.gtk3_im_cache.clone();
        let out = apply(&p, &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Applied(_)), "{:?}", out.message());
        assert_eq!(std::fs::read_to_string(gtk_cache).unwrap(), "fresh-cache-content");
        assert!(!p.ibus_cache_dir().join("registry").exists());
        assert!(p.ibus_cache_dir().is_dir(), "目录本身应保留");
    }

    #[test]
    fn fix_reinstall_dict_reports_missing_tool() {
        let t = TempDir::new("fx11");
        let mut p = paths(&t);
        p.project_dir = Some(t.path.join("proj"));
        let sys = FakeSystem::new(0); // 无 dicttool
        let a = FixAction { id: "reinstall-dict".into(), issue: "data".into(), description: String::new() };
        let out = apply(&p, &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Failed(_)));
        assert!(out.message().contains("dicttool"));
    }

    #[test]
    fn fix_reinstall_dict_runs_dicttool_when_available() {
        let t = TempDir::new("fx12");
        let mut p = paths(&t);
        p.project_dir = Some(t.path.join("proj"));
        temp_write(&p.wubi_db, "sqlite");
        let tool = t.path.join("bin/lyyime-dicttool");
        temp_write(&tool, "#!/bin/sh");
        p.dicttool = Some(tool);
        let sys = FakeSystem::new(0).with_cmd(
            "lyyime-dicttool",
            &["convert", "--wubi-db", &p.wubi_db.display().to_string(), "--out", &p.runtime_dir().unwrap().display().to_string()],
            RunOutput::ok("wubi.tsv: 100000 rows"),
        );
        let a = FixAction { id: "reinstall-dict".into(), issue: "data".into(), description: String::new() };
        let out = apply(&p, &sys, &a, false).unwrap();
        assert!(matches!(out, ApplyOutcome::Applied(_)), "{:?}", out.message());
    }

    #[test]
    fn rewrite_block_never_touches_etc() {
        // 防回归:所有写入路径都由 Paths 派生,天然不涉 /etc;
        // 这里显式断言 rewrite 函数只处理传入内容。
        let out = rewrite_with_block("existing\n", "export XMODIFIERS=@im=ibus\n");
        assert!(out.starts_with("existing"));
        assert!(out.contains(BLOCK_BEGIN));
    }
}
