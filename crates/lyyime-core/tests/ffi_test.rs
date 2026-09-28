//! FFI 集成测试(合同 §3):直接以 Rust 函数形式调用 `extern "C"` 导出,
//! 验证 JSON 效果流一字不差、`-needed` 容量约定、候选/注释读取与模式切换。
//! (真实 .so 的 ctypes 冒烟另见 ffi-test.py。)

mod common;

use common::{fixtures, TempDir};
use lyyime_core::ffi::{
    lyyime_cand, lyyime_cand_comment, lyyime_free, lyyime_mode, lyyime_new,
    lyyime_set_enter_english, lyyime_set_exact_char_freq_rank, lyyime_set_shift_english,
    lyyime_process_key, lyyime_reset, lyyime_set_commit_after_four,
    lyyime_set_commit_first_at_four, lyyime_set_commit_unique_four,
    lyyime_set_phrase_hint, lyyime_toggle_mode,
    LKEY_BACKSPACE, LKEY_CHAR, LKEY_DIGIT, LKEY_ENTER, LKEY_ESC, LKEY_OTHER, LKEY_PAGEDOWN,
    LKEY_PAGEUP, LKEY_PUNCT, LKEY_SHIFTPRESS, LKEY_SPACE,
};
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::sync::Once;

/// 把进程 HOME 指向临时目录(仅一次):FFI 层没有配置入口,
/// lyyime_new 构造的引擎默认用户词典在 $HOME 下,须封闭以免污染真实 HOME。
///
/// 各测试并行共享本进程 HOME,若允许 user.tsv 落盘,先跑用例提交的学习词
/// 会改变后跑用例的候选排序(顺序敏感)。注意测试常以 root 运行,目录权限位
/// 挡不住写入;因此把 user.tsv 预创建为**目录**:load 读入失败 → 空词典,
/// save 的 rename(文件→目录)必然失败且被静默忽略 —— 学习只在各引擎内存内
/// 生效,引擎之间零共享可变状态,与测试执行顺序无关。
fn hermetic_home() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let td = Box::leak(Box::new(TempDir::new()));
        let user_tsv_dir = td.path.join(".local/share/lyyime/user.tsv");
        let _ = std::fs::create_dir_all(&user_tsv_dir);
        std::env::set_var("HOME", &td.path);
    });
}

struct FfiEngine(*mut lyyime_core::Engine);

impl FfiEngine {
    fn new(dir: &std::path::Path) -> Self {
        hermetic_home();
        let c = CString::new(dir.to_str().unwrap()).unwrap();
        let ptr = unsafe { lyyime_new(c.as_ptr()) };
        assert!(!ptr.is_null(), "lyyime_new 不应失败");
        Self(ptr)
    }

    /// 喂键:返回原始字节数;`n<0` 时为 -needed 且引擎状态不变(不写入)。
    fn key_raw(&mut self, key_id: c_int, chr: u32, buf: *mut c_char, cap: i64) -> i64 {
        unsafe { lyyime_process_key(self.0, key_id, chr, buf, cap) }
    }

    /// 喂键并取回 JSON(用足够大的缓冲)。
    fn key(&mut self, key_id: c_int, chr: u32) -> String {
        let mut buf = vec![0u8; 8192];
        let n = unsafe {
            lyyime_process_key(
                self.0,
                key_id,
                chr,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as i64,
            )
        };
        assert!(n > 0, "process_key 返回 {n}");
        String::from_utf8_lossy(&buf[..(n - 1) as usize]).into_owned()
    }

    fn key_char(&mut self, c: char) -> String {
        self.key(LKEY_CHAR, c as u32)
    }

    fn cand(&mut self, i: c_int) -> (String, String) {
        let mut b1 = vec![0u8; 512];
        let mut b2 = vec![0u8; 512];
        let n1 = unsafe { lyyime_cand(self.0, i, b1.as_mut_ptr() as *mut c_char, 512) };
        let n2 = unsafe { lyyime_cand_comment(self.0, i, b2.as_mut_ptr() as *mut c_char, 512) };
        assert!(n1 > 0 && n2 > 0);
        (
            String::from_utf8_lossy(&b1[..n1 as usize - 1]).into_owned(),
            String::from_utf8_lossy(&b2[..n2 as usize - 1]).into_owned(),
        )
    }
}

