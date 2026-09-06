//! C ABI FFI(合同 §3):`liblyyime_core.so`,供 Mode A python(ibs 引擎,ctypes)与测试使用。
//!
//! 全部函数线程不安全,宿主保证单线程调用。
//! `lyyime_process_key` 把效果流序列化为 JSON 数组(一字不差的下述格式),
//! 候选文本另经 `lyyime_cand` / `lyyime_cand_comment` 逐条获取:
//!
//! ```json
//! [{"t":"commit","s":"你好"},{"t":"preedit","s":"nihao"},
//!  {"t":"cands","n":5,"page":0,"pages":3},{"t":"pass"},{"t":"consumed"},
//!  {"t":"notice","s":"已造词:你好(wqvb)"},{"t":"mode","m":1}]
//! ```
//!
//! 约定:
//! - JSON 手工拼装而非 serde 序列化,保证键序 `t` 在前、与合同示例一致;
//! - `preedit` 为 None 时输出 `{"t":"preedit"}`(无 s 字段,宿主据此清除预编辑);
//! - 返回值 = 写入 buf 所需字节数(含 `\0`);buf 容量不足时不写入并返回 `-needed`
//!   (可先用 `buf=NULL, cap=0` 探测所需大小)。

use std::os::raw::{c_char, c_int};
use std::ptr;

use crate::engine::Engine;
use crate::types::{Effect, LKey, Mode};

// ---- key_id 枚举(python 侧同名常量,合同 §3)----
pub const LKEY_CHAR: c_int = 0;
pub const LKEY_DIGIT: c_int = 1;
pub const LKEY_SPACE: c_int = 2;
pub const LKEY_ENTER: c_int = 3;
pub const LKEY_BACKSPACE: c_int = 4;
pub const LKEY_ESC: c_int = 5;
pub const LKEY_PAGEUP: c_int = 6;
pub const LKEY_PAGEDOWN: c_int = 7;
pub const LKEY_PUNCT: c_int = 8;
pub const LKEY_SHIFTPRESS: c_int = 9;
pub const LKEY_OTHER: c_int = 10;
// ---- 造词(合同 §12):热键与方向键 ----
pub const LKEY_COIN: c_int = 11;
pub const LKEY_LEFT: c_int = 12;
pub const LKEY_RIGHT: c_int = 13;
pub const LKEY_UP: c_int = 14;
pub const LKEY_DOWN: c_int = 15;

/// 模式 → FFI 整数:0 = 中文,1 = 英文。
pub fn mode_int(mode: Mode) -> c_int {
    match mode {
        Mode::Chinese => 0,
        Mode::English => 1,
    }
}

/// JSON 字符串转义(保持 UTF-8 原样,仅转义引号/反斜杠/控制字符)。
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    push_escaped(s, &mut out);
    out
}

fn push_escaped(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

/// 效果流 → 合同 §3 的 JSON 数组。
///
/// `page`/`pages` 为引擎当前页状态(`cands` 效果的页信息在效果流本身之外,
/// 由引擎处理完该键后的状态给出)。同一效果流中只会有一个 `cands` 效果。
pub fn effects_json(effects: &[Effect], page: usize, pages: usize) -> String {
    let mut out = String::from("[");
    for (i, e) in effects.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match e {
            Effect::Commit(s) => {
                out.push_str("{\"t\":\"commit\",\"s\":\"");
                push_escaped(s, &mut out);
                out.push_str("\"}");
            }
            Effect::Preedit(Some(s)) => {
                out.push_str("{\"t\":\"preedit\",\"s\":\"");
                push_escaped(s, &mut out);
                out.push_str("\"}");
            }
            Effect::Preedit(None) => out.push_str("{\"t\":\"preedit\"}"),
            Effect::Candidates(list) => {
                out.push_str(&format!(
                    "{{\"t\":\"cands\",\"n\":{},\"page\":{},\"pages\":{}}}",
                    list.len(),
                    page,
                    pages
                ));
            }
            Effect::Pass => out.push_str("{\"t\":\"pass\"}"),
            Effect::Consumed => out.push_str("{\"t\":\"consumed\"}"),
            Effect::Notice(s) => {
                out.push_str("{\"t\":\"notice\",\"s\":\"");
                push_escaped(s, &mut out);
                out.push_str("\"}");
            }
            Effect::ModeChanged(m) => {
                out.push_str(&format!("{{\"t\":\"mode\",\"m\":{}}}", mode_int(*m)));
            }
        }
    }
    out.push(']');
    out
}

