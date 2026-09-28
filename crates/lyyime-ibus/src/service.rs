//! ibus 引擎的 zbus 服务层:org.freedesktop.IBus.Factory / Engine 接口。
//!
//! 报文形态对照真机抓包(见 ibus_wire.rs 头注释);按键流合同见
//! ARCHITECTURE.md §3/§6/§7:放行一律返回 false(不用 forward_key_event,
//! Qt5 输入模块不支持);异常降级英文直通,绝不卡死按键。

use crate::ibus_wire as wire;
use crate::logger;
use crate::logic;
use crate::logic::EngineLogic;
use lyyime_ai::AiConfig;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use zbus::message::Message;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const ENGINE_IFACE: &str = "org.freedesktop.IBus.Engine";
const BUS_NAME: &str = "org.freedesktop.IBus.Lyyime";
const FACTORY_PATH: &str = "/org/freedesktop/IBus/Factory";

/// EngineLogic 的动作收集宿主:先同步执行逻辑,再把动作转成 D-Bus 信号。
#[derive(Default)]
pub struct CollectingHost {
    pub actions: Vec<Action>,
}

pub enum Action {
    Commit(String),
    Preedit(Option<String>),
    Candidates {
        cands: Vec<(String, String)>,
        page: usize,
        pages: usize,
        aux: String,
    },
    ModeChanged(u8),
    Notice(String),
    /// 词组效率提示:更新辅助区但**不自排清除定时**,保留到下一次输入
    /// (下一次 Candidates/Notice 效果自然替换或隐藏)。
    Hint(String),
    AiSubmit(String),
    Shot,
    /// 快速功能键命中(§14):值 = quick_actions 下标,调用方单独执行
    QuickRun(usize),
    /// 自定义查询(§15 菜单第 4 项):xdg-open 打开已代入词的网址,调用方单独执行
    OpenUrl(String),
}

impl logic::Host for CollectingHost {
    fn on_commit(&mut self, text: &str) {
        self.actions.push(Action::Commit(text.to_string()));
    }
    fn on_preedit(&mut self, text: Option<&str>) {
        self.actions.push(Action::Preedit(text.map(str::to_string)));
    }
    fn on_candidates(
        &mut self,
        cands: &[(String, String)],
        page: usize,
        pages: usize,
        aux: &str,
    ) {
        self.actions.push(Action::Candidates {
            cands: cands.to_vec(),
            page,
            pages,
            aux: aux.to_string(),
        });
    }
    fn on_mode_changed(&mut self, mode: u8) {
        self.actions.push(Action::ModeChanged(mode));
    }
    fn on_notice(&mut self, text: &str) {
        self.actions.push(Action::Notice(text.to_string()));
    }
    fn on_hint(&mut self, text: &str) {
        self.actions.push(Action::Hint(text.to_string()));
    }
    fn on_ai_submit(&mut self, prompt: String) {
        self.actions.push(Action::AiSubmit(prompt));
    }
    fn on_shot(&mut self) {
        self.actions.push(Action::Shot);
    }
    fn on_action(&mut self, index: usize) {
        self.actions.push(Action::QuickRun(index));
    }
    fn on_open_url(&mut self, url: &str) {
        self.actions.push(Action::OpenUrl(url.to_string()));
    }
}

/// 引擎对外状态发布(悬浮窗「内置:中/EN」联动):
/// /tmp/lyyime-engine-state.json, 模式/启用变更即写, 心跳每 10s 刷 ts;
/// 悬浮窗 30s 读不到新鲜时间戳即视为引擎未运行。
static ENGINE_PUB: Mutex<EnginePub> = Mutex::new(EnginePub {
    mode: None,
    enabled: None,
});

#[derive(Default)]
struct EnginePub {
    mode: Option<u8>,
    enabled: Option<bool>,
}

