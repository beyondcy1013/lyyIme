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
    lyyime_set_next_word_prediction, lyyime_set_phrase_hint, lyyime_toggle_mode,
    lyyime_set_chinese_punctuation, lyyime_toggle_chinese_punctuation,
    lyyime_menu_trigger_cancel, lyyime_menu_trigger_commit,
    lyyime_menu_trigger_configure, lyyime_menu_trigger_count,
    lyyime_menu_trigger_free, lyyime_menu_trigger_id, lyyime_menu_trigger_label,
    lyyime_menu_trigger_new, lyyime_menu_trigger_pending, lyyime_menu_trigger_reset,
    lyyime_menu_trigger_set_shot_hotkey,
    lyyime_menu_trigger_take,
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
    // 本测逐字节断言"上屏→清候选→词组提示"的旧序列;联想行会占候选条,
    // 关掉联想隔离被测行为(联想自身 JSON 见 prediction 用例)。
    assert_eq!(unsafe { lyyime_set_next_word_prediction(eng.0, 0) }, 0);
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
    // 专验词组提示:联想行与提示共用候选条位置(有联想则不发提示),
    // 关掉联想以隔离被测行为。
    assert_eq!(unsafe { lyyime_set_next_word_prediction(eng.0, 0) }, 0);
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
fn ffi_中文标点开关_设置与翻转_组合不丢() {
    let mut eng = FfiEngine::new(&fixtures());
    // 默认开:逗号上屏全角 ,(U+FF0C)。
    let json = eng.key(LKEY_PUNCT, ',' as u32);
    assert!(json.contains("\"s\":\"\u{FF0C}\""), "{json}");
    // 关闭:setter 回环返回生效值,标点直通(空缓冲)。
    assert_eq!(unsafe { lyyime_set_chinese_punctuation(eng.0, 0) }, 0);
    let json = eng.key(LKEY_PUNCT, ',' as u32);
    assert!(json.contains("\"t\":\"pass\""), "{json}");
    // 组合中切换不清缓冲/候选:打 n 再翻转,空格仍上屏候选。
    eng.key_char('n');
    assert_eq!(unsafe { lyyime_toggle_chinese_punctuation(eng.0) }, 1);
    let json = eng.key(LKEY_SPACE, 0);
    assert!(
        json.contains("\"t\":\"commit\""),
        "切换标点不得丢进行中的组合: {json}"
    );
    // 再次翻转回 0;toggle NULL 也安全返回 0。
    assert_eq!(unsafe { lyyime_toggle_chinese_punctuation(eng.0) }, 0);
    assert_eq!(
        unsafe { lyyime_set_chinese_punctuation(std::ptr::null_mut(), 1) },
        0
    );
    assert_eq!(
        unsafe { lyyime_toggle_chinese_punctuation(std::ptr::null_mut()) },
        0
    );
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

#[test]
fn ffi_重试纪律_联想触发与联想选择() {
    // 联想路径同样服从两段式纪律:触发 commit 与联想选择在小缓冲下都
    // 返回 -needed 且不落状态;扩容重试与一次成功逐字节一致。
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

    // 两台同态引擎都打到缓冲 "ni"(空格将上屏"你"并触发联想行)。
    // 联想默认关:显式开启后才验证触发/选择路径。
    let mut e1 = FfiEngine::new(&fixtures());
    let mut e2 = FfiEngine::new(&fixtures());
    assert_eq!(unsafe { lyyime_set_next_word_prediction(e1.0, 1) }, 1);
    assert_eq!(unsafe { lyyime_set_next_word_prediction(e2.0, 1) }, 1);
    for c in "ni".chars() {
        json_of(&mut e1, LKEY_CHAR, c as u32);
        json_of(&mut e2, LKEY_CHAR, c as u32);
    }

    // —— 触发联想的上屏 commit:e1 小缓冲 → -needed,组合候选原样保持 ——
    let mut tiny = [0u8; 4];
    let n = e1.key_raw(
        LKEY_SPACE,
        0,
        tiny.as_mut_ptr() as *mut c_char,
        tiny.len() as i64,
    );
    assert!(n < 0, "小缓冲应返回 -needed,得到 {n}");
    assert!(tiny.iter().all(|&b| b == 0), "-needed 时不得写入");
    assert_eq!(e1.cand(0).0, "你", "-needed 后候选页不得变更");
    // 扩容重试:效果与 e2 一次成功一致,联想行([好, 好吗])落位。
    let retry = json_of(&mut e1, LKEY_SPACE, 0);
    let once = json_of(&mut e2, LKEY_SPACE, 0);
    assert_eq!(retry, once, "联想触发重试效果须与一次成功一致");
    assert!(retry.contains("{\"t\":\"commit\",\"s\":\"你\"}"));
    assert_eq!(e1.cand(0), ("好".to_string(), String::new()));
    assert_eq!(e1.cand(1).0, "好吗");

    // —— 联想选择:e1 小缓冲喂空格 → -needed,联想行不得变更 ——
    let n = e1.key_raw(
        LKEY_SPACE,
        0,
        tiny.as_mut_ptr() as *mut c_char,
        tiny.len() as i64,
    );
    assert!(n < 0, "联想选择小缓冲应返回 -needed,得到 {n}");
    assert_eq!(e1.cand(0).0, "好", "-needed 后联想行不得变更");
    // 扩容重试:与 e2 直接空格逐字节一致,只上屏尾巴"好"。
    let retry = json_of(&mut e1, LKEY_SPACE, 0);
    let once = json_of(&mut e2, LKEY_SPACE, 0);
    assert_eq!(retry, once, "联想选择重试效果须与一次成功一致");
    assert!(retry.contains("{\"t\":\"commit\",\"s\":\"好\"}"));
    assert!(!retry.contains("\"s\":\"你好\""), "不得二次上屏前缀");
    // 续接:上下文"你好" → 二级联想只剩「吗」。
    assert_eq!(e1.cand(0), ("吗".to_string(), String::new()));
    // 两台引擎后续键序仍完全一致(联想状态恰好生效一次)。
    let a = json_of(&mut e1, LKEY_ESC, 0);
    let b = json_of(&mut e2, LKEY_ESC, 0);
    assert_eq!(a, b);
}

// ======================================================================
// 造词(合同 §12):FFI 键值 11–15 与 notice 效果 JSON
// ======================================================================

#[test]
fn ffi_造词_热键方向键与notice效果流() {
    let mut eng = FfiEngine::new(&fixtures());
    // 专验造词键序:上屏「你好」会出联想行,其后的空格会选中联想尾巴
    // 而非直通——关掉联想保持造词历史的按键语义不变。
    assert_eq!(unsafe { lyyime_set_next_word_prediction(eng.0, 0) }, 0);
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

// ---- 菜单触发(可选符号组;独立对象,与引擎无关)----

/// 新建+配置为默认(开/F7/空黑名单)的菜单触发器。
struct FfiMenuTrigger(*mut lyyime_core::menu_trigger::MenuTrigger);

impl FfiMenuTrigger {
    fn new() -> Self {
        let mt = unsafe { lyyime_menu_trigger_new() };
        assert!(!mt.is_null());
        let empty = CString::new("").unwrap();
        assert_eq!(unsafe { lyyime_menu_trigger_configure(mt, 1, 7, empty.as_ptr()) }, 0);
        Self(mt)
    }

    /// 喂上屏文本,返回提示串(空 = 未命中)。
    fn commit(&mut self, text: &str) -> String {
        let t = CString::new(text).unwrap();
        let mut buf = vec![0u8; 512];
        let n = unsafe {
            lyyime_menu_trigger_commit(
                self.0,
                t.as_ptr(),
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as i64,
            )
        };
        assert!(n > 0, "commit 返回 {n}");
        String::from_utf8_lossy(&buf[..n as usize - 1]).into_owned()
    }
}

impl Drop for FfiMenuTrigger {
    fn drop(&mut self) {
        unsafe { lyyime_menu_trigger_free(self.0) };
    }
}

#[test]
fn ffi_菜单触发_目录枚举与命中流程() {
    // 目录枚举:16 项,id/label 可读,越界取空串
    let n = lyyime_menu_trigger_count();
    assert_eq!(n, 16);
    let mut id = vec![0u8; 128];
    let mut label = vec![0u8; 128];
    let ni = unsafe { lyyime_menu_trigger_id(0, id.as_mut_ptr() as *mut c_char, 128) };
    let nl = unsafe { lyyime_menu_trigger_label(0, label.as_mut_ptr() as *mut c_char, 128) };
    assert!(ni > 1 && nl > 1);
    assert_eq!(String::from_utf8_lossy(&id[..ni as usize - 1]), "settings");
    assert_eq!(String::from_utf8_lossy(&label[..nl as usize - 1]), "设置");
    let nx = unsafe { lyyime_menu_trigger_id(99, id.as_mut_ptr() as *mut c_char, 128) };
    assert_eq!(nx, 1, "越界 id 写空串返回 1");
    // 负下标同样视为越界:写空串返回 1(不得回退到第 0 项)
    let neg = unsafe { lyyime_menu_trigger_id(-1, id.as_mut_ptr() as *mut c_char, 128) };
    assert_eq!(neg, 1, "负下标 id 写空串返回 1");
    assert_eq!(id[0], 0);
    let negl = unsafe { lyyime_menu_trigger_label(-1, label.as_mut_ptr() as *mut c_char, 128) };
    assert_eq!(negl, 1, "负下标 label 写空串返回 1");
    assert_eq!(label[0], 0);

    // 上屏「设置」→ 提示 + pending=0;take 一次性取出
    let mut mt = FfiMenuTrigger::new();
    let hint = mt.commit("设置");
    assert_eq!(hint, "匹配了菜单功能「设置」,按 F7 进入该功能");
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, 0);
    assert_eq!(unsafe { lyyime_menu_trigger_take(mt.0) }, 0);
    assert_eq!(unsafe { lyyime_menu_trigger_take(mt.0) }, -1, "take 只取一次");
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, -1);

    // NULL 安全
    assert_eq!(unsafe { lyyime_menu_trigger_take(std::ptr::null_mut()) }, -1);
    assert_eq!(unsafe { lyyime_menu_trigger_cancel(std::ptr::null_mut()) }, 0);
    assert_eq!(unsafe { lyyime_menu_trigger_reset(std::ptr::null_mut()) }, 0);
    unsafe { lyyime_menu_trigger_free(std::ptr::null_mut()) };
}