impl Drop for FfiEngine {
    fn drop(&mut self) {
        unsafe { lyyime_free(self.0) };
    }
}

#[test]
fn ffi_新建与释放_目录不存在得到空引擎() {
    hermetic_home();
    let c = CString::new("/nonexistent/lyyime-ffi-empty").unwrap();
    let eng = unsafe { lyyime_new(c.as_ptr()) };
    assert!(!eng.is_null(), "目录不存在也应返回空引擎而非 NULL");
    unsafe { lyyime_free(eng) };
    // NULL 参数 → 失败返回 NULL;free(NULL) 安全。
    assert!(unsafe { lyyime_new(std::ptr::null()) }.is_null());
    unsafe { lyyime_free(std::ptr::null_mut()) };
}

#[test]
fn ffi_键入首字母_效果json一字不差() {
    let mut eng = FfiEngine::new(&fixtures());
    let json = eng.key_char('n');
    assert_eq!(
        json,
        "[{\"t\":\"preedit\",\"s\":\"n\"},{\"t\":\"cands\",\"n\":2,\"page\":0,\"pages\":1}]"
    );
}

#[test]
fn ffi_空格顶屏_commit与清除的json序列() {
    let mut eng = FfiEngine::new(&fixtures());
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_SPACE, 0);
    assert_eq!(
        json,
        "[{\"t\":\"commit\",\"s\":\"你好\"},{\"t\":\"preedit\"},{\"t\":\"cands\",\"n\":0,\"page\":0,\"pages\":0},{\"t\":\"hint\",\"s\":\"词组提示:「你好」可用 wqvb 打出\"}]"
    );
}

#[test]
fn ffi_词组提示开关() {
    let mut eng = FfiEngine::new(&fixtures());
    // 默认开启:逐字上屏「你」「好」后,第二次 commit 追加词组提示。
    for c in "wqiy".chars() {
        eng.key_char(c);
    }
    eng.key(LKEY_SPACE, 0);
    for c in "vbg".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_SPACE, 0);
    assert!(
        json.contains("{\"t\":\"hint\",\"s\":\"词组提示:「你好」可用 wqvb 打出\"}"),
        "{json}"
    );
    // 关闭:同样键序不再出 hint;setter 回环返回生效值。
    assert_eq!(unsafe { lyyime_set_phrase_hint(eng.0, 0) }, 0);
    for c in "wqiy".chars() {
        eng.key_char(c);
    }
    eng.key(LKEY_SPACE, 0);
    for c in "vbg".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_SPACE, 0);
    assert!(!json.contains("\"t\":\"hint\""), "{json}");
    assert_eq!(unsafe { lyyime_set_phrase_hint(eng.0, 1) }, 1);
    // NULL 引擎:安全忽略,返回 0。
    assert_eq!(unsafe { lyyime_set_phrase_hint(std::ptr::null_mut(), 1) }, 0);
}

#[test]
fn ffi_buffer不足_返回负的所需字节数() {
    let eng = FfiEngine::new(&fixtures());
    // NULL 缓冲探测:返回 -(所需字节数),同时该键已生效。
    let probe =
        unsafe { lyyime_process_key(eng.0, LKEY_CHAR, 'n' as u32, std::ptr::null_mut(), 0) };
    assert!(probe < 0);
    let need = (-probe) as usize;
    // reset 清状态后重放同一键:容量差 1 字节 → 不写入,仍返回 -needed。
    unsafe { lyyime_reset(eng.0) };
    let mut small = vec![0u8; need - 1];
    let n2 = unsafe {
        lyyime_process_key(
            eng.0,
            LKEY_CHAR,
            'n' as u32,
            small.as_mut_ptr() as *mut c_char,
            (need - 1) as i64,
        )
    };
    assert_eq!(n2, probe, "容量不足应返回 -needed");
    assert!(small.iter().all(|&b| b == 0), "容量不足时不得写入");
    // 足量缓冲:写入成功,返回正的 needed,内容含结尾 \0。
    unsafe { lyyime_reset(eng.0) };
    let mut big = vec![0u8; need];
    let n3 = unsafe {
        lyyime_process_key(
            eng.0,
            LKEY_CHAR,
            'n' as u32,
            big.as_mut_ptr() as *mut c_char,
            need as i64,
        )
    };
    assert_eq!(n3, need as i64);
    assert_eq!(
        &big[..need - 1],
        "[{\"t\":\"preedit\",\"s\":\"n\"},{\"t\":\"cands\",\"n\":2,\"page\":0,\"pages\":1}]"
            .as_bytes()
    );
    assert_eq!(big[need - 1], 0, "末尾应有 \\0");
}

