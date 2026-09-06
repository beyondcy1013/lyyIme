//! 目标窗口追踪: 轮询根窗口的 _NET_ACTIVE_WINDOW(EWMH),
//! 识别"最近激活的非自身窗口", 读取 _NET_WM_NAME(UTF8)/WM_NAME 作显示。
//! 用 x11rb 的 RustConnection(纯 Rust X11 连接, 不引入额外 C 依赖)。

use anyhow::Result;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
use x11rb::rust_connection::RustConnection;

pub struct XTrack {
    conn: RustConnection,
    root: u32,
    atom_active: u32,
    atom_net_wm_name: u32,
    atom_utf8: u32,
}

impl XTrack {
    pub fn connect() -> Result<XTrack> {
        let (conn, screen) = RustConnection::connect(None)
            .map_err(|e| anyhow::anyhow!("无法连接 X 显示器: {e}"))?;
        let root = conn.setup().roots[screen].root;
        let atom_active =
            conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?.reply()?.atom;
        let atom_net_wm_name = conn.intern_atom(false, b"_NET_WM_NAME")?.reply()?.atom;
        let atom_utf8 = conn.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
        Ok(XTrack { conn, root, atom_active, atom_net_wm_name, atom_utf8 })
    }

    /// 当前活动窗口; 无(0 或属性缺失)时 None。
    pub fn active_window(&self) -> Option<u32> {
        let r = self
            .conn
            .get_property(false, self.root, self.atom_active, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        r.value32().and_then(|mut v| v.next()).filter(|w| *w != 0)
    }

    /// 窗口标题: 先 _NET_WM_NAME(UTF8), 退回 WM_NAME(按 UTF-8 宽容解码)。
    pub fn window_title(&self, wid: u32) -> String {
        for (atom, ty) in [
            (self.atom_net_wm_name, self.atom_utf8),
            (AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()),
        ] {
            let reply = self
                .conn
                .get_property(false, wid, atom, ty, 0, u32::MAX / 4)
                .ok()
                .and_then(|c| c.reply().ok());
            if let Some(r) = reply {
                if !r.value.is_empty() {
                    return String::from_utf8_lossy(&r.value).into_owned();
                }
            }
        }
        "?".into()
    }
}

/// 连接失败的完整人话提示(含 DISPLAY 值)。
pub fn connect_error_report(e: anyhow::Error) -> String {
    let disp = std::env::var("DISPLAY").unwrap_or_else(|_| "(未设置)".into());
    format!(
        "错误:{e:#}\nlyyime-float 是图形程序, 请在图形会话中启动, 或先 export DISPLAY=:0\n\
         若在终端测试, 请带上会话的 XAUTHORITY(参照 ~/.Xauthority)。当前 DISPLAY={disp}"
    )
}
