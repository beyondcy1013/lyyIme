//! lyyIme 悬浮窗主界面(GTK3): 类万能五笔外挂悬浮窗。
//!
//! 结构与已删除的 floatapp(Python 版)一致, 保证行为与文档对齐:
//!   标题行: 拖动手柄 + 发送模式切换 + 菜单/隐藏
//!   输入行: Entry 打五笔86; 候选行: 9 个候选按钮 + 翻页;
//!   状态行: 目标窗口 + 码表名 / 错误提示。
//! 发送模式: 直输(xdotool type)/ 粘贴(剪贴板+Ctrl+V), 长短语自动改粘贴。
//! 自定义短语(精确码置顶, 动态变量)见 phrases.rs; 管理对话框在本文件。
//!
//! 与 Python 版差异: GTK3 的 Gtk.StatusIcon 在 gtk-rs 0.18 已不可用,
//! 托盘入口由常驻悬浮窗的 ☰ 菜单与"二次启动唤起"承担。
//!
//! 借用约定: 事件回调全部拿 Rc<App>;App 字段用内部可变性,
//! 单一 GTK 主线程访问, 无跨线程共享。

use crate::config::{self, Config};
use crate::dict::WubiDict;
use crate::phrases::{expand_text, merged_candidates, now_civil, PhraseBook};
use crate::sender;
use crate::xtrack::XTrack;
use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

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
}

/// 构建 App + 全部布线 + 显示 + 定时器。返回后调用方进 gtk::main()。
pub fn create() -> Result<Rc<App>, anyhow::Error> {
    gtk::init().expect("GTK 初始化失败");
    glib::set_prgname(Some("lyyime-float"));
    let xtrack = XTrack::connect()?;
    let cfg = Config::load();
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
    reload_dict(&app);

    // 定时器: 目标窗口追踪 / 置顶保持 / 位置保存
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
        .set_placeholder_text(Some("打五笔: 空格上屏 · 数字选词 · Esc清空"));
    {
        let app2 = app.clone();
        app.entry.connect_changed(move |_| refresh_cands(&app2));
        let app3 = app.clone();
        app.entry
            .connect_key_press_event(move |_, ev| on_entry_key(&app3, ev));
    }
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
        let r =
            WubiDict::load(&path).map(Arc::new).map_err(|e| format!("{e:#}"));
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

fn update_status(app: &Rc<App>) {
    let t: String = app.last_target.borrow().1.chars().take(30).collect();
    let dict = app.cfg.borrow().dict.clone();
    app.status.set_text(&format!(
        "目标: {}  |  码表 {}",
        if t.is_empty() { "(无)" } else { &t },
        dict
    ));
}

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
            if changed && app.dict.borrow().is_some() {
                update_status(app);
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
const KEY_PAGE_UP: u32 = 0xff55;
const KEY_PAGE_DOWN: u32 = 0xff56;

fn on_entry_key(app: &Rc<App>, ev: &gdk::EventKey) -> glib::Propagation {
    let kv = *ev.keyval(); // Key 解引用为 u32 keysym
    match kv {
        KEY_SPACE => {
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
        _ => glib::Propagation::Proceed,
    }
}

fn commit(app: &Rc<App>, text: String) {
    let (wid, _) = *app.last_target.borrow();
    if wid == 0 {
        app.status.set_text("⚠ 先点一下要输入字的应用窗口, 再回来打字");
        return;
    }
    let mut method = app.cfg.borrow().send.clone();
    let mut note = String::new();
    if method == "type" && text.chars().count() > sender::AUTO_PASTE_LEN {
        // 长短语直输逐字注入太慢, 自动改粘贴(会覆盖剪贴板)
        method = "paste".into();
        note = "长短语已自动用粘贴方式上屏(剪贴板被占用) | ".into();
    }
    let clipboard = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    match sender::send(wid, &text, &method, Some(&clipboard)) {
        Ok(()) => {
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
    // 焦点回悬浮窗继续打字
    let xid = app.self_xid.get();
    if xid != 0 {
        let _ = std::process::Command::new("xdotool")
            .args(["windowactivate", &xid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    let entry = app.entry.clone();
    glib::idle_add_local_once(move || entry.grab_focus());
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
        box_.pack_start(
            &gtk::Label::new(Some("短语内容(任意长度, 可换行):")),
            false,
            false,
            0,
        );
        let text_view = gtk::TextView::new();
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
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
    }))
}
