//! 「模拟内置」模式: 候选窗贴着目标应用的光标显示(合同风格对齐内置 IME)。
//!
//! 组成:
//!   1. 光标助手: 内嵌 Python(pyatspi, 本机 ibus 引擎同栈已保证存在)常驻进程,
//!      监听桌面 `object:text-caret-moved` 事件, 把光标屏幕坐标按行 JSON 输出。
//!      GTK/Qt 应用开箱可用; Electron(Chromium) 需以 ACCESSIBILITY=1 启动才暴露
//!      无障碍树; 终端类不支持 —— 这些场景悬浮窗退化为"指针位置"锚点。
//!   2. 候选小窗: override-redirect 无边框 GTK 窗, 显示当前编码 + 当前页候选,
//!      定位 = 光标下方; 光标未知时退化为指针下方。
//!   3. 锚点策略: 上一次提交会让目标光标移动 → 触发事件 → 下一屏候选精确锚定;
//!      首次在文本框输入(还没有光标事件)时用指针位置(用户刚点击过输入框)。
//!
//! 纯逻辑(parse_caret_line/markup/anchor)与 GTK 构造分离, 便于单测。

use gtk::prelude::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 光标锚点: 屏幕坐标 + 行高 + 采集时刻(判新鲜度)。
#[derive(Debug, Clone, Copy)]
pub struct CaretPos {
    pub x: i32,
    pub y: i32,
    pub h: i32,
}

/// 解析助手输出的 JSON 行。
///
/// 坐标是「光标相对目标窗口客户区」的值: 这套桌面的 GTK a11y 层对 screen 坐标
/// 一律谎报为窗口相对值(实测 frame 的 screen 也返回 0,0), 绝对定位由 Rust 端
/// 用 X11 取目标窗口根位置后相加(见 ui::render_cands)。
pub fn parse_caret_line(line: &str) -> Option<CaretPos> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let wx = v.get("wx")?.as_i64()? as i32;
    let wy = v.get("wy")?.as_i64()? as i32;
    let wh = v.get("wh")?.as_i64()? as i32;
    if wh <= 0 {
        return None;
    }
    Some(CaretPos { x: wx, y: wy, h: wh })
}

/// 候选小窗的 pango 标记: 第一行编码, 第二行当前页候选(序号高亮)。
pub fn markup(code: &str, page_cands: &[String]) -> String {
    let mut s = format!("<b>{}</b>", glib_markup_escape(code));
    if !page_cands.is_empty() {
        s.push_str("\n");
        let items: Vec<String> = page_cands
            .iter()
            .enumerate()
            .map(|(i, t)| {
                format!(
                    "<span foreground=\"#8fd18f\">{}</span>{}",
                    i + 1,
                    glib_markup_escape(t)
                )
            })
            .collect();
        s.push_str(&items.join("  "));
    }
    s
}

fn glib_markup_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 锚点坐标: 光标下方; 越界收敛回屏幕内(估算小窗约 460x90)。
pub fn anchor(caret: CaretPos, screen_w: i32, screen_h: i32) -> (i32, i32) {
    let mut x = caret.x;
    let mut y = caret.y + caret.h + 4;
    const POP_W: i32 = 460;
    const POP_H: i32 = 90;
    if x + POP_W > screen_w {
        x = (screen_w - POP_W).max(0);
    }
    if y + POP_H > screen_h {
        // 光标在屏幕下部时改为显示在光标上方
        y = (caret.y - POP_H - 4).max(0);
    }
    (x, y)
}

/// 内嵌的 Python 光标助手。父进程退出(随 stdin EOF)时自动退出。
const CARET_HELPER_PY: &str = r#"
import gi, json, sys, time, os
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib

IGNORE_TITLE = os.environ.get("LYYIME_IGNORE_TITLE", "")
TARGET_FILE = os.environ.get("LYYIME_TARGET_FILE", "/tmp/lyyime-float-target")
state = {"title": None, "obj": None, "last": None}