#[test]
fn ffi_菜单触发_commit短缓冲不落状态_重试不重复() {
    let mt = FfiMenuTrigger::new();
    let t = CString::new("设置").unwrap();

    // ① 极小缓冲:返回 -needed,尾串与待执行保持按键前(不落状态)
    let mut tiny = [0u8; 4];
    let n = unsafe {
        lyyime_menu_trigger_commit(mt.0, t.as_ptr(), tiny.as_mut_ptr() as *mut c_char, 4)
    };
    assert!(n < 0, "小缓冲应返回 -needed,得到 {n}");
    assert!(tiny.iter().all(|&b| b == 0), "-needed 时不得写入");
    assert_eq!(
        unsafe { lyyime_menu_trigger_pending(mt.0) },
        -1,
        "-needed 时待执行不得置起"
    );

    // ② 扩容重试:提示正常写入且 pending=0;若状态被 ① 落过,这里会重复
    //    追加尾串(「设置设置」同样命中),只能靠重试前后一致性区分——
    //    追加断言:重试后再 commit 一个非 CJK 边界,尾串应只剩一段。
    let need = (-n) as usize;
    let mut buf = vec![0u8; need];
    let n2 = unsafe {
        lyyime_menu_trigger_commit(
            mt.0,
            t.as_ptr(),
            buf.as_mut_ptr() as *mut c_char,
            need as i64,
        )
    };
    assert_eq!(n2, n.abs(), "重试返回所需字节数");
    assert_eq!(
        String::from_utf8_lossy(&buf[..need - 1]),
        "匹配了菜单功能「设置」,按 F7 进入该功能"
    );
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, 0);
}

