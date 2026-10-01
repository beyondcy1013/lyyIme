//! C ABI FFI(合同 §3):`liblyyime_core.so`,供 Mode A python(ibs 引擎,ctypes)与测试使用。
//!
//! 全部函数线程不安全,宿主保证单线程调用。
//! `lyyime_process_key` 把效果流序列化为 JSON 数组(一字不差的下述格式),
//! 候选文本另经 `lyyime_cand` / `lyyime_cand_comment` 逐条获取:
//!
//! ```json
//! [{"t":"commit","s":"你好"},{"t":"preedit","s":"nihao"},
//!  {"t":"cands","n":5,"page":0,"pages":3},{"t":"pass"},{"t":"consumed"},
//!  {"t":"notice","s":"已造词:你好(wqvb)"},{"t":"mode","m":1},
//!  {"t":"hint","s":"词组提示:「你好」可用 wqvb 打出"},
//!  {"t":"action","i":0}]
//! ```
//!
//! 约定:
//! - JSON 手工拼装而非 serde 序列化,保证键序 `t` 在前、与合同示例一致;
//! - `preedit` 为 None 时输出 `{"t":"preedit"}`(无 s 字段,宿主据此清除预编辑);
//! - `action`(合同 §14 快速功能键)的 `i` 为配置列表 `quick_actions` 的下标,
//!   宿主经 `lyyime_action_command` 取 command 后执行,不上屏文本;
//! - 返回值 = 写入 buf 所需字节数(含 `\0`);buf 容量不足时不写入并返回 `-needed`
//!   (可先用 `buf=NULL, cap=0` 探测所需大小)。

use std::os::raw::{c_char, c_int};
use std::ptr;

use crate::config::{EnCommit, QuickAction};
use crate::engine::Engine;
use crate::menu_trigger::{MenuTrigger, MENU_CATALOG};
use crate::types::{CandOp, Effect, LKey, Mode};

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
            Effect::Hint(s) => {
                out.push_str("{\"t\":\"hint\",\"s\":\"");
                push_escaped(s, &mut out);
                out.push_str("\"}");
            }
            Effect::ModeChanged(m) => {
                out.push_str(&format!("{{\"t\":\"mode\",\"m\":{}}}", mode_int(*m)));
            }
            Effect::Action(i) => {
                out.push_str(&format!("{{\"t\":\"action\",\"i\":{i}}}"));
            }
        }
    }
    out.push(']');
    out
}

/// key_id + 码点 → 抽象键;非法组合按 `Other` 处理。
///
/// Char 接受小写(普通组词)与大写(Shift+字母 原样传入,全大写输入候选,
/// 2026-09-28 需求);core 内部自行小写化并镜像敲入原形。
fn key_from(key_id: c_int, chr: u32) -> LKey {
    match key_id {
        LKEY_CHAR => char::from_u32(chr)
            .filter(|c| c.is_ascii_lowercase() || c.is_ascii_uppercase())
            .map_or(LKey::Other, LKey::Char),
        LKEY_DIGIT => {
            if (u32::from(b'0')..=u32::from(b'9')).contains(&chr) {
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

/// 设置“四码顶屏”(恰好四码且首选是五笔命中时,再输入字母先上屏首选,
/// 该字母开启新组合;首选是拼音/英文/功能键或缓冲是触发词前缀时不顶屏)。
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

/// 设置“四码首选自动上屏”(恰好四码、首选是五笔命中且候选条只此一条
/// 时免空格直接上屏;候选多于一条——同码重码或混排候选——保留组合
/// 待选/顶屏;首选是拼音/英文不触发)。
///
/// 非 0 启用,0 关闭;NULL 引擎忽略。返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_commit_first_at_four(
    eng: *mut Engine,
    enabled: c_int,
) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.commit_first_at_four = enabled != 0;
            e.set_config(cfg);
            c_int::from(e.config().commit_first_at_four)
        }
        None => 0,
    }
}

/// 设置“四码唯一自动上屏”(恰好四码且中文候选唯一时免空格直接上屏;
/// 唯一候选是英文词不触发;`lyyime_set_commit_first_at_four` 开启时被覆盖)。
///
/// 非 0 启用,0 关闭;NULL 引擎忽略。返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_commit_unique_four(
    eng: *mut Engine,
    enabled: c_int,
) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.commit_unique_four = enabled != 0;
            e.set_config(cfg);
            c_int::from(e.config().commit_unique_four)
        }
        None => 0,
    }
}

