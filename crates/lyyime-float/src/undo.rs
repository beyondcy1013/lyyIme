//! 简易撤销/重做: GTK3 的 GtkEntry / GtkTextView 均无内建 undo,
//! 悬浮窗应用的各文本框(主窗编码框 / 打字板 / 短语编辑器)共用本模块。
//! 语义: 每次缓冲变更(键入/删除/程序 set_text)记录一个状态, Ctrl+Z 逐步
//! 回退, Ctrl+Y 或 Ctrl+Shift+Z 重做; 撤销后输入新内容即丢弃重做分支。
//! 主窗编码框的附加合同(空缓冲直通)在 ui.rs on_entry_key: 编码框历史按词
//! 重开(reset), 空缓冲且无可撤销时 Ctrl+Z/Ctrl+Y 直通目标窗口, 撤销的是
//! 目标应用里刚上屏的文本——与空缓冲退格直通同一合同。

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const KEY_Z: u32 = 0x007a;
const KEY_Y: u32 = 0x0079;
const DEFAULT_LIMIT: usize = 200;

#[derive(Debug)]
pub struct Undo {
    states: Vec<String>,
    pos: usize,
    /// 撤销/重做回填期间为真, observe 跳过记录
    applying: bool,
    limit: usize,
}

impl Undo {
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_LIMIT)
    }

    pub fn with_limit(limit: usize) -> Self {
        Undo {
            states: vec![String::new()],
            pos: 0,
            applying: false,
            limit: limit.max(1),
        }
    }

    /// 缓冲变化时记录新状态(回填期间跳过, 内容未变时去重)。
    pub fn observe(&mut self, text: &str) {
        if self.applying || self.states[self.pos] == text {
            return;
        }
        self.states.truncate(self.pos + 1);
        self.states.push(text.to_string());
        if self.states.len() > self.limit {
            self.states.remove(0);
        } else {
            self.pos += 1;
        }
    }

    /// 回退一步; None = 已在最早状态。
    pub fn undo(&mut self) -> Option<String> {
        if self.pos == 0 {
            return None;
        }
        self.pos -= 1;
        Some(self.states[self.pos].clone())
    }

    /// 前进一步; None = 已在最新状态。
    pub fn redo(&mut self) -> Option<String> {
        if self.pos + 1 >= self.states.len() {
            return None;
        }
        self.pos += 1;
        Some(self.states[self.pos].clone())
    }

    /// 清空历史(编码框上屏/Esc 后调用: 编码是瞬态的, 历史按词重开)。
    pub fn reset(&mut self) {
        self.states.clear();
        self.states.push(String::new());
        self.pos = 0;
    }

    fn set_applying(&mut self, v: bool) {
        self.applying = v;
    }
}

impl Default for Undo {
    fn default() -> Self {
        Self::new()
    }
}

/// 撤销键的处理结果。
pub enum KeyOutcome {
    /// 已撤销/重做, 调用方吞键(Stop)
    Handled,
    /// 撤销键但栈已到头, 调用方决定直通或吞掉
    NothingToDo,
    /// 与撤销无关的键, 调用方维持原处理
    NotOurs,
}