#[test]
fn ffi_菜单触发_cancel与reset语义() {
    let mut mt = FfiMenuTrigger::new();
    // 分字上屏命中
    assert_eq!(mt.commit("设"), "");
    assert!(mt.commit("置").contains("「设置」"));
    // cancel:清待执行留尾串,返回 1;再 cancel 返回 0
    assert_eq!(unsafe { lyyime_menu_trigger_cancel(mt.0) }, 1);
    assert_eq!(unsafe { lyyime_menu_trigger_cancel(mt.0) }, 0);
    // 尾串保留:续接「帮助」→「设置帮助」命中帮助(index 1)
    assert!(mt.commit("帮助").contains("「帮助」"));
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, 1);
    // reset:尾串+待执行清空;标点 commit 截断尾串
    assert_eq!(unsafe { lyyime_menu_trigger_reset(mt.0) }, 1);
    let t = CString::new("，").unwrap();
    let mut buf = [0u8; 64];
    let n = unsafe {
        lyyime_menu_trigger_commit(mt.0, t.as_ptr(), buf.as_mut_ptr() as *mut c_char, 64)
    };
    assert_eq!(n, 1, "标点 commit 无提示(空串)");
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, -1);
}

#[test]
fn ffi_菜单触发_黑名单与确认键配置() {
    let mut mt = FfiMenuTrigger::new();
    let dis = CString::new("settings,help").unwrap();
    // F8 + 黑名单;配置即复位(此前无态,返回无妨)
    assert_eq!(unsafe { lyyime_menu_trigger_configure(mt.0, 1, 8, dis.as_ptr()) }, 0);
    assert_eq!(mt.commit("设置"), "", "黑名单内不提示");
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, -1);
    // 换确认键:帮助放行,F8 提示
    let only_s = CString::new("settings").unwrap();
    assert_eq!(unsafe { lyyime_menu_trigger_configure(mt.0, 1, 8, only_s.as_ptr()) }, 0);
    assert_eq!(mt.commit("帮助"), "匹配了菜单功能「帮助」,按 F8 进入该功能");
    // 全局关闭
    assert_eq!(unsafe { lyyime_menu_trigger_configure(mt.0, 0, 7, std::ptr::null()) }, 0);
    assert_eq!(mt.commit("英文"), "", "全局关闭无提示");
    // 非 UTF-8 disabled 拒绝配置
    let bad = [0xffu8, 0xfe, 0x00];
    assert_eq!(
        unsafe {
            lyyime_menu_trigger_configure(mt.0, 1, 7, bad.as_ptr() as *const c_char)
        },
        -1
    );
}

