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
    /// 菜单触发确认(2026-09-30):值 = core 可信目录 MENU_CATALOG 下标,
    /// 调用方按目录动作分派(settings/help/english/…),绝不落 shell
    MenuRun(usize),
    MenuGeneral(String),
    /// 菜单提示撤下:隐藏辅助区(仅在我们贴的菜单提示仍显示时产生)
    AuxClear,
    /// 解除系统 CapsLock(logic 在 Shift 单击确认英文→中文时请求)
    CapsLockOff,
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
    fn on_menu_action(&mut self, index: usize) {
        self.actions.push(Action::MenuRun(index));
    }
    fn on_menu_general(&mut self, id: &str) {
        self.actions.push(Action::MenuGeneral(id.to_string()));
    }
    fn on_menu_hint_clear(&mut self) {
        self.actions.push(Action::AuxClear);
    }
    fn on_caps_lock_off(&mut self) {
        self.actions.push(Action::CapsLockOff);
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
    let path = std::env::var("LYYIME_ENGINE_STATE_FILE")
        .unwrap_or_else(|_| "/tmp/lyyime-engine-state.json".to_string());
    let _ = std::fs::write(path, format!("{{{}}}", parts.join(",")));
}

fn engine_state_set_mode(mode: u8) {
    ENGINE_PUB.lock().unwrap().mode = Some(mode);
    engine_state_publish();
}

fn engine_state_set_enabled(enabled: bool) {
    ENGINE_PUB.lock().unwrap().enabled = Some(enabled);
    engine_state_publish();
}

/// 防重复拉起设置启动器:槽内子进程仍存活则拒绝重复;已退出则
/// try_wait 收割后再拉起。kill(pid,0) 对已退出未回收的 zombie 仍返回 0,
/// 不能用来判活(zombie 误判存活会让第二次拉起永久被拒);调用方须
/// 持锁覆盖"检查+拉起"全程。
fn spawn_setup_child(
    slot: &mut Option<std::process::Child>,
    argv: &[String],
) -> std::io::Result<bool> {
    if let Some(child) = slot.as_mut() {
        if child.try_wait()?.is_none() {
            let reopen = std::path::Path::new(&argv[0])
                .file_name()
                .map(|n| n == "lyyime-xim")
                .unwrap_or(false)
                && argv.iter().any(|a| a == "--settings")
                && std::fs::canonicalize(&argv[0])
                    .ok()
                    .zip(std::fs::read_link(format!("/proc/{}/exe", child.id())).ok())
                    .map(|(a, b)| a == b)
                    .unwrap_or(false);
            if !reopen {
                return Ok(false);
            }
            if unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGUSR1) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            return Ok(true);
        }
        *slot = None;
    }
    let child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    *slot = Some(child);
    Ok(true)
}