def read_target():
    try:
        with open(TARGET_FILE) as f:
            return f.read().strip()
    except Exception:
        return None

def find_toplevel(title):
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        try:
            app = desktop.get_child_at_index(i)
            for j in range(min(app.get_child_count(), 60)):
                w = app.get_child_at_index(j)
                if w.get_name() == title:
                    return w
        except Exception:
            pass
    return None

def find_focused_text(top):
    hits = []
    def walk(obj, depth):
        if obj is None or depth > 14 or len(hits) >= 4:
            return
        try:
            role = obj.get_role_name()
            focused = obj.get_state_set().contains(Atspi.StateType.FOCUSED)
            n = obj.get_child_count()
        except Exception:
            return
        if role in ("text", "文本", "terminal", "终端") and focused:
            hits.append(obj)
        for i in range(min(n, 100)):
            try:
                walk(obj.get_child_at_index(i), depth + 1)
            except Exception:
                pass
    walk(top, 0)
    return hits[0] if hits else None

def poll():
    try:
        title = read_target()
        if not title or title == IGNORE_TITLE:
            return True
        if title != state["title"]:
            state["title"] = title
            state["obj"] = None
            state["last"] = None
        obj = state["obj"]
        if obj is not None:
            try:
                ok = obj.get_state_set().contains(Atspi.StateType.FOCUSED)
            except Exception:
                ok = False
            if not ok:
                top = find_toplevel(title)
                obj = find_focused_text(top) if top is not None else None
                state["obj"] = obj
        if obj is None:
            return True
        tif = obj.get_text_iface()
        if tif is None:
            return True
        off = tif.get_caret_offset()
        wx = wy = wh = 0
        try:
            ext = tif.get_character_extents(max(off - 1, 0), Atspi.CoordType.WINDOW)
            wx, wy, wh = ext.x, ext.y, ext.height
        except Exception:
            wh = 0
        if wh <= 0:
            comp = obj.get_component_iface()
            if comp is None:
                return True
            ext = comp.get_extents(Atspi.CoordType.WINDOW)
            wx, wy, wh = ext.x, ext.y, ext.height
        if wh <= 0:
            return True
        key = (wx, wy, wh)
        if key == state["last"]:
            return True
        state["last"] = key
        print(json.dumps({"wx": wx, "wy": wy, "wh": wh}), flush=True)
    except Exception:
        pass
    return True

def watch_parent():
    if os.getppid() == 1:
        sys.exit(0)
    return True

GLib.timeout_add_seconds(2, watch_parent)
GLib.timeout_add(400, poll)
print(json.dumps({"ready": True}), flush=True)
GLib.MainLoop().run()
"#;

/// 光标跟踪器: 常驻助手进程 + 后台读线程; 任意时刻给最新锚点。
pub struct CaretTracker {
    latest: Arc<Mutex<Option<(CaretPos, Instant)>>>,
    child: Mutex<Option<Child>>,
}

impl CaretTracker {
    pub fn spawn() -> Arc<CaretTracker> {
        let latest = Arc::new(Mutex::new(None));
        let tracker = Arc::new(CaretTracker {
            latest: Arc::clone(&latest),
            child: Mutex::new(None),
        });
        tracker.start();
        tracker
    }

    fn start(&self) {
        let mut child = match Command::new("python3")
            .arg("-c")
            .arg(CARET_HELPER_PY)
            .env("LYYIME_IGNORE_TITLE", "lyyIme 悬浮输入法")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::piped()) // 父进程退出即 EOF → 助手自杀
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return, // 无 python3: 光标跟踪不可用, 用指针兜底
        };
        let stdout = child.stdout.take();
        *self.child.lock().unwrap() = Some(child);
        let latest = Arc::clone(&self.latest);
        std::thread::spawn(move || {
            let Some(out) = stdout else { return };
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(pos) = parse_caret_line(&line) {
                    *latest.lock().unwrap() = Some((pos, Instant::now()));
                }
            }
        });
    }

    /// 最新光标锚点; 超过 stale_secs 视为过期(返回 None)。
    pub fn latest(&self, stale_secs: u64) -> Option<CaretPos> {
        let g = self.latest.lock().unwrap();
        g.as_ref()
            .filter(|(_, t)| t.elapsed().as_secs() < stale_secs)
            .map(|(p, _)| *p)
    }
}

