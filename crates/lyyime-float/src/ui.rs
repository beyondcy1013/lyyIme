//! lyyIme 悬浮窗主界面(GTK3): 类万能五笔外挂悬浮窗。
//!
//! 结构与已删除的 floatapp(Python 版)一致, 保证行为与文档对齐:
//!   标题行: 拖动手柄 + 发送模式切换 + 菜单/隐藏
//!   输入行: Entry 打五笔86; 候选行: 9 个候选按钮 + 翻页;
//!   状态行: 目标窗口 + 码表名 / 错误提示。
//! 发送模式: 直输(xdotool type)/ 粘贴(剪贴板+Ctrl+V)/ 模拟内置(候选贴目标
//! 光标, 见 inline.rs), 长短语自动改粘贴。
//! 不抢焦点模式(keep_target_focus): 悬浮窗不参与 X 焦点, 键盘经 GdkSeat
//! 抓取送达, 目标文本框光标全程保持; 注入时解抓→发送→重抓。
//! 标点(合同 §6): 空缓冲→中文标点直上目标; 有缓冲→先上首选/原码再上标点;
//! 引号按开合交替; 未映射标点维持进输入框。空缓冲退格/删除直通目标窗口删字。
//! 自定义短语(精确码置顶, 动态变量)见 phrases.rs; 管理对话框在本文件。
//!
//! 与 Python 版差异: GTK3 的 Gtk.StatusIcon 在 gtk-rs 0.18 已不可用,
//! 托盘入口由常驻悬浮窗的 ☰ 菜单与"二次启动唤起"承担。
//!
//! 借用约定: 事件回调全部拿 Rc<App>;App 字段用内部可变性,
//! 单一 GTK 主线程访问, 无跨线程共享。

use crate::config::{self, Config};
use crate::dict::WubiDict;
use crate::inline;
use crate::phrases::{expand_text, merged_candidates, now_civil, PhraseBook};
use crate::sender;
use crate::undo;
use crate::xtrack::XTrack;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use lyyime_core::{punct, stats};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const PAGE: usize = 9;
const MAXC: usize = 45;
const WIN_TITLE: &str = "lyyIme 悬浮输入法";
const PAD_TITLE: &str = "中文输入板 - Ctrl+空格切极点五笔, 打完复制粘贴到ZCode";

pub struct App {
    pub(crate) win: gtk::Window,
    pub(crate) entry: gtk::Entry,
    pub(crate) cand_btns: Vec<gtk::Button>,
    pub(crate) prev_btn: gtk::Button,
    pub(crate) next_btn: gtk::Button,
    pub(crate) status: gtk::Label,
    pub(crate) cfg: Rc<RefCell<Config>>,
    pub(crate) dict: Rc<RefCell<Option<Arc<WubiDict>>>>,
    pub(crate) user_freq: Rc<RefCell<HashMap<String, i64>>>,
    pub(crate) phrase: Rc<RefCell<PhraseBook>>,
    pub(crate) cands: Rc<RefCell<Vec<(String, String)>>>,
    pub(crate) page: Rc<Cell<usize>>,
    pub(crate) xtrack: Rc<XTrack>,
    pub(crate) last_target: Rc<RefCell<(u32, String)>>,
    pub(crate) self_xid: Cell<u64>,
    pub(crate) phrase_dlg: RefCell<Option<gtk::Dialog>>,
    /// 最近一次输入动作(打字/上屏),停顿检测基准
    pub(crate) last_activity: Cell<Instant>,
    /// 状态行当前是否在显示今日统计(输入恢复时还原)
    pub(crate) stats_shown: Cell<bool>,
    /// 单/双引号开合状态(中文标点交替输出, 同 core 引擎语义)
    pub(crate) quotes: Cell<punct::QuoteState>,
    /// 编码框撤销/重做栈(Ctrl+Z/Y; 空缓冲直通合同见 on_entry_key)
    pub(crate) undo: Rc<RefCell<undo::Undo>>,
    /// 统一配置 config.toml 监视(设置窗改统计键即时生效)
    pub(crate) stats_monitor: RefCell<Option<gio::FileMonitor>>,
    /// 「模拟内置」候选小窗(贴目标光标) + 光标跟踪器
    pub(crate) inline_pop: inline::InlinePopup,
    pub(crate) caret: Arc<inline::CaretTracker>,
    /// 指针兜底提示只示一次
    pub(crate) ptr_hint: Cell<bool>,
    /// 内置输入法(Mode A)状态前缀缓存(每秒轮询刷新, 变化才重绘状态行)
    pub(crate) engine_prefix: RefCell<String>,
}

/// 构建 App + 全部布线 + 显示 + 定时器。返回后调用方进 gtk::main()。
pub fn create() -> Result<Rc<App>, anyhow::Error> {
    gtk::init().expect("GTK 初始化失败");
    glib::set_prgname(Some("lyyime-float"));
    let xtrack = XTrack::connect()?;
    // 统计三项真源在统一配置 config.toml(设置窗管理);旧 config.json 键回退
    let mut cfg = Config::load();
    let stats = config::load_stats();
    cfg.apply_stats(stats);
    let app = Rc::new(App {
        win: gtk::Window::new(gtk::WindowType::Toplevel),
        entry: gtk::Entry::new(),
        cand_btns: (0..PAGE).map(|_| gtk::Button::new()).collect(),
        prev_btn: gtk::Button::with_label("◀"),
        next_btn: gtk::Button::with_label("▶"),
        status: gtk::Label::new(None),
        cfg: Rc::new(RefCell::new(cfg)),
        dict: Rc::new(RefCell::new(None)),
        user_freq: Rc::new(RefCell::new(config::load_user_freq())),
        phrase: Rc::new(RefCell::new(PhraseBook::open(config::phrase_json_path()))),
        cands: Rc::new(RefCell::new(Vec::new())),
        page: Rc::new(Cell::new(0)),
        xtrack: Rc::new(xtrack),
        last_target: Rc::new(RefCell::new((0, String::new()))),
        self_xid: Cell::new(0),
        phrase_dlg: RefCell::new(None),
        last_activity: Cell::new(Instant::now()),
        stats_shown: Cell::new(false),
        quotes: Cell::new(punct::QuoteState::new()),
        undo: Rc::new(RefCell::new(undo::Undo::new())),
        stats_monitor: RefCell::new(None),
        inline_pop: inline::InlinePopup::new(),
        caret: inline::CaretTracker::spawn(),
        ptr_hint: Cell::new(false),
        engine_prefix: RefCell::new(String::new()),
    });
    wire(&app);
    app.win.show_all();
    let xid = app
        .win
        .window()
        .and_then(|w| w.downcast::<gdkx11::X11Window>().ok())
        .map(|x| x.xid())
        .unwrap_or(0);
    app.self_xid.set(xid);
    if app.cfg.borrow().keep_target_focus {
        apply_focus_mode(&app, true);
    }
    reload_dict(&app);

    // 定时器: 目标窗口追踪 / 置顶保持 / 位置保存 / 输入停顿统计
    {
        let app = app.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            poll_target(&app);
            glib::ControlFlow::Continue
        });
    }
    {
        let app = app.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(5), move || {
            app.reassert_above();
            glib::ControlFlow::Continue
        });
    }
    {
        let app = app.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(3), move || {
            app.save_position();
            glib::ControlFlow::Break
        });
    }
    {
        let app = app.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            poll_stats(&app);
            glib::ControlFlow::Continue
        });
    }
    {
        // 统一设置窗改 config.toml 的统计键 → 悬浮窗即时生效, 无需重启
        let app_m = app.clone();
        let file = gio::File::for_path(config::config_dir().join("config.toml"));
        if let Ok(mon) = file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            mon.connect_changed(move |_, _, _, _| {
                let s = config::load_stats();
                app_m.cfg.borrow_mut().apply_stats(s);
            });
            *app.stats_monitor.borrow_mut() = Some(mon);
        }
    }
    {
        let entry = app.entry.clone();
        glib::idle_add_local_once(move || entry.grab_focus());
    }
    Ok(app)
}