pub struct EngineState {
    pub conn: zbus::Connection,
    pub path: String,
    pub logic: Mutex<EngineLogic>,
    pub notice_gen: AtomicU64,
    pub setup_child: Mutex<Option<std::process::Child>>,
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
        let st = Arc::new(EngineState {
            conn,
            path,
            logic: Mutex::new(logic),
            notice_gen: AtomicU64::new(0),
            setup_child: Mutex::new(None),
            icon_dir,
            enabled: std::sync::atomic::AtomicBool::new(false),
        });
        crate::candidate_ui::register(&st.path, &st);
        EngineService(st)
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
                let mode = if crate::candidate_ui::ui().is_some() {
                    wire::PREEDIT_FOCUS_MODE_CLEAR
                } else {
                    wire::PREEDIT_FOCUS_MODE_COMMIT
                };
                self.emit(
                    "UpdatePreeditText",
                    &(tv.try_clone().expect("clone"), cursor, true, mode),
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
                if let Some(ui) = crate::candidate_ui::ui() {
                    enum U {
                        Hide(u64),
                        Show(u64),
                        Keep,
                    }
                    let v = {
                        let mut lg = self.0.logic.lock().unwrap();
                        if lg.cand_menu_active() {
                            U::Keep
                        } else if !lg.ui_gate_open()
                            || (cands.is_empty() && aux.is_empty())
                        {
                            U::Hide(lg.ui_invalidate())
                        } else {
                            U::Show(lg.ui_invalidate())
                        }
                    };
                    match v {
                        U::Hide(g) => ui.hide(&self.0.path, g),
                        U::Show(g) => {
                            let mut aux_text = aux.clone();
                            if *pages > 1 {
                                let ind = format!("  [{}/{}]", page + 1, pages);
                                aux_text = if aux_text.is_empty() {
                                    ind.trim_start().to_string()
                                } else {
                                    format!("{aux_text}{ind}")
                                };
                            }
                            ui.show(&self.0.path, g, cands, &aux_text, *page, *pages);
                        }
                        U::Keep => {}
                    }
                    self.emit("HideLookupTable", &()).await?;
                    self.emit("HideAuxiliaryText", &()).await?;
                    return Ok(());
                }
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
                self.aux_show(t).await;
                self.schedule_notice_clear();
                Ok(())
            }
            Action::Hint(t) => {
                logger::info(&format!("hint: {t}"));
                self.aux_show(t).await;
                // 作废未到的 notice 清除定时,提示保留到下一次输入(合同 §6)。
                self.0.notice_gen.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Action::AiSubmit(_) => Ok(()), // 由调用方单独处理
            Action::Shot => Ok(()),        // 由调用方单独处理
            Action::QuickRun(_) => Ok(()), // 由调用方单独处理(§14)
            Action::OpenUrl(_) => Ok(()),  // 由调用方单独处理(§15 自定义查询)
            Action::MenuRun(_) => Ok(()),  // 由调用方单独处理(2026-09-30 菜单触发)
            Action::MenuGeneral(_) => Ok(()),
            Action::AuxClear => {
                // 菜单提示撤下:只隐藏辅助区;同时作废未到的 notice 清除定时
                self.0.notice_gen.fetch_add(1, Ordering::SeqCst);
                self.aux_hide().await;
                Ok(())
            }
            Action::CapsLockOff => {
                crate::keyboard::caps_lock_off();
                Ok(())
            }
        }
    }

    async fn aux_show(&self, text: &str) {
        if let Some(ui) = crate::candidate_ui::ui() {
            let gen = {
                let lg = self.0.logic.lock().unwrap();
                if !lg.ui_gate_open() {
                    return;
                }
                lg.ui_gen()
            };
            ui.notice(&self.0.path, gen, text);
            return;
        }
        let _ = self
            .emit("UpdateAuxiliaryText", &(wire::ibus_text(text, false), true))
            .await;
    }

    fn aux_show_sync(&self, text: &str) {
        if let Some(ui) = crate::candidate_ui::ui() {
            let gen = {
                let lg = self.0.logic.lock().unwrap();
                if !lg.ui_gate_open() {
                    return;
                }
                lg.ui_gen()
            };
            ui.notice(&self.0.path, gen, text);
            return;
        }
        let _ = zbus::block_on(
            self.emit("UpdateAuxiliaryText", &(wire::ibus_text(text, false), true)),
        );
    }

    async fn aux_hide(&self) {
        if let Some(ui) = crate::candidate_ui::ui() {
            let gen = {
                let lg = self.0.logic.lock().unwrap();
                if !lg.ui_gate_open() {
                    return;
                }
                lg.ui_gen()
            };
            ui.notice(&self.0.path, gen, "");
            return;
        }
        let _ = self.emit("HideAuxiliaryText", &()).await;
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
            let _ = zbus::block_on(this.aux_hide());
        });
    }

    /// 面板翻页统一入口(候选窗「<」「>」按钮与滚轮,合同 §6):
    /// ibus-ui-gtk3 点击翻页箭头/在候选区滚轮时,ibus-daemon 以
    /// Engine.PageUp/PageDown(CursorUp/CursorDown)回调引擎;此前未实现,
    /// zbus 回 UnknownMethod → 面板点击无反应。日志落 ibus.log 供 e2e 断言。
    async fn panel_page(&self, down: bool) {
        let (actions, page, pages) = {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            logic.flip_page(&mut host, down);
            let pos = logic.page_pos();
            (host.actions, pos.0, pos.1)
        };
        let dir = if down { "下一页" } else { "上一页" };
        if pages > 0 {
            logger::info(&format!(
                "候选窗面板翻页:{dir} → 第 {}/{} 页",
                page + 1,
                pages
            ));
        } else {
            logger::info(&format!("候选窗面板翻页:{dir}(无候选,忽略)"));
        }
        for a in &actions {
            if let Err(e) = self.emit_action(a).await {
                crate::logger::error(&format!("翻页信号发送失败:{e}"));
            }
        }
    }

    fn spawn_ai(&self, prompt: String) {
        let cfg = lyyime_ai::load_config();
        let model = cfg.model.clone();
        let this = self.clone();
        self.aux_show_sync(&format!("AI 生成中…({model})"));
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
                        this.aux_show_sync("AI 已上屏");
                        this.schedule_notice_clear();
                    }
                    Err(e) => {
                        this.aux_show_sync(&format!("AI 失败:{e}"));
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
            let msg = "未找到 lyyime-shot:请先安装 lyyIme 截屏组件(scripts/install-all.sh)";
            self.aux_show_sync(msg);
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
            self.aux_show_sync(&msg);
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
                    self.aux_show_sync(&format!("{title} 完成,结果已写入日志"));
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
                self.aux_show_sync(&format!("已拉起截屏(热键 {hk})"));
                self.schedule_notice_clear();
                self.spawn_shot();
            }
            "@help" => {
                crate::logger::info("快速功能键命中:帮助(@help)");
                self.aux_show_sync("帮助:Shift单击=中英切换  1-9选词  -/=翻页  Ctrl+=造词  Ctrl+Alt+A截屏  /AI+提示词=AI  peizhi/shezhi=设置 jietu=截图 bangzhu=帮助");
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

    /// 菜单触发确认动作(2026-09-30,与 Mode B 同合同):可信目录下标 →
    /// 内置动作分派;绝无 shell 命令路径。XIM 侧工具(悬浮窗/主窗口)经
    /// lyyime-float / lyyime-xim --mainwin 拉起,缺失时辅助区提示而不是
    /// 静默失败。
    async fn run_menu_action(&self, index: usize) {
        let Some(item) = lyyime_core::menu_trigger::MENU_CATALOG.get(index) else {
            crate::logger::warn(&format!("菜单触发下标越界:{index}"));
            return;
        };
        crate::logger::info(&format!(
            "菜单触发:确认执行 {}(idx={index})",
            item.id
        ));
        use lyyime_core::MenuAction as A;
        match item.action {
            A::OpenSettings => self.launch_setup(),
            A::OpenSettingsPage(p) => self.launch_setup_page(p),
            A::Help => {
                self.emit_menu_hint(
                    "帮助:Shift单击=中英切换  1-9选词  -/=翻页  Ctrl+=造词  \
                     Ctrl+Alt+A截屏  /AI+提示词=AI",
                )
                .await;
            }
            A::EnglishMode => {
                // 语义=切英文(非 toggle):已英文则无操作
                let actions = {
                    let mut logic = self.0.logic.lock().unwrap();
                    if logic.mode != 0 {
                        Vec::new()
                    } else {
                        let mut host = CollectingHost::default();
                        logic.switch_mode(&mut host);
                        host.actions
                    }
                };
                for a in &actions {
                    let _ = self.emit_action(a).await;
                }
            }
            A::Shot => self.spawn_shot(),
            A::FixIme => {
                self.run_in_terminal(
                    &["lyyime-doctor", "fix", "--all", "--dry-run"],
                    "lyyIme 修复输入法(dry-run 预览)",
                );
            }
            A::ManageIme => {
                self.run_in_terminal(
                    &["lyyime-doctor", "ime-list"],
                    "lyyIme 输入法管理(列表)",
                );
            }
            A::ReloadDict => {
                let actions = {
                    let mut logic = self.0.logic.lock().unwrap();
                    let mut host = CollectingHost::default();
                    match logic.reload_dict(&mut host) {
                        Ok(()) => host
                            .actions
                            .push(Action::Notice("词库已重载".into())),
                        Err(e) => host
                            .actions
                            .push(Action::Notice(format!("词库重载失败:{e:#}"))),
                    }
                    host.actions
                };
                for a in &actions {
                    let _ = self.emit_action(a).await;
                }
            }
            A::OpenLog => {
                let log_dir = crate::logger::log_dir();
                let _ = std::fs::create_dir_all(&log_dir);
                if let Err(e) = std::process::Command::new("xdg-open")
                    .arg(&log_dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    crate::logger::error(&format!("打开日志目录失败:{e}"));
                }
            }
            A::FloatWindow => self.spawn_tool(
                &["lyyime-float"],
                "未找到 lyyime-float:请先安装直输悬浮窗组件",
            ),
            A::MainWindow => self.spawn_tool(
                &["lyyime-xim", "--mainwin"],
                "未找到 lyyime-xim:主窗口由 Mode B 组件提供",
            ),
        }
    }

    async fn run_general_action(&self, id: &str) {
        crate::logger::info(&format!("候选菜单:通用动作 {id}"));
        match id {
            logic::GA_SETTINGS => self.launch_setup(),
            logic::GA_SETTINGS_INPUT => self.launch_setup_page(1),
            logic::GA_SETTINGS_SKIN => self.launch_setup_page(6),
            logic::GA_TOGGLE_MODE => {
                let actions = {
                    let mut lg = self.0.logic.lock().unwrap();
                    let mut host = CollectingHost::default();
                    lg.switch_mode(&mut host);
                    host.actions
                };
                for a in &actions {
                    let _ = self.emit_action(a).await;
                }
            }
            logic::GA_TOGGLE_PINYIN => {
                let cur = crate::read_config_bool("pinyin_only")
                    .unwrap_or_else(|| crate::menu_bool_default("pinyin_only"));
                let Some(want) = self.persist_flag("pinyin_only", "输入方案", cur).await else {
                    return;
                };
                let actions = {
                    let mut lg = self.0.logic.lock().unwrap();
                    let mut host = CollectingHost::default();
                    lg.set_pinyin_only(&mut host, want);
                    host.actions
                };
                for a in &actions {
                    let _ = self.emit_action(a).await;
                }
                self.emit_menu_hint(if want { "已切换:纯拼音" } else { "已切换:五笔/拼音混输" })
                    .await;
            }
            logic::GA_TOGGLE_PUNCT => {
                let cur = self
                    .0
                    .logic
                    .lock()
                    .unwrap()
                    .runtime_cn_punct()
                    .or_else(|| crate::read_config_bool("chinese_punct"))
                    .or_else(|| crate::read_config_bool("cn_punct"))
                    .unwrap_or_else(|| crate::menu_bool_default("chinese_punct"));
                let Some(want) = self.persist_flag("chinese_punct", "中文标点", cur).await else {
                    return;
                };
                self.0.logic.lock().unwrap().set_chinese_punctuation(want);
            }
            logic::GA_TOGGLE_LEARN => {
                let cur = crate::read_config_bool("learning")
                    .unwrap_or_else(|| crate::menu_bool_default("learning"));
                let Some(want) = self.persist_flag("learning", "用户词学习", cur).await else {
                    return;
                };
                self.0.logic.lock().unwrap().set_learning(want);
            }
            logic::GA_TOGGLE_PRED => {
                let cur = crate::read_config_bool("next_word_prediction")
                    .unwrap_or_else(|| crate::menu_bool_default("next_word_prediction"));
                let Some(want) = self
                    .persist_flag("next_word_prediction", "上屏后联想", cur)
                    .await
                else {
                    return;
                };
                self.0.logic.lock().unwrap().set_next_word_prediction(want);
            }
            logic::GA_TOGGLE_QA => {
                let cur = crate::read_config_bool("quick_actions_enabled")
                    .unwrap_or_else(|| crate::menu_bool_default("quick_actions_enabled"));
                let Some(want) = self
                    .persist_flag("quick_actions_enabled", "快速功能键", cur)
                    .await
                else {
                    return;
                };
                self.0.logic.lock().unwrap().set_quick_actions_enabled(want);
            }
            logic::GA_SHOT => self.spawn_shot(),
            logic::GA_RELOAD => {
                let actions = {
                    let mut lg = self.0.logic.lock().unwrap();
                    let mut host = CollectingHost::default();
                    match lg.reload_dict(&mut host) {
                        Ok(()) => host.actions.push(Action::Notice("词库已重载".into())),
                        Err(e) => host
                            .actions
                            .push(Action::Notice(format!("词库重载失败:{e:#}"))),
                    }
                    host.actions
                };
                for a in &actions {
                    let _ = self.emit_action(a).await;
                }
            }
            _ => crate::logger::warn(&format!("候选菜单:未知通用动作 {id}(忽略)")),
        }
    }
    async fn persist_flag(&self, key: &str, name: &str, cur: bool) -> Option<bool> {
        let want = !cur;
        match crate::update_config_bool(key, want) {
            Ok(()) => {
                crate::logger::info(&format!("候选菜单:{name} → {want}(已保存)"));
                Some(want)
            }
            Err(e) => {
                crate::logger::error(&format!("候选菜单:{name} 保存失败:{e}"));
                self.emit_menu_hint(&format!("{name} 保存失败")).await;
                None
            }
        }
    }
    /// 辅助区提示(notice 通道,4s 自清)
    async fn emit_menu_hint(&self, text: &str) {
        self.aux_show(text).await;
        self.schedule_notice_clear();
    }

    /// 同步辅助区提示:spawn/探测失败等错误路径给用户可见反馈(人话)
    fn aux_notice(&self, msg: &str) {
        self.aux_show_sync(msg);
    }

    /// 拉起外部工具(PATH 探测;argv[0] 为程序名)。缺失时辅助区提示。
    fn spawn_tool(&self, argv: &[&str], missing_msg: &str) {
        let Some(prog) = which(argv[0]) else {
            crate::logger::warn(&format!("菜单触发:{missing_msg}"));
            self.aux_notice(missing_msg);
            return;
        };
        match std::process::Command::new(&prog)
            .args(&argv[1..])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => crate::logger::info(&format!("菜单触发:已拉起 {prog}")),
            Err(e) => {
                crate::logger::error(&format!("菜单触发拉起失败({prog}):{e}"));
                self.aux_notice(&format!("菜单功能拉起失败:{e}"));
            }
        }
    }

    /// 打开设置窗指定子页(0 基):经 lyyime-xim --settings-page N
    /// (单实例信号机制不变);无 XIM 组件时回退主设置入口。
    fn launch_setup_page(&self, page: u8) {
        let Some(prog) = which("lyyime-xim") else {
            crate::logger::warn(&format!(
                "菜单触发:未找到 lyyime-xim,无法打开设置页 {page}"
            ));
            self.aux_notice(&format!(
                "无法打开设置页 {page}:未找到 lyyime-xim 组件,改开主设置界面"
            ));
            self.launch_setup();
            return;
        };
        crate::logger::info(&format!("菜单触发:打开设置页 {page}(经 lyyime-xim)"));
        if let Err(e) = std::process::Command::new(&prog)
            .arg("--settings-page")
            .arg(page.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            crate::logger::error(&format!("拉起设置页失败({prog}):{e}"));
            self.aux_notice(&format!("打开设置页 {page} 失败:{e}"));
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
            self.aux_notice("未找到设置程序:请先安装 lyyIme 应用或 lyyime-xim");
            return;
        };
        // 防重复拉起:持锁完成"收割/判活+拉起",锁外再打日志/提示
        let spawned = {
            let mut slot = self.0.setup_child.lock().unwrap();
            spawn_setup_child(&mut slot, &argv)
        };
        match spawned {
            Ok(true) => crate::logger::info("设置界面启动请求已发送"),
            Ok(false) => crate::logger::info("设置启动器仍在运行,忽略重复请求"),
            Err(e) => {
                crate::logger::error(&format!("拉起设置界面失败({}):{e}", argv[0]));
                self.aux_notice(&format!("打开设置界面失败:{e}"));
            }
        }
    }

    async fn dispatch_logic_actions(&self, actions: Vec<Action>) {
        let mut quick: Option<usize> = None;
        let mut open_url: Option<String> = None;
        let mut menu_general: Option<String> = None;
        let mut emit_actions = Vec::new();
        for a in actions {
            match a {
                Action::QuickRun(i) => quick = Some(i),
                Action::OpenUrl(u) => open_url = Some(u),
                Action::MenuGeneral(id) => menu_general = Some(id),
                other => emit_actions.push(other),
            }
        }
        for a in &emit_actions {
            let _ = self.emit_action(a).await;
        }
        if let Some(i) = quick {
            self.run_quick_action(i);
        }
        if let Some(u) = open_url {
            self.open_query_url(&u);
        }
        if let Some(id) = menu_general {
            self.run_general_action(&id).await;
        }
    }

    fn ui_menu_open(&self, idx: Option<usize>) {
        let Some(ui) = crate::candidate_ui::ui() else {
            return;
        };
        let (items, gen) = {
            let mut lg = self.0.logic.lock().unwrap();
            if !lg.ui_gate_open() {
                return;
            }
            let mut host = CollectingHost::default();
            let ok = match idx {
                Some(i) => lg.cand_menu_open(&mut host, i),
                None => lg.cand_menu_open_general(&mut host),
            };
            if !ok {
                return;
            }
            (lg.cand_menu_items(), lg.ui_invalidate())
        };
        if items.is_empty() {
            return;
        }
        crate::logger::info(&format!(
            "候选窗菜单:{} idx={:?} 项数={} 项={}",
            if idx.is_some() { "词" } else { "通用" },
            idx,
            items.len(),
            items.join("|")
        ));
        ui.menu(&self.0.path, gen, items);
    }

    pub(crate) async fn ui_event(&self, ev: crate::candidate_ui::UiEvent) {
        use crate::candidate_ui::UiEventKind as K;
        enum Out {
            Acts(Vec<Action>),
            MenuOpen(Option<usize>),
            Page(bool, usize, usize, Vec<Action>),
            Settings,
            Drop(&'static str),
        }
        let out = {
            let mut lg = self.0.logic.lock().unwrap();
            if ev.owner != self.0.path || ev.gen != lg.ui_gen() || !lg.ui_gate_open() {
                Out::Drop("过期或失活")
            } else {
                let mut host = CollectingHost::default();
                match ev.kind {
                    K::Select { idx } => {
                        crate::logger::info(&format!("候选窗:点选行 {idx}"));
                        let _ = lg.select_candidate(&mut host, idx);
                        Out::Acts(host.actions)
                    }
                    K::Page { down } => {
                        lg.flip_page(&mut host, down);
                        let (p, n) = lg.page_pos();
                        Out::Page(down, p, n, host.actions)
                    }
                    K::WordMenu { idx } => Out::MenuOpen(Some(idx)),
                    K::GeneralMenu => Out::MenuOpen(None),
                    K::Settings => Out::Settings,
                    K::ActivateMenu { abs } => {
                        crate::logger::info(&format!("候选窗菜单点选:abs={abs}"));
                        let _ = lg.cand_menu_select_absolute(&mut host, abs);
                        Out::Acts(host.actions)
                    }
                    K::CancelMenu => {
                        crate::logger::info("候选窗菜单取消:还原真实候选");
                        lg.cand_menu_cancel(&mut host);
                        Out::Acts(host.actions)
                    }
                }
            }
        };
        match out {
            Out::Acts(actions) => self.dispatch_logic_actions(actions).await,
            Out::MenuOpen(idx) => self.ui_menu_open(idx),
            Out::Page(down, page, pages, actions) => {
                let dir = if down { "下一页" } else { "上一页" };
                if pages > 0 {
                    crate::logger::info(&format!(
                        "候选窗面板翻页:{dir} → 第 {}/{} 页",
                        page + 1,
                        pages
                    ));
                }
                self.dispatch_logic_actions(actions).await;
            }
            Out::Settings => {
                crate::logger::info("候选窗:齿轮打开设置");
                self.launch_setup();
            }
            Out::Drop(why) => crate::logger::debug(&format!(
                "候选窗 UI 事件丢弃({why}):owner={} gen={}",
                ev.owner, ev.gen
            )),
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
        let mut menu: Option<usize> = None;
        let mut menu_general: Option<String> = None;
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
                    Action::MenuRun(i) => menu = Some(i),
                    Action::MenuGeneral(id) => menu_general = Some(id),
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
        if let Some(i) = menu {
            self.run_menu_action(i).await;
        }
        if let Some(id) = menu_general {
            self.run_general_action(&id).await;
        }
        consumed
    }

    async fn candidate_clicked(&self, index: u32, button: u32, _state: u32) {
        // §15 右键候选(button=3):候选区换成操作行(固定/删除/反查英文),
        // 数字 1-3 或再点选执行——ibus 面板无弹菜单 API,行内替换成操作行。
        if button == 3 {
            if crate::candidate_ui::ui().is_some() {
                self.ui_menu_open(Some(index as usize));
                return;
            }
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
        let actions = {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            let _consumed = logic.select_candidate(&mut host, index as usize);
            host.actions
        };
        self.dispatch_logic_actions(actions).await;
    }

    /// 候选窗「<」「>」翻页按钮(合同 §6):与键盘 -/= 同一条 core 路径。
    async fn page_up(&self) {
        self.panel_page(false).await;
    }

    async fn page_down(&self) {
        self.panel_page(true).await;
    }

    /// 面板候选区滚轮(ibus-ui-gtk3 滚动发 CursorUp/CursorDown):映射翻页,
    /// 与主流输入法滚轮翻候选一致;造词模式下承担多选/少选一字。
    async fn cursor_up(&self) {
        self.panel_page(false).await;
    }

    async fn cursor_down(&self) {
        self.panel_page(true).await;
    }

    async fn focus_in(&self) {
        // AI 配置在每次焦点进入时热读(设置保存后无需重启 ibus)
        let cfg: Option<AiConfig> = {
            let c = lyyime_ai::load_config();
            Some(c)
        };
        // core 配置一次热读:菜单触发三键 + 实际工具快捷键(冲突已消解)
        let core_cfg = crate::read_core_config();
        let (actions, mode) = {
            let mut logic = self.0.logic.lock().unwrap();
            logic.set_ai_cfg(cfg);
            // §15 自定义查询同样热读(config.toml custom_query_*)
            logic.set_custom_query(crate::read_custom_query());
            // 菜单触发三键与工具热键同帧热读(2026-09-30;设置保存即生效)
            logic.set_menu_trigger_cfg(
                core_cfg.menu_trigger_enabled,
                core_cfg.menu_trigger_key,
                &core_cfg.menu_trigger_disabled,
            );
            logic.set_tool_hotkeys(&core_cfg.coin_hotkey, &core_cfg.shot_hotkey);
            // 联想开关联动热读(设置保存即生效;关闭时 core 自清联想行)
            logic.set_next_word_prediction(core_cfg.next_word_prediction);
            // 中文标点默认热读:仅默认值变更才下发(运行时 Ctrl+.
            // 切换跨焦点保留),窄化下发不打断输入也不动缓冲。
            logic.set_punctuation_default(core_cfg.cn_punct);
            let mut host = CollectingHost::default();
            logic.set_pinyin_only(&mut host, core_cfg.pinyin_only);
            logic.set_mixed_en(core_cfg.mixed_en);
            logic.set_learning(core_cfg.learning);
            logic.set_quick_actions_enabled(core_cfg.quick_actions_enabled);
            logic.reset_session(&mut host);
            logic.ui_focused = true;
            logic.ui_invalidate();
            (host.actions, logic.mode)
        };
        if let Some(ui) = crate::candidate_ui::ui() {
            let (skin, font_size) = crate::ui_style();
            ui.style(&skin, font_size);
        }
        for a in &actions {
            let _ = self.emit_action(a).await;
        }
        // 菜单属性树(幂等):面板每次获得焦点都刷新一次
        let boxed = zbus::zvariant::Value::Value(Box::new(self.root_property(mode)));
        let _ = self.emit("UpdateProperty", &(boxed,)).await;
    }

    async fn focus_out(&self) {
        let (actions, gen, focused, enabled) = {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            logic.reset_session(&mut host);
            logic.ui_focused = false;
            let gen = logic.ui_invalidate();
            (host.actions, gen, logic.ui_focused, logic.ui_enabled)
        };
        logger::debug(&format!(
            "focus_out owner={} gen={gen} focused={focused} enabled={enabled}",
            self.0.path
        ));
        if let Some(ui) = crate::candidate_ui::ui() {
            ui.hide(&self.0.path, gen);
        }
        for a in &actions {
            let _ = self.emit_action(a).await;
        }
    }

    async fn reset(&self) {
        let (actions, gen, focused, enabled) = {
            let mut logic = self.0.logic.lock().unwrap();
            let mut host = CollectingHost::default();
            let gen = logic.reset_ui_session(&mut host);
            (host.actions, gen, logic.ui_focused, logic.ui_enabled)
        };
        logger::debug(&format!(
            "reset owner={} gen={gen} focused={focused} enabled={enabled}",
            self.0.path
        ));
        if let Some(ui) = crate::candidate_ui::ui() {
            ui.hide(&self.0.path, gen);
        }
        for a in &actions {
            let _ = self.emit_action(a).await;
        }
    }

    async fn enable(&self) {
        engine_state_set_enabled(true);
        let (gen, focused, enabled) = {
            let mut logic = self.0.logic.lock().unwrap();
            logic.ui_enabled = true;
            (logic.ui_gen(), logic.ui_focused, logic.ui_enabled)
        };
        logger::debug(&format!(
            "enable owner={} gen={gen} focused={focused} enabled={enabled}",
            self.0.path
        ));
    }

    async fn disable(&self) {
        engine_state_set_enabled(false);
        let (gen, focused, enabled) = {
            let mut logic = self.0.logic.lock().unwrap();
            logic.ui_enabled = false;
            logic.ui_focused = false;
            let gen = logic.ui_invalidate();
            (gen, logic.ui_focused, logic.ui_enabled)
        };
        logger::debug(&format!(
            "disable owner={} gen={gen} focused={focused} enabled={enabled}",
            self.0.path
        ));
        if let Some(ui) = crate::candidate_ui::ui() {
            ui.hide(&self.0.path, gen);
        }
    }

    async fn set_cursor_location(&self, x: i32, y: i32, w: i32, h: i32) {
        if let Some(ui) = crate::candidate_ui::ui() {
            ui.cursor(&self.0.path, x, y, w, h);
        }
    }

    async fn set_capabilities(&self, _caps: u32) {}

    async fn set_content_type(&self, purpose: u32, hint: u32) {
        let gen = {
            let mut logic = self.0.logic.lock().unwrap();
            logic.input_purpose = purpose;
            if purpose == crate::keysym::PURPOSE_PASSWORD || purpose == crate::keysym::PURPOSE_PIN {
                Some(logic.ui_invalidate())
            } else {
                None
            }
        };
        if let (Some(ui), Some(g)) = (crate::candidate_ui::ui(), gen) {
            ui.hide(&self.0.path, g);
        }
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
                self.aux_show(&msg).await;
                self.schedule_notice_clear();
            }
            _ => {}
        }
    }

    async fn property_show(&self, _name: String) {}

    async fn property_hide(&self, _name: String) {}

    async fn destroy(&self) {
        let gen = {
            let mut logic = self.0.logic.lock().unwrap();
            logic.ui_enabled = false;
            logic.ui_focused = false;
            logic.ui_invalidate()
        };
        if let Some(ui) = crate::candidate_ui::ui() {
            ui.hide(&self.0.path, gen);
        }
        crate::candidate_ui::unregister(&self.0.path);
    }

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
    let _ = crate::candidate_ui::init();
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

#[cfg(test)]
mod tests {
    use super::spawn_setup_child;

    /// 有界等待子进程进入 zombie。刻意不用 Child::wait:那会直接收割,
    /// 掩盖"kill(pid,0) 对 zombie 误判存活"的原始缺陷。
    fn wait_zombie(pid: u32) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(stat) => {
                    // comm 可含空格/括号;最后一个 ") " 之后的首字符即 state
                    if let Some((_, rest)) = stat.rsplit_once(") ") {
                        if rest.chars().next() == Some('Z') {
                            return true;
                        }
                    }
                }
                Err(_) => return false, // 读不到 /proc 不能宣称已退出,直接判超时失败
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    fn argv(prog: &str) -> Vec<String> {
        vec![prog.to_string()]
    }

    #[test]
    fn setup_launcher_reaps_exited_child() {
        // /bin/true 立即退出:三轮拉起都必须成功;每轮新调用须收割上一轮
        // 的 zombie 并拉起新进程(旧实现 kill(pid,0)==0 把 zombie 当存活,
        // 第二次 F7 起永久拒绝)。
        let mut slot: Option<std::process::Child> = None;
        let mut prev_pid: Option<u32> = None;
        for round in 1..=3 {
            assert!(
                spawn_setup_child(&mut slot, &argv("/bin/true")).unwrap(),
                "第 {round} 轮:已退出启动器应被收割并允许再次拉起"
            );
            if let Some(old) = prev_pid {
                assert!(
                    std::fs::metadata(format!("/proc/{old}")).is_err(),
                    "旧 zombie {old} 未被 try_wait 收割,/proc 条目应消失"
                );
            }
            let pid = slot.as_ref().unwrap().id();
            assert!(wait_zombie(pid), "第 {round} 轮子进程 {pid} 未在 2s 内退出");
            prev_pid = Some(pid);
        }
        slot.as_mut().unwrap().wait().unwrap();
    }

    #[test]
    fn setup_launcher_live_child_blocks_duplicate() {
        // /bin/cat 常驻:存活启动器必须拒绝重复拉起且不换槽位进程;
        // 关闭其 stdin 令其退出成 zombie 后,下一次调用应收割并允许拉起。
        let child = std::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let live_pid = child.id();
        let mut slot = Some(child);
        assert!(
            !spawn_setup_child(&mut slot, &argv("/bin/true")).unwrap(),
            "存活启动器必须拒绝重复拉起"
        );
        assert_eq!(slot.as_ref().unwrap().id(), live_pid, "拒绝时不得更换槽位进程");
        drop(slot.as_mut().unwrap().stdin.take()); // 关管道 → cat EOF 退出
        assert!(wait_zombie(live_pid), "/bin/cat 未在 2s 内退出");
        assert!(spawn_setup_child(&mut slot, &argv("/bin/true")).unwrap());
        let pid = slot.as_ref().unwrap().id();
        assert!(wait_zombie(pid));
        slot.as_mut().unwrap().wait().unwrap();
    }

    #[test]
    fn setup_launcher_reopens_owned_xim_child_without_replacing_pid() {
        struct ChildGuard(Option<std::process::Child>);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if let Some(mut c) = self.0.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "lyyime-setup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let alias = dir.join("lyyime-xim");
        std::os::unix::fs::symlink("/bin/sh", &alias).unwrap();
        let marker = dir.join("sig.txt");
        let ready = dir.join("ready.txt");
        let argv = vec![
            alias.to_string_lossy().into_owned(),
            "-c".to_string(),
            r#"trap 'printf r >> "$1"' USR1; printf ready > "$2"; while :; do sleep 0.05; done"#.to_string(),
            "--settings".to_string(),
            marker.to_string_lossy().into_owned(),
            ready.to_string_lossy().into_owned(),
        ];
        let mut slot = ChildGuard(None);
        assert!(spawn_setup_child(&mut slot.0, &argv).unwrap());
        let pid = slot.0.as_ref().unwrap().id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready.is_file() {
            assert!(std::time::Instant::now() < deadline, "子进程就绪文件未生成");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(spawn_setup_child(&mut slot.0, &argv).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(&marker).unwrap_or_default() != "r" {
            assert!(std::time::Instant::now() < deadline, "SIGUSR1 标记未落盘");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(slot.0.as_ref().unwrap().id(), pid, "重开不得更换槽位进程");
        assert!(spawn_setup_child(&mut slot.0, &argv).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::fs::read_to_string(&marker).unwrap_or_default() != "rr" {
            assert!(std::time::Instant::now() < deadline, "第二次 SIGUSR1 标记未落盘");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(slot.0.as_ref().unwrap().id(), pid);
        drop(slot);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_launcher_spawn_failure_leaves_slot_empty() {
        // 绝对路径不存在:spawn 失败须返回 Err 且槽位保持 None;
        // 随后 /bin/true 应正常拉起(失败不得毒化槽位)。
        let mut slot: Option<std::process::Child> = None;
        let bad = vec!["/nonexistent/lyyime-setup-x".to_string()];
        assert!(spawn_setup_child(&mut slot, &bad).is_err());
        assert!(slot.is_none(), "spawn 失败不得占用槽位");
        assert!(spawn_setup_child(&mut slot, &argv("/bin/true")).unwrap());
        let pid = slot.as_ref().unwrap().id();
        assert!(wait_zombie(pid));
        slot.as_mut().unwrap().wait().unwrap();
    }
}