#[test]
fn ffi_候选与注释读取() {
    let mut eng = FfiEngine::new(&fixtures());
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    let (text, comment) = eng.cand(0);
    assert_eq!(text, "你好");
    assert_eq!(comment, "wqvb", "拼音命中的候选注释应反查为五笔编码");
    // 越界下标:写空串,返回 1(内容仅 \0)。
    let mut buf = vec![0u8; 64];
    let n = unsafe { lyyime_cand(eng.0, 9, buf.as_mut_ptr() as *mut c_char, 64) };
    assert_eq!(n, 1);
    assert_eq!(buf[0], 0);
}

#[test]
fn ffi_模式切换与reset() {
    let mut eng = FfiEngine::new(&fixtures());
    assert_eq!(unsafe { lyyime_mode(eng.0) }, 0);
    assert_eq!(unsafe { lyyime_toggle_mode(eng.0) }, 1);
    assert_eq!(unsafe { lyyime_mode(eng.0) }, 1);
    unsafe { lyyime_reset(eng.0) };
    // 英文态字母直通。
    assert_eq!(eng.key_char('n'), "[{\"t\":\"pass\"}]");
    assert_eq!(unsafe { lyyime_toggle_mode(eng.0) }, 0);
}

#[test]
fn ffi_英文上屏去向_回车临时与shift切模式() {
    let mut eng = FfiEngine::new(&fixtures());
    // 回车默认临时(enter_english = temp):上屏原串,无 mode 效果。
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_ENTER, 0);
    assert!(json.contains("{\"t\":\"commit\",\"s\":\"nihao\"}"), "{json}");
    assert!(!json.contains("\"t\":\"mode\""), "{json}");
    assert_eq!(unsafe { lyyime_mode(eng.0) }, 0);

    // Shift 默认 en(shift_english = en):上屏原串并切英文,效果流附 mode。
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_SHIFTPRESS, 0);
    assert!(json.contains("{\"t\":\"commit\",\"s\":\"nihao\"}"), "{json}");
    assert!(json.contains("{\"t\":\"mode\",\"m\":1}"), "{json}");
    assert_eq!(unsafe { lyyime_mode(eng.0) }, 1);
    assert_eq!(eng.key_char('n'), "[{\"t\":\"pass\"}]");
    assert_eq!(unsafe { lyyime_toggle_mode(eng.0) }, 0);

    // setter 回环:Shift 配成 temp 恢复"仅上屏"。
    assert_eq!(unsafe { lyyime_set_shift_english(eng.0, 0) }, 0);
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_SHIFTPRESS, 0);
    assert!(json.contains("{\"t\":\"commit\",\"s\":\"nihao\"}"), "{json}");
    assert!(!json.contains("\"t\":\"mode\""), "{json}");
    assert_eq!(unsafe { lyyime_mode(eng.0) }, 0);

    // 回车配成 en:上屏并切英文。
    assert_eq!(unsafe { lyyime_set_enter_english(eng.0, 1) }, 1);
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_ENTER, 0);
    assert!(json.contains("{\"t\":\"mode\",\"m\":1}"), "{json}");
    assert_eq!(unsafe { lyyime_mode(eng.0) }, 1);
    // NULL 引擎:安全忽略,返回 0。
    assert_eq!(unsafe { lyyime_set_enter_english(std::ptr::null_mut(), 1) }, 0);
    assert_eq!(unsafe { lyyime_set_shift_english(std::ptr::null_mut(), 1) }, 0);
}

