# 输入法排障与实现经验（2026-09-05 真机事件复盘）

> 来源：本机（openEuler 24.03 / XFCE / xrdp 双用户）真实故障"现在输入法出现了问题，无法输入，我使用极点"。
> 全程在 root(:11) 与 beyondcy(:10) 两个 xrdp 会话上诊断并修复，结论均经真机验证。
> 本文是 lyyIme 项目的设计依据之一：为什么需要"完全不依赖 ibus 的兜底形态"（floatapp/ 与 xim/）。

## 1. 事件与双根因

现象：用户打不出中文（英文正常），用户自述"使用极点（五笔）"。

| # | 根因 | 机制 |
|---|---|---|
| 1 | **会话没有输入法环境变量** | xrdp 的 `startxfce.sh` 直接 `dbus-run-session -- startxfce4`，不经过 `/etc/X11/xinit/xinitrc.d`，导致 `GTK_IM_MODULE/QT_IM_MODULE/XMODIFIERS` 从未导出。**ibus 活着也没用——应用根本没把按键交给它** |
| 2 | **ibus-daemon 总线挂死** | 进程在跑，但 `~/.cache/ibus/dbus-*` socket 已失联（`busctl` 报 Transport endpoint is not connected），`ibus engine` 永久挂起。早上有人配置时不得不用 `timeout 10` 包住 ibus 命令，就是当时的症状 |

两个根因叠加的表现极具迷惑性：托盘/进程看着一切正常，候选框却永远不出现。

## 2. 排障命令箱（真机验证有效）

```bash
# 0) 找到"哪个会话在抱怨"：xrdp 多用户各自一套 ibus，别修错对象
loginctl list-sessions; ls /tmp/.X11-unix/
ps -ef | grep -E 'ibus-daemon|xfce4-session' | grep -v grep

# 1) 检查会话进程有没有 IM 环境变量（ Applications 是从会话继承的）
tr '\0' '\n' < /proc/<xfce4-session-pid>/environ | grep -E 'GTK_IM|QT_IM|XMODIFIERS'

# 2) 摸 ibus 总线是否还活着（bus 文件是 ibus 自己写的"名片"）
cat ~/.config/ibus/bus/*-unix-<display>
busctl --address=unix:path=<IBUS_ADDRESS> list        # ENOTCONN = 总线已死
timeout 10 ibus engine                                 # 永远加 timeout!

# 3) 修复 = 带全套环境变量重启 ibus（-r 替换旧实例）
env DBUS_SESSION_BUS_ADDRESS=<会话bus> DISPLAY=:11.0 HOME=~ \
    GTK_IM_MODULE=ibus QT_IM_MODULE=ibus XMODIFIERS=@im=ibus \
    setsid ibus-daemon -drx

# 4) 不重启应用让已开的 GTK 窗口接上 ibus：XSETTINGS 通道
xfconf-query -c xsettings -p /Gtk/IMModule -n -t string -s ibus
# 验证新 GTK 应用能否解析到 ibus（模拟无环境变量的干净进程）:
env -u GTK_IM_MODULE python3 -c "import gi;gi.require_version('Gtk','3.0');\
from gi.repository import Gtk,Gdk;Gtk.init();\
print(Gtk.Settings.get_default().get_property('gtk-im-module'))"   # 期望 'ibus'
```

## 3. 机制知识（踩坑换来的）

- **GTK 输入法模块解析顺序**：`GTK_IM_MODULE` 环境变量 → XSETTINGS `Gtk/IMModule`（xfconf 的
  `xsettings/Gtk/IMModule`）→ 默认 simple。**XSETTINGS 改动只对"新建的输入上下文"生效**——
  已打开的窗口/输入框不会中途切换，必须重开窗口或重登。这就是"修好了但还是英文"的原因。
- **Qt 没有 XSETTINGS 通道**：只认 `QT_IM_MODULE` 环境变量 → 已运行的 Qt 应用无解，只能重开。
- **xrdp 双用户会话隔离**：root 与 beyondcy 各自有 ibus/xfconf/总线，`sudo -u user env ... ibus-daemon`
  才能把修复送进对方会话；`gsettings/xfconf-query` 都必须带上对方会话的 `DBUS_SESSION_BUS_ADDRESS`。
- **ibus bus 文件**：`~/.config/ibus/bus/<machine-id>-unix-<display>` 记录 IBUS_ADDRESS；ibus-daemon
  用 `-r` 起时会替换旧实例并重写此文件。判断 ibus 死活以"能否连上 bus 文件地址"为准，**不是看进程**。