/// key_id + 码点 → 抽象键;非法组合按 `Other` 处理。
fn key_from(key_id: c_int, chr: u32) -> LKey {
    match key_id {
        LKEY_CHAR => char::from_u32(chr)
            .filter(|c| c.is_ascii_lowercase())
            .map_or(LKey::Other, LKey::Char),
        LKEY_DIGIT => {
            if (u32::from(b'1')..=u32::from(b'9')).contains(&chr) {
                LKey::Digit((chr - u32::from(b'0')) as u8)
            } else {
                LKey::Other
            }
        }
        LKEY_SPACE => LKey::Space,
        LKEY_ENTER => LKey::Enter,
        LKEY_BACKSPACE => LKey::Backspace,
        LKEY_ESC => LKey::Esc,
        LKEY_PAGEUP => LKey::PageUp,
        LKEY_PAGEDOWN => LKey::PageDown,
        LKEY_PUNCT => char::from_u32(chr).map_or(LKey::Other, LKey::Punct),
        LKEY_SHIFTPRESS => LKey::ShiftPress,
        LKEY_COIN => LKey::Coin,
        LKEY_LEFT => LKey::ArrowLeft,
        LKEY_RIGHT => LKey::ArrowRight,
        LKEY_UP => LKey::ArrowUp,
        LKEY_DOWN => LKey::ArrowDown,
        _ => LKey::Other,
    }
}

/// 把字符串写进 C 缓冲:返回所需字节数(含 `\0`);容量不足不写入,返回负的所需数。
///
/// # Safety
/// `buf` 必须可写 `cap` 字节(cap ≤ 0 或空指针时不会解引用)。
unsafe fn put_cstr(buf: *mut c_char, cap: i64, s: &str) -> i64 {
    let needed = s.len() as i64 + 1;
    if buf.is_null() || cap < needed {
        return -needed;
    }
    ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, s.len());
    *buf.add(s.len()) = 0;
    needed
}

/// 创建引擎。`data_dir` 为 UTF-8 路径;目录不存在得到空引擎(仍非 NULL)。
/// 参数为 NULL / 非 UTF-8 / 引擎构造失败时返回 NULL。
///
/// # Safety
/// `data_dir` 必须是以 `\0` 结尾的有效 C 字符串(可为 NULL)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_new(data_dir: *const c_char) -> *mut Engine {
    if data_dir.is_null() {
        return ptr::null_mut();
    }
    let Ok(dir) = std::ffi::CStr::from_ptr(data_dir).to_str() else {
        return ptr::null_mut();
    };
    match Engine::new(std::path::Path::new(dir)) {
        Ok(eng) => Box::into_raw(Box::new(eng)),
        Err(_) => ptr::null_mut(),
    }
}

/// 销毁引擎(NULL 安全)。
///
/// # Safety
/// `eng` 必须来自 `lyyime_new`,且此后不得再被使用。
#[no_mangle]
pub unsafe extern "C" fn lyyime_free(eng: *mut Engine) {
    if !eng.is_null() {
        drop(Box::from_raw(eng));
    }
}

/// 清空组合缓冲(焦点切换时调用;NULL 安全)。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_reset(eng: *mut Engine) {
    if let Some(e) = eng.as_mut() {
        e.reset();
    }
}

