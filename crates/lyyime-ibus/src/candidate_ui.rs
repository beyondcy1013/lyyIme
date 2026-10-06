use crate::logger;
use crate::service::{EngineService, EngineState};
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::Duration;

extern "C" {
    fn lyy_skin_css(id: *const c_char, dark: c_int, font_size: c_int) -> *mut c_char;
    fn lyy_skin_system_is_dark() -> c_int;
    fn g_free(p: *mut c_void);
}

pub enum UiEventKind {
    Select { idx: usize },
    Page { down: bool },
    WordMenu { idx: usize },
    GeneralMenu,
    Settings,
    ActivateMenu { abs: usize },
    CancelMenu,
}

pub struct UiEvent {
    pub owner: String,
    pub gen: u64,
    pub kind: UiEventKind,
}

enum UiCmd {
    Show {
        owner: String,
        gen: u64,
        cands: Vec<(String, String)>,
        aux: String,
        page: usize,
        pages: usize,
    },
    Hide {
        owner: String,
        gen: u64,
    },
    Cursor {
        owner: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    Menu {
        owner: String,
        gen: u64,
        items: Vec<String>,
    },
    Notice {
        owner: String,
        gen: u64,
        text: String,
    },
    Style {
        skin: String,
        font_size: i32,
    },
    Quit,
}

static REGISTRY: std::sync::LazyLock<Mutex<HashMap<String, Weak<EngineState>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(path: &str, st: &Arc<EngineState>) {
    let mut m = REGISTRY.lock().unwrap();
    m.retain(|_, w| w.upgrade().is_some());
    m.insert(path.to_string(), Arc::downgrade(st));
}

pub fn unregister(path: &str) {
    let mut m = REGISTRY.lock().unwrap();
    m.remove(path);
    m.retain(|_, w| w.upgrade().is_some());
}

fn resolve(owner: &str) -> Option<EngineService> {
    REGISTRY
        .lock()
        .unwrap()
        .get(owner)
        .and_then(|w| w.upgrade())
        .map(EngineService)
}

fn frame_active(owner: &str, gen: u64) -> bool {
    resolve(owner)
        .map(|svc| {
            let lg = svc.0.logic.lock().unwrap();
            lg.ui_gate_open() && lg.ui_gen() == gen
        })
        .unwrap_or(false)
}

pub struct Ui {
    cmd_tx: mpsc::Sender<UiCmd>,
    healthy: AtomicBool,
}

static UI_ONCE: std::sync::OnceLock<Option<Ui>> = std::sync::OnceLock::new();

pub fn init() -> Option<&'static Ui> {
    UI_ONCE
        .get_or_init(|| {
            let (cmd_tx, cmd_rx) = mpsc::channel::<UiCmd>();
            let (evt_tx, evt_rx) = mpsc::channel::<UiEvent>();
            let (ready_tx, ready_rx) = mpsc::channel::<bool>();
            if std::thread::Builder::new()
                .name("lyyime-candwin".into())
                .spawn(move || gtk_thread(cmd_rx, evt_tx, ready_tx))
                .is_err()
            {
                return None;
            }
            let ok = ready_rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false);
            if !ok {
                logger::warn("自有 GTK 候选窗初始化失败,回退原生 ibus 候选面板");
                let _ = cmd_tx.send(UiCmd::Quit);
                return None;
            }
            if std::thread::Builder::new()
                .name("lyyime-uiev".into())
                .spawn(move || {
                    for ev in evt_rx {
                        let owner = ev.owner.clone();
                        match resolve(&owner) {
                            Some(svc) => {
                                let _ = zbus::block_on(svc.ui_event(ev));
                            }
                            None => logger::debug(&format!(
                                "UI 事件无对应引擎,丢弃:owner={owner}"
                            )),
                        }
                    }
                })
                .is_err()
            {
                let _ = cmd_tx.send(UiCmd::Quit);
                return None;
            }
            logger::info("自有 GTK 候选窗已启动(单线程/单窗口)");
            Some(Ui {
                cmd_tx,
                healthy: AtomicBool::new(true),
            })
        })
        .as_ref()
        .filter(|u| u.healthy.load(Ordering::SeqCst))
}

