//! 文本发送: 把上屏文本送达目标窗口。
//!   直输(type): xdotool type 逐字注入 — 快, 绝大多数 GTK/Qt/Electron/终端可用
//!   粘贴(paste): GTK 剪贴板 + xdotool 模拟 Ctrl+V — 兼容拒收合成键的应用
//! 与已验证的 floatapp(Python 版)链路一致: 先激活目标窗口再注入。
//! 超时随文本长度伸缩(约 20ms/字 + 5s), 长文本不误报。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const AUTO_PASTE_LEN: usize = 40; // 直输逐字注入超过该长度太慢, 自动改粘贴

pub fn activate_window(wid: u32) -> bool {
    Command::new("xdotool")
        .args(["windowactivate", "--sync", &wid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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
    if !activate_window(wid) {
        return Err("无法激活目标窗口".into());
    }
    std::thread::sleep(Duration::from_millis(60));
    let res = if method == "paste" {
        let Some(cb) = clipboard else {
            return Err("粘贴模式必须在主线程调用".into());
        };
        cb.set_text(text);
        cb.store();
        std::thread::sleep(Duration::from_millis(80));
        run_with_timeout(
            Command::new("xdotool")
                .args(["key", "--clearmodifiers", "ctrl+v"])
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            Duration::from_secs(5),
        )
    } else {
        run_with_timeout(
            Command::new("xdotool")
                .args(["type", "--delay", "8", text])
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            Duration::from_secs_f64(0.02 * text.chars().count() as f64 + 5.0).max(
                Duration::from_secs(15),
            ),
        )
    };
    match res {
        Ok(true) => Ok(()),
        Ok(false) => Err("发送失败: xdotool 非零退出".into()),
        Err(e) => Err(format!("发送失败: {e}")),
    }
}
