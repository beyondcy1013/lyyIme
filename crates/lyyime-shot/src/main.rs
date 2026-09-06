//! lyyime-shot —— lyyIme 截屏助手(Mode A/B 共用;ARCHITECTURE.md §13)。
//!
//! 交互习惯对齐搜狗/QQ 截屏:全屏冻结画面上拖拽框选,Esc 取消、Enter 确认、
//! 双击=整屏;确认后存图片目录 + 复制剪贴板 + 桌面通知(notify-send)。
//! 由输入法热键(默认 Ctrl+Alt+A,`shot_hotkey` 可配置)或托盘菜单拉起,
//! 本进程不监听按键 —— 热键拦截在输入法各模式宿主内完成。
//!
//! 用法:
//!   lyyime-shot                     框选截屏(默认)
//!   lyyime-shot --full              整屏截屏
//!   lyyime-shot --auto=X,Y,W,H      自动截取指定矩形(无窗口,E2E/脚本用)
//!   lyyime-shot --dir=DIR           指定保存目录(默认 $LYYIME_SHOT_DIR → ~/图片 → ~/Pictures → ~)
//!   lyyime-shot --no-clipboard      不复制剪贴板
//!   lyyime-shot --no-notify         不发桌面通知
//!
//! 退出码:0 成功;1 用户取消(Esc/点选无拖拽);2 出错。成功时 stdout
//! 打印 `已保存:<路径>`(E2E 断言用)。
//!
//! 截屏实现:`gdk_pixbuf_get_from_window(root)`(X11 XGetImage 语义),
//! 框选覆盖窗展示**先于覆盖窗截好的**整屏快照 —— 经典冻结画面方案,
//! 借鉴 GNOME Screenshot / flameshot 的"先截屏后框选"流程。

mod save;
mod selector;

use anyhow::{anyhow, Result};

/// 解析 `--auto=X,Y,W,H`;非法给"人话"错误。
fn parse_auto_arg(arg: &str) -> Result<(i32, i32, i32, i32)> {
    let v = arg
        .trim_start_matches("--auto=")
        .split(',')
        .map(|s| s.trim().parse::<i32>())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| anyhow!("--auto 参数应为 X,Y,W,H 四个整数(如 --auto=0,0,800,600),收到:{arg}"))?;
    if v.len() != 4 {
        return Err(anyhow!(
            "--auto 参数应为 X,Y,W,H 四个整数,收到 {arg}({} 个值)",
            v.len()
        ));
    }
    let (x, y, w, h) = (v[0], v[1], v[2], v[3]);
    if w <= 0 || h <= 0 {
        return Err(anyhow!("--auto 宽高必须为正整数,收到:{arg}"));
    }
    Ok((x, y, w, h))
}

struct Options {
    mode: Mode,
    dir: Option<String>,
    clipboard: bool,
    notify: bool,
}

enum Mode {
    Region,
    Full,
    Auto(i32, i32, i32, i32),
}

fn print_usage() {
    println!(
        "用法: lyyime-shot [选项]\n\
         \n\
         选项:\n\
         \x20 (无)              框选截屏(拖拽选区;Esc 取消,Enter 确认,双击=整屏)\n\
         \x20 --full            整屏截屏\n\
         \x20 --auto=X,Y,W,H    自动截取指定矩形,不弹窗口(测试/脚本)\n\
         \x20 --dir=DIR         保存目录(默认 $LYYIME_SHOT_DIR → ~/图片 → ~/Pictures → ~)\n\
         \x20 --no-clipboard    不复制到剪贴板\n\
         \x20 --no-notify       不发桌面通知\n\
         \x20 -h / --help       本说明\n\
         \x20 --version         版本\n\
         \n\
         截屏后图片存入图片目录并复制剪贴板;文件名 lyyIme_日期_时间.png。"
    );
}

fn parse_args() -> Result<Options> {
    let mut o = Options {
        mode: Mode::Region,
        dir: None,
        clipboard: true,
        notify: true,
    };
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--full" => o.mode = Mode::Full,
            "--no-clipboard" => o.clipboard = false,
            "--no-notify" => o.notify = false,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--version" => {
                println!("lyyime-shot {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ if a.starts_with("--auto=") => {
                let (x, y, w, h) = parse_auto_arg(&a)?;
                o.mode = Mode::Auto(x, y, w, h);
            }
            _ if a.starts_with("--dir=") => o.dir = Some(a["--dir=".len()..].to_string()),
            other => {
                return Err(anyhow!(
                    "未知参数:{other}(--help 查看用法;注意 --auto 写法为 --auto=X,Y,W,H)"
                ));
            }
        }
    }
    Ok(o)
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("lyyime-shot: {e:#}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(opts) {
        eprintln!("lyyime-shot: {e:#}");
        std::process::exit(2);
    }
}