pub fn ui() -> Option<&'static Ui> {
    UI_ONCE
        .get()
        .and_then(|o| o.as_ref())
        .filter(|u| u.healthy.load(Ordering::SeqCst))
}

impl Ui {
    fn send(&self, cmd: UiCmd) -> bool {
        if self.cmd_tx.send(cmd).is_err() {
            self.healthy.store(false, Ordering::SeqCst);
            logger::warn("候选窗 GTK 线程已退出,后续帧回退原生 lookup");
            return false;
        }
        true
    }
    pub fn show(
        &self,
        owner: &str,
        gen: u64,
        cands: &[(String, String)],
        aux: &str,
        page: usize,
        pages: usize,
    ) {
        self.send(UiCmd::Show {
            owner: owner.to_string(),
            gen,
            cands: cands.to_vec(),
            aux: aux.to_string(),
            page,
            pages,
        });
    }
    pub fn hide(&self, owner: &str, gen: u64) {
        self.send(UiCmd::Hide {
            owner: owner.to_string(),
            gen,
        });
    }
    pub fn cursor(&self, owner: &str, x: i32, y: i32, w: i32, h: i32) {
        self.send(UiCmd::Cursor {
            owner: owner.to_string(),
            x,
            y,
            w,
            h,
        });
    }
    pub fn menu(&self, owner: &str, gen: u64, items: Vec<String>) {
        self.send(UiCmd::Menu {
            owner: owner.to_string(),
            gen,
            items,
        });
    }
    pub fn notice(&self, owner: &str, gen: u64, text: &str) {
        self.send(UiCmd::Notice {
            owner: owner.to_string(),
            gen,
            text: text.to_string(),
        });
    }
    pub fn style(&self, skin: &str, font_size: i32) {
        self.send(UiCmd::Style {
            skin: skin.to_string(),
            font_size,
        });
    }
}

const MAX_ROWS: usize = 10;

const MENU_CSS: &str = ".lyy-menu {background-color:@theme_bg_color;color:@theme_fg_color;border:1px solid alpha(@theme_fg_color,0.25);padding:3px;} .lyy-menu button {border:0;box-shadow:none;border-radius:0;background:transparent;padding:4px 12px;} .lyy-menu button:hover {background-color:@theme_selected_bg_color;color:@theme_selected_fg_color;}";

struct CellUi {
    bx: gtk::Box,
    num: gtk::Label,
    word: gtk::Label,
    comment: gtk::Label,
}

struct MenuCtx {
    menu: gtk::Window,
}

struct GtkUi {
    win: gtk::Window,
    aux: gtk::Label,
    zone: gtk::Box,
    page: gtk::Label,
    prev: gtk::Button,
    next: gtk::Button,
    gear: gtk::Button,
    cells: Vec<CellUi>,
    shown: usize,
    cur_owner: Option<String>,
    cur_gen: u64,
    cursor: Option<(String, i32, i32, i32, i32)>,
    menu: Option<MenuCtx>,
    pending_trigger: Option<gtk::gdk::EventButton>,
    compose: String,
    notice: String,
    skin: String,
    font_size: i32,
    css: Option<gtk::CssProvider>,
    evt_tx: mpsc::Sender<UiEvent>,
}

fn inside(w: &impl IsA<gtk::Widget>, top: &gtk::Window, x: f64, y: f64) -> bool {
    if !w.is_visible() {
        return false;
    }
    let Some((wx, wy)) = w.translate_coordinates(top, 0, 0) else {
        return false;
    };
    let (wx, wy) = (f64::from(wx), f64::from(wy));
    let a = w.allocation();
    x >= wx && x < wx + f64::from(a.width()) && y >= wy && y < wy + f64::from(a.height())
}

fn skin_css(id: &str, font_size: i32) -> String {
    let id = CString::new(id).unwrap_or_else(|_| CString::new("system").unwrap());
    unsafe {
        let p = lyy_skin_css(id.as_ptr(), lyy_skin_system_is_dark(), font_size);
        if p.is_null() {
            return String::new();
        }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        g_free(p as *mut c_void);
        s
    }
}

