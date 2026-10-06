//! 系统 CapsLock 解除:专用短命 X 连接 XkbLockModifiers 仅清 LockMask。
//! 不发合成按键(会翻转锁存并把假按键泄给应用);失败仅记日志。

use std::os::raw::{c_char, c_int, c_uint, c_void};

/// XkbUseCoreKbd(X11/XKB.h):核心键盘虚拟设备
const XKB_USE_CORE_KBD: c_uint = 0x100;
/// X11 LockMask(X.h),与 keysym::MASK_LOCK 同值
const LOCK_MASK: c_uint = 1 << 1;

#[link(name = "X11")]
extern "C" {
    fn XInitThreads() -> c_int;
    fn XOpenDisplay(display_name: *const c_char) -> *mut c_void;
    fn XkbLockModifiers(
        dpy: *mut c_void,
        device_spec: c_uint,
        affect: c_uint,
        values: c_uint,
    ) -> c_int;
    fn XSync(dpy: *mut c_void, discard: c_int) -> c_int;
    fn XCloseDisplay(dpy: *mut c_void) -> c_int;
}

/// 须在首个 Xlib 调用前执行一次;失败仅告警,不共享连接。
pub fn init_threads() {
    if unsafe { XInitThreads() } == 0 {
        crate::logger::warn("XInitThreads 失败:X11 非线程安全模式");
    }
}

/// 解除系统 CapsLock;成功 true,失败已记日志返回 false。
pub fn caps_lock_off() -> bool {
    unsafe {
        let dpy = XOpenDisplay(std::ptr::null());
        if dpy.is_null() {
            crate::logger::error("CapsLock 解除失败:无法打开 DISPLAY");
            return false;
        }
        let ok = XkbLockModifiers(dpy, XKB_USE_CORE_KBD, LOCK_MASK, 0) != 0;
        XSync(dpy, 0);
        XCloseDisplay(dpy);
        if !ok {
            crate::logger::error("CapsLock 解除失败:XkbLockModifiers 返回 False");
            return false;
        }
        crate::logger::info("CapsLock 已解除(Shift 单击确认中文态)");
        true
    }
}