#[test]
fn ffi_精确单字频率排位开关() {
    // 带 char_tier.tsv + 语料的临时词库:低频「氢」(aaa 全码)默认降档,
    // 首选是 aaaa 前缀词组「恭恭敬敬」;关掉开关恢复首选氢。
    let td = TempDir::new();
    for f in [
        "wubi.tsv",
        "pinyin_char.tsv",
        "pinyin_phrase.tsv",
        "suggestion.tsv",
        "english.tsv",
        "meta.json",
    ] {
        let src = fixtures().join(f);
        if src.exists() {
            std::fs::copy(&src, td.join(f)).unwrap();
        }
    }
    let mut py = std::fs::read_to_string(fixtures().join("pinyin_char.tsv")).unwrap();
    py.push_str("qing\t氢\t500\n");
    std::fs::write(td.join("pinyin_char.tsv"), py).unwrap();
    std::fs::write(td.join("char_tier.tsv"), "氢\t1\n").unwrap();
    let mut wb = std::fs::read_to_string(fixtures().join("wubi.tsv")).unwrap();
    wb.push_str("aaa\t氢\t3000\n");
    std::fs::write(td.join("wubi.tsv"), wb).unwrap();

    let mut eng = FfiEngine::new(&td.path);
    for c in "aaa".chars() {
        eng.key_char(c);
    }
    assert_eq!(eng.cand(0).0, "恭恭敬敬", "默认开:低频氢让位词组");
    // 关闭(setter 回环返回 0):恢复精确单字恒居首位。
    assert_eq!(unsafe { lyyime_set_exact_char_freq_rank(eng.0, 0) }, 0);
    eng.key(LKEY_ESC, 0);
    for c in "aaa".chars() {
        eng.key_char(c);
    }
    assert_eq!(eng.cand(0).0, "氢", "关闭后恢复首选氢");
    // 重新打开返回 1;NULL 引擎安全忽略。
    assert_eq!(unsafe { lyyime_set_exact_char_freq_rank(eng.0, 1) }, 1);
    assert_eq!(unsafe { lyyime_set_exact_char_freq_rank(std::ptr::null_mut(), 1) }, 0);
}

#[test]
fn ffi_四码顶屏开关() {
    let eng = FfiEngine::new(&fixtures());
    let mut eng = eng;
    // 输入 aa 有候选"式";关闭选项时第 3 个字母继续组词。
    eng.key_char('a');
    eng.key_char('a');
    let json = eng.key_char('a');
    assert!(json.contains("\"s\":\"aaa\""), "{json}");
    assert!(!json.contains("{\"t\":\"commit\"}"), "{json}");

    eng.key(LKEY_ESC, 0);
    // 本测单独验证四码顶屏:先关掉四码自动上屏,缓冲才能停在四码。
    assert_eq!(unsafe { lyyime_set_commit_first_at_four(eng.0, 0) }, 0);
    assert_eq!(unsafe { lyyime_set_commit_unique_four(eng.0, 0) }, 0);
    assert_eq!(unsafe { lyyime_set_commit_after_four(eng.0, 1) }, 1);
    for c in ['a', 'a', 'a', 'a'] {
        eng.key_char(c);
    }
    let json = eng.key_char('g');
    assert!(
        json.contains("{\"t\":\"commit\",\"s\":\"恭恭敬敬\"}"),
        "{json}"
    );
    assert!(json.contains("\"s\":\"g\""), "{json}");
    assert_eq!(unsafe { lyyime_set_commit_after_four(eng.0, 0) }, 0);
}

#[test]
fn ffi_四码唯一上屏开关() {
    let eng = FfiEngine::new(&fixtures());
    let mut eng = eng;
    // 默认开启:wqvb 唯一候选「你好」在第 4 键直接上屏(免空格)。
    let mut json = String::new();
    for c in "wqvb".chars() {
        json = eng.key_char(c);
    }
    assert!(
        json.contains("{\"t\":\"commit\",\"s\":\"你好\"}"),
        "第 4 键应直接上屏:{json}"
    );
    assert!(json.contains("\"t\":\"preedit\""), "上屏应伴随预编辑清除");
    // 四码自动上屏全关:第 4 键保持组合,仍由空格顶屏。
    eng.key(LKEY_ESC, 0);
    assert_eq!(unsafe { lyyime_set_commit_first_at_four(eng.0, 0) }, 0);
    assert_eq!(unsafe { lyyime_set_commit_unique_four(eng.0, 0) }, 0);
    for c in "wqvb".chars() {
        eng.key_char(c);
    }
    let json = eng.key(LKEY_SPACE, 0);
    assert!(json.contains("{\"t\":\"commit\",\"s\":\"你好\"}"), "{json}");
}