#[test]
fn ffi_菜单触发_整段精确别名() {
    let mut mt = FfiMenuTrigger::new();
    // 整段上屏文本精确等于混合别名 → settings_ai(目录下标 13)
    let t = CString::new("AI 助手").unwrap();
    let mut buf = vec![0u8; 256];
    let n = unsafe {
        lyyime_menu_trigger_commit(mt.0, t.as_ptr(), buf.as_mut_ptr() as *mut c_char, 256)
    };
    assert!(n > 1, "「AI 助手」应出提示,得到 {n}");
    assert_eq!(
        String::from_utf8_lossy(&buf[..n as usize - 1]),
        "匹配了菜单功能「AI 助手」,按 F7 进入该功能"
    );
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, 13);
    // 「切换中/英文」整段精确 → english(2)
    let t2 = CString::new("切换中/英文").unwrap();
    let n2 = unsafe {
        lyyime_menu_trigger_commit(mt.0, t2.as_ptr(), buf.as_mut_ptr() as *mut c_char, 256)
    };
    assert!(n2 > 1);
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, 2);
    // 中文别名「人工智能助手」同样到达 AI 页(CJK 后缀与整段精确殊途同归)
    let t3 = CString::new("人工智能助手").unwrap();
    let n3 = unsafe {
        lyyime_menu_trigger_commit(mt.0, t3.as_ptr(), buf.as_mut_ptr() as *mut c_char, 256)
    };
    assert!(n3 > 1);
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, 13);
    // AI 项入黑名单:整段精确命中被禁项即整体不触发(不回退更短后缀)
    let dis = CString::new("settings_ai").unwrap();
    assert_eq!(
        unsafe { lyyime_menu_trigger_configure(mt.0, 1, 7, dis.as_ptr()) },
        0
    );
    let t4 = CString::new("AI 助手").unwrap();
    let n4 = unsafe {
        lyyime_menu_trigger_commit(mt.0, t4.as_ptr(), buf.as_mut_ptr() as *mut c_char, 256)
    };
    assert_eq!(n4, 1, "黑名单内 AI 项不提示(空串)");
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, -1);
    let t5 = CString::new("人工智能助手").unwrap();
    let n5 = unsafe {
        lyyime_menu_trigger_commit(mt.0, t5.as_ptr(), buf.as_mut_ptr() as *mut c_char, 256)
    };
    assert_eq!(n5, 1, "中文别名同样被黑名单挡下");
    assert_eq!(unsafe { lyyime_menu_trigger_pending(mt.0) }, -1);
}