/// 设置“词组效率提示”(上屏后最近几字有更省键的词组时,候选条提示词与编码)。
///
/// 非 0 启用,0 关闭;NULL 引擎忽略。返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_phrase_hint(eng: *mut Engine, enabled: c_int) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.phrase_hint = enabled != 0;
            e.set_config(cfg);
            c_int::from(e.config().phrase_hint)
        }
        None => 0,
    }
}

/// 设置“回车上屏英文原串后的模式去向”(§6):0 = temp 临时(默认,保持
/// 中文模式),非 0 = en(上屏并切英文模式)。NULL 引擎忽略,返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_enter_english(eng: *mut Engine, en_mode: c_int) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.enter_english = if en_mode != 0 { EnCommit::English } else { EnCommit::Temp };
            e.set_config(cfg);
            c_int::from(e.config().enter_english == EnCommit::English)
        }
        None => 0,
    }
}

/// 设置“Shift 上屏英文原串后的模式去向”(§6):非 0 = en(默认,上屏并进入
/// 英文模式),0 = temp(仅上屏,保持中文模式)。NULL 引擎忽略,返回生效后 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_shift_english(eng: *mut Engine, en_mode: c_int) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.shift_english = if en_mode != 0 { EnCommit::English } else { EnCommit::Temp };
            e.set_config(cfg);
            c_int::from(e.config().shift_english == EnCommit::English)
        }
        None => 0,
    }
}

/// 设置"精确层单字按词频排位"(§5):非 0 = 开(默认,低频/生僻全码单字按
/// 语料词频降档让位高频词组),0 = 关(恢复恒居首位旧行为)。
/// NULL 引擎忽略,返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_exact_char_freq_rank(eng: *mut Engine, enabled: c_int) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.exact_char_freq_rank = enabled != 0;
            e.set_config(cfg);
            c_int::from(e.config().exact_char_freq_rank)
        }
        None => 0,
    }
}

/// 设置"上屏后联想"(中文词上屏后,候选条给出接下来可能输入的词句尾巴;
/// 默认开):非 0 启用,0 关闭(关闭同时清掉正在展示的联想行与上下文)。
/// NULL 引擎忽略,返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_next_word_prediction(
    eng: *mut Engine,
    enabled: c_int,
) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.next_word_prediction = enabled != 0;
            e.set_config(cfg);
            c_int::from(e.config().next_word_prediction)
        }
        None => 0,
    }
}

/// 设置中文标点开关(运行时窄化路径:只改开关并复位引号配对状态,
/// 不重装载词表、不清组合缓冲/候选/联想上下文)。非 0 = 中文标点,
/// 0 = 标点原样直通。NULL 引擎忽略,返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_chinese_punctuation(
    eng: *mut Engine,
    enabled: c_int,
) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            e.set_chinese_punctuation(enabled != 0);
            c_int::from(e.config().cn_punct)
        }
        None => 0,
    }
}

/// 翻转中文标点开关(Ctrl+. 热键路径),返回切换后的 0/1。
/// NULL 引擎忽略,返回 0。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_toggle_chinese_punctuation(eng: *mut Engine) -> c_int {
    match eng.as_mut() {
        Some(e) => c_int::from(e.toggle_chinese_punctuation()),
        None => 0,
    }
}

/// 喂一个键,把效果流 JSON 写入 `buf`。
///
/// 返回所需字节数(含 `\0`);容量不足时不写入并返回 `-needed`;
/// `eng` 为 NULL 返回 0。`chr` 为 `Char`/`Digit`/`Punct` 的码点,其余键填 0
/// (Digit 传 '0'..'9' 的 ASCII 码点,core 按 chr-'0' 解码取数字,
/// 0 = 选第 10 个候选)。
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

/// 设置“快速功能键”总开关(合同 §14)。
///
/// 非 0 启用,0 关闭;NULL 引擎忽略。返回生效后的 0/1。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_set_quick_actions_enabled(
    eng: *mut Engine,
    enabled: c_int,
) -> c_int {
    match eng.as_mut() {
        Some(e) => {
            let mut cfg = e.config().clone();
            cfg.quick_actions_enabled = enabled != 0;
            e.set_config(cfg);
            c_int::from(e.config().quick_actions_enabled)
        }
        None => 0,
    }
}