fn run(opts: Options) -> Result<()> {
    gtk::init().map_err(|e| anyhow!("无法初始化 GTK(需要图形会话/X11 显示):{e}"))?;

    // 框选:截整屏 → 覆盖窗冻结画面上拖拽选区 → 裁剪;None = 用户取消。
    let (pixbuf, rect) = match opts.mode {
        Mode::Auto(x, y, w, h) => {
            let (pb, sw, sh) = capture_root()?;
            let sc = pb_scale();
            let crop = clamp_crop(x * sc, y * sc, w * sc, h * sc, sw, sh);
            (pb, Some(crop))
        }
        Mode::Full => {
            let (pb, sw, sh) = capture_root()?;
            (pb, Some((0, 0, sw, sh)))
        }
        Mode::Region => match selector::run_region()? {
            Some((pb, crop)) => (pb, Some(crop)),
            None => {
                println!("已取消");
                return Ok(()); // 主动取消是正常操作,退出码 0
            }
        },
    };
    let Some((cx, cy, cw, ch)) = rect else {
        println!("已取消");
        return Ok(()); // 退出码 0:主动取消是正常操作,不算错误
    };
    let shot = pixbuf.new_subpixbuf(cx, cy, cw, ch);

    let path = save::save_png(&shot, opts.dir.as_deref())?;
    let mut steps = vec![format!("已保存:{}", path.display())];

    if opts.clipboard {
        save::copy_to_clipboard(&shot);
        steps.push("已复制剪贴板".into());
    }
    if opts.notify {
        save::notify(&path, &format!("{cw}×{ch}"));
        steps.push("已通知".into());
    }
    println!("{}", steps.join(","));

    // 剪贴板内容挂在本进程上,存量片刻再退出,保证粘贴可用
    gtk::main_iteration_do(false);
    Ok(())
}

/// 设备缩放(X11 通常 1;HiDPI 下逻辑坐标 × scale = 像素坐标)。
fn pb_scale() -> i32 {
    use gdk::Window as GdkWindow;
    use gdk::prelude::WindowExtManual;
    <GdkWindow as WindowExtManual>::default_root_window()
        .scale_factor()
        .max(1)
}

/// 截取整个根窗口,返回 (pixbuf, 宽, 高)(设备像素)。
fn capture_root() -> Result<(gtk::gdk_pixbuf::Pixbuf, i32, i32)> {
    use gdk::Window as GdkWindow;
    use gdk::prelude::WindowExtManual;
    let root = <GdkWindow as WindowExtManual>::default_root_window();
    let (_, _, w, h) = root.geometry();
    if w <= 0 || h <= 0 {
        return Err(anyhow!("无法获取屏幕尺寸({w}×{h}),请检查 X 显示"));
    }
    let pb = root
        .pixbuf(0, 0, w, h)
        .ok_or_else(|| anyhow!("屏幕内容截取失败(X11 下需屏幕可见;合成器/嵌套显示可能限制截图)"))?;
    Ok((pb, w, h))
}

/// 裁剪矩形夹取到屏幕范围(避免越界;至少保留 1×1)。
fn clamp_crop(x: i32, y: i32, w: i32, h: i32, sw: i32, sh: i32) -> (i32, i32, i32, i32) {
    let x = x.clamp(0, (sw - 1).max(0));
    let y = y.clamp(0, (sh - 1).max(0));
    let w = w.clamp(1, sw - x);
    let h = h.clamp(1, sh - y);
    (x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_参数解析() {
        assert_eq!(parse_auto_arg("--auto=0,0,64,48").unwrap(), (0, 0, 64, 48));
        assert_eq!(parse_auto_arg("--auto= 10 , 20 , 300 , 200 ").unwrap(), (10, 20, 300, 200));
        assert!(parse_auto_arg("--auto=1,2,3").is_err());
        assert!(parse_auto_arg("--auto=a,b,c,d").is_err());
        assert!(parse_auto_arg("--auto=1,2,0,4").is_err());
    }

    #[test]
    fn 裁剪夹取() {
        assert_eq!(clamp_crop(0, 0, 9999, 9999, 640, 480), (0, 0, 640, 480));
        assert_eq!(clamp_crop(-10, -5, 100, 100, 640, 480), (0, 0, 100, 100));
        assert_eq!(clamp_crop(600, 400, 100, 100, 640, 480), (600, 400, 40, 80));
    }
}