#[test]
fn ffi_key_id映射_数字选词与标点翻页() {
    let mut eng = FfiEngine::new(&fixtures());
    eng.key_char('n');
    // Digit 约定(§3 v1.1):chr 传 '1'..'9' 的 ASCII 码点,core 按 chr-'0' 解码;
    // 下例 chr='2'(0x32)应选中当前页第 2 个候选。
    let json = eng.key(LKEY_DIGIT, '2' as u32);
    assert!(
        json.contains("{\"t\":\"commit\",\"s\":\"尼\"}"),
        "数字 2 应选中尼:{json}"
    );
    // 标点:空缓冲出中文标点(全角逗号 U+FF0C)。
    let json = eng.key(LKEY_PUNCT, ',' as u32);
    assert_eq!(json, "[{\"t\":\"commit\",\"s\":\"\u{FF0C}\"}]");
}

#[test]
fn ffi_key_id映射_其余控制键() {
    let mut eng = FfiEngine::new(&fixtures());
    eng.key_char('n');
    // Esc 清缓冲 → consumed + 清除效果。
    let json = eng.key(LKEY_ESC, 0);
    assert!(json.contains("{\"t\":\"consumed\"}"), "{json}");
    assert!(json.contains("{\"t\":\"preedit\"}"), "{json}");
    // 空缓冲:Enter/Backspace/PageUp/PageDown/Other 均 pass。
    for kid in [
        LKEY_ENTER,
        LKEY_BACKSPACE,
        LKEY_PAGEUP,
        LKEY_PAGEDOWN,
        LKEY_OTHER,
    ] {
        assert_eq!(eng.key(kid, 0), "[{\"t\":\"pass\"}]");
    }
    // ShiftPress 恒 consumed(单击判定在宿主)。
    assert_eq!(eng.key(LKEY_SHIFTPRESS, 0), "[{\"t\":\"consumed\"}]");
    // 未知 key_id 按 Other 处理。
    assert_eq!(eng.key(99, 0), "[{\"t\":\"pass\"}]");
    // 大写字母:core 接收并走"全大写输入"候选通道(2026-09-28 需求;
    // preedit 保留敲入原形,候选为大写/首字母大写/小写变体)。
    let j = eng.key(LKEY_CHAR, 'A' as u32);
    assert!(j.contains("\"t\":\"preedit\",\"s\":\"A\""), "{j}");
}

#[test]
fn ffi_重试纪律_小缓冲needed不落状态_扩容重试与一次成功一致() {
    let json_of = |eng: &mut FfiEngine, key_id: c_int, chr: u32| {
        let mut buf = vec![0u8; 8192];
        let n = unsafe {
            lyyime_process_key(
                eng.0,
                key_id,
                chr,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as i64,
            )
        };
        assert!(n > 0);
        String::from_utf8_lossy(&buf[..(n - 1) as usize]).into_owned()
    };

    // 两台同态引擎都打到缓冲 "ni"(有候选,空格将顶屏"你"并学习)。
    let mut e1 = FfiEngine::new(&fixtures());
    let mut e2 = FfiEngine::new(&fixtures());
    for c in "ni".chars() {
        json_of(&mut e1, LKEY_CHAR, c as u32);
        json_of(&mut e2, LKEY_CHAR, c as u32);
    }

    // e1 用小缓冲喂空格 → -needed,内部状态必须保持按键前。
    let mut tiny = [0u8; 4];
    let n = e1.key_raw(
        LKEY_SPACE,
        0,
        tiny.as_mut_ptr() as *mut c_char,
        tiny.len() as i64,
    );
    assert!(n < 0, "小缓冲应返回 -needed,得到 {n}");
    assert!(tiny.iter().all(|&b| b == 0), "-needed 时不得写入");
    // 非破坏性检查:候选页保持按键前状态(lyyime_cand 只读)。
    let (cand_text, _) = e1.cand(0);
    assert_eq!(cand_text, "你", "-needed 后内部状态不得变更");

    // e1 扩容重试同一键:效果应与 e2 一次成功完全一致。
    let retry = json_of(&mut e1, LKEY_SPACE, 0);
    let once = json_of(&mut e2, LKEY_SPACE, 0);
    assert_eq!(retry, once, "重试效果必须与一次成功一致");
    assert!(retry.contains("{\"t\":\"commit\",\"s\":\"你\"}"));

    // 后续按键序列也完全一致(证明学习/清缓冲恰好生效一次,没有二次生效)。
    for c in "hao".chars() {
        let a = json_of(&mut e1, LKEY_CHAR, c as u32);
        let b = json_of(&mut e2, LKEY_CHAR, c as u32);
        assert_eq!(a, b, "重试后的后续状态应与一次成功一致");
    }
}