/// 清空快速功能键列表(宿主按配置重注入前的复位步骤,合同 §14)。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_clear_quick_actions(eng: *mut Engine) {
    if let Some(e) = eng.as_mut() {
        let mut cfg = e.config().clone();
        cfg.quick_actions.clear();
        e.set_config(cfg);
    }
}

/// 追加一条快速功能键(合同 §14)。返回 0 成功;-1 参数非法/为空/超上限。
///
/// # Safety
/// `eng` 必须是有效的引擎指针;三个字符串均须是以 `\0` 结尾的有效 C 字符串
/// (可为 NULL,视作非法)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_add_quick_action(
    eng: *mut Engine,
    trigger: *const c_char,
    label: *const c_char,
    command: *const c_char,
) -> c_int {
    let Some(e) = eng.as_mut() else { return -1 };
    let read = |p: *const c_char| -> Option<String> {
        if p.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(p).to_str().ok().map(str::to_string)
    };
    let (Some(trigger), Some(label), Some(command)) = (read(trigger), read(label), read(command))
    else {
        return -1;
    };
    let mut cfg = e.config().clone();
    if cfg.quick_actions.len() >= crate::config::QUICK_ACTIONS_MAX
        || !QuickAction::trigger_valid(&trigger)
        || label.trim().is_empty()
        || command.trim().is_empty()
    {
        return -1;
    }
    cfg.quick_actions.push(QuickAction {
        trigger,
        label,
        command,
    });
    e.set_config(cfg);
    0
}

/// 取快速功能键第 `i` 条的 command(`@settings`/`@help` 内置或 shell 命令),
/// 返回值语义同 [`lyyime_cand`]。
///
/// # Safety
/// `eng` 必须是有效的引擎指针;`buf` 可写 `cap` 字节。
#[no_mangle]
pub unsafe extern "C" fn lyyime_action_command(
    eng: *mut Engine,
    i: c_int,
    buf: *mut c_char,
    cap: c_int,
) -> c_int {
    let Some(e) = eng.as_ref() else { return 0 };
    let text = e
        .config()
        .quick_actions
        .get(i.max(0) as usize)
        .map(|a| a.command.as_str())
        .unwrap_or("");
    put_cstr(buf, cap as i64, text) as c_int
}

/// 选中当前页第 `idx`(0 起)个候选(鼠标/面板点选,合同 §14),
/// 效果流 JSON 写入 `buf`;返回值与重试纪律同 [`lyyime_process_key`]。
///
/// # Safety
/// `eng` 必须是有效的引擎指针;`buf` 可写 `buf_cap` 字节(可为 NULL)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_select_candidate(
    eng: *mut Engine,
    idx: c_int,
    buf: *mut c_char,
    buf_cap: i64,
) -> i64 {
    let Some(e) = eng.as_mut() else { return 0 };
    let (effects, plan) = e.plan_select_candidate(idx.max(0) as usize);
    let json = effects_json(
        &effects,
        plan.page,
        Engine::pages_for(plan.cands.len(), e.page_size()),
    );
    let n = put_cstr(buf, buf_cap, &json);
    if n >= 0 {
        e.apply_plan(plan);
    }
    n
}

// ---- §15 候选右键操作(可选符号组;旧库缺符号时宿主禁用右键菜单即可) ----

/// `lyyime_cand_op` 的 op 编码:1=固定首位/取消固定,2=删除词组,3=反查英文。
pub const LYY_CAND_OP_PIN: c_int = 1;
pub const LYY_CAND_OP_DELETE: c_int = 2;
pub const LYY_CAND_OP_EN: c_int = 3;

/// 当前页第 `idx` 个候选的固定状态:1=已固定,0=未固定,-1=非法
/// (功能键候选/越界/NULL 引擎)。宿主据此决定菜单文案"固定首位/取消固定",
/// -1 时应禁用整个操作菜单。
///
/// # Safety
/// `eng` 必须是有效的引擎指针。
#[no_mangle]
pub unsafe extern "C" fn lyyime_cand_pinned(eng: *mut Engine, idx: c_int) -> c_int {
    let Some(e) = eng.as_ref() else {
        return -1;
    };
    match e.cand_pinned(idx.max(0) as usize) {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    }
}