/// 文本框抽象: 单行 Entry 或 TextView 的 buffer。
pub enum Target<'a> {
    Entry(&'a gtk::Entry),
    Buffer(&'a gtk::TextBuffer),
}

/// 判定 Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z 并执行撤销/重做。
pub fn handle(target: Target<'_>, undo: &Rc<RefCell<Undo>>, ev: &gdk::EventKey) -> KeyOutcome {
    let st = ev.state();
    if !st.contains(gdk::ModifierType::CONTROL_MASK) {
        return KeyOutcome::NotOurs;
    }
    let kv = *ev.keyval();
    let is_undo = kv == KEY_Z && !st.contains(gdk::ModifierType::SHIFT_MASK);
    let is_redo = kv == KEY_Y || (kv == KEY_Z && st.contains(gdk::ModifierType::SHIFT_MASK));
    if !is_undo && !is_redo {
        return KeyOutcome::NotOurs;
    }
    let next = {
        let mut u = undo.borrow_mut();
        if is_undo { u.undo() } else { u.redo() }
    };
    let Some(text) = next else {
        return KeyOutcome::NothingToDo;
    };
    undo.borrow_mut().set_applying(true);
    match target {
        Target::Entry(e) => e.set_text(&text),
        Target::Buffer(b) => b.set_text(&text),
    }
    undo.borrow_mut().set_applying(false);
    KeyOutcome::Handled
}

fn observe_buffer(undo: &Rc<RefCell<Undo>>, buf: &gtk::TextBuffer) {
    let undo = undo.clone();
    buf.connect_changed(move |b| {
        let t = b
            .text(&b.start_iter(), &b.end_iter(), false)
            .unwrap_or_default()
            .to_string();
        undo.borrow_mut().observe(&t);
    });
}

/// 观察 Entry 文本变更(主窗编码框与短语编辑器编码框共用)。
/// 按键拦截由调用方做: 主窗编码框在 on_entry_key 里有空缓冲直通逻辑。
pub fn observe_entry(undo: &Rc<RefCell<Undo>>, entry: &gtk::Entry) {
    let undo = undo.clone();
    entry.connect_changed(move |e| {
        let t = e.text().to_string();
        undo.borrow_mut().observe(&t);
    });
}

/// TextView 一站式: 观察变更 + 拦截撤销/重做键(打字板/短语内容区)。
pub fn wire_text_view(tv: &gtk::TextView) -> Rc<RefCell<Undo>> {
    let undo = Rc::new(RefCell::new(Undo::new()));
    observe_buffer(&undo, &tv.buffer().expect("TextView 应有默认 buffer"));
    {
        let undo = undo.clone();
        tv.connect_key_press_event(move |tv, ev| {
            let buf = tv.buffer().expect("TextView 应有默认 buffer");
            if let KeyOutcome::NotOurs = handle(Target::Buffer(&buf), &undo, ev) {
                glib::Propagation::Proceed
            } else {
                glib::Propagation::Stop
            }
        });
    }
    undo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 撤销重做往返() {
        let mut u = Undo::new();
        u.observe("a");
        u.observe("ab");
        u.observe("abc");
        assert_eq!(u.undo().as_deref(), Some("ab"));
        assert_eq!(u.undo().as_deref(), Some("a"));
        assert_eq!(u.undo().as_deref(), Some(""));
        assert_eq!(u.undo(), None, "已在最早状态");
        assert_eq!(u.redo().as_deref(), Some("a"));
        assert_eq!(u.redo().as_deref(), Some("ab"));
    }

    #[test]
    fn 撤销后新输入丢弃重做分支() {
        let mut u = Undo::new();
        u.observe("ab");
        assert_eq!(u.undo().as_deref(), Some(""));
        u.observe("ax");
        assert_eq!(u.redo(), None, "重做分支应被丢弃");
        assert_eq!(u.undo().as_deref(), Some(""));
    }

    #[test]
    fn 回填期间不记录() {
        let mut u = Undo::new();
        u.observe("ab");
        u.set_applying(true);
        u.observe("");
        u.set_applying(false);
        // 回填的 "" 不入栈: undo 仍逐步回到 ""
        assert_eq!(u.undo().as_deref(), Some(""));
        assert_eq!(u.undo(), None);
    }

    #[test]
    fn 重置后按词重开历史() {
        let mut u = Undo::new();
        u.observe("wqvb");
        u.reset();
        assert_eq!(u.undo(), None);
        assert_eq!(u.redo(), None);
        u.observe("nk");
        assert_eq!(u.undo().as_deref(), Some(""));
    }

    #[test]
    fn 上限截断最旧状态() {
        let mut u = Undo::with_limit(3);
        for s in ["a", "ab", "abc", "abcd"] {
            u.observe(s);
        }
        // 保留最近 3 个: [ab, abc, abcd]
        assert_eq!(u.undo().as_deref(), Some("abc"));
        assert_eq!(u.undo().as_deref(), Some("ab"));
        assert_eq!(u.undo(), None, "更早状态应被截断");
    }
}