/// 全部事件布线。
fn wire(app: &Rc<App>) {
    let win = &app.win;
    win.set_title(WIN_TITLE);
    win.set_decorated(false);
    win.set_keep_above(true);
    win.set_skip_taskbar_hint(true);
    win.set_skip_pager_hint(true);
    win.set_resizable(false);
    {
        let app = app.clone();
        win.connect_destroy(move |_| {
            app.save_position();
            gtk::main_quit();
        });
    }
    {
        // 隐藏即让出全局键盘(不抢焦点模式下抓取随窗口可见性启停)
        let app = app.clone();
        win.connect_hide(move |_| {
            if app.cfg.borrow().keep_target_focus {
                ungrab_kb(&app);
            }
        });
    }

    let v = gtk::Box::new(gtk::Orientation::Vertical, 4);
    v.set_margin_top(4);
    v.set_margin_bottom(6);
    v.set_margin_start(6);
    v.set_margin_end(6);

    // ---- 标题行: 拖动手柄 + 发送模式 + 菜单/隐藏 ----
    let title = gtk::EventBox::new();
    let th = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let grip = gtk::Label::new(Some("⠿ lyyIme"));
    grip.style_context().add_class("grip");
    th.pack_start(&grip, false, false, 0);

    let combo = gtk::ComboBoxText::new();
    combo.append(Some("type"), "直输模式");
    combo.append(Some("paste"), "粘贴模式");
    combo.append(Some("inline"), "模拟内置(候选贴光标)");
    combo.set_active_id(Some(app.cfg.borrow().send.as_str()));
    {
        let cfg = app.cfg.clone();
        combo.connect_changed(move |c| {
            let mut cfg = cfg.borrow_mut();
            cfg.send = c.active_id().unwrap_or_else(|| "type".into()).into();
            let _ = cfg.save();
        });
    }
    th.pack_start(&combo, false, false, 0);

    let menu_btn = gtk::Button::with_label("☰");
    menu_btn.set_relief(gtk::ReliefStyle::None);
    {
        let app = app.clone();
        menu_btn.connect_clicked(move |_| open_menu(&app));
    }
    th.pack_end(&menu_btn, false, false, 0);

    let hide_btn = gtk::Button::with_label("—");
    hide_btn.set_relief(gtk::ReliefStyle::None);
    {
        let win = app.win.clone();
        hide_btn.connect_clicked(move |_| win.hide());
    }
    th.pack_end(&hide_btn, false, false, 0);

    {
        let win = app.win.clone();
        title.connect_button_press_event(move |_, ev| {
            if ev.button() == 1 {
                let (rx, ry) = ev.root();
                win.begin_move_drag(ev.button() as i32, rx as i32, ry as i32, ev.time());
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    title.add(&th);
    v.pack_start(&title, false, false, 0);

    // ---- 输入行 ----
    app.entry
        .set_placeholder_text(Some("打五笔: 空格上屏 · 数字选词 · 标点/退格直通 · Esc清空"));
    {
        let app2 = app.clone();
        app.entry.connect_changed(move |_| {
            note_activity(&app2);
            refresh_cands(&app2);
        });
        let app3 = app.clone();
        app.entry
            .connect_key_press_event(move |_, ev| on_entry_key(&app3, ev));
    }
    undo::observe_entry(&app.undo, &app.entry);
    v.pack_start(&app.entry, false, false, 0);

    // ---- 候选行 ----
    let cand_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    for (i, b) in app.cand_btns.iter().enumerate() {
        b.style_context().add_class("cand");
        b.set_no_show_all(true);
        let app2 = app.clone();
        b.connect_clicked(move |_| {
            let j = app2.page.get() * PAGE + i;
            let text = app2.cands.borrow().get(j).map(|(t, _)| t.clone());
            if let Some(t) = text {
                commit(&app2, t);
            }
        });
        cand_box.pack_start(b, false, false, 0);
    }
    app.prev_btn.set_no_show_all(true);
    app.next_btn.set_no_show_all(true);
    {
        let app2 = app.clone();
        app.prev_btn.connect_clicked(move |_| flip(&app2, -1));
        let app3 = app.clone();
        app.next_btn.connect_clicked(move |_| flip(&app3, 1));
    }
    cand_box.pack_end(&app.next_btn, false, false, 0);
    cand_box.pack_end(&app.prev_btn, false, false, 0);
    v.pack_start(&cand_box, false, false, 0);

    // ---- 状态行 ----
    app.status.set_ellipsize(pango::EllipsizeMode::Start);
    app.status.set_widget_name("status");
    app.status.set_halign(gtk::Align::Start);
    app.status.set_text("码表加载中…");
    v.pack_start(&app.status, false, false, 0);

    app.win.add(&v);
    if let Some([x, y]) = app.cfg.borrow().position {
        app.win.move_(x, y);
    }

    apply_css();

    // 单实例唤醒(SIGUSR1): 显示+聚焦
    {
        let app = app.clone();
        glib::unix_signal_add_local(libc::SIGUSR1, move || {
            if !app.win.get_visible() {
                app.win.show_all();
            }
            app.win.present();
            app.reassert_above();
            if app.cfg.borrow().keep_target_focus {
                // 显示即接管键盘; 窗口不抢焦点(accept_focus=false), 目标光标不受影响
                grab_kb(&app);
            }
            app.entry.grab_focus();
            glib::ControlFlow::Continue
        });
    }
    // SIGTERM/SIGINT: 存位置再退出
    for sig in [libc::SIGTERM, libc::SIGINT] {
        let app = app.clone();
        glib::unix_signal_add_local(sig, move || {
            app.save_position();
            gtk::main_quit();
            glib::ControlFlow::Break
        });
    }
}

fn apply_css() {
    let css = b"
        window { background: #fbfbf2; border: 1px solid #999; }
        .cand { font-size: 15px; padding: 1px 5px; background: transparent;
                border: none; box-shadow: none; }
        .grip { color: #567; font-weight: bold; padding-right: 6px; }
        #status { color: #6a6a5a; font-size: 10px; }
        #inline-pop { background: rgba(45,45,45,0.96); border: 1px solid #666;
                      border-radius: 4px; }
        #inline-pop label { color: #ffffff; font-size: 14px; padding: 5px 9px; }
        ";
    let prov = gtk::CssProvider::new();
    prov.load_from_data(css).expect("CSS 加载失败");
    if let Some(screen) = gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &prov,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

// ---------- 菜单 ----------
fn open_menu(app: &Rc<App>) {
    let m = gtk::Menu::new();
    let cur_dict = app.cfg.borrow().dict.clone();
    let keep_focus = app.cfg.borrow().keep_target_focus;
    // 不抢焦点模式: 菜单自身要收键盘, 打开期间先让出抓取, 关闭后再重抓
    if keep_focus {
        ungrab_kb(app);
    }
    {
        let app = app.clone();
        m.connect_deactivate(move |_| {
            if app.cfg.borrow().keep_target_focus && app.win.get_visible() {
                grab_kb(&app);
            }
        });
    }
    for (name, val) in [("使用极点86码表", "jidian86"), ("使用海峰86码表", "haifeng86")] {
        let it = gtk::CheckMenuItem::with_label(name);
        it.set_active(cur_dict == val);
        {
            let app = app.clone();
            it.connect_activate(move |_| {
                if app.cfg.borrow().dict != val {
                    app.cfg.borrow_mut().dict = val.to_string();
                    let _ = app.cfg.borrow().save();
                    reload_dict(&app);
                }
            });
        }
        m.append(&it);
    }
    let it = gtk::CheckMenuItem::with_label("光标留在目标窗口(不抢焦点)");
    it.set_active(keep_focus);
    it.set_tooltip_text(Some(
        "悬浮窗不再夺取焦点: 打字与选词期间, 目标文本框的光标保持闪烁。\n\
         代价: 悬浮窗可见期间全局键盘由它接管(含 Alt+Tab 等组合键),\n\
         点标题行「—」隐藏悬浮窗即临时还原正常键盘。",
    ));
    {
        let app = app.clone();
        it.connect_toggled(move |c| {
            let on = c.is_active();
            if app.cfg.borrow().keep_target_focus != on {
                app.cfg.borrow_mut().keep_target_focus = on;
                let _ = app.cfg.borrow().save();
                apply_focus_mode(&app, on);
            }
        });
    }
    m.append(&it);
    m.append(&gtk::SeparatorMenuItem::new());
    let it = gtk::MenuItem::with_label("自定义短语管理…");
    {
        let app = app.clone();
        it.connect_activate(move |_| open_manage_phrases(&app));
    }
    m.append(&it);
    let it = gtk::MenuItem::with_label("显示系统打字板(复制中转)");
    it.connect_activate(|_| launch_pad());
    m.append(&it);
    m.append(&gtk::SeparatorMenuItem::new());
    let it = gtk::MenuItem::with_label("退出");
    {
        let app = app.clone();
        it.connect_activate(move |_| {
            app.save_position();
            config::remove_pidfile();
            gtk::main_quit();
        });
    }
    m.append(&it);
    m.show_all();
    m.popup_at_pointer(None);
}

fn open_manage_phrases(app: &Rc<App>) {
    if let Some(d) = app.phrase_dlg.borrow().as_ref() {
        d.present();
        return;
    }
    let dlg = PhrasesDialog::new(app, Some(&app.win));
    *app.phrase_dlg.borrow_mut() = Some(dlg.dialog.clone());
    {
        let cell = app.phrase_dlg.clone();
        dlg.dialog.connect_destroy(move |_| *cell.borrow_mut() = None);
    }
    dlg.dialog.show_all();
}

// ---------- 码表 ----------
fn reload_dict(app: &Rc<App>) {
    app.status.set_text("码表加载中…");
    let path = format!(
        "/usr/share/ibus-table/tables/wubi-{}.db",
        app.cfg.borrow().dict
    );
    #[allow(deprecated)] // glib 0.18 的 MainContext::channel 虽标记弃用, 主线程同步收发最简
    let (tx, rx) =
        glib::MainContext::channel::<Result<Arc<WubiDict>, String>>(glib::Priority::DEFAULT);
    std::thread::spawn(move || {
        // 单字语料频次 + GB2312 分档(core 词库目录): 缺失时静默降级为按码表频。
        let data_dir = std::env::var("LYYIME_DATA_DIR")
            .unwrap_or_else(|_| "/usr/local/share/lyyime/data".to_string());
        let r = WubiDict::load_with_data(&path, Some(&data_dir))
            .map(Arc::new)
            .map_err(|e| format!("{e:#}"));
        let _ = tx.send(r);
    });
    let app2 = app.clone();
    rx.attach(None, move |res| {
        match res {
            Ok(d) => {
                *app2.dict.borrow_mut() = Some(d);
                update_status(&app2);
                refresh_cands(&app2);
            }
            Err(e) => app2.status.set_text(&format!("码表加载失败: {e}")),
        }
        glib::ControlFlow::Break
    });
}

/// 内置输入法(Mode A, ibus 上的 lyyime 引擎)状态前缀:
/// 引擎把模式/启用态发布到 /tmp/lyyime-engine-state.json(变更即写+10s 心跳),
/// 悬浮窗每秒读它联动显示; 30s 无新鲜心跳视为未运行。
fn engine_prefix() -> String {
    let body = std::fs::read_to_string("/tmp/lyyime-engine-state.json").unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    let fresh = v
        .get("ts")
        .and_then(|t| t.as_u64())
        .map(|ts| {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            now.saturating_sub(ts) < 30_000
        })
        .unwrap_or(false);
    if !fresh {
        return "内置:未运行".into();
    }
    if v.get("enabled") == Some(&serde_json::Value::Bool(false)) {
        return "内置:停用".into();
    }
    match v.get("mode").and_then(|m| m.as_str()) {
        Some("en") => "内置:EN".into(),
        _ => "内置:中".into(),
    }
}

fn update_status(app: &Rc<App>) {
    let t: String = app.last_target.borrow().1.chars().take(30).collect();
    let (dict, keep) = {
        let c = app.cfg.borrow();
        (c.dict.clone(), c.keep_target_focus)
    };
    app.status.set_text(&format!(
        "{} | 目标: {}  |  码表 {}{}",
        engine_prefix(),
        if t.is_empty() { "(无)" } else { &t },
        dict,
        if keep { "  |  不抢焦点" } else { "" }
    ));
}

// ---------- 输入统计(停顿显示今日字数与速度) ----------
/// 当前 epoch 毫秒(统计时间戳用)。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 上屏文本计入今日统计(core stats 模块, 与 ibus 模式共用同一数据目录;
/// 失败静默, 绝不影响输入主链路)。
fn record_stats(text: &str) {
    if let Some(dir) = stats::default_dir() {
        let chars = text.chars().filter(|c| !c.is_whitespace()).count() as u64;
        stats::record(&dir, chars, now_ms());
    }
}

/// 有输入动作: 刷新活动时间; 若统计提示正在显示, 恢复正常状态行。
fn note_activity(app: &Rc<App>) {
    app.last_activity.set(Instant::now());
    if app.stats_shown.get() {
        app.stats_shown.set(false);
        update_status(app);
    }
}

/// 停顿检测(每秒): 输入停顿超过设定秒数且今日有输入时, 状态行改显
/// 「今日已输入 N 字 · 约 M 字/分」; 重新输入由 note_activity 立即还原。
fn poll_stats(app: &Rc<App>) {
    // 内置输入法状态联动: 前缀变化才重绘(统计提示显示期间让位)
    let prefix = engine_prefix();
    if app.engine_prefix.borrow().as_str() != prefix {
        *app.engine_prefix.borrow_mut() = prefix.clone();
        if !app.stats_shown.get() {
            update_status(app);
        }
    }
    let (enabled, pause_secs, idle_exclude) = {
        let c = app.cfg.borrow();
        (
            c.stats_enabled,
            c.stats_pause_secs.max(1) as u64,
            c.stats_idle_exclude_secs as u64,
        )
    };
    let idle = app.last_activity.get().elapsed().as_secs();
    if app.stats_shown.get() {
        // 显示态: 关闭功能或重新开始输入 → 还原
        if !enabled || idle < pause_secs {
            app.stats_shown.set(false);
            update_status(app);
        }
        return;
    }
    if !enabled || !app.win.get_visible() || idle < pause_secs {
        return;
    }
    let Some(dir) = stats::default_dir() else {
        return;
    };
    let s = stats::today_summary(&dir, now_ms(), idle_exclude);
    if s.chars == 0 {
        return; // 今日还没有输入, 无统计可显示
    }
    let text = match s.speed_per_min() {
        Some(spd) => format!("今日已输入 {} 字 · 约 {} 字/分", s.chars, spd),
        None => format!("今日已输入 {} 字", s.chars),
    };
    app.status.set_text(&text);
    app.stats_shown.set(true);
}

/// 输入统计设置已统一到 lyyIme 设置窗口(常规页统计区,写 config.toml
/// 顶层 stats_* 键,悬浮窗经文件监视即时生效);本文件只负责停顿显示。

// ---------- 目标窗口追踪 ----------
fn poll_target(app: &Rc<App>) {
    if let Some(wid) = app.xtrack.active_window() {
        if wid != 0 && wid as u64 != app.self_xid.get() {
            let title = app.xtrack.window_title(wid);
            let changed = {
                let mut lt = app.last_target.borrow_mut();
                if wid != lt.0 || title != lt.1 {
                    *lt = (wid, title);
                    true
                } else {
                    false
                }
            };
            if changed {
                // 「模拟内置」光标助手按窗口标题定位目标应用
                let t = app.last_target.borrow().1.clone();
                let _ = std::fs::write("/tmp/lyyime-float-target", t);
                if app.dict.borrow().is_some() {
                    update_status(app);
                }
            }
        }
    }
}

// ---------- 候选 ----------
fn refresh_cands(app: &Rc<App>) {
    let code = app.entry.text().trim().to_string();
    app.page.set(0);
    let dict = app.dict.borrow().clone();
    *app.cands.borrow_mut() = match dict {
        Some(d) if !code.is_empty() => {
            let uf = app.user_freq.borrow().clone();
            merged_candidates(
                &code,
                &app.phrase.borrow(),
                |c, l| d.lookup(c, l, &uf),
                MAXC,
                None,
            )
        }
        _ => Vec::new(),
    };
    render_cands(app);
}

fn render_cands(app: &Rc<App>) {
    let cands = app.cands.borrow();
    let n = cands.len();
    let start = app.page.get() * PAGE;
    for (i, b) in app.cand_btns.iter().enumerate() {
        let j = start + i;
        if j < n {
            let text = &cands[j].0;
            let chars: Vec<char> = text.chars().collect();
            let shown: String = if chars.len() <= 8 {
                text.clone()
            } else {
                chars[..7].iter().collect::<String>() + "…"
            };
            b.set_label(&format!("{}{}", i + 1, shown));
            // 长候选/长短语看不全, 悬停显示全文
            b.set_tooltip_text(if chars.len() > 8 { Some(text.as_str()) } else { None });
            b.show();
        } else {
            b.set_tooltip_text(None);
            b.hide();
        }
    }
    let multi = n > PAGE;
    app.prev_btn.set_visible(multi);
    app.next_btn.set_visible(multi);
    if multi {
        app.prev_btn.set_sensitive(app.page.get() > 0);
        app.next_btn.set_sensitive((app.page.get() + 1) * PAGE < n);
    }
    // 「模拟内置」: 候选窗贴目标光标(位置源: AT-SPI 光标事件 > 指针位置兜底)
    if app.cfg.borrow().send == "inline" && !app.entry.text().trim().is_empty() {
        let page_cands: Vec<String> = cands[start..n.min(start + PAGE)]
            .iter()
            .map(|(t, _)| t.clone())
            .collect();
        let mk = inline::markup(app.entry.text().trim(), &page_cands);
        let (sw, sh) = app.xtrack.screen_size();
        let (ax, ay) = match app.caret.latest(120) {
            Some(cp) => {
                app.ptr_hint.set(false);
                // 助手坐标是目标窗口客户区相对值, 加上窗口根位置得屏幕绝对坐标
                let (wid, _) = *app.last_target.borrow();
                let (rx, ry) = app.xtrack.window_root_position(wid).unwrap_or((0, 0));
                inline::anchor(
                    inline::CaretPos { x: rx + cp.x, y: ry + cp.y, h: cp.h },
                    sw,
                    sh,
                )
            }
            None => {
                let (px, py) = app.xtrack.pointer_position().unwrap_or((200, 200));
                if !app.ptr_hint.replace(true) {
                    app.status.set_text(
                        "模拟内置: 未取到目标光标(应用需支持无障碍), 候选暂贴指针处",
                    );
                }
                inline::anchor(inline::CaretPos { x: px, y: py, h: 26 }, sw, sh)
            }
        };
        app.inline_pop.update(Some((mk, ax, ay)));
    } else {
        app.inline_pop.update(None);
    }
}

fn flip(app: &Rc<App>, d: i32) {
    let np = app.page.get() as i32 + d;
    if np >= 0 && (np as usize) * PAGE < app.cands.borrow().len().max(1) {
        app.page.set(np as usize);
        render_cands(app);
    }
}

// ---------- 按键 ----------
// GDK keysyms: ASCII 数字与 ASCII 同值; 功能键在 0xFF00 区。
const KEY_SPACE: u32 = 0x0020;
const KEY_1: u32 = 0x0031;
const KEY_9: u32 = 0x0039;
const KEY_MINUS: u32 = 0x002d;
const KEY_EQUAL: u32 = 0x003d;
const KEY_RETURN: u32 = 0xff0d;
const KEY_ESCAPE: u32 = 0xff1b;
const KEY_BACKSPACE: u32 = 0xff08;
const KEY_DELETE: u32 = 0xffff;
const KEY_Z: u32 = 0x007a;
const KEY_Y: u32 = 0x0079;
const KEY_PAGE_UP: u32 = 0xff55;
const KEY_PAGE_DOWN: u32 = 0xff56;

/// 标点收尾的上屏文本(合同 §6): 有缓冲先带出页内首选(无候选=原始字母,
/// 同 core finish_text), 再拼上中文标点; 空缓冲即标点本身。
fn punct_commit_text(code: &str, top: Option<&str>, punct: char) -> String {
    let mut s = match top {
        Some(t) if !code.is_empty() => t.to_string(),
        _ => code.to_string(),
    };
    s.push(punct);
    s
}

/// 查中文标点映射; 命中时同步引号开合状态进 App。
fn mapped_punct(app: &Rc<App>, c: char) -> Option<char> {
    let mut q = app.quotes.get();
    let m = punct::to_chinese(c, &mut q);
    if m.is_some() {
        app.quotes.set(q);
    }
    m
}

/// 空缓冲的退格/删除直通目标窗口(合同 §6「空:Pass」的悬浮窗等价实现:
/// 应用收到该键, 删的是目标文本框里的字符)。保持悬浮窗焦点可连续操作。
fn passthrough_key(app: &Rc<App>, key: &str) {
    let (wid, _) = *app.last_target.borrow();
    if wid == 0 {
        app.status.set_text("⚠ 先点一下要输入字的应用窗口, 再回来打字");
        return;
    }
    if let Err(e) = release_kb_while(app, || sender::send_key(wid, key)) {
        app.status.set_text(&format!("⚠ {e}"));
    }
    refocus_self(app);
}

/// 上屏后焦点回悬浮窗继续打字。
fn refocus_self(app: &Rc<App>) {
    // 不抢焦点模式: 绝不 X 激活自己(目标焦点/光标就是本模式的全部意义),
    // 只恢复悬浮窗内部的 Entry 焦点, 键盘仍经抓取送达
    if !app.cfg.borrow().keep_target_focus {
        let xid = app.self_xid.get();
        if xid != 0 {
            // 异步回焦(同步等待会被 WM 的防窃取策略挂死主循环);
            // 回焦与下一次上屏之间隔着手动打字的时间, 竞态窗口足够小
            let _ = std::process::Command::new("xdotool")
                .args(["windowactivate", &xid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            let _ = std::process::Command::new("xdotool")
                .args(["windowfocus", &xid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
    let entry = app.entry.clone();
    glib::idle_add_local_once(move || entry.grab_focus());
}

// ---------- 不抢焦点模式(keep_target_focus) ----------
// 悬浮窗不参与 X 焦点(accept_focus/focus_on_map=false): 点击、映射都不夺焦,
// 目标文本框的光标全程保持。键盘经 GdkSeat 抓取(等价 XGrabKeyboard 主动抓)
// 仍送达悬浮窗 Entry; owner_events=true 保证本进程自己的窗口(菜单/对话框)
// 照常收键。隐藏窗口即让出键盘。
fn focus_kept(app: &Rc<App>) -> bool {
    app.cfg.borrow().keep_target_focus
}

fn grab_kb(app: &Rc<App>) {
    if attempt_grab(app) {
        return;
    }
    // 窗口刚显示时 WM 可能尚未完成映射(NotViewable), 短暂重试至多 5s
    let app = app.clone();
    let mut tries = 0u32;
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        tries += 1;
        if !focus_kept(&app) || attempt_grab(&app) || tries >= 50 {
            if !focus_kept(&app) {
                return glib::ControlFlow::Break;
            }
            if tries >= 50 {
                app.status
                    .set_text("⚠ 键盘抓取失败, 不抢焦点模式未生效");
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// 单次抓取尝试。Success=抓到; AlreadyGrabbed=已有键盘抓取(极可能就是
/// 自己上一次抓取未解, 重入安全), 都视为持有。失败打 stderr 供排障取证。
fn attempt_grab(app: &Rc<App>) -> bool {
    let Some(gdkwin) = app.win.window() else {
        return false;
    };
    let Some(seat) = gdk::Display::default().and_then(|d| d.default_seat()) else {
        return false;
    };
    let status = seat.grab(
        &gdkwin,
        gdk::SeatCapabilities::KEYBOARD,
        true,
        None,
        None,
        None,
    );
    let ok = status == gdk::GrabStatus::Success || status == gdk::GrabStatus::AlreadyGrabbed;
    if !ok {
        eprintln!("lyyime-float: 键盘抓取失败 = {status:?}");
    }
    ok
}

fn ungrab_kb(_app: &Rc<App>) {
    if let Some(seat) = gdk::Display::default().and_then(|d| d.default_seat()) {
        seat.ungrab();
    }
}

/// 启停不抢焦点模式: 焦点提示 + 键盘抓取启停。菜单一键切换, 改动即保存。
fn apply_focus_mode(app: &Rc<App>, on: bool) {
    if let Some(gdkwin) = app.win.window() {
        gdkwin.set_accept_focus(!on);
        gdkwin.set_focus_on_map(!on);
    }
    if on {
        grab_kb(app);
    } else {
        ungrab_kb(app);
    }
    update_status(app);
}

/// XTest 注入(xdotool)期间必须临时让出键盘抓取: 抓取在, 注入会被自己的
/// grab 拦回悬浮窗而不是进目标窗口。让出窗口期用户按键直达目标窗口(本就
/// 持有焦点, 与常规模式注入时序一致)。
fn release_kb_while<T>(app: &Rc<App>, f: impl FnOnce() -> T) -> T {
    if !focus_kept(app) {
        return f();
    }
    ungrab_kb(app);
    let r = f();
    grab_kb(app);
    r
}

fn on_entry_key(app: &Rc<App>, ev: &gdk::EventKey) -> glib::Propagation {
    let kv = *ev.keyval(); // Key 解引用为 u32 keysym
    let st = ev.state();
    // 撤销/重做(Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z; GTK3 Entry 无内建 undo):
    // 有本地历史 → 撤销框内编辑; 空缓冲且无可撤销(刚上屏/刚清空)→ 直通目标
    // 窗口撤销/重做刚上屏的文本(与空缓冲退格直通同一合同)。
    if st.contains(gdk::ModifierType::CONTROL_MASK) {
        match undo::handle(undo::Target::Entry(&app.entry), &app.undo, ev) {
            undo::KeyOutcome::Handled => return glib::Propagation::Stop,
            undo::KeyOutcome::NothingToDo => {
                if app.entry.text().is_empty() {
                    // Shift+Z 与 Y 都是重做语义, 直通统一用 ctrl+y 表达
                    let key = if kv == KEY_Y || st.contains(gdk::ModifierType::SHIFT_MASK) {
                        "ctrl+y"
                    } else {
                        "ctrl+z"
                    };
                    passthrough_key(app, key);
                    return glib::Propagation::Stop;
                }
                // 有缓冲但无可撤销: 吞掉, 不透传其它含义
                return glib::Propagation::Stop;
            }
            undo::KeyOutcome::NotOurs => {}
        }
    }
    // Ctrl/Alt 组合键放行(Entry 复制粘贴等快捷键), 不参与候选/标点/直通
    if st.contains(gdk::ModifierType::CONTROL_MASK)
        || st.contains(gdk::ModifierType::MOD1_MASK)
    {
        return glib::Propagation::Proceed;
    }
    match kv {
        KEY_SPACE => {
            // CapsLock 大写态(合同 §6):字母直通英文,原样上屏输入框内容
            // (含大写),不顶屏中文候选;非大写态维持候选顶屏。
            if st.contains(gdk::ModifierType::LOCK_MASK) {
                let code = app.entry.text().trim().to_string();
                if !code.is_empty() {
                    commit(app, code);
                }
                return glib::Propagation::Stop;
            }
            let text = app
                .cands
                .borrow()
                .get(app.page.get() * PAGE)
                .map(|(t, _)| t.clone());
            if let Some(t) = text {
                commit(app, t);
            }
            glib::Propagation::Stop
        }
        // 数字 1–9 选候选; 无候选也恒吞(与 Python 版一致)
        KEY_1..=KEY_9 => {
            let idx = (kv - KEY_1) as usize;
            let j = app.page.get() * PAGE + idx;
            let text = app.cands.borrow().get(j).map(|(t, _)| t.clone());
            if let Some(t) = text {
                commit(app, t);
            }
            glib::Propagation::Stop
        }
        KEY_RETURN => {
            let code = app.entry.text().trim().to_string();
            if !code.is_empty() {
                commit(app, code); // 原样上屏(英文)
            }
            glib::Propagation::Stop
        }
        KEY_ESCAPE => {
            app.entry.set_text("");
            app.page.set(0);
            app.undo.borrow_mut().reset();
            glib::Propagation::Stop
        }
        KEY_MINUS | KEY_PAGE_UP => {
            flip(app, -1);
            glib::Propagation::Stop
        }
        KEY_EQUAL | KEY_PAGE_DOWN => {
            flip(app, 1);
            glib::Propagation::Stop
        }
        KEY_BACKSPACE | KEY_DELETE => {
            if app.entry.text().is_empty() {
                // 空缓冲: 退格/删除直通目标窗口, 删目标文本框字符
                let name = if kv == KEY_BACKSPACE { "BackSpace" } else { "Delete" };
                passthrough_key(app, name);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed // 非空: GTK 原生删尾
            }
        }
        _ => {
            // 中文标点(合同 §6): 空缓冲→上中文标点; 有缓冲→先上首选/原码
            // 再上中文标点(合并一次发送)。未映射标点不拦截, 维持进 Entry。
            if let Some(p) = gdk::keys::Key::from(kv).to_unicode().and_then(|c| mapped_punct(app, c)) {
                let code = app.entry.text().trim().to_string();
                let top = app.cands.borrow().get(app.page.get() * PAGE).map(|(t, _)| t.clone());
                commit(app, punct_commit_text(&code, top.as_deref(), p));
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        }
    }
}

fn commit(app: &Rc<App>, text: String) {
    let (wid, _) = *app.last_target.borrow();
    if wid == 0 {
        app.status.set_text("⚠ 先点一下要输入字的应用窗口, 再回来打字");
        return;
    }
    let mut method = app.cfg.borrow().send.clone();
    if method == "inline" {
        // 模拟内置: 候选贴光标只是显示层, 上屏仍走直输注入
        method = "type".into();
    }
    let mut note = String::new();
    if method == "type" && text.chars().count() > sender::AUTO_PASTE_LEN {
        // 长短语直输逐字注入太慢, 自动改粘贴(会覆盖剪贴板)
        method = "paste".into();
        note = "长短语已自动用粘贴方式上屏(剪贴板被占用) | ".into();
    }
    let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    match release_kb_while(app, || sender::send(wid, &text, &method, Some(&clipboard))) {
        Ok(()) => {
            // 输入统计(停顿显示的数据层): 上屏即记一次非空白字符数
            record_stats(&text);
            // 选词学习: 用户词 +1, 持久化
            *app.user_freq.borrow_mut().entry(text.clone()).or_insert(0) += 1;
            config::save_user_freq(&app.user_freq.borrow());
            if !note.is_empty() {
                app.status.set_text(&format!("⚠ {note}"));
            }
        }
        Err(e) => app.status.set_text(&format!("⚠ {e}")),
    }
    app.entry.set_text("");
    app.page.set(0);
    // 编码是瞬态的, 撤销历史按词重开; 此后空缓冲 Ctrl+Z 直通目标撤销刚上屏文本
    app.undo.borrow_mut().reset();
    // 焦点回悬浮窗继续打字
    refocus_self(app);
}

// ---------- 位置/置顶 ----------
impl App {
    fn save_position(&self) {
        let (x, y) = self.win.position();
        self.cfg.borrow_mut().position = Some([x, y]);
        let _ = self.cfg.borrow().save();
    }

    fn reassert_above(&self) {
        self.win.set_keep_above(true);
    }
}

fn launch_pad() {
    let exe = std::env::current_exe().unwrap_or_else(|_| "lyyime-float".into());
    let _ = std::process::Command::new(exe).arg("--pad").spawn();
}

// ---------- 打字板(--pad) ----------
pub fn run_pad() {
    gtk::init().expect("GTK 初始化失败");
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title(PAD_TITLE);
    win.set_default_size(560, 320);
    win.set_keep_above(true);
    win.connect_destroy(|_| gtk::main_quit());

    let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
    v.set_margin_top(10);
    v.set_margin_bottom(10);
    v.set_margin_start(10);
    v.set_margin_end(10);
    let tip = gtk::Label::new(Some(
        "Ctrl+空格 / Win+空格 切换中英文\n打字 → Ctrl+A 全选 → Ctrl+C 复制 → 去应用里 Ctrl+V",
    ));
    v.pack_start(&tip, false, false, 0);
    let tv = gtk::TextView::new();
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    undo::wire_text_view(&tv); // Ctrl+Z / Ctrl+Y 撤销重做
    let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_vexpand(true);
    scroll.add(&tv);
    v.pack_start(&scroll, true, true, 0);
    win.add(&v);
    win.connect_key_press_event(|_, e| {
        glib::Propagation::from(*e.keyval() == KEY_ESCAPE) // Escape 关闭
    });
    win.show_all();
    gtk::main();
}

// ---------- 短语管理对话框 ----------
pub struct PhrasesDialog {
    pub dialog: gtk::Dialog,
    pub store: gtk::ListStore,
    pub reload: Rc<dyn Fn()>,
}

impl PhrasesDialog {
    /// parent=None 时(冒烟)无瞬态窗。改动即写盘并刷新主窗候选。
    pub fn new(app: &Rc<App>, parent: Option<&gtk::Window>) -> PhrasesDialog {
        let dlg = gtk::Dialog::with_buttons(
            Some("自定义短语管理"),
            parent,
            gtk::DialogFlags::empty(),
            &[("关闭", gtk::ResponseType::Close)],
        );
        dlg.set_default_size(560, 420);
        {
            let d = dlg.clone();
            dlg.connect_response(move |_, _| unsafe { d.destroy() });
        }

        let box_ = dlg.content_area();
        box_.set_spacing(6);
        box_.set_border_width(8);
        let hint = gtk::Label::new(Some(
            "编码 = 1–12 个小写字母; 文本不限长度(可换行)。\n\
             输入编码精确命中后, 短语排在候选最前, 空格直接上屏。\n\
             文本可含动态变量(候选与上屏时实时展开): $date(yyyy年M月d日) \
             $time(HH:mm) $week; $$ 为字面 $。",
        ));
        hint.set_halign(gtk::Align::Start);
        hint.style_context().add_class("dim-label");
        box_.pack_start(&hint, false, false, 0);

        let store = gtk::ListStore::new(&[glib::Type::STRING; 3]); // 编码, 内容, 上屏预览
        let view = gtk::TreeView::with_model(&store);
        view.set_tooltip_column(1);

        let make_col = |title: &str, id: i32, min_w: i32, ellipsize: bool| {
            let cell = gtk::CellRendererText::new();
            if ellipsize {
                cell.set_ellipsize(pango::EllipsizeMode::End);
            }
            let col = gtk::TreeViewColumn::new();
            col.set_title(title);
            col.set_min_width(min_w);
            if id == 1 {
                col.set_expand(true);
            }
            gtk::prelude::CellLayoutExt::pack_start(&col, &cell, true);
            gtk::prelude::CellLayoutExt::add_attribute(&col, &cell, "text", id);
            view.append_column(&col);
        };
        make_col("编码", 0, 90, false);
        make_col("短语内容", 1, -1, true);
        let dim_cell = gtk::CellRendererText::new();
        dim_cell.set_ellipsize(pango::EllipsizeMode::End);
        dim_cell.set_foreground(Some("dim"));
        let col3 = gtk::TreeViewColumn::new();
        col3.set_title("上屏预览");
        col3.set_min_width(110);
        gtk::prelude::CellLayoutExt::pack_start(&col3, &dim_cell, true);
        gtk::prelude::CellLayoutExt::add_attribute(&col3, &dim_cell, "text", 2);
        view.append_column(&col3);

        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_shadow_type(gtk::ShadowType::In);
        scroll.set_vexpand(true);
        scroll.add(&view);
        box_.pack_start(&scroll, true, true, 0);

        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let add_btn = gtk::Button::with_label("新增");
        let edit_btn = gtk::Button::with_label("编辑");
        let del_btn = gtk::Button::with_label("删除");
        btns.pack_start(&add_btn, false, false, 0);
        btns.pack_start(&edit_btn, false, false, 0);
        btns.pack_start(&del_btn, false, false, 0);
        box_.pack_start(&btns, false, false, 0);

        // reload: 从短语簿重建列表(含动态变量上屏预览)
        let reload: Rc<dyn Fn()> = {
            let store = store.clone();
            let book = app.phrase.clone();
            Rc::new(move || {
                store.clear();
                let now = now_civil();
                for (code, texts) in book.borrow().snapshot() {
                    for t in texts {
                        let preview = expand_text(&t, now);
                        store.insert_with_values(None, &[
                            (0, &code.to_value()),
                            (1, &t.to_value()),
                            (2, &preview.to_value()),
                        ]);
                    }
                }
            })
        };
        reload();

        let selected = {
            let store = store.clone();
            let view = view.clone();
            move || -> Option<(String, String)> {
                let sel = view.selection();
                sel.selected().map(|(_, it)| {
                    let code: String =
                        store.value(&it, 0).get().unwrap_or_default();
                    let text: String =
                        store.value(&it, 1).get().unwrap_or_default();
                    (code, text)
                })
            }
        };

        {
            let app = app.clone();
            let reload = reload.clone();
            add_btn.connect_clicked(move |_| {
                PhraseEditor::new(&app, None, reload.clone()).dialog.show_all();
            });
        }
        {
            let app = app.clone();
            let reload = reload.clone();
            let selected = selected.clone();
            let parent = dlg.clone();
            edit_btn.connect_clicked(move |_| {
                if let Some(old) = selected() {
                    PhraseEditor::new(&app, Some(old), reload.clone())
                        .dialog
                        .show_all();
                } else {
                    info_dialog(&parent, "请先在列表中选中一条短语");
                }
            });
        }
        {
            let app = app.clone();
            let reload = reload.clone();
            let selected = selected.clone();
            let parent = dlg.clone();
            del_btn.connect_clicked(move |_| {
                let Some((code, text)) = selected() else {
                    info_dialog(&parent, "请先在列表中选中一条短语");
                    return;
                };
                let mut book = app.phrase.borrow_mut();
                if let Err(e) = book.remove(&code, &text) {
                    drop(book);
                    info_dialog(&parent, &e);
                    reload();
                    return;
                }
                let _ = book.save();
                drop(book);
                reload();
                refresh_cands(&app);
            });
        }

        PhrasesDialog { dialog: dlg, store, reload }
    }
}

fn info_dialog(parent: &gtk::Dialog, msg: &str) {
    let md = gtk::MessageDialog::new(
        Some(parent),
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Info,
        gtk::ButtonsType::Ok,
        msg,
    );
    md.run();
    unsafe { md.destroy() };
}

struct PhraseEditor {
    dialog: gtk::Dialog,
    #[allow(dead_code)]
    code_entry: gtk::Entry,
    #[allow(dead_code)]
    text_view: gtk::TextView,
}

impl PhraseEditor {
    /// old: None=新增; Some((code, text))=编辑。保存走真实 response 回调链。
    fn new(
        app: &Rc<App>,
        old: Option<(String, String)>,
        reload: Rc<dyn Fn()>,
    ) -> PhraseEditor {
        let title = if old.is_none() { "新增短语" } else { "编辑短语" };
        let dlg = gtk::Dialog::with_buttons(
            Some(title),
            Some(&app.win),
            gtk::DialogFlags::MODAL,
            &[("取消", gtk::ResponseType::Cancel), ("保存", gtk::ResponseType::Ok)],
        );
        dlg.set_default_response(gtk::ResponseType::Ok);
        dlg.set_default_size(480, 320);

        let box_ = dlg.content_area();
        box_.set_spacing(6);
        box_.set_border_width(8);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.pack_start(&gtk::Label::new(Some("编码:")), false, false, 0);
        let code_entry = gtk::Entry::new();
        code_entry.set_max_length(12);
        code_entry.set_placeholder_text(Some("1–12 个小写字母, 如 yf"));
        row.pack_start(&code_entry, true, true, 0);
        box_.pack_start(&row, false, false, 0);
        // 编码框撤销/重做: 观察变更 + 拦 Ctrl+Z/Y/Shift+Z(无空缓冲直通问题)
        let code_undo = Rc::new(RefCell::new(undo::Undo::new()));
        undo::observe_entry(&code_undo, &code_entry);
        {
            let code_undo = code_undo.clone();
            code_entry.connect_key_press_event(move |ce, ev| {
                if let undo::KeyOutcome::NotOurs =
                    undo::handle(undo::Target::Entry(ce), &code_undo, ev)
                {
                    glib::Propagation::Proceed
                } else {
                    glib::Propagation::Stop
                }
            });
        }
        box_.pack_start(
            &gtk::Label::new(Some("短语内容(任意长度, 可换行):")),
            false,
            false,
            0,
        );
        let text_view = gtk::TextView::new();
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
        undo::wire_text_view(&text_view); // Ctrl+Z / Ctrl+Y 撤销重做
        let scroll = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        scroll.set_shadow_type(gtk::ShadowType::In);
        scroll.set_size_request(-1, 160);
        scroll.set_vexpand(true);
        scroll.add(&text_view);
        box_.pack_start(&scroll, true, true, 0);

        let var_tip = gtk::Label::new(Some(
            "动态变量(上屏时展开): $date(yyyy年M月d日) $time(HH:mm) $week; \
             $$ = 字面 $;月=M 分=m 秒=s, 补零用双写(MM/dd/HH/mm/ss)。",
        ));
        var_tip.set_halign(gtk::Align::Start);
        var_tip.style_context().add_class("dim-label");
        box_.pack_start(&var_tip, false, false, 0);
        let var_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        var_row.pack_start(&gtk::Label::new(Some("插入变量:")), false, false, 0);
        for (label, snippet) in [
            ("今天日期", "$date(yyyy-MM-dd)"),
            ("中文日期", "$date(yyyy年M月d日)"),
            ("时间", "$time(HH:mm)"),
            ("星期", "$week"),
        ] {
            let b = gtk::Button::with_label(label);
            b.set_relief(gtk::ReliefStyle::None);
            {
                let tv = text_view.clone();
                b.connect_clicked(move |_| {
                    // 在短语内容光标处插入变量片段(点按钮后焦点回文本区可继续编辑)
                    tv.buffer()
                        .expect("TextView 应有默认 buffer")
                        .insert_at_cursor(snippet);
                    tv.grab_focus();
                });
            }
            var_row.pack_start(&b, false, false, 0);
        }
        box_.pack_start(&var_row, false, false, 0);

        if let Some((code, text)) = &old {
            code_entry.set_text(code);
            text_view
                .buffer()
                .expect("TextView 应有默认 buffer")
                .set_text(text);
        }

        let app = app.clone();
        let ce = code_entry.clone();
        let tv = text_view.clone();
        dlg.connect_response(move |d, r| {
            if r != gtk::ResponseType::Ok {
                unsafe { d.destroy() };
                return;
            }
            let code = ce.text().trim().to_lowercase();
            let buf = tv.buffer().expect("TextView 应有默认 buffer");
            let text = buf
                .text(&buf.start_iter(), &buf.end_iter(), false)
                .unwrap_or_default()
                .to_string();
            let mut book = app.phrase.borrow_mut();
            let res = match &old {
                None => book.add(&code, &text),
                Some((oc, ot)) => book.update(oc, ot, &code, &text),
            };
            if let Err(e) = res {
                drop(book);
                let md = gtk::MessageDialog::new(
                    Some(d),
                    gtk::DialogFlags::MODAL,
                    gtk::MessageType::Error,
                    gtk::ButtonsType::Ok,
                    &format!("保存失败: {e}"),
                );
                md.run();
                unsafe { md.destroy() };
                return; // 留在编辑器让用户改
            }
            let _ = book.save();
            drop(book);
            reload();
            refresh_cands(&app);
            unsafe { d.destroy() };
        });

        PhraseEditor { dialog: dlg, code_entry, text_view }
    }
}

/// --smoke-phrases: 短语管理对话框冒烟(e2e 用, 不进主循环)。
/// 以编程方式预填编辑器并触发 response, 走真实保存回调链后断言。
pub fn run_smoke_phrases() -> Result<(), String> {
    gtk::init().map_err(|e| format!("GTK 初始化失败: {e}"))?;
    let app = create_app_for_smoke()?;
    let dlg = PhrasesDialog::new(&app, None);
    dlg.dialog.show_all();
    pump_events();

    // 新增: uiy → 界面新增短语
    let ed = PhraseEditor::new(&app, None, dlg.reload.clone());
    ed.code_entry.set_text("uiy");
    ed.text_view
        .buffer()
        .expect("TextView 应有默认 buffer")
        .set_text("界面新增短语");
    ed.dialog.response(gtk::ResponseType::Ok); // 同步触发真实保存链
    pump_events();
    assert_eq!(app.phrase.borrow().lookup("uiy"), vec!["界面新增短语"]);
    assert!(config::phrase_json_path().exists(), "应已落盘");
    assert_eq!(rows(&dlg.store), 1, "列表应刷新为 1 行");

    // 编辑改码: uiy → uiz
    let old = ("uiy".to_string(), "界面新增短语".to_string());
    let ed2 = PhraseEditor::new(&app, Some(old), dlg.reload.clone());
    ed2.code_entry.set_text("uiz");
    ed2.text_view
        .buffer()
        .expect("TextView 应有默认 buffer")
        .set_text("改码后的短语");
    ed2.dialog.response(gtk::ResponseType::Ok);
    pump_events();
    {
        let book = app.phrase.borrow();
        assert!(book.lookup("uiy").is_empty());
        assert_eq!(book.lookup("uiz"), vec!["改码后的短语"]);
    }

    println!("DIALOG-OK");
    Ok(())
}

/// --smoke-stats: 输入统计冒烟(e2e 用, 不进主循环):
/// 上屏记录 → 停顿显示 → 输入恢复;HOME 由 e2e 脚本指向临时目录,
/// 不读写用户真实统计。断言失败即 panic(进程非 0 退出)。
pub fn run_smoke_stats() -> Result<(), String> {
    gtk::init().map_err(|e| format!("GTK 初始化失败: {e}"))?;
    let app = create_app_for_smoke()?;
    app.win.show_all();
    pump_events();

    // ① 上屏即记录: 4 个非空白字
    record_stats("你好世界");
    let dir = stats::default_dir().ok_or("无统计目录(HOME 未设置)")?;
    let s = stats::today_summary(&dir, now_ms(), 30);
    assert_eq!(s.chars, 4, "记录后今日字数应为 4");
    assert_eq!(s.events, 1);

    // ② 未达停顿阈值(刚输入完): 不显示统计
    poll_stats(&app);
    assert!(!app.stats_shown.get(), "刚输入完不应显示统计");
    assert!(!app.status.text().contains("今日已输入"), "状态行不应是统计: {}", app.status.text());

    // ③ 停顿超过阈值(默认 10s → 回拨 11s): 状态行显示统计
    app.last_activity
        .set(Instant::now() - std::time::Duration::from_secs(11));
    poll_stats(&app);
    assert!(app.stats_shown.get(), "停顿后应显示统计");
    let shown = app.status.text().to_string();
    assert!(
        shown.contains("今日已输入 4 字"),
        "状态行应含今日字数: {shown}"
    );
    // 单次上屏无活跃时长 → 不显示速度
    assert!(!shown.contains("字/分"), "单次上屏不应显示速度: {shown}");

    // ④ 重新输入: 立即还原状态行
    note_activity(&app);
    assert!(!app.stats_shown.get(), "输入恢复后应还原状态行");
    assert!(
        !app.status.text().contains("今日已输入"),
        "还原后状态行不应是统计: {}",
        app.status.text()
    );

    println!("STATS-OK");
    Ok(())
}

fn pump_events() {
    let ctx = glib::MainContext::default();
    while ctx.pending() {
        ctx.iteration(false);
    }
}

fn rows(store: &gtk::ListStore) -> usize {
    let mut n = 0;
    store.foreach(|_, _, _| {
        n += 1;
        true
    });
    n
}

/// 冒烟专用: 构造完整 App(不启动定时器/码表加载)。
fn create_app_for_smoke() -> Result<Rc<App>, String> {
    let xtrack =
        XTrack::connect().map_err(|e| crate::xtrack::connect_error_report(e))?;
    Ok(Rc::new(App {
        win: gtk::Window::new(gtk::WindowType::Toplevel),
        entry: gtk::Entry::new(),
        cand_btns: (0..PAGE).map(|_| gtk::Button::new()).collect(),
        prev_btn: gtk::Button::with_label("◀"),
        next_btn: gtk::Button::with_label("▶"),
        status: gtk::Label::new(None),
        cfg: Rc::new(RefCell::new(Config::default())),
        dict: Rc::new(RefCell::new(None)),
        user_freq: Rc::new(RefCell::new(HashMap::new())),
        phrase: Rc::new(RefCell::new(PhraseBook::open(config::phrase_json_path()))),
        cands: Rc::new(RefCell::new(Vec::new())),
        page: Rc::new(Cell::new(0)),
        xtrack: Rc::new(xtrack),
        last_target: Rc::new(RefCell::new((0, String::new()))),
        self_xid: Cell::new(0),
        phrase_dlg: RefCell::new(None),
        last_activity: Cell::new(Instant::now()),
        stats_shown: Cell::new(false),
        quotes: Cell::new(punct::QuoteState::new()),
        undo: Rc::new(RefCell::new(undo::Undo::new())),
        stats_monitor: RefCell::new(None),
        inline_pop: inline::InlinePopup::new(),
        caret: inline::CaretTracker::spawn(),
        ptr_hint: Cell::new(false),
        engine_prefix: RefCell::new(String::new()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空缓冲标点只上标点本身() {
        assert_eq!(punct_commit_text("", None, '，'), "，");
    }

    #[test]
    fn 有缓冲标点先带首选再上标点() {
        assert_eq!(punct_commit_text("nk", Some("你"), '，'), "你，");
        assert_eq!(punct_commit_text("nk", Some("你"), '。'), "你。");
    }

    #[test]
    fn 无候选时标点带出原始字母() {
        // 同 core finish_text: 页内首选 > 原始字母
        assert_eq!(punct_commit_text("wxyz", None, '。'), "wxyz。");
    }

    #[test]
    fn 引号开合交替且命中才推进状态() {
        let app_quotes = Cell::new(punct::QuoteState::new());
        let a = mapped_punct_for_test(&app_quotes, '\'');
        let b = mapped_punct_for_test(&app_quotes, '\'');
        assert_eq!(a, Some('\u{2018}'));
        assert_eq!(b, Some('\u{2019}'));
        // 未映射标点不推进状态
        assert_eq!(mapped_punct_for_test(&app_quotes, '/'), None);
        assert_eq!(mapped_punct_for_test(&app_quotes, '\''), Some('\u{2018}'));
    }

    /// mapped_punct 依赖 Rc<App>(GTK 构造), 测试里只抽引号状态这一小块。
    fn mapped_punct_for_test(q: &Cell<punct::QuoteState>, c: char) -> Option<char> {
        let mut s = q.get();
        let m = punct::to_chinese(c, &mut s);
        if m.is_some() {
            q.set(s);
        }
        m
    }
}