/// 当前模式:0 = 中文,1 = 英文。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_mode(eng: *mut Engine) -> c_int {
    match eng.as_ref() {
        Some(e) => mode_int(e.mode()),
        None => 0,
    }
}

/// 切换模式,返回新模式的整数(0/1)。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_toggle_mode(eng: *mut Engine) -> c_int {
    match eng.as_mut() {
        Some(e) => mode_int(e.toggle_mode()),
        None => 0,
    }
}

/// 设置“四码后继续输入字母先顶屏当前选中”。
///
/// 非 0 启用,0 关闭;NULL 引擎忽略。返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_commit_after_four(eng: *mut Engine, enabled: c_int) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.commit_on_extra_after_four = enabled != 0;
            e.set_config(cfg);
            c_int::from(e.config().commit_on_extra_after_four)
        }
        None => 0,
    }
}

/// 喂一个键,把效果流 JSON 写入 `buf`。
///
/// 返回所需字节数(含 `\0`);容量不足时不写入并返回 `-needed`;
/// `eng` 为 NULL 返回 0。`chr` 为 `Char`/`Digit`/`Punct` 的码点,其余键填 0
/// (Digit 传 '1'..'9' 的 ASCII 码点,core 按 chr-'0' 解码取数字)。
///
/// 重试纪律(§3 v1.1):按键先纯读取规划,效果流 JSON 确认能写入 buf 后
/// 才落内部状态(学习/清缓冲/翻页等);返回 -needed 时引擎状态保持按键前,
/// 宿主扩容重试同一键不会二次生效。
///
/// # Safety
/// `eng` 必须是有效的引擎指针;`buf` 可写 `buf_cap` 字节(可为 NULL)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_process_key(
    eng: *mut Engine,
    key_id: c_int,
    chr: u32,
    buf: *mut c_char,
    buf_cap: i64,
) -> i64 {
    let Some(e) = eng.as_mut() else { return 0 };
    // ① 纯读取规划:产出效果流与下一状态,内部状态保持按键前。
    let (effects, plan) = e.plan_key(key_from(key_id, chr));
    let json = effects_json(
        &effects,
        plan.page,
        Engine::pages_for(plan.cands.len(), e.page_size()),
    );
    // ② 确认能写入宿主缓冲,才提交内部状态。
    let n = put_cstr(buf, buf_cap, &json);
    if n >= 0 {
        e.apply_plan(plan);
    }
    n
}

/// 取当前页第 `i`(0 起)个候选文本写入 `buf`(含 `\0` 的所需字节数,
/// 负值 = 容量不足;无效下标写空串返回 1)。
///
/// # Safety
/// `eng` 必须是有效的引擎指针;`buf` 可写 `cap` 字节。
#[no_mangle]
pub unsafe extern "C" fn lyyime_cand(
    eng: *mut Engine,
    i: c_int,
    buf: *mut c_char,
    cap: c_int,
) -> c_int {
    let Some(e) = eng.as_ref() else { return 0 };
    let text = e
        .flush_page()
        .get(i.max(0) as usize)
        .map(|c| c.text.as_str())
        .unwrap_or("");
    put_cstr(buf, cap as i64, text) as c_int
}

/// 取当前页第 `i` 个候选的注释(编码/拼音提示),返回值语义同 [`lyyime_cand`]。
///
/// # Safety
/// `eng` 必须是有效的引擎指针;`buf` 可写 `cap` 字节。
#[no_mangle]
pub unsafe extern "C" fn lyyime_cand_comment(
    eng: *mut Engine,
    i: c_int,
    buf: *mut c_char,
    cap: c_int,
) -> c_int {
    let Some(e) = eng.as_ref() else { return 0 };
    let text = e
        .flush_page()
        .get(i.max(0) as usize)
        .map(|c| c.comment.as_str())
        .unwrap_or("");
    put_cstr(buf, cap as i64, text) as c_int
}