- **XSETTINGS 需要窗口管理器侧的 xfsettingsd 在跑**（XFCE 默认有）；设置后可用 Xlib 读
  `_XSETTINGS_SETTINGS` 属性验证是否真的发布（`bytes 中含 b'IMModule'`）。
- **持久化三件套**：`/etc/xrdp/startxfce.sh`（RDP 登录导出 IM 变量）、`/etc/environment`（兜底所有登录）、
  `/etc/xdg/autostart/org.freedesktop.ibus.desktop`（自启 ibus）。只做其中一处都会漏场景。

## 4. 本次落地的修复（保留在系统上）

1. `/etc/xrdp/startxfce.sh` 增加三个 IM 变量导出（新登录会话全量生效）。
2. `/etc/environment` 追加 `GTK_IM_MODULE=ibus`、`QT_IM_MODULE=ibus`、`XMODIFIERS=@im=ibus`。
3. 两个会话的 ibus-daemon 带环境重启，`ibus engine table:wubi-jidian86` 验证可切换。
4. 两会话 xsettings `Gtk/IMModule=ibus`（救活已打开的 GTK 应用的新输入框）。

## 5. 对 lyyIme 的设计启示（为什么是这个项目）

1. **框架链路太长、故障面太大**：env → IM 模块 → daemon → 引擎 → 面板 → 应用，任何一环断都全灭，
   且表象相同（"打不出字"）。所以 Mode B（独立外挂）必须做到**零依赖、零会话要求**。
2. **已运行进程改不了环境变量**：任何"装完输入法要重启应用/重登"的方案都有巨大体验成本。
   floatapp 的悬浮窗模式不需要应用做任何配合（没有 IM 模块也能收 xdotool 注入的中文），
   这是它兼容性最好的根本原因。
3. **键盘"拦截-回放"路线在本机不可靠**：XRecord 只能"看"不能"拦"；XGrabKey 回放会被 active grab
   吞掉（RESEARCH.md §2.1 用 C 探针实证，两轮复测一致）。Mode B 的"应用内直输"因此走
   XIM server 正统路线（`xim/`），悬浮窗模式（floatapp/）作为零配置兜底，两者互补。
4. **keysym 中文注入本身是可靠的**：`xdotool type`（XChangeKeyboardMapping 空闲 keycode + XTest）
   在 GTK3/Electron/终端上实测全部上屏（本机多次验证，RESEARCH §2.1 同结论）——这是 floatapp
   "直输模式"与 e2e 自动化测试的基石。

## 6. 码表与实现常识

- ibus-table 1.17 码表就是 SQLite（`/usr/share/ibus-table/tables/wubi-{haifeng|jidian}86.db`）：
  表 `ime`(元信息) / `phrases(tabkeys, phrase, freq, user_freq)` / `goucima`(构词码) /
  `pinyin(pinyin, zi, freq)`(单字反查拼音) / `suggestion`。13.7 万行，全量内存前缀索引 <1s，
  简码=前缀查询，词组=完整 tabkeys（`wqvb`→你好）。
- `phrases.freq` 数值差异极大（10^7~10^9），直接按 freq 排序即可得合理词序；学习加权在
  user_freq 上乘大系数叠加（见 floatapp/dictload.py）。
- GUI 主循环纪律：**GLib 回调里抛异常 = 整个主循环退出**（本次真实翻车：未初始化的
  `self.code` 在码表异步加载完成回调里炸掉应用）。所有回调访问的状态必须在 `__init__` 先建好。
- python-xlib（本机 pip 版）`Display` **没有** `event_classes` 属性，网上常见 XRecord 解析写法
  `rq.EventField(None).parse_binary_value(...)` 会 AttributeError。可靠做法：record 回调里手工按
  32 字节切原始事件，用 `Xlib.protocol.event.KeyPress/KeyRelease(display=, binarydata=)` 解出
  `detail`(keycode)/`state`。（若走 XIM 路线则用不到此条，留作 XRecord 场景备忘。）

## 7. 关联

- `floatapp/README.md` — 悬浮窗模式使用与设计（已真机验证的即用版）
- `floatapp/e2e_floatapp.sh` — 双发送模式端到端自测（可复跑）
- `docs/RESEARCH.md` §2 — grab 回放不可靠的实证与 XIM 路线定稿
- `xim/` — Mode B"应用内直输"的生产实现（XIM server，规划中）
- `crates/lyyime-doctor/` — 把 §2 排障命令箱产品化为"修复 Linux 中文输入法"工具
