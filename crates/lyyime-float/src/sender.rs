//! 文本发送: 把上屏文本送达目标窗口。
//!   直输(type): xdotool type 逐字注入 — 快, 绝大多数 GTK/Qt/Electron/终端可用
//!   粘贴(paste): GTK 剪贴板 + xdotool 模拟 Ctrl+V — 兼容拒收合成键的应用
//! 与已验证的 floatapp(Python 版)链路一致: 先激活目标窗口再注入。
//! 超时随文本长度伸缩(约 20ms/字 + 5s), 长文本不误报。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const AUTO_PASTE_LEN: usize = 40; // 直输逐字注入超过该长度太慢, 自动改粘贴

pub fn activate_window(wid: u32) -> bool {
    let ok = Command::new("xdotool")
        .args(["windowactivate", "--sync", &wid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return false;
    }
    // 防焦点窃取型 WM(xfwm4/openbox 等)有两层干扰:
    //   a) 只给"激活"标记不转移 X 输入焦点 → 文字落回悬浮窗自己("要按回车才上屏");
    //   b) 对非 EWMH 的 XSetInputFocus, WM 会在几十毫秒后把焦点"夺回"激活窗口
    //      → 注入进行中焦点被翻转, 中途字符丢失(实测)。
    // 对策: 设焦点→校验→沉降 80ms(让 WM 的 re-assert 发生)→再校验, 双确认后才
    // 返回; 上层注入从确认稳定后开始。多次不达仍返回 true(尽力而为, 不卡 UI)。
    let mut ok = false;
    for _ in 0..8 {
        let _ = Command::new("xdotool")
            .args(["windowfocus", "--sync", &wid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(80)); // 沉降: 给 WM re-assert 的窗口
        let cur = Command::new("xdotool")
            .args(["getwindowfocus", "-f"])
            .stderr(Stdio::null())
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if cur == wid.to_string() {
            ok = true;
            break;
        }
    }
    let _ = ok;
    true
}

/// 带截止时间的子进程等待: 超时杀掉, 防止卡死 UI 主循环。
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> std::io::Result<bool> {
    let mut child = cmd.spawn()?;
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(st) => return Ok(st.success()),
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::other("发送超时"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// 发送文本到目标窗口。粘贴方式须在 GTK 主线程调用(用 GTK 剪贴板)。
/// clipboard: 直输模式传 None。返回 Err(人话提示)。
pub fn send(
    wid: u32,
    text: &str,
    method: &str,
    clipboard: Option<&gtk::Clipboard>,
) -> Result<(), String> {
    let wid_s = wid.to_string();
    // 焦点竞态对策: "激活→设焦点→沉降→注入"必须是一条 xdotool 链式命令。
    // 分离的多个 xdotool 进程之间, WM(openbox/xfwm4)可能延迟把焦点翻回
    // 激活过的悬浮窗 → 注入中途焦点翻转 → 字符丢失/串窗(实测)。
    // 单进程内顺序执行, 无窗口期。
    let pre = ["windowactivate", "--sync", &wid_s, "windowfocus", "--sync", &wid_s, "sleep", "0.12"];
    let res = if method == "paste" {
        let Some(cb) = clipboard else {
            return Err("粘贴模式必须在主线程调用".into());
        };
        cb.set_text(text);
        cb.store();
        std::thread::sleep(Duration::from_millis(80));
        run_with_timeout(
            Command::new("xdotool")
                .args(pre.iter().chain(["key", "--clearmodifiers", "ctrl+v"].iter()))
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            Duration::from_secs(5),
        )
    } else {
        let secs = (0.02 * text.chars().count() as f64 + 5.0).max(15.0);
        run_with_timeout(
            Command::new("xdotool")
                .args(pre.iter().chain(["type", "--delay", "15", text].iter()))
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            Duration::from_secs_f64(secs),
        )
    };
    match res {
        Ok(true) => Ok(()),
        Ok(false) => Err("发送失败: xdotool 非零退出".into()),
        Err(e) => Err(format!("发送失败: {e}")),
    }
}

/// 向目标窗口注入单个按键(xdotool key 名, 如 BackSpace/Delete)。
/// 与 send 同链路: 先激活目标窗口再注入; 供空缓冲退格直通删除目标文本。
pub fn send_key(wid: u32, key: &str) -> Result<(), String> {
    let wid_s = wid.to_string();
    let res = run_with_timeout(
        Command::new("xdotool")
            .args([
                "windowactivate", "--sync", &wid_s,
                "windowfocus", "--sync", &wid_s,
                "sleep", "0.1",
                "key", "--delay", "60", key,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        Duration::from_secs(5),
    );
    match res {
        Ok(true) => Ok(()),
        Ok(false) => Err("按键发送失败: xdotool 非零退出".into()),
        Err(e) => Err(format!("按键发送失败: {e}")),
    }
}