/// 对当前页第 `idx` 个候选执行右键操作(合同 §15),效果流 JSON 写入 `buf`;
/// 返回值与两段式重试纪律同 [`lyyime_process_key`](固定/删除仅在 JSON
/// 确认写入后才落盘)。`op` 见 `LYY_CAND_OP_*`;非法 op 回 `[{"t":"consumed"}]`。
///
/// # Safety
/// `eng` 必须是有效的引擎指针;`buf` 可写 `buf_cap` 字节(可为 NULL)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_cand_op(
    eng: *mut Engine,
    idx: c_int,
    op: c_int,
    buf: *mut c_char,
    buf_cap: i64,
) -> i64 {
    let Some(e) = eng.as_mut() else { return 0 };
    let op = match op {
        LYY_CAND_OP_PIN => CandOp::PinToggle,
        LYY_CAND_OP_DELETE => CandOp::Delete,
        LYY_CAND_OP_EN => CandOp::EnLookup,
        _ => return put_cstr(buf, buf_cap, "[{\"t\":\"consumed\"}]"),
    };
    let (effects, plan) = e.plan_cand_op(idx.max(0) as usize, op);
    let json = effects_json(
        &effects,
        plan.page,
        Engine::pages_for(plan.cands.len(), e.page_size()),
    );
    let n = put_cstr(buf, buf_cap, &json);
    if n >= 0 {
        e.apply_plan(plan);
    }
    n
}

// ---- 菜单触发(可选符号组;旧库缺符号时宿主整体禁用该特性即可) ----
//
// 该对象与 Engine 完全解耦:宿主只在真实上屏文本产生后喂入
// `lyyime_menu_trigger_commit`,不占用引擎规划/重试路径,因此不会与 §3
// 两段式契约互相干扰。`commit` 自身同样守两段式纪律:先在克隆副本上计算
// 提示,确认能完整写入 buf 才落状态;返回 -needed 时尾串/待执行保持原样,
// 宿主扩容重试不会重复记尾串或重复产出提示。

/// 创建菜单触发器(独立对象,与引擎无关)。
///
/// # Safety
/// 返回值须由 `lyyime_menu_trigger_free` 释放;宿主保证单线程调用。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_new() -> *mut MenuTrigger {
    Box::into_raw(Box::new(MenuTrigger::new()))
}

/// 销毁菜单触发器(NULL 安全)。
///
/// # Safety
/// `mt` 必须来自 `lyyime_menu_trigger_new`,且此后不得再被使用。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_free(mt: *mut MenuTrigger) {
    if !mt.is_null() {
        drop(Box::from_raw(mt));
    }
}

/// 配置菜单触发器:`enabled` 非 0 启用;`key` 为 F1–F12 序号(1..=12,越界
/// 回退默认 7);`disabled` 为逗号分隔的稳定 id 黑名单(NULL 视为空)。
/// 配置变更视为硬边界:尾串与待执行一起复位。
/// 返回 0;`mt` 为 NULL 或 `disabled` 非 UTF-8 时返回 -1。
///
/// # Safety
/// `mt` 必须是有效指针;`disabled` 须是以 `\0` 结尾的 C 字符串或 NULL。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_configure(
    mt: *mut MenuTrigger,
    enabled: c_int,
    key: c_int,
    disabled: *const c_char,
) -> c_int {
    let Some(t) = mt.as_mut() else { return -1 };
    let disabled = if disabled.is_null() {
        ""
    } else {
        match std::ffi::CStr::from_ptr(disabled).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };
    // 越界/负数交给 configure 回退默认键
    t.configure(enabled != 0, u8::try_from(key).unwrap_or(0), disabled);
    0
}

/// 注入宿主实际截屏快捷键;NULL/空/非法写法清除替代快捷键提示。
/// 返回 0;`mt` = NULL 或 `spec` 非 UTF-8 返回 -1。
/// # Safety
/// `mt` 必须指向有效 MenuTrigger;`spec` 须为 NUL 结尾字符串或 NULL。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_set_shot_hotkey(
    mt: *mut MenuTrigger,
    spec: *const c_char,
) -> c_int {
    let Some(t) = mt.as_mut() else { return -1 };
    let spec = if spec.is_null() {
        ""
    } else {
        match std::ffi::CStr::from_ptr(spec).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        }
    };
    t.set_shot_hotkey(spec);
    0
}