/// 截屏快捷键提示:短缓冲两段式重试不丢串;NULL/非法 UTF-8 语义。
#[test]
fn ffi_菜单触发_截屏快捷键短缓冲() {
    // NULL mt → -1
    assert_eq!(unsafe {
        lyyime_menu_trigger_set_shot_hotkey(std::ptr::null_mut(), std::ptr::null())
    }, -1);

    let mut mt = FfiMenuTrigger::new();
    let spec = CString::new("ctrl+shift+F9").unwrap();
    assert_eq!(unsafe {
        lyyime_menu_trigger_set_shot_hotkey(mt.0, spec.as_ptr())
    }, 0);

    // 4 字节小缓冲 → 返回负数所需字节数,内部状态不被消耗
    let text = CString::new("截屏").unwrap();
    let mut tiny = [0u8; 4];
    let need = unsafe {
        lyyime_menu_trigger_commit(mt.0, text.as_ptr(),
            tiny.as_mut_ptr() as *mut c_char, tiny.len() as i64)
    };
    assert!(need < 0, "短缓冲须返回 -needed,实得 {need}");
    let pending = unsafe { lyyime_menu_trigger_pending(mt.0) };
    assert_eq!(pending, -1, "两段式:短缓冲不置待执行(内部态未被消耗)");

    // 精确容量重试 → 提示含 F7 + Ctrl+Shift+F9
    let mut buf = vec![0u8; (-need) as usize];
    let n = unsafe {
        lyyime_menu_trigger_commit(mt.0, text.as_ptr(),
            buf.as_mut_ptr() as *mut c_char, buf.len() as i64)
    };
    assert!(n > 0);
    let hint = String::from_utf8_lossy(&buf[..n as usize - 1]).into_owned();
    assert!(hint.contains('F'), "{hint}");
    assert!(hint.contains("F7"), "{hint}");
    assert!(hint.contains("Ctrl+Shift+F9"), "{hint}");

    // NULL spec → 清除后缀,截屏项只剩 F 确认键(不带 也可按)
    assert_eq!(unsafe {
        lyyime_menu_trigger_set_shot_hotkey(mt.0, std::ptr::null())
    }, 0);
    assert_eq!(mt.commit("截屏"), "匹配了菜单功能「截屏」,按 F7 进入该功能");

    // 非法 UTF-8 → -1 且前值保留
    let bad = [0xffu8, 0xfe, 0];
    assert_eq!(unsafe {
        lyyime_menu_trigger_set_shot_hotkey(mt.0, bad.as_ptr() as *const c_char)
    }, -1);
    let spec2 = CString::new("ctrl+alt+a").unwrap();
    assert_eq!(unsafe {
        lyyime_menu_trigger_set_shot_hotkey(mt.0, spec2.as_ptr())
    }, 0);
    let bad2 = [0xf0u8, 0x9f, 0];
    assert_eq!(unsafe {
        lyyime_menu_trigger_set_shot_hotkey(mt.0, bad2.as_ptr() as *const c_char)
    }, -1);
    let hint3 = mt.commit("截图");
    assert!(hint3.contains("Ctrl+Alt+A"), "{hint3}");   // 非法输入未清掉旧值
}

// ======================================================================
// 前缀候选选词的两段式重试纪律(缺词夹具:consumed>0 只消费前缀)
// ======================================================================

/// 缺词夹具:jie→截/接、ping→屏、pin→品;词库无「截屏」词组,
/// 「截」是 consumed=3 的前缀候选。
fn jieping_dir() -> TempDir {
    let td = TempDir::new();
    std::fs::write(
        td.join("pinyin_char.tsv"),
        "jie\t截\t6000\njie\t接\t3000\nping\t屏\t5000\npin\t品\t2000\n",
    )
    .unwrap();
    td
}