// ======================================================================
// 造词(合同 §12):FFI 键值 11–15 与 notice 效果 JSON
// ======================================================================

#[test]
fn ffi_造词_热键方向键与notice效果流() {
    let mut eng = FfiEngine::new(&fixtures());
    // 上屏「你好」:wqvb + 空格。
    for c in "wqvb".chars() {
        eng.key_char(c);
    }
    eng.key(LKEY_SPACE, 0);
    // Ctrl+= → 进入造词:preedit + 单候选,候选/注释经 lyyime_cand 可取。
    let json = eng.key(11, 0); // LKEY_COIN
    assert!(json.contains("{\"t\":\"preedit\",\"s\":\"造词:你好\"}"), "{json}");
    assert!(json.contains("\"t\":\"cands\",\"n\":1"), "{json}");
    let (text, comment) = eng.cand(0);
    assert_eq!(text, "你好");
    assert_eq!(comment, "wqvb");
    // → 多选一字,← 少选一字(方向键 12/13);历史只有 2 字:扩到顶/缩到下限都吞键。
    assert_eq!(eng.key(13, 0), "[{\"t\":\"consumed\"}]", "选长已到历史上限");
    assert_eq!(eng.key(12, 0), "[{\"t\":\"consumed\"}]", "二字词下限保持");
    // Enter 存词 → notice 效果(含编码),preedit 清除。
    let json = eng.key(LKEY_ENTER, 0);
    assert!(
        json.contains("{\"t\":\"notice\",\"s\":\"已造词:你好(wqvb),可直接用该编码打出\"}"),
        "{json}"
    );
    assert!(json.contains("{\"t\":\"preedit\"}"), "{json}");
    // Esc 未进入造词时空缓冲直通;箭头键非造词模式直通。
    assert_eq!(eng.key(12, 0), "[{\"t\":\"pass\"}]");
    assert_eq!(eng.key(15, 0), "[{\"t\":\"pass\"}]");
}

// ---- 快速功能键(合同 §14)----

use lyyime_core::ffi::{
    lyyime_action_command, lyyime_add_quick_action, lyyime_clear_quick_actions,
    lyyime_select_candidate, lyyime_set_quick_actions_enabled,
};

/// 经 lyyime_select_candidate 取点选效果流 JSON(缓冲足够大)。
fn select_json(eng: *mut lyyime_core::Engine, idx: c_int) -> String {
    let mut buf = vec![0u8; 8192];
    let n = unsafe {
        lyyime_select_candidate(eng, idx, buf.as_mut_ptr() as *mut c_char, buf.len() as i64)
    };
    assert!(n > 0, "select_candidate 返回 {n}");
    String::from_utf8_lossy(&buf[..(n - 1) as usize]).into_owned()
}

#[test]
fn ffi_快速功能键_默认表候选与action效果流() {
    let mut eng = FfiEngine::new(&fixtures());
    // 默认表含 peizhi→打开配置:无词库命中,功能候选置顶。
    for c in "peizhi".chars() {
        eng.key_char(c);
    }
    assert_eq!(eng.cand(0), ("打开配置".to_string(), "功能键".to_string()));
    // 数字 1 选中功能键:action 效果流,不上屏文本。
    let json = eng.key(LKEY_DIGIT, '1' as u32);
    assert_eq!(
        json,
        "[{\"t\":\"action\",\"i\":0},{\"t\":\"preedit\"},{\"t\":\"cands\",\"n\":0,\"page\":0,\"pages\":0}]"
    );
    // command 经 lyyime_action_command 可取(@settings 内置)。
    let mut buf = vec![0u8; 512];
    let n = unsafe { lyyime_action_command(eng.0, 0, buf.as_mut_ptr() as *mut c_char, 512) };
    assert!(n > 0);
    assert_eq!(String::from_utf8_lossy(&buf[..n as usize - 1]), "@settings");
}