fn engine_state_publish() {
    let g = ENGINE_PUB.lock().unwrap();
    let mut parts: Vec<String> = vec![r#""source":"ibus""#.to_string()];
    if let Some(m) = g.mode {
        parts.push(format!(r#""mode":"{}""#, if m == 0 { "cn" } else { "en" }));
    }
    if let Some(en) = g.enabled {
        parts.push(format!(r#""enabled":{}"#, en));
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    parts.push(format!(r#""ts":{ts}"#));
    let _ = std::fs::write(
        "/tmp/lyyime-engine-state.json",
        format!("{{{}}}", parts.join(",")),
    );
}

fn engine_state_set_mode(mode: u8) {
    ENGINE_PUB.lock().unwrap().mode = Some(mode);
    engine_state_publish();
}

fn engine_state_set_enabled(enabled: bool) {
    ENGINE_PUB.lock().unwrap().enabled = Some(enabled);
    engine_state_publish();
}

pub struct EngineState {
    pub conn: zbus::Connection,
    pub path: String,
    pub logic: Mutex<EngineLogic>,
    pub notice_gen: AtomicU64,
    pub setup_pid: Mutex<u32>,
    pub icon_dir: String,
    /// 引擎是否被 ibus 启用(enable/disable 回调维护;悬浮窗状态联动读它)
    pub enabled: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
pub struct EngineService(pub Arc<EngineState>);

impl EngineService {
    pub fn new(
        conn: zbus::Connection,
        path: String,
        logic: EngineLogic,
        icon_dir: String,
    ) -> EngineService {
        EngineService(Arc::new(EngineState {
            conn,
            path,
            logic: Mutex::new(logic),
            notice_gen: AtomicU64::new(0),
            setup_pid: Mutex::new(0),
            icon_dir,
            enabled: std::sync::atomic::AtomicBool::new(false),
        }))
    }

    fn mode_icon(&self, mode: u8) -> String {
        // 图标相对引擎目录 ../icons 解析(安装前后一致);缺失回退主题名
        let name = if mode == 0 { "lyyime-zh.svg" } else { "lyyime-en.svg" };
        let p = format!("{}/{}", self.0.icon_dir, name);
        if std::path::Path::new(&p).is_file() {
            p
        } else {
            "lyyime".into()
        }
    }

    fn mode_property(&self, mode: u8) -> zbus::zvariant::Value<'static> {
        wire::property(
            "InputMode",
            wire::PROP_TYPE_TOGGLE,
            "中英切换(Shift 单击)",
            &self.mode_icon(mode),
            &format!(
                "当前:{}。单击切换中/英文,与 Shift 单击同效。",
                if mode == 0 { "中文" } else { "英文" }
            ),
            if mode == 0 { "中" } else { "EN" },
            Vec::new(),
        )
    }

    /// 普通菜单项(IBusProperty NORMAL)
    fn menu_item(&self, key: &str, label: &str, icon: &str, tip: &str) -> OwnedValue {
        OwnedValue::try_from(wire::property(
            key,
            wire::PROP_TYPE_NORMAL,
            label,
            icon,
            tip,
            "",
            Vec::new(),
        ))
        .expect("IBusProperty -> OwnedValue")
    }

    /// 托盘菜单属性树:中英切换 + 截屏 + 设置 + 工具 + 关于。
    /// 各项的响应在 property_activate 的同名分支(此前面板一直无菜单可点,
    /// 本次随 §13 截屏入口一并接活)。
    fn root_property(&self, mode: u8) -> zbus::zvariant::Value<'static> {
        let shot_hotkey = self.0.logic.lock().unwrap().shot_hotkey_text().unwrap_or_else(|| "ctrl+alt+a".to_string());
        let subs = vec![
            OwnedValue::try_from(self.mode_property(mode)).expect("toggle -> OwnedValue"),
            self.menu_item(
                "tools.shot",
                "截屏",
                "camera-photo",
                &format!(
                    "框选截屏(拖拽选区,存图片目录并复制剪贴板;热键 {shot_hotkey},shot_hotkey 可配置)"
                ),
            ),
            self.menu_item("setup", "设置…", "preferences-system", "打开 lyyIme 设置界面"),
            self.menu_item(
                "tools.fix",
                "修复输入法…",
                "system-run",
                "诊断并修复输入法环境(dry-run 预览)",
            ),
            self.menu_item("tools.ime", "输入法管理…", "input-keyboard", "列出本机输入法"),
            self.menu_item("tools.reload", "重载词库", "view-refresh", "重新加载词典与配置"),
            self.menu_item("tools.logs", "打开日志目录", "text-x-generic", "定位 lyyime 日志文件"),
            self.menu_item("about", "关于 lyyIme", "help-about", "版本信息"),
        ];
        wire::property(
            "lyyime",
            wire::PROP_TYPE_MENU,
            "lyyIme",
            &self.mode_icon(mode),
            "lyyIme 五笔拼音菜单",
            "",
            subs,
        )
    }

    async fn emit<T>(&self, name: &str, body: &T) -> zbus::Result<()>
    where
        T: serde::Serialize + zbus::zvariant::Type,
    {
        // 核级二分开关(e2e 排障用):LYYIME_SKIP=UpdateProperty,UpdateAuxiliaryText,…
        if let Ok(skip) = std::env::var("LYYIME_SKIP") {
            if skip.split(',').any(|n| n.trim() == name) {
                logger::debug(&format!("emit {name} SKIPPED"));
                return Ok(());
            }
        }
        let msg = Message::signal(self.0.path.as_str(), ENGINE_IFACE, name)?.build(body)?;
        logger::debug(&format!(
            "emit {name} sig={}",
            <&T as zbus::zvariant::Type>::signature().as_str()
        ));
        self.0.conn.send(&msg).await
    }

    async fn emit_action(&self, a: &Action) -> zbus::Result<()> {
        match a {
            Action::Commit(t) => self.emit("CommitText", &(wire::ibus_text(t, false),)).await,
            // 注意:ibus 1.5.29 daemon 对名为 UpdatePreeditText 的信号按
            // (vubu) 四元组解包(带 preedit mode);python GI 的
            // update_preedit_text_with_mode 发出的正是这个名字。
            Action::Preedit(Some(t)) => {
                let tv = wire::ibus_text(t, true);
                let cursor = t.chars().count() as u32;
                self.emit(
                    "UpdatePreeditText",
                    &(
                        tv.try_clone().expect("clone"),
                        cursor,
                        true,
                        wire::PREEDIT_FOCUS_MODE_COMMIT,
                    ),
                )
                .await?;
                Ok(())
            }
            Action::Preedit(None) => {
                let tv = wire::ibus_text("", false);
                self.emit(
                    "UpdatePreeditText",
                    &(
                        tv.try_clone().expect("clone"),
                        0u32,
                        false,
                        wire::PREEDIT_FOCUS_MODE_CLEAR,
                    ),
                )
                .await
            }
            Action::Candidates { cands, page, pages, aux } => {
                if cands.is_empty() {
                    self.emit("HideLookupTable", &()).await?;
                    self.emit("HideAuxiliaryText", &()).await
                } else {
                    let mut aux_text = aux.clone();
                    if *pages > 1 {
                        let ind = format!("  [{}/{}]", page + 1, pages);
                        aux_text = if aux_text.is_empty() {
                            ind.trim_start().to_string()
                        } else {
                            format!("{aux_text}{ind}")
                        };
                    }
                    if !aux_text.is_empty() {
                        self.emit(
                            "UpdateAuxiliaryText",
                            &(wire::ibus_text(&aux_text, false), true),
                        )
                        .await?;
                    } else {
                        self.emit("HideAuxiliaryText", &()).await?;
                    }
                    self.emit(
                        "UpdateLookupTable",
                        &(wire::lookup_table(cands), true),
                    )
                    .await
                }
            }
            Action::ModeChanged(mode) => {
                engine_state_set_mode(*mode);
                // 显式 variant 装箱(绕开 OwnedValue 序列化的不确定性)
                let boxed = zbus::zvariant::Value::Value(Box::new(self.mode_property(*mode)));
                logger::debug(&format!("UpdateProperty tree={boxed:?}"));
                if let Ok(d) = zvariant::to_bytes(
                    zbus::zvariant::serialized::Context::new(zbus::zvariant::serialized::Format::GVariant, zbus::zvariant::Endian::Little, 0),
                    &(boxed.try_clone().expect("clone"),),
                ) {
                    logger::debug(&format!(
                        "UpdateProperty gv-hex={}",
                        d.bytes().iter().map(|b| format!("{b:02x}")).collect::<String>()
                    ));
                }
                self.emit("UpdateProperty", &(boxed,)).await
            }
            Action::Notice(t) => {
                self.emit("UpdateAuxiliaryText", &(wire::ibus_text(t, false), true))
                    .await?;
                self.schedule_notice_clear();
                Ok(())
            }
            Action::Hint(t) => {
                logger::info(&format!("hint: {t}"));
                self.emit("UpdateAuxiliaryText", &(wire::ibus_text(t, false), true))
                    .await?;
                // 作废未到的 notice 清除定时,提示保留到下一次输入(合同 §6)。
                self.0.notice_gen.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Action::AiSubmit(_) => Ok(()), // 由调用方单独处理
            Action::Shot => Ok(()),        // 由调用方单独处理
            Action::QuickRun(_) => Ok(()), // 由调用方单独处理(§14)
            Action::OpenUrl(_) => Ok(()),  // 由调用方单独处理(§15 自定义查询)
        }
    }

    /// 4 秒后清辅助区提示(新提示会带新代号,旧定时器失效)。
    fn schedule_notice_clear(&self) {
        let gen = self.0.notice_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let this = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(4));
            if this.0.notice_gen.load(Ordering::SeqCst) != gen {
                return;
            }
            let _ = zbus::block_on(async {
                this.emit("HideAuxiliaryText", &()).await
            });
        });
    }

    fn spawn_ai(&self, prompt: String) {
        let cfg = lyyime_ai::load_config();
        let model = cfg.model.clone();
        let this = self.clone();
        let _ = zbus::block_on(async {
            self.emit(
                "UpdateAuxiliaryText",
                &(
                    wire::ibus_text(&format!("AI 生成中…({model})"), false),
                    true,
                ),
            )
            .await
        });
        std::thread::spawn(move || {
            let result = lyyime_ai::chat(&prompt, &cfg, None);
            let _ = zbus::block_on(async move {
                match result {
                    Ok(reply) => {
                        if let Err(e) = this
                            .emit("CommitText", &(wire::ibus_text(&reply, false),))
                            .await
                        {
                            crate::logger::error(&format!("AI 回复上屏失败:{e}"));
                            return;
                        }
                        this.emit(
                            "UpdateAuxiliaryText",
                            &(wire::ibus_text("AI 已上屏", false), true),
                        )
                        .await
                        .ok();
                        this.schedule_notice_clear();
                    }
                    Err(e) => {
                        this.emit(
                            "UpdateAuxiliaryText",
                            &(wire::ibus_text(&format!("AI 失败:{e}"), false), true),
                        )
                        .await
                        .ok();
                        this.schedule_notice_clear();
                        crate::logger::warn(&format!("AI 调用失败:{e}"));
                    }
                }
            });
        });
    }

    /// 拉起截屏助手 lyyime-shot(合同 §13):解析顺序 $LYYIME_SHOT → PATH →
    /// 引擎二进制同级目录 → /usr/local/bin;缺失时辅助区给"人话"指引。
    fn spawn_shot(&self) {
        let sibling = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("lyyime-shot")))
            .filter(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned());
        let system = {
            let p = "/usr/local/bin/lyyime-shot";
            std::path::Path::new(p).is_file().then(|| p.to_string())
        };
        let prog = std::env::var("LYYIME_SHOT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| which("lyyime-shot"))
            .or(sibling)
            .or(system);
        let Some(prog) = prog else {
            let this = self.clone();
            let msg = "未找到 lyyime-shot:请先安装 lyyIme 截屏组件(scripts/install-all.sh)";
            let _ = zbus::block_on(async {
                this.emit("UpdateAuxiliaryText", &(wire::ibus_text(msg, false), true))
                    .await
            });
            return;
        };
        match std::process::Command::new(&prog)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => crate::logger::info(&format!("已拉起截屏助手:{prog}")),
            Err(e) => crate::logger::error(&format!("拉起 lyyime-shot 失败({prog}):{e}")),
        }
    }

    fn run_in_terminal(&self, argv: &[&str], title: &str) {
        // 在终端里运行 doctor 子命令,让用户真实看到 dry-run 预览输出
        let doctor = which(argv[0]);
        if doctor.is_none() {
            let msg = format!("未找到 {}:请先安装 lyyIme 工具组件", argv[0]);
            let this = self.clone();
            let _ = zbus::block_on(async {
                this.emit("UpdateAuxiliaryText", &(wire::ibus_text(&msg, false), true))
                    .await
            });
            return;
        }
        let cmd = if let Some(term) = which("xfce4-terminal") {
            let mut c: Vec<String> = vec![term, format!("--title={title}"), "-x".into()];
            c.extend(doctor_iter(&doctor, argv));
            c
        } else if let Some(xterm) = which("xterm") {
            let mut c: Vec<String> = vec![xterm, "-title".into(), title.into(), "-e".into()];
            c.extend(doctor_iter(&doctor, argv));
            c
        } else {
            // 无终端可用:前台跑完,输出进日志,辅助区给摘要
            let out = std::process::Command::new(doctor.unwrap())
                .args(&argv[1..])
                .output();
            match out {
                Ok(o) => {
                    crate::logger::info(&format!(
                        "{title} 输出:\n{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    ));
                    let this = self.clone();
                    let msg = format!("{title} 完成,结果已写入日志");
                    let _ = zbus::block_on(async {
                        this.emit("UpdateAuxiliaryText", &(wire::ibus_text(&msg, false), true))
                            .await
                    });
                }
                Err(e) => crate::logger::error(&format!("运行 {argv:?} 失败:{e}")),
            }
            return;
        };
        let _ = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    /// 执行快速功能键(合同 §14):`@settings`/`@help` 宿主内置,
    /// 其余按 shell 命令执行(不阻塞按键流)。
    fn run_quick_action(&self, index: usize) {
        let cmd = {
            let logic = self.0.logic.lock().unwrap();
            logic.quick_actions().get(index).map(|a| a.command.clone())
        };
        let Some(cmd) = cmd else {
            crate::logger::warn(&format!("快速功能键下标越界:{index}"));
            return;
        };
        match cmd.as_str() {
            "@settings" => {
                crate::logger::info("快速功能键命中:打开配置(@settings)");
                self.launch_setup();
            }
            "@shot" => {
                let hk = {
                    let logic = self.0.logic.lock().unwrap();
                    logic
                        .shot_hotkey_text()
                        .unwrap_or_else(|| "ctrl+alt+a".to_string())
                };
                crate::logger::info(&format!("快速功能键命中:截图(@shot),热键 {hk}"));
                // 先提示(含热键);助手缺失时 spawn_shot 的安装指引会覆盖本提示
                let this = self.clone();
                let msg = format!("已拉起截屏(热键 {hk})");
                let _ = zbus::block_on(async {
                    this.emit("UpdateAuxiliaryText", &(wire::ibus_text(&msg, false), true))
                        .await
                });
                self.schedule_notice_clear();
                self.spawn_shot();
            }
            "@help" => {
                crate::logger::info("快速功能键命中:帮助(@help)");
                let msg = "帮助:Shift单击=中英切换  1-9选词  -/=翻页  Ctrl+=造词  Ctrl+Alt+A截屏  /AI+提示词=AI  peizhi/shezhi=设置 jietu=截图 bangzhu=帮助";
                let this = self.clone();
                let _ = zbus::block_on(async {
                    this.emit("UpdateAuxiliaryText", &(wire::ibus_text(msg, false), true))
                        .await
                });
                self.schedule_notice_clear();
            }
            custom => {
                crate::logger::info(&format!("快速功能键命中[{index}]:执行 {custom}"));
                match std::process::Command::new("sh")
                    .arg("-c")
                    .arg(custom)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(_) => crate::logger::info("快速功能命令已拉起"),
                    Err(e) => crate::logger::error(&format!("快速功能命令失败({custom}):{e}")),
                }
            }
        }
    }

    /// 自定义查询(§15 菜单第 4 项):xdg-open 拉起浏览器(与 Mode B/C
    /// 同一执行方式;异步,不阻塞按键流)。
    fn open_query_url(&self, url: &str) {
        crate::logger::info(&format!("自定义查询:xdg-open {url}"));
        if let Err(e) = std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            crate::logger::error(&format!("自定义查询打开失败(xdg-open):{e}"));
        }
    }

    fn launch_setup(&self) {
        // 拉起设置窗:lyyime-app 缺失时回退 lyyime-xim --settings(共用 config.toml)
        let argv: Option<Vec<String>> = which("lyyime-app")
            .map(|a| vec![a, "--settings".into()])
            .or_else(|| which("lyyime-xim").map(|a| vec![a, "--settings".into()]));
        let Some(argv) = argv else {
            crate::logger::warn("未找到 lyyime-app/lyyime-xim,无法打开设置界面");
            let this = self.clone();
            let msg = "未找到设置程序:请先安装 lyyIme 应用或 lyyime-xim";
            let _ = zbus::block_on(async {
                this.emit("UpdateAuxiliaryText", &(wire::ibus_text(msg, false), true))
                    .await
            });
            return;
        };
        // 防重复拉起:已记录的 setup pid 仍在运行则忽略
        {
            let mut pid = self.0.setup_pid.lock().unwrap();
            if *pid != 0 {
                unsafe {
                    if libc::kill(*pid as i32, 0) == 0 {
                        return;
                    }
                }
                *pid = 0;
            }
        }
        if let Ok(child) = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            *self.0.setup_pid.lock().unwrap() = child.id();
        }
    }
}

fn doctor_iter(doctor: &Option<String>, argv: &[&str]) -> Vec<String> {
    let mut v = vec![doctor.clone().unwrap_or_default()];
    v.extend(argv[1..].iter().map(|s| s.to_string()));
    v
}

fn which(prog: &str) -> Option<String> {
    // 简版 which:PATH 探测(含当前可执行目录的兄弟路径兜底)
    if prog.contains('/') {
        return std::path::Path::new(prog)
            .is_file()
            .then(|| prog.to_string());
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let p = std::path::Path::new(dir).join(prog);
        if p.is_file() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

#[zbus::interface(name = "org.freedesktop.IBus.Engine")]
impl EngineService {
    async fn process_key_event(&self, keyval: u32, _keycode: u32, state: u32) -> bool {
        let mut actions = Vec::new();
        let mut ai_prompt: Option<String> = None;
        let mut shot = false;
        let mut quick: Option<usize> = None;
        let mut open_url: Option<String> = None;
        let consumed = {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            let consumed = logic.process_key_event(&mut host, keyval, state);
            for a in host.actions {
                match a {
                    Action::AiSubmit(p) => ai_prompt = Some(p),
                    Action::Shot => shot = true,
                    Action::QuickRun(i) => quick = Some(i),
                    Action::OpenUrl(u) => open_url = Some(u),
                    other => actions.push(other),
                }
            }
            consumed
        };
        for a in &actions {
            if let Err(e) = self.emit_action(a).await {
                crate::logger::error(&format!("信号发送失败({name}):{e}", name = "action", e = e));
            }
        }
        if let Some(p) = ai_prompt {
            self.spawn_ai(p);
        }
        if shot {
            self.spawn_shot();
        }
        if let Some(i) = quick {
            self.run_quick_action(i);
        }
        if let Some(u) = open_url {
            self.open_query_url(&u);
        }
        consumed
    }

    async fn candidate_clicked(&self, index: u32, button: u32, _state: u32) {
        // §15 右键候选(button=3):候选区换成操作行(固定/删除/反查英文),
        // 数字 1-3 或再点选执行——ibus 面板无弹菜单 API,行内替换成操作行。
        if button == 3 {
            let actions = {
                let mut logic = self.0.logic.lock().unwrap();
                let mut host = CollectingHost::default();
                let _ = logic.cand_menu_open(&mut host, index as usize);
                host.actions
            };
            for a in &actions {
                let _ = self.emit_action(a).await;
            }
            return;
        }
        // 候选点击(合同 §14 鼠标点选):与数字选词同一条 core 路径
        let mut actions = Vec::new();
        let mut quick: Option<usize> = None;
        let mut open_url: Option<String> = None;
        {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            let _consumed = logic.select_candidate(&mut host, index as usize);
            for a in host.actions {
                match a {
                    Action::QuickRun(i) => quick = Some(i),
                    Action::OpenUrl(u) => open_url = Some(u),
                    other => actions.push(other),
                }
            }
        }
        for a in &actions {
            let _ = self.emit_action(a).await;
        }
        if let Some(i) = quick {
            self.run_quick_action(i);
        }
        if let Some(u) = open_url {
            self.open_query_url(&u);
        }
    }

    async fn focus_in(&self) {
        // AI 配置在每次焦点进入时热读(设置保存后无需重启 ibus)
        let cfg: Option<AiConfig> = {
            let c = lyyime_ai::load_config();
            Some(c)
        };
        let (actions, mode) = {
            let mut logic = self.0.logic.lock().unwrap();
            logic.set_ai_cfg(cfg);
            // §15 自定义查询同样热读(config.toml custom_query_*)
            logic.set_custom_query(crate::read_custom_query());
            let mut host = CollectingHost::default();
            logic.reset_session(&mut host);
            (host.actions, logic.mode)
        };
        for a in &actions {
            let _ = self.emit_action(a).await;
        }
        // 菜单属性树(幂等):面板每次获得焦点都刷新一次
        let boxed = zbus::zvariant::Value::Value(Box::new(self.root_property(mode)));
        let _ = self.emit("UpdateProperty", &(boxed,)).await;
    }

    async fn focus_out(&self) {
        let actions = {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            logic.reset_session(&mut host);
            host.actions
        };
        for a in &actions {
            let _ = self.emit_action(a).await;
        }
    }

    async fn reset(&self) {
        self.focus_out().await;
    }

    async fn enable(&self) {
        engine_state_set_enabled(true);
    }

    async fn disable(&self) {
        engine_state_set_enabled(false);
    }

    async fn set_cursor_location(&self, _x: i32, _y: i32, _w: i32, _h: i32) {}

    async fn set_capabilities(&self, _caps: u32) {}

    async fn set_content_type(&self, purpose: u32, hint: u32) {
        self.0.logic.lock().unwrap().input_purpose = purpose;
        let _ = self.emit("ContentType", &(purpose, hint)).await;
    }

    async fn property_activate(&self, name: String, _state: u32) {
        match name.as_str() {
            "InputMode" => {
                let actions = {
                    let mut logic = self.0.logic.lock().unwrap();
                    let mut host = CollectingHost::default();
                    logic.switch_mode(&mut host);
                    host.actions
                };
                for a in &actions {
                    let _ = self.emit_action(a).await;
                }
            }
            "setup" => self.launch_setup(),
            "tools.shot" => self.spawn_shot(),
            "tools.fix" => {
                self.run_in_terminal(&["lyyime-doctor", "fix", "--all", "--dry-run"],
                                     "lyyIme 修复输入法(dry-run 预览)");
            }
            "tools.ime" => {
                self.run_in_terminal(&["lyyime-doctor", "ime-list"],
                                     "lyyIme 输入法管理(列表)");
            }
            "tools.reload" => {
                let actions = {
                    let mut logic = self.0.logic.lock().unwrap();
                    let mut host = CollectingHost::default();
                    let r = logic.reload_dict(&mut host);
                    if let Err(e) = r {
                        host.actions.push(Action::Notice(format!("词库重载失败:{e:#}")));
                    } else {
                        host.actions.push(Action::Notice("词库已重载".into()));
                    }
                    host.actions
                };
                for a in &actions {
                    let _ = self.emit_action(a).await;
                }
            }
            "tools.logs" => {
                let log_dir = crate::logger::log_dir();
                let _ = std::fs::create_dir_all(&log_dir);
                let _ = std::process::Command::new("xdg-open")
                    .arg(&log_dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
            "about" => {
                let msg = format!(
                    "lyyIme 五笔拼音 v{}(Mode A · ibus 引擎,Rust)——五笔/拼音/英文混合输入",
                    wire::VERSION
                );
                let _ = self
                    .emit("UpdateAuxiliaryText", &(wire::ibus_text(&msg, false), true))
                    .await;
                self.schedule_notice_clear();
            }
            _ => {}
        }
    }

    async fn property_show(&self, _name: String) {}

    async fn property_hide(&self, _name: String) {}

    async fn destroy(&self) {}

    #[zbus(property)]
    fn focus_id(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn active_surrounding_text(&self) -> bool {
        false
    }
}

pub struct FactoryService {
    pub conn: zbus::Connection,
    pub icon_dir: String,
    pub engine_id: Mutex<u64>,
}

#[zbus::interface(name = "org.freedesktop.IBus.Factory")]
impl FactoryService {
    async fn create_engine(&mut self, name: String) -> zbus::fdo::Result<OwnedObjectPath> {
        let id = {
            let mut id = self.engine_id.lock().unwrap();
            let v = *id;
            *id += 1;
            v
        };
        let path = format!("/com/lyyime/IBus/engines/{}/engine/{id}", name.replace(':', "_"));
        crate::logger::info(&format!("引擎实例已创建:{name}({path})"));

        let data_dir = crate::resolve_data_dir();
        let core_cfg = crate::read_core_config();
        let ai_cfg = lyyime_ai::load_config();
        // FFI/核心初始化失败不退出:EngineLogic 进入降级英文直通(合同 §7)
        let mut logic = EngineLogic::new(data_dir, Some(ai_cfg), core_cfg);
        logic.set_custom_query(crate::read_custom_query());
        let svc = EngineService::new(self.conn.clone(), path.clone(), logic, self.icon_dir.clone());
        self.conn
            .object_server()
            .at(zbus::zvariant::ObjectPath::try_from(path.clone()).map_err(|e| {
                zbus::fdo::Error::Failed(format!("object path: {e}"))
            })?, svc)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(format!("注册引擎对象失败: {e}")))?;
        Ok(OwnedObjectPath::try_from(path)
            .map_err(|e| zbus::fdo::Error::Failed(format!("object path: {e}")))?)
    }
}

/// 连接 ibus 并完成注册:由 ibus-daemon 拉起(--ibus)时申请总线名
/// (与 python 版一致走标准 D-Bus RequestName);手动运行动态注册 component。
pub async fn run(
    exec_by_ibus: bool,
    icon_dir: String,
    ibus_address: Option<String>,
) -> anyhow::Result<()> {
    // 组件必须连 ibus 私有总线(daemon 的 CreateEngine 回调发往该总线);
    // 无地址文件(如 daemon 尚未启动)才回退 session bus。
    let conn = match ibus_address.as_deref() {
        Some(addr) => {
            logger::info(&format!("连接 ibus 私有总线: {addr}"));
            zbus::connection::Builder::address(addr)?
                .internal_executor(true)
                .build()
                .await?
        }
        None => zbus::Connection::session().await?,
    };
    let factory = FactoryService {
        conn: conn.clone(),
        icon_dir,
        engine_id: Mutex::new(0),
    };
    conn.object_server()
        .at(zbus::zvariant::ObjectPath::try_from(FACTORY_PATH)?, factory)
        .await?;

    if exec_by_ibus {
        // 由 ibus-daemon 拉起:申请总线名(ibustable 同款;ibus 按组件名
        // 把这条连接与静态 XML 组件关联,随后回调 CreateEngine)
        use zbus::fdo::DBusProxy;
        let dbus = DBusProxy::new(&conn).await?;
        let _ = dbus
            .request_name(BUS_NAME.try_into()?, zbus::fdo::RequestNameFlags::DoNotQueue.into())
            .await?;
    } else {
        // 手动运行(未装静态 XML 时):动态注册 component,
        // 正式安装请用 install.sh 的静态 lyyime.xml
        let exe = std::env::current_exe()?
            .to_string_lossy()
            .into_owned();
        let engine = wire::engine_desc(
            "lyyime",
            "lyyIme 五笔拼音",
            "lyyIme 五笔/拼音/英文混合输入",
            "zh_CN",
            &crate::default_icon(),
            "伍",
        );
        let comp = wire::component(&format!("{exe} --ibus"), engine);
        let _reply = conn
            .call_method(
                Some("org.freedesktop.IBus"),
                "/org/freedesktop/IBus",
                Some("org.freedesktop.IBus"),
                "RegisterComponent",
                &(&comp,),
            )
            .await?;
    }
    crate::logger::info(&format!(
        "lyyIme ibus 引擎启动(版本 {})(Rust/zbus)",
        wire::VERSION
    ));
    // 初始状态发布(模式默认中文; enabled 等 ibus 回调置位) + 10s 心跳刷 ts,
    // 悬浮窗超过 30s 读不到新鲜 ts 即显示「未运行」
    engine_state_set_mode(0);
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
        engine_state_publish();
    });
    // ibus-daemon 退出/重启时，zbus 4 没有 Connection::closed() API。
    // 定期探测总线存活；否则引擎会被孤儿化并长期占用 swap，
    // 其子进程也无法被回收（典型于隔离 E2E 会话结束后）。
    // 注:ibus 私有总线的 dbus 子集没有 org.freedesktop.DBus.Ping,也没有
    // org.freedesktop.Peer.Ping(实测均 UnknownMethod,引擎会在 5s 后误退),
    // 故用标准 NameHasOwner(org.freedesktop.IBus) 兼做存活探测 ——
    // daemon 退出时连接关闭调用即报错,或名字消失返回 false。
    let watchdog_conn = conn.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let probe = watchdog_conn.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &("org.freedesktop.IBus",),
            );
            match zbus::block_on(probe) {
                Ok(reply) => {
                    let owned: bool = reply.body().deserialize().unwrap_or(false);
                    if !owned {
                        logger::warn("ibus 总线已断开(org.freedesktop.IBus 名字消失),退出引擎");
                        std::process::exit(0);
                    }
                }
                Err(err) => {
                    logger::warn(&format!("ibus 总线已断开,退出引擎: {err}"));
                    std::process::exit(0);
                }
            }
        }
    });
    // 常驻:对象服务器由 zbus 内部执行器驱动,主线程挂起即可。
    std::future::pending::<()>().await;
    Ok(())
}
