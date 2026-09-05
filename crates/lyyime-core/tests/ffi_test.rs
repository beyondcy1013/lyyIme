//! FFI 集成测试(合同 §3):直接以 Rust 函数形式调用 `extern "C"` 导出,
//! 验证 JSON 效果流一字不差、`-needed` 容量约定、候选/注释读取与模式切换。
//! (真实 .so 的 ctypes 冒烟另见 ffi-test.py。)

mod common;

use common::{fixtures, TempDir};
use lyyime_core::ffi::{
    lyyime_cand, lyyime_cand_comment, lyyime_free, lyyime_mode, lyyime_new, lyyime_process_key,
    lyyime_reset, lyyime_toggle_mode, LKEY_BACKSPACE, LKEY_CHAR, LKEY_DIGIT, LKEY_ENTER, LKEY_ESC,
    LKEY_OTHER, LKEY_PAGEDOWN, LKEY_PAGEUP, LKEY_PUNCT, LKEY_SHIFTPRESS, LKEY_SPACE,
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
        "[{\"t\":\"commit\",\"s\":\"你好\"},{\"t\":\"preedit\"},{\"t\":\"cands\",\"n\":0,\"page\":0,\"pages\":0}]"
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
    assert_eq!(comment, "ni hao");
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
    // 标点:空缓冲出中文标点。
    let json = eng.key(LKEY_PUNCT, ',' as u32);
    assert_eq!(json, "[{\"t\":\"commit\",\"s\":\",\"}]");
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
    // 大写字母宿主必须先小写化;core 防御性直通。
    assert_eq!(eng.key(LKEY_CHAR, 'A' as u32), "[{\"t\":\"pass\"}]");
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