#[test]
fn ffi_快速功能键_点选select_candidate的json() {
    let mut eng = FfiEngine::new(&fixtures());
    for c in "peizhi".chars() {
        eng.key_char(c);
    }
    assert_eq!(
        select_json(eng.0, 0),
        "[{\"t\":\"action\",\"i\":0},{\"t\":\"preedit\"},{\"t\":\"cands\",\"n\":0,\"page\":0,\"pages\":0}]"
    );
    // 越界点选:consumed。
    for c in "peizhi".chars() {
        eng.key_char(c);
    }
    assert_eq!(select_json(eng.0, 9), "[{\"t\":\"consumed\"}]");
}

#[test]
fn ffi_快速功能键_注入列表开关与非法参数() {
    let mut eng = FfiEngine::new(&fixtures());
    // 清空默认表:peizhi 不再出现功能候选。
    unsafe { lyyime_clear_quick_actions(eng.0) };
    for c in "peizhi".chars() {
        eng.key_char(c);
    }
    assert!(eng.flush_page_texts().is_empty(), "清空后 peizhi 无候选");
    for _ in 0..6 {
        eng.key(LKEY_BACKSPACE, 0);
    }

    // 追加 nihao 触发词;非法触发词(大写/空格)与 NULL 参数拒绝。
    let t = CString::new("nihao").unwrap();
    let t_bad = CString::new("Ni Hao").unwrap();
    let l = CString::new("功能").unwrap();
    let c1 = CString::new("@help").unwrap();
    assert_eq!(unsafe { lyyime_add_quick_action(eng.0, t.as_ptr(), l.as_ptr(), c1.as_ptr()) }, 0);
    assert_eq!(unsafe { lyyime_add_quick_action(eng.0, t_bad.as_ptr(), l.as_ptr(), c1.as_ptr()) }, -1);
    assert_eq!(unsafe { lyyime_add_quick_action(eng.0, std::ptr::null(), l.as_ptr(), c1.as_ptr()) }, -1);

    // nihao 候选:首选「你好」+ 第 2 条功能键;i=1 的 command = @help。
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    assert_eq!(eng.cand(1), ("功能".to_string(), "功能键".to_string()));
    // 清空后注入的这条在配置列表里下标 0(页面位置是第 2 条)。
    let mut buf = vec![0u8; 256];
    let n = unsafe { lyyime_action_command(eng.0, 0, buf.as_mut_ptr() as *mut c_char, 256) };
    assert!(n > 0);
    assert_eq!(String::from_utf8_lossy(&buf[..n as usize - 1]), "@help");
    // 越界 command 为空串(返回 1,内容空)。
    let n = unsafe { lyyime_action_command(eng.0, 99, buf.as_mut_ptr() as *mut c_char, 256) };
    assert_eq!(n, 1);

    // 总开关关闭:返回 0,功能候选消失;重新打开返回 1。
    assert_eq!(unsafe { lyyime_set_quick_actions_enabled(eng.0, 0) }, 0);
    for _ in 0..5 {
        eng.key(LKEY_BACKSPACE, 0);
    }
    for c in "nihao".chars() {
        eng.key_char(c);
    }
    let page = eng.flush_page_texts();
    assert!(!page.iter().any(|t| t == "功能"), "关闭后无功能候选 {page:?}");
    assert_eq!(unsafe { lyyime_set_quick_actions_enabled(eng.0, 1) }, 1);
}

impl FfiEngine {
    /// 当前页候选文本(测试辅助)。
    fn flush_page_texts(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..9 {
            let mut b = vec![0u8; 512];
            let n = unsafe { lyyime_cand(self.0, i, b.as_mut_ptr() as *mut c_char, 512) };
            if n <= 1 {
                break;
            }
            out.push(String::from_utf8_lossy(&b[..n as usize - 1]).into_owned());
        }
        out
    }
}

#[test]
fn ffi_大写字母原样进入大写候选() {
    let mut eng = FfiEngine::new(&fixtures());
    for c in "WHO".chars() {
        eng.key_char(c);
    }
    assert_eq!(eng.cand(0), ("WHO".to_string(), "en".to_string()));
    assert_eq!(eng.cand(1), ("Who".to_string(), "en".to_string()));
    assert_eq!(eng.cand(2), ("who".to_string(), "en".to_string()));
    assert_eq!(eng.cand(3), ("世界卫生组织".to_string(), "WHO".to_string()));
    assert_eq!(eng.cand(4), ("谁".to_string(), "WHO".to_string()));
}
