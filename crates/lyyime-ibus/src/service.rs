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
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

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
    AiSubmit(String),
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
    fn on_ai_submit(&mut self, prompt: String) {
        self.actions.push(Action::AiSubmit(prompt));
    }
}

pub struct EngineState {
    pub conn: zbus::Connection,
    pub path: String,
    pub logic: Mutex<EngineLogic>,
    pub notice_gen: AtomicU64,
    pub setup_pid: Mutex<u32>,
    pub icon_dir: String,
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
        use zbus::zvariant::DynamicType;
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
            Action::AiSubmit(_) => Ok(()), // 由调用方单独处理
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
        let consumed = {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            let consumed = logic.process_key_event(&mut host, keyval, state);
            for a in host.actions {
                match a {
                    Action::AiSubmit(p) => ai_prompt = Some(p),
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
        consumed
    }

    async fn focus_in(&self) {
        // AI 配置在每次焦点进入时热读(设置保存后无需重启 ibus)
        let cfg: Option<AiConfig> = {
            let c = lyyime_ai::load_config();
            Some(c)
        };
        let actions = {
            let mut logic = self.0.logic.lock().unwrap();
            logic.set_ai_cfg(cfg);
            let mut host = CollectingHost::default();
            logic.reset_session(&mut host);
            host.actions
        };
        for a in &actions {
            let _ = self.emit_action(a).await;
        }
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

    async fn enable(&self) {}

    async fn disable(&self) {}

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
        let (page_size, commit_after_four) = crate::read_core_config();
        let mut core_cfg = lyyime_core::Config::default();
        core_cfg.page_size = page_size;
        core_cfg.commit_on_extra_after_four = commit_after_four;
        let ai_cfg = lyyime_ai::load_config();
        // FFI/核心初始化失败不退出:EngineLogic 进入降级英文直通(合同 §7)
        let logic = EngineLogic::new(data_dir, Some(ai_cfg), core_cfg);
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
    // 常驻:对象服务器由 zbus 内部执行器驱动,主线程挂起即可
    std::future::pending::<()>().await;
    Ok(())
}