/// 「模拟内置」候选小窗: 单标签 override-redirect 窗。
pub struct InlinePopup {
    win: gtk::Window,
    label: gtk::Label,
    shown: std::cell::Cell<bool>,
}

impl InlinePopup {
    pub fn new() -> InlinePopup {
        let win = gtk::Window::new(gtk::WindowType::Popup);
        win.set_decorated(false);
        win.set_skip_taskbar_hint(true);
        win.set_skip_pager_hint(true);
        win.set_keep_above(true);
        win.set_resizable(false);
        win.set_widget_name("inline-pop");
        let label = gtk::Label::new(None);
        label.set_use_markup(true);
        label.set_xalign(0.0);
        label.set_valign(gtk::Align::Start);
        win.add(&label);
        InlinePopup {
            win,
            label,
            shown: std::cell::Cell::new(false),
        }
    }

    /// 显示/更新候选内容并定位。anchor_xy=None 时藏起。
    pub fn update(&self, markup: Option<(String, i32, i32)>) {
        match markup {
            Some((text, x, y)) => {
                self.label.set_markup(&text);
                self.win.move_(x, y);
                if !self.shown.get() {
                    self.win.show_all();
                    self.shown.set(true);
                }
            }
            None => self.hide(),
        }
    }

    pub fn hide(&self) {
        if self.shown.get() {
            self.win.hide();
            self.shown.set(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析助手json行() {
        // 坐标为目标窗口客户区相对值; 绝对定位由 Rust 端加窗口根位置
        let p = parse_caret_line(r#"{"wx":51,"wy":107,"wh":34}"#).unwrap();
        assert_eq!((p.x, p.y, p.h), (51, 107, 34));
        assert!(parse_caret_line(r#"{"ready":true}"#).is_none());
        assert!(parse_caret_line(r#"{"wx":1,"wy":2,"wh":0}"#).is_none());
        assert!(parse_caret_line("not json").is_none());
    }

    #[test]
    fn 标记含编码与带序号候选() {
        let m = markup("aa", &["式".into(), "啊".into()]);
        assert!(m.contains("<b>aa</b>"));
        assert!(m.contains(">1<"));
        assert!(m.contains("式") && m.contains("啊"));
        assert!(!m.contains('\u{0}'));
    }

    #[test]
    fn 标记转义目标文本特殊字符() {
        let m = markup("a<b>", &["<贴>".into()]);
        assert!(m.contains("a&lt;b&gt;"));
        assert!(m.contains("&lt;贴&gt;"));
    }

    #[test]
    fn 无候选时只显示编码() {
        let m = markup("nk", &[]);
        assert!(m.contains("<b>nk</b>"));
        assert!(!m.contains("span")); // 没有序号高亮(无候选)
        assert_eq!(m.matches('\n').count(), 0);
    }

    #[test]
    fn 锚点在光标下方且收敛回屏幕() {
        let c = CaretPos { x: 100, y: 100, h: 20 };
        assert_eq!(anchor(c, 1920, 1080), (100, 124));
        // 靠右: 收敛 x
        let c2 = CaretPos { x: 1900, y: 100, h: 20 };
        assert_eq!(anchor(c2, 1920, 1080).0, 1920 - 460);
        // 靠下: 翻到光标上方
        let c3 = CaretPos { x: 100, y: 1040, h: 20 };
        assert_eq!(anchor(c3, 1920, 1080).1, 1040 - 90 - 4);
    }
}