fn walk_tree(w: &gtk::Widget, add: bool, prov: &gtk::CssProvider) {
    let ctx = w.style_context();
    if add {
        ctx.add_provider(prov, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    } else {
        ctx.remove_provider(prov);
    }
    if let Ok(c) = w.clone().downcast::<gtk::Container>() {
        c.forall(|ch| walk_tree(ch, add, prov));
    }
}

impl GtkUi {
    fn new(evt_tx: mpsc::Sender<UiEvent>) -> Rc<RefCell<GtkUi>> {
        let win = gtk::Window::new(gtk::WindowType::Popup);
        win.set_decorated(false);
        win.set_resizable(false);
        win.set_title("lyyime-candwin");
        win.set_keep_above(true);
        win.set_type_hint(gtk::gdk::WindowTypeHint::Tooltip);
        win.set_accept_focus(false);
        win.set_can_focus(false);
        win.set_skip_taskbar_hint(true);
        win.set_skip_pager_hint(true);
        win.add_events(
            gtk::gdk::EventMask::BUTTON_PRESS_MASK | gtk::gdk::EventMask::SCROLL_MASK,
        );
        win.style_context().add_class("lyy-frame");

        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        outer.style_context().add_class("lyy-rows");
        win.add(&outer);

        let aux = gtk::Label::new(None);
        aux.style_context().add_class("lyy-preedit");
        aux.set_xalign(0.0);
        aux.set_ellipsize(gtk::pango::EllipsizeMode::End);
        outer.pack_start(&aux, false, false, 0);

        let mut cells = Vec::with_capacity(MAX_ROWS);
        for i in 0..MAX_ROWS {
            let bx = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            bx.set_halign(gtk::Align::Start);
            bx.style_context().add_class("lyy-row");
            if i == 0 {
                bx.style_context().add_class("lyy-first");
            }
            let num = gtk::Label::new(None);
            num.style_context().add_class("lyy-num");
            let word = gtk::Label::new(None);
            word.style_context().add_class("lyy-word");
            let comment = gtk::Label::new(None);
            comment.style_context().add_class("lyy-comment");
            bx.pack_start(&num, false, false, 0);
            bx.pack_start(&word, false, false, 0);
            bx.pack_start(&comment, false, false, 0);
            bx.hide();
            outer.pack_start(&bx, false, false, 0);
            cells.push(CellUi {
                bx,
                num,
                word,
                comment,
            });
        }

        let zone = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        zone.style_context().add_class("lyy-syszone");
        zone.set_size_request(36, -1);
        if let Some(acc) = zone.accessible() {
            acc.set_name("候选系统区");
        }
        outer.pack_start(&zone, true, true, 0);

        let prev = gtk::Button::with_label("‹");
        prev.set_relief(gtk::ReliefStyle::None);
        prev.set_can_focus(false);
        if let Some(acc) = prev.accessible() {
            acc.set_name("候选上一页");
        }
        outer.pack_start(&prev, false, false, 0);
        let next = gtk::Button::with_label("›");
        next.set_relief(gtk::ReliefStyle::None);
        next.set_can_focus(false);
        if let Some(acc) = next.accessible() {
            acc.set_name("候选下一页");
        }
        outer.pack_start(&next, false, false, 0);
        let page = gtk::Label::new(None);
        page.style_context().add_class("lyy-page");
        outer.pack_start(&page, false, false, 0);
        let gear = gtk::Button::new();
        gear.set_relief(gtk::ReliefStyle::None);
        gear.set_can_focus(false);
        gear.set_tooltip_text(Some("打开设置"));
        gear.set_image(Some(&gtk::Image::from_icon_name(
            Some("emblem-system-symbolic"),
            gtk::IconSize::Menu,
        )));
        if let Some(acc) = gear.accessible() {
            acc.set_name("打开设置");
        }
        outer.pack_start(&gear, false, false, 0);

        let (skin, font_size) = crate::ui_style();
        let mut ui = GtkUi {
            win: win.clone(),
            aux,
            zone,
            page,
            prev,
            next,
            gear,
            cells,
            shown: 0,
            cur_owner: None,
            cur_gen: 0,
            cursor: None,
            menu: None,
            pending_trigger: None,
            compose: String::new(),
            notice: String::new(),
            skin,
            font_size,
            css: None,
            evt_tx,
        };
        ui.apply_skin();
        let ui = Rc::new(RefCell::new(ui));

        {
            let button = ui.borrow().prev.clone();
            let ui = ui.clone();
            button.connect_button_press_event(move |_, ev| {
                if ui.borrow().menu.is_some() {
                    ui.borrow().send(UiEventKind::CancelMenu);
                    return gtk::glib::Propagation::Stop;
                }
                match ev.button() {
                    1 => ui.borrow().send(UiEventKind::Page { down: false }),
                    3 => {
                        let mut u = ui.borrow_mut();
                        u.pending_trigger = Some(ev.clone());
                        u.send(UiEventKind::GeneralMenu);
                    }
                    _ => return gtk::glib::Propagation::Proceed,
                }
                gtk::glib::Propagation::Stop
            });
        }
        {
            let button = ui.borrow().next.clone();
            let ui = ui.clone();
            button.connect_button_press_event(move |_, ev| {
                if ui.borrow().menu.is_some() {
                    ui.borrow().send(UiEventKind::CancelMenu);
                    return gtk::glib::Propagation::Stop;
                }
                match ev.button() {
                    1 => ui.borrow().send(UiEventKind::Page { down: true }),
                    3 => {
                        let mut u = ui.borrow_mut();
                        u.pending_trigger = Some(ev.clone());
                        u.send(UiEventKind::GeneralMenu);
                    }
                    _ => return gtk::glib::Propagation::Proceed,
                }
                gtk::glib::Propagation::Stop
            });
        }
        {
            let button = ui.borrow().gear.clone();
            let ui = ui.clone();
            button.connect_button_press_event(move |_, ev| {
                if ui.borrow().menu.is_some() {
                    ui.borrow().send(UiEventKind::CancelMenu);
                    return gtk::glib::Propagation::Stop;
                }
                match ev.button() {
                    1 => ui.borrow().send(UiEventKind::Settings),
                    3 => {
                        let mut u = ui.borrow_mut();
                        u.pending_trigger = Some(ev.clone());
                        u.send(UiEventKind::GeneralMenu);
                    }
                    _ => return gtk::glib::Propagation::Proceed,
                }
                gtk::glib::Propagation::Stop
            });
        }

        if let Some(settings) = gtk::Settings::default() {
            let ui = ui.clone();
            settings.connect_gtk_application_prefer_dark_theme_notify(move |_| {
                ui.borrow_mut().apply_skin();
            });
        }
        {
            let ui = ui.clone();
            win.connect_button_press_event(move |_, ev| {
                if ui.borrow_mut().on_button(ev) {
                    gtk::glib::Propagation::Stop
                } else {
                    gtk::glib::Propagation::Proceed
                }
            });
        }
        {
            let ui = ui.clone();
            win.connect_scroll_event(move |_, ev| {
                let down = if ev.direction() == gtk::gdk::ScrollDirection::Smooth {
                    ev.scroll_deltas().map(|(_, dy)| dy > 0.0).unwrap_or(false)
                } else {
                    matches!(ev.direction(), gtk::gdk::ScrollDirection::Down)
                };
                ui.borrow().send(UiEventKind::Page { down });
                gtk::glib::Propagation::Stop
            });
        }
        ui
    }

    fn apply_skin(&mut self) {
        let css = skin_css(&self.skin, self.font_size);
        if css.is_empty() {
            return;
        }
        let prov = gtk::CssProvider::new();
        if prov.load_from_data(css.as_bytes()).is_err() {
            return;
        }
        if let Some(old) = self.css.take() {
            walk_tree(&self.win.clone().upcast(), false, &old);
        }
        walk_tree(&self.win.clone().upcast(), true, &prov);
        self.css = Some(prov);
    }

    fn send(&self, kind: UiEventKind) {
        if let Some(o) = &self.cur_owner {
            let _ = self.evt_tx.send(UiEvent {
                owner: o.clone(),
                gen: self.cur_gen,
                kind,
            });
        }
    }

    fn on_button(&mut self, ev: &gtk::gdk::EventButton) -> bool {
        if self.cur_owner.is_none() {
            return false;
        }
        if self.menu.is_some() {
            self.send(UiEventKind::CancelMenu);
            return true;
        }
        let (x, y) = ev.position();
        let button = ev.button();
        if inside(&self.gear, &self.win, x, y)
            || inside(&self.prev, &self.win, x, y)
            || inside(&self.next, &self.win, x, y)
        {
            return true;
        }
        for i in 0..self.shown.min(self.cells.len()) {
            if inside(&self.cells[i].bx, &self.win, x, y) {
                if button == 1 {
                    self.send(UiEventKind::Select { idx: i });
                } else if button == 3 {
                    self.pending_trigger = Some(ev.clone());
                    self.send(UiEventKind::WordMenu { idx: i });
                }
                return true;
            }
        }
        if button == 3 {
            self.pending_trigger = Some(ev.clone());
            self.send(UiEventKind::GeneralMenu);
        }
        true
    }

    fn close_menu(&mut self) {
        if let Some(ctx) = self.menu.take() {
            unsafe { ctx.menu.destroy() };
        }
    }

    fn refresh_aux(&self) {
        self.aux.set_text(if self.notice.is_empty() {
            &self.compose
        } else {
            &self.notice
        });
    }

    fn handle(&mut self, cmd: UiCmd) {
        match cmd {
            UiCmd::Show {
                owner,
                gen,
                cands,
                aux,
                page,
                pages,
            } => {
                if !frame_active(&owner, gen) {
                    return;
                }
                if self.cur_owner.as_deref() == Some(owner.as_str()) && gen <= self.cur_gen {
                    return;
                }
                self.cur_owner = Some(owner);
                self.cur_gen = gen;
                self.close_menu();
                self.pending_trigger = None;
                self.notice.clear();
                self.render(&cands, &aux, page, pages);
                self.win.resize(1, 1);
                self.win.show_all();
                for c in self.cells.iter().skip(self.shown) {
                    c.bx.hide();
                }
                if pages <= 1 {
                    self.page.hide();
                    self.prev.hide();
                    self.next.hide();
                }
                self.reposition();
            }
            UiCmd::Hide { owner, gen } => {
                if self.cur_owner.as_deref() != Some(owner.as_str()) || gen < self.cur_gen {
                    return;
                }
                self.cur_owner = None;
                self.close_menu();
                self.pending_trigger = None;
                self.win.hide();
                self.shown = 0;
                self.compose.clear();
                self.notice.clear();
                self.aux.set_text("");
                self.page.set_text("");
                self.page.hide();
                self.prev.hide();
                self.next.hide();
                for c in &self.cells {
                    c.num.set_text("");
                    c.word.set_text("");
                    c.comment.set_text("");
                    c.bx.hide();
                }
                if self.cursor.as_ref().map(|c| &c.0) == Some(&owner) {
                    self.cursor = None;
                }
            }
            UiCmd::Cursor { owner, x, y, w, h } => {
                let cur = self.cur_owner.as_deref() == Some(owner.as_str());
                self.cursor = Some((owner, x, y, w, h));
                if cur && self.win.is_visible() && self.menu.is_none() {
                    self.reposition();
                }
            }
            UiCmd::Menu { owner, gen, items } => {
                if !frame_active(&owner, gen) {
                    return;
                }
                if self.cur_owner.as_deref() == Some(owner.as_str())
                    && gen > self.cur_gen
                    && !items.is_empty()
                {
                    self.cur_gen = gen;
                    match self.pending_trigger.take() {
                        Some(trig) => self.popup_menu(&owner, gen, &items, &trig),
                        None => {
                            logger::warn(&format!(
                                "候选窗菜单缺触发事件,回退取消 gen={gen}"
                            ));
                            let _ = self.evt_tx.send(UiEvent {
                                owner,
                                gen,
                                kind: UiEventKind::CancelMenu,
                            });
                        }
                    }
                }
            }
            UiCmd::Notice { owner, gen, text } => {
                if !frame_active(&owner, gen) {
                    return;
                }
                if text.is_empty() {
                    if self.cur_owner.as_deref() != Some(owner.as_str()) {
                        return;
                    }
                    if self.compose.is_empty()
                        && self.shown == 0
                        && self.menu.is_none()
                        && self.pending_trigger.is_none()
                    {
                        self.cur_owner = None;
                        self.notice.clear();
                        self.aux.set_text("");
                        self.win.hide();
                    } else {
                        self.notice.clear();
                        self.refresh_aux();
                    }
                    return;
                }
                if self.win.is_visible() {
                    if self.cur_owner.as_deref() == Some(owner.as_str()) {
                        self.notice = text;
                        self.refresh_aux();
                    }
                    return;
                }
                self.notice = text;
                self.cur_owner = Some(owner);
                self.cur_gen = gen;
                self.compose.clear();
                self.shown = 0;
                self.refresh_aux();
                self.win.resize(1, 1);
                self.win.show_all();
                for c in &self.cells {
                    c.num.set_text("");
                    c.word.set_text("");
                    c.comment.set_text("");
                    c.bx.hide();
                }
                self.page.set_text("");
                self.page.hide();
                self.prev.hide();
                self.next.hide();
                self.reposition();
            }
            UiCmd::Style { skin, font_size } => {
                self.skin = skin;
                self.font_size = font_size.clamp(10, 28);
                self.apply_skin();
            }
            UiCmd::Quit => gtk::main_quit(),
        }
    }

    fn render(&mut self, cands: &[(String, String)], aux: &str, page: usize, pages: usize) {
        self.compose = aux.to_string();
        self.refresh_aux();
        self.shown = cands.len().min(self.cells.len());
        for (i, c) in self.cells.iter().enumerate() {
            if i < self.shown {
                c.num.set_text(&format!("{}.", (i + 1) % 10));
                c.word.set_text(&cands[i].0);
                c.comment.set_text(&cands[i].1);
            }
        }
        if pages > 1 {
            self.page.set_text(&format!("{}/{}", page + 1, pages));
        } else {
            self.page.set_text("");
        }
    }

    fn popup_menu(&mut self, owner: &str, gen: u64, items: &[String], trigger: &gtk::gdk::EventButton) {
        self.close_menu();
        let pop = gtk::Window::new(gtk::WindowType::Popup);
        pop.set_title("lyyime-menu");
        pop.set_decorated(false);
        pop.set_resizable(false);
        pop.set_accept_focus(false);
        pop.set_can_focus(false);
        pop.set_skip_taskbar_hint(true);
        pop.set_skip_pager_hint(true);
        pop.set_keep_above(true);
        pop.set_type_hint(gtk::gdk::WindowTypeHint::PopupMenu);
        pop.add_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK);
        pop.style_context().add_class("lyy-menu");
        if let Some(acc) = pop.accessible() {
            acc.set_role(gtk::atk::Role::Menu);
        }
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let mut row_btns: Vec<gtk::Button> = Vec::with_capacity(items.len());
        for (i, label) in items.iter().enumerate() {
            let b = gtk::Button::new();
            let lb = gtk::Label::new(Some(label.as_str()));
            lb.set_xalign(0.0);
            b.add(&lb);
            b.set_relief(gtk::ReliefStyle::None);
            b.set_can_focus(false);
            b.set_hexpand(true);
            if let Some(acc) = b.accessible() {
                acc.set_role(gtk::atk::Role::MenuItem);
                acc.set_name(label);
            }
            {
                let tx = self.evt_tx.clone();
                let ow = owner.to_string();
                b.connect_button_press_event(move |_, ev| {
                    if ev.button() == 1 {
                        let _ = tx.send(UiEvent {
                            owner: ow.clone(),
                            gen,
                            kind: UiEventKind::ActivateMenu { abs: i },
                        });
                        return gtk::glib::Propagation::Stop;
                    }
                    gtk::glib::Propagation::Proceed
                });
            }
            list.pack_start(&b, true, true, 0);
            row_btns.push(b);
        }
        {
            let tx = self.evt_tx.clone();
            let ow = owner.to_string();
            pop.connect_button_press_event(move |popw, ev| {
                if ev.button() == 1 {
                    let (x, y) = ev.position();
                    for (i, b) in row_btns.iter().enumerate() {
                        if inside(b, popw, x, y) {
                            let _ = tx.send(UiEvent {
                                owner: ow.clone(),
                                gen,
                                kind: UiEventKind::ActivateMenu { abs: i },
                            });
                            return gtk::glib::Propagation::Stop;
                        }
                    }
                }
                gtk::glib::Propagation::Proceed
            });
        }
        pop.add(&list);
        let prov = gtk::CssProvider::new();
        if prov.load_from_data(MENU_CSS.as_bytes()).is_ok() {
            walk_tree(&pop.clone().upcast(), true, &prov);
        }
        let (rx, ry) = trigger.root();
        let (rx, ry) = (rx as i32, ry as i32);
        pop.move_(rx, ry);
        pop.show_all();
        let wa = gtk::gdk::Display::default().and_then(|d| {
            d.monitor_at_point(rx, ry)
                .or_else(|| d.primary_monitor())
                .or_else(|| d.monitor(0))
                .map(|m| m.workarea())
        });
        if let Some(wa) = wa {
            let (_, lnat) = list.preferred_size();
            let (_, pnat) = pop.preferred_size();
            let chrome_w = (pnat.width - lnat.width).max(0);
            let chrome_h = (pnat.height - lnat.height).max(0);
            let max_w = (wa.width() - 8 - chrome_w).max(1);
            let max_h = (wa.height() - 8 - chrome_h).max(1);
            if lnat.height > max_h || lnat.width > max_w {
                pop.remove(&list);
                let scroll =
                    gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
                scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
                scroll.set_min_content_width(lnat.width.min(max_w));
                scroll.set_min_content_height(lnat.height.min(max_h));
                scroll.add(&list);
                pop.add(&scroll);
                pop.show_all();
            }
            let (_, fin) = pop.preferred_size();
            let x = rx.clamp(wa.x(), (wa.x() + wa.width() - fin.width).max(wa.x()));
            let y = ry.clamp(wa.y(), (wa.y() + wa.height() - fin.height).max(wa.y()));
            pop.move_(x, y);
        }
        self.menu = Some(MenuCtx { menu: pop });
    }

    fn reposition(&mut self) {
        let caret = match &self.cursor {
            Some((o, cx, cy, w, h)) if Some(o) == self.cur_owner.as_ref() => {
                Some((*cx, *cy, *w, *h))
            }
            _ => None,
        };
        let (_, nat) = self.win.preferred_size();
        let (bx, by) = caret
            .map(|(x, y, _w, h)| (x, y + h + 4))
            .unwrap_or_else(|| {
                gtk::gdk::Display::default()
                    .and_then(|d| d.default_seat())
                    .and_then(|s| s.pointer())
                    .map(|dev| {
                        let (_, x, y) = dev.position();
                        (x, y)
                    })
                    .unwrap_or((0, 0))
            });
        if let Some(disp) = gtk::gdk::Display::default() {
            if let Some(mon) = disp
                .monitor_at_point(bx, by)
                .or_else(|| disp.primary_monitor())
                .or_else(|| disp.monitor(0))
            {
                let wa = mon.workarea();
                let mut x = bx.clamp(wa.x(), (wa.x() + wa.width() - nat.width).max(wa.x()));
                let mut y = by;
                if y + nat.height > wa.y() + wa.height() {
                    if let Some((_, cy, _, _)) = caret {
                        y = cy - nat.height - 4;
                    }
                }
                y = y.clamp(wa.y(), (wa.y() + wa.height() - nat.height).max(wa.y()));
                x = x.clamp(wa.x(), wa.x() + wa.width().max(nat.width) - nat.width);
                self.win.move_(x, y);
                return;
            }
        }
        self.win.move_(bx, by);
    }
}

fn gtk_thread(
    cmd_rx: mpsc::Receiver<UiCmd>,
    evt_tx: mpsc::Sender<UiEvent>,
    ready: mpsc::Sender<bool>,
) {
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gtk::init().is_ok()))
        .unwrap_or(false);
    if !ok {
        let _ = ready.send(false);
        return;
    }
    let ui = GtkUi::new(evt_tx);
    let _ = ready.send(true);
    {
        let ui = ui.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(15), move || {
            let mut u = ui.borrow_mut();
            while let Ok(cmd) = cmd_rx.try_recv() {
                u.handle(cmd);
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    gtk::main();
}