/// 喂入一次真实上屏文本:更新尾串并做最长后缀匹配;命中时把提示文本
/// (`匹配了菜单功能「{label}」,按 F{n} 进入该功能`)写入 `buf`,返回
/// 所需字节数(含 `\0`);未命中写空串返回 1;`mt`/`text` 为 NULL 或非
/// UTF-8 返回 0;容量不足返回 `-needed` 且**不改内部状态**(两段式,可重试)。
///
/// # Safety
/// `mt` 必须是有效指针;`text` 须为 UTF-8 C 字符串;`buf` 可写 `buf_cap` 字节。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_commit(
    mt: *mut MenuTrigger,
    text: *const c_char,
    buf: *mut c_char,
    buf_cap: i64,
) -> i64 {
    let Some(t) = mt.as_mut() else { return 0 };
    if text.is_null() {
        return 0;
    }
    let Ok(s) = std::ffi::CStr::from_ptr(text).to_str() else {
        return 0;
    };
    let mut probe = t.clone();
    let hint = probe.on_commit(s).unwrap_or_default();
    let n = put_cstr(buf, buf_cap, &hint);
    if n >= 0 {
        *t = probe;
    }
    n
}

/// 每个非确认按键调用:仅清待执行(保留尾串,允许跨上屏续接)。
/// 返回 1 = 确实清掉了待执行(宿主可清掉提示条),0 = 本就无待执行。
///
/// # Safety
/// `mt` 必须是有效指针或 NULL(后者返回 0)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_cancel(mt: *mut MenuTrigger) -> c_int {
    mt.as_mut().map_or(0, |t| c_int::from(t.cancel_pending()))
}

/// 硬边界复位:尾串与待执行一起清空(退格/翻页/Esc/修饰键/直通文本/
/// 焦点与模式切换/引擎与配置重载/AI 进出等)。返回值语义同上。
///
/// # Safety
/// `mt` 必须是有效指针或 NULL(后者返回 0)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_reset(mt: *mut MenuTrigger) -> c_int {
    mt.as_mut().map_or(0, |t| c_int::from(t.reset()))
}

/// 一次性取出待执行目录下标;无待执行返回 -1。不删文本、不重复上屏。
///
/// # Safety
/// `mt` 必须是有效指针或 NULL(后者返回 -1)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_take(mt: *mut MenuTrigger) -> c_int {
    mt.as_mut()
        .and_then(|t| t.take_pending())
        .and_then(|i| c_int::try_from(i).ok())
        .unwrap_or(-1)
}

/// 当前待执行目录下标(不消费);无待执行返回 -1。
///
/// # Safety
/// `mt` 必须是有效指针或 NULL(后者返回 -1)。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_pending(mt: *mut MenuTrigger) -> c_int {
    mt.as_ref()
        .and_then(|t| t.pending())
        .and_then(|i| c_int::try_from(i).ok())
        .unwrap_or(-1)
}

/// 目录条目数(设置窗据此动态生成黑名单勾选行)。
#[no_mangle]
pub extern "C" fn lyyime_menu_trigger_count() -> c_int {
    MENU_CATALOG.len() as c_int
}

/// 目录第 `i` 项稳定 id 写入 `buf`(返回值语义同 [`lyyime_cand`];
/// 越界写空串返回 1)。
///
/// # Safety
/// `buf` 可写 `cap` 字节。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_id(i: c_int, buf: *mut c_char, cap: c_int) -> c_int {
    let s = usize::try_from(i)
        .ok()
        .and_then(|u| MENU_CATALOG.get(u))
        .map(|m| m.id)
        .unwrap_or("");
    put_cstr(buf, cap as i64, s) as c_int
}

/// 目录第 `i` 项显示名写入 `buf`(语义同 [`lyyime_menu_trigger_id`])。
///
/// # Safety
/// `buf` 可写 `cap` 字节。
#[no_mangle]
pub unsafe extern "C" fn lyyime_menu_trigger_label(i: c_int, buf: *mut c_char, cap: c_int) -> c_int {
    let s = usize::try_from(i)
        .ok()
        .and_then(|u| MENU_CATALOG.get(u))
        .map(|m| m.label)
        .unwrap_or("");
    put_cstr(buf, cap as i64, s) as c_int
}
