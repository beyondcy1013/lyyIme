//! 框选截屏覆盖窗:整屏快照打底(冻结画面),拖拽选区,Esc 取消 /
//! Enter 确认 / 双击整屏。交互习惯对齐搜狗/QQ 截屏。
//!
//! 线程模型:单 GTK 主循环,选区状态用 Rc<Cell>/Rc<RefCell> 在闭包间共享;
//! 结束统一走 `finish()`(记录结果 → 隐藏窗 → 主循环退出),保证只结束一次。
//!
//! 坐标约定:交互矩形为逻辑坐标;快照为设备像素(X11 缩放通常 1:1,
//! HiDPI 下裁剪前按 root scale factor 换算)。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

/// 选区(逻辑坐标;负方向拖拽已归一化为左上角+宽高)。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn from_points(x1: f64, y1: f64, x2: f64, y2: f64) -> Rect {
        Rect {
            x: x1.min(x2),
            y: y1.min(y2),
            w: (x1 - x2).abs(),
            h: (y1 - y2).abs(),
        }
    }
    fn area(&self) -> f64 {
        self.w * self.h
    }
}

enum Outcome {
    Cancelled,
    Full,
    Cropped(Rect),
}

/// 运行框选,返回 `None` = 用户取消;`Some((整屏快照, 设备像素裁剪矩形))`。
pub fn run_region() -> anyhow::Result<Option<(gtk::gdk_pixbuf::Pixbuf, (i32, i32, i32, i32))>> {
    use gdk::prelude::WindowExtManual;
    use gdk::Window as GdkWindow;
    let root = <GdkWindow as WindowExtManual>::default_root_window();
    let (_, _, sw, sh) = root.geometry();
    if sw <= 0 || sh <= 0 {
        anyhow::bail!("无法获取屏幕尺寸({sw}×{sh})");
    }
    let snapshot = root
        .pixbuf(0, 0, sw, sh)
        .ok_or_else(|| anyhow::anyhow!("屏幕内容截取失败(合成器/嵌套显示可能限制截图)"))?;

    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title("lyyIme 截屏");
    win.set_decorated(false);
    win.set_skip_taskbar_hint(true);
    win.set_keep_above(true);
    win.move_(0, 0);
    win.set_default_size(sw, sh);
    win.resize(sw, sh);
    // fullscreen:有 WM 时全屏化,无 WM(Xvfb)时 GDK 客户端自行放大到整屏
    win.fullscreen();
    win.set_position(gtk::WindowPosition::None);

    let da = gtk::DrawingArea::new();
    da.set_can_focus(true);
    da.add_events(
        gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::BUTTON_MOTION_MASK,
    );
    win.add(&da);

    // ---- 共享状态 ----
    let anchor: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
    let dragging: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let sel: Rc<RefCell<Option<Rect>>> = Rc::new(RefCell::new(None));
    let result: Rc<RefCell<Option<Outcome>>> = Rc::new(RefCell::new(None));
    // 双击消歧:单击抬起(无拖拽)延迟 280ms 才取消,期间到来的双击优先
    let cancel_pending: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    let snap = snapshot.clone();

    // ---- 绘制:快照打底 + 选区外压暗 + 虚线边框 + 尺寸角标 ----
    {
        let anchor = anchor.clone();
        let sel = sel.clone();
        da.connect_draw(move |_, cr| {
            cr.set_source_pixbuf(&snap, 0.0, 0.0);
            let _ = cr.paint();

            match current_rect(&anchor, &sel) {
                Some(r) => {
                    let (w, h) = (sw as f64, sh as f64);
                    // 选区外四条压暗带(上/下/左/右)
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.4);
                    cr.rectangle(0.0, 0.0, w, r.y);
                    cr.rectangle(0.0, r.y + r.h, w, h - (r.y + r.h));
                    cr.rectangle(0.0, r.y, r.x, r.h);
                    cr.rectangle(r.x + r.w, r.y, w - (r.x + r.w), r.h);
                    let _ = cr.fill();

                    // 虚线边框(GNOME/XFCE 蓝)
                    cr.set_source_rgb(0.21, 0.52, 0.89);
                    cr.set_line_width(1.5);
                    cr.set_dash(&[6.0, 4.0], 0.0);
                    cr.rectangle(r.x + 0.5, r.y + 0.5, r.w, r.h);
                    let _ = cr.stroke();
                    cr.set_dash(&[], 0.0);

                    // 尺寸角标(选区上方,越界时放选区内)
                    let label = format!("{} × {}", r.w as i32, r.h as i32);
                    cr.select_font_face(
                        "monospace",
                        gtk::cairo::FontSlant::Normal,
                        gtk::cairo::FontWeight::Normal,
                    );
                    cr.set_font_size(12.0);
                    let Ok(ext) = cr.text_extents(&label) else {
                        return glib::Propagation::Proceed;
                    };
                    let (lw, lh) = (ext.width() + 12.0, ext.height() + 8.0);
                    let lx = (r.x + r.w - lw).max(0.0);
                    let ly = if r.y - lh - 4.0 >= 0.0 {
                        r.y - lh - 4.0
                    } else {
                        r.y + 4.0
                    };
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
                    cr.rectangle(lx, ly, lw, lh);
                    let _ = cr.fill();
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    let _ = cr.move_to(lx + 6.0, ly + lh - 5.0);
                    let _ = cr.show_text(&label);
                }
                None => {
                    // 未开始拖拽:整屏轻压暗
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.15);
                    cr.rectangle(0.0, 0.0, sw as f64, sh as f64);
                    let _ = cr.fill();
                }
            }
            glib::Propagation::Proceed
        });
    }

    // ---- 鼠标:按下定锚 / 拖动更新 / 抬起确认;双击=整屏 ----
    {
        let anchor = anchor.clone();
        let sel = sel.clone();
        let result = result.clone();
        let dragging = dragging.clone();
        let cancel_pending = cancel_pending.clone();
        let win2 = win.clone();
        da.connect_button_press_event(move |_, ev| {
            if ev.button() != 1 {
                return glib::Propagation::Proceed;
            }
            if ev.event_type() == gdk::EventType::DoubleButtonPress {
                // 双击 = 整屏:先撤掉单击挂起的取消定时
                if let Some(id) = cancel_pending.take() {
                    id.remove();
                }
                finish(&win2, &result, Outcome::Full);
                return glib::Propagation::Stop;
            }
            let (x, y) = ev.position();
            anchor.set(Some((x, y)));
            dragging.set(true);
            *sel.borrow_mut() = Some(Rect {
                x,
                y,
                w: 0.0,
                h: 0.0,
            });
            win2.queue_draw();
            glib::Propagation::Proceed
        });
    }
    {
        let anchor = anchor.clone();
        let sel = sel.clone();
        let dragging = dragging.clone();
        let da2 = da.clone();
        da.connect_motion_notify_event(move |_, ev| {
            if dragging.get() {
                if let Some((ax, ay)) = anchor.get() {
                    let (x, y) = ev.position();
                    *sel.borrow_mut() = Some(Rect::from_points(ax, ay, x, y));
                    da2.queue_draw();
                }
            }
            glib::Propagation::Proceed
        });
    }
    {
        let anchor = anchor.clone();
        let result = result.clone();
        let cancel_pending = cancel_pending.clone();
        let dragging = dragging.clone();
        let win = win.clone();
        da.connect_button_release_event(move |_, ev| {
            if ev.button() != 1 {
                return glib::Propagation::Proceed;
            }
            let Some((ax, ay)) = anchor.get() else {
                return glib::Propagation::Proceed;
            };
            dragging.set(false);
            let (x, y) = ev.position();
            let r = Rect::from_points(ax, ay, x, y);
            if r.w < 3.0 && r.h < 3.0 {
                // 单击未拖拽(<3px)= 取消,避免误截;延迟 280ms 留给双击整屏
                let result2 = result.clone();
                let win3 = win.clone();
                let sid =
                    glib::timeout_add_local(std::time::Duration::from_millis(280), move || {
                        finish(&win3, &result2, Outcome::Cancelled);
                        glib::ControlFlow::Break
                    });
                cancel_pending.set(Some(sid));
                return glib::Propagation::Stop;
            }
            finish(&win, &result, Outcome::Cropped(r));
            glib::Propagation::Stop
        });
    }

    // ---- 键盘:Esc 取消,Enter/Space 确认(无选区=整屏) ----
    {
        let sel = sel.clone();
        let result = result.clone();
        let win2 = win.clone();
        win.connect_key_press_event(move |_, ev| {
            let kv = *ev.keyval();
            let res = match kv {
                0xff1b => Some(Outcome::Cancelled), // Escape
                0xff0d | 0xff8d | 0x20 => Some(match sel.borrow().as_ref() {
                    // Return / KP_Enter / Space
                    Some(r) if r.area() >= 1.0 => Outcome::Cropped(*r),
                    _ => Outcome::Full,
                }),
                _ => None,
            };
            match res {
                Some(res) => {
                    finish(&win2, &result, res);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
    }

    // map 后再交焦点(SetInputFocus 对未映射窗口会 BadMatch 而失效)
    {
        let win2 = win.clone();
        win.connect_map_event(move |_, _| {
            focus_overlay(&win2);
            glib::Propagation::Proceed
        });
    }
    win.show_all();
    da.grab_add();
    da.grab_focus();
    set_crosshair(&win);

    gtk::main();
    let outcome = result.borrow_mut().take().unwrap_or(Outcome::Cancelled);
    // 让覆盖窗真正从屏幕消失,保证时序干净(剪贴板/通知在窗口消失后出现)
    while gtk::events_pending() {
        gtk::main_iteration();
    }
    Ok(apply_outcome(outcome, snapshot))
}

/// 当前选区:锚点 + 最新指针位置(负方向归一化)。
fn current_rect(
    anchor: &Rc<Cell<Option<(f64, f64)>>>,
    sel: &Rc<RefCell<Option<Rect>>>,
) -> Option<Rect> {
    let r = *sel.borrow().as_ref()?;
    let (ax, ay) = anchor.get()?;
    Some(Rect::from_points(ax, ay, r.x + r.w, r.y + r.h))
}

/// 结果 → 裁剪矩形(设备像素;整屏/取消路径同样返回快照)。
fn apply_outcome(
    o: Outcome,
    snap: gtk::gdk_pixbuf::Pixbuf,
) -> Option<(gtk::gdk_pixbuf::Pixbuf, (i32, i32, i32, i32))> {
    use gdk::prelude::WindowExtManual;
    use gdk::Window as GdkWindow;
    let (sw, sh) = (snap.width(), snap.height());
    let scale = <GdkWindow as WindowExtManual>::default_root_window()
        .scale_factor()
        .max(1) as f64;
    let crop = match o {
        Outcome::Cancelled => return None,
        Outcome::Full => (0, 0, sw, sh),
        Outcome::Cropped(r) => (
            (r.x * scale) as i32,
            (r.y * scale) as i32,
            ((r.w * scale) as i32).max(1),
            ((r.h * scale) as i32).max(1),
        ),
    };
    // 夹取到快照范围
    let (x, y) = (
        crop.0.clamp(0, (sw - 1).max(0)),
        crop.1.clamp(0, (sh - 1).max(0)),
    );
    let w = crop.2.clamp(1, sw - x);
    let h = crop.3.clamp(1, sh - y);
    Some((snap, (x, y, w, h)))
}

/// 把 X 输入焦点显式交给覆盖窗(SetInputFocus):无窗口管理器(Xvfb 测试、
/// 特殊会话)时 GTK 拿不到键盘焦点会收不到 Esc/Enter,有 WM 时该调用也无害。
/// 连接失败静默 —— 由 WM 正常分焦兜底。
fn focus_overlay(win: &gtk::Window) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, InputFocus};
    let Some(gdkwin) = win.window() else { return };
    let Some(x11win) = gdkwin.downcast::<gdkx11::X11Window>().ok() else {
        return;
    };
    let xid = x11win.xid() as u32;
    let Ok((conn, screen)) = x11rb::connect(None) else {
        return;
    };
    let Some(root) = conn.setup().roots.get(screen).map(|s| s.root) else {
        return;
    };
    let _ = root;
    if conn
        .set_input_focus(InputFocus::POINTER_ROOT, xid, x11rb::CURRENT_TIME)
        .is_ok()
    {
        let _ = conn.flush();
    }
}

fn set_crosshair(win: &gtk::Window) {
    if let Some(w) = win.window() {
        let display = w.display();
        if let Some(cursor) = gdk::Cursor::for_display(&display, gdk::CursorType::Crosshair) {
            w.set_cursor(Some(&cursor));
        }
    }
}

fn finish(win: &gtk::Window, result: &Rc<RefCell<Option<Outcome>>>, res: Outcome) {
    if result.borrow().is_some() {
        return; // 已结束(防重复确认)
    }
    *result.borrow_mut() = Some(res);
    win.hide();
    gtk::main_quit();
}