#[test]
fn ffi_重试纪律_前缀候选选词_失败不落状态() {
    // 前缀消费路径同样服从两段式:JSON 写不进宿主缓冲时 -needed,
    // 缓冲/候选保持按键前;扩容重试与一次成功逐字节一致,
    // 学习(截 入 user 表)也只生效一次。
    let td = jieping_dir();
    let mut e1 = FfiEngine::new(&td.path);
    let mut e2 = FfiEngine::new(&td.path);
    for c in "jieping".chars() {
        e1.key_char(c);
        e2.key_char(c);
    }
    assert_eq!(e1.cand(0).0, "截");
    assert_eq!(e1.cand(1).0, "接");

    // 容量 0 探测 → -needed;过小容量重试同样不落状态、不写缓冲。
    let n = e1.key_raw(LKEY_SPACE, 0, std::ptr::null_mut(), 0);
    assert!(n < 0, "容量探测应返回 -needed,得到 {n}");
    let need = (-n) as usize;
    let mut small = vec![0u8; need - 1];
    let n2 = e1.key_raw(LKEY_SPACE, 0, small.as_mut_ptr() as *mut c_char, (need - 1) as i64);
    assert_eq!(n2, n);
    assert!(small.iter().all(|&b| b == 0));
    // 两次失败后候选页与缓冲保持按键前:前缀候选还在,组合未被截断。
    assert_eq!(e1.cand(0).0, "截", "-needed 后候选不得变更");
    assert_eq!(e1.cand(1).0, "接");

    // 足量重试:Commit(截)+Preedit(ping)+后缀候选 屏。
    let mut big = vec![0u8; need];
    let n3 = e1.key_raw(LKEY_SPACE, 0, big.as_mut_ptr() as *mut c_char, need as i64);
    assert_eq!(n3, need as i64);
    let json = String::from_utf8_lossy(&big[..need - 1]).into_owned();
    assert_eq!(
        json,
        "[{\"t\":\"commit\",\"s\":\"截\"},{\"t\":\"preedit\",\"s\":\"ping\"},{\"t\":\"cands\",\"n\":1,\"page\":0,\"pages\":1}]",
        "前缀选词 JSON:{json}"
    );
    // 与 e2 一次成功逐字节一致。
    assert_eq!(json, e2.key(LKEY_SPACE, 0), "重试效果须与一次成功一致");
    assert_eq!(e1.cand(0).0, "屏", "余下 ping 的候选应为 屏");
    // 继续空格:只上屏 屏(截不得二次上屏)。
    for eng in [&mut e1, &mut e2] {
        let json = eng.key(LKEY_SPACE, 0);
        assert!(json.contains("{\"t\":\"commit\",\"s\":\"屏\"}"), "{json}");
        assert!(!json.contains("\"s\":\"截"), "{json}");
    }
}

#[test]
fn ffi_重试纪律_点选前缀候选_失败不落状态() {
    // select_candidate 路径同样:容量不足不消费前缀、不清候选。
    let td = jieping_dir();
    let mut eng = FfiEngine::new(&td.path);
    for c in "jieping".chars() {
        eng.key_char(c);
    }
    let n = unsafe { lyyime_select_candidate(eng.0, 0, std::ptr::null_mut(), 0) };
    assert!(n < 0, "点选容量探测应返回 -needed,得到 {n}");
    let need = (-n) as usize;
    let mut small = vec![0u8; need - 1];
    let n2 = unsafe {
        lyyime_select_candidate(eng.0, 0, small.as_mut_ptr() as *mut c_char, (need - 1) as i64)
    };
    assert_eq!(n2, n);
    assert_eq!(eng.cand(0).0, "截", "-needed 后候选不得变更");
    assert_eq!(eng.cand(1).0, "接");

    let mut big = vec![0u8; need];
    let n3 = unsafe {
        lyyime_select_candidate(eng.0, 0, big.as_mut_ptr() as *mut c_char, need as i64)
    };
    assert_eq!(n3, need as i64);
    let json = String::from_utf8_lossy(&big[..need - 1]).into_owned();
    assert!(json.contains("{\"t\":\"commit\",\"s\":\"截\"}"), "{json}");
    assert!(json.contains("{\"t\":\"preedit\",\"s\":\"ping\"}"), "{json}");
    assert_eq!(eng.cand(0).0, "屏");
    let json = eng.key(LKEY_SPACE, 0);
    assert!(json.contains("{\"t\":\"commit\",\"s\":\"屏\"}"), "{json}");
    assert!(!json.contains("\"s\":\"截"), "{json}");
}
