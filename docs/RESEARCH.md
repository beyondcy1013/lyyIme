# lyyIme 调研结论汇总

> 各路调研的落地结论(含出处)。实现时"借鉴自"标注以本文件为准。

## 1. ibus 自定义引擎开发(主控亲自读本机源码得出;借鉴自 ibus-table 1.17.2 与 ibus-libpinyin 1.15.5)

一手来源:`/usr/share/ibus-table/engine/{main,factory,table,it_util}.py`、`/usr/share/ibus/component/{table,libpinyin}.xml`(本机 ibus 1.5.29)。

### 1.1 进程与注册模型
- ibus-daemon 按 component XML 的 `<exec>` **直接拉起引擎进程并附加 `--ibus` 参数**(table.xml:`/usr/libexec/ibus-engine-table --ibus`)。
- 引擎进程内:`IBus.Bus()` 连接 → `IBus.Factory(connection=bus.get_connection(), object_path=IBus.PATH_FACTORY)` → 子类化并实现 `do_create_engine(engine_name) -> IBus.Engine 子类`(factory.py:41-102,逐行借鉴)。
- **静态注册**(推荐,libpinyin.xml 模式):component XML 内直接写 `<engines><engine><name>lyyime</name><language>zh_CN</language><icon>…</icon><layout>default</layout><longname>…</longname><rank>…</rank><symbol>伍</symbol><icon_prop_key>InputMode</icon_prop_key><setup>…</setup></engine></engines>`。
- 安装位置:**⚠️ 实证纠正(实现期发现)**:本机 ibus 1.5.29 源码 `ibusregistry.c::ibus_registry_load()` 只扫 `IBUS_DATA_DIR/component`(即 /usr/share/ibus/component),**用户目录扫描被上游 `#if 0` 禁用**(经金丝雀实验确证)。因此 install.sh 采用**桥接**:合同位置(~/.local/share/ibus/component 或 /usr/local/share/ibus/component)照装,再桥接一份到 /usr/share/ibus/component;无 root 时以 `IBUS_COMPONENT_PATH` 指引(该变量整体替换搜索路径,须含 /usr/share)。
- `ibus list-engine` 可列出全部引擎;`--xml` 动态枚举模式(table.xml 的 `<engines exec=… --xml/>`)不需要,我们只有一个 engine。

### 1.2 按键处理(借鉴 table.py:3816-3940)
- 签名 `do_process_key_event(self, keyval, keycode, state) -> bool`:True=已消费,False=放行给应用。
- 放行的坑:**Qt5 的 ibus 输入模块不实现 forward_key_event**,所以正常路径必须 `return False` 放行;`forward_key_event()` 只在单元测试的 MockEngine 里用(table.py `_return_false` 注释,3816-3851)。我们的引擎照抄这个双路径设计。
- key release 判定:`state & IBus.ModifierType.RELEASE_MASK`,release 事件一般直接放行;字符:`IBus.keyval_to_unicode(keyval)`。
- **密码框防护**:InputPurpose==PASSWORD/PIN 时全放行(table.py:3870-3876),照抄。
- **Shift 单击切换**:记录 Shift_L/Shift_R 的 press(无 RELEASE_MASK、无 Ctrl/Alt/Super 修饰);若 press 后到 release 之间没有任何其它按键事件 → 判定单击,toggle 中英。其余 Shift 事件一律放行。参考 table.py `_handle_hotkeys` 的修饰键白名单写法(3810-3813 对 Super_L/Super_R/ISO_Level3_Shift 的处理)。

### 1.3 候选窗与预编辑(借鉴 table.py:938-950、3052-3155)
- `IBus.LookupTable()`:set_page_size(page_size)、set_orientation(IBus.Orientation.HORIZONTAL);`self.update_lookup_table(lt, True)`;翻页 `lt.page_up()/page_down()` 后再 update;`lt.set_number_on_cursor` 由 ibus 面板渲染序号。
- 辅助编码提示:`update_auxiliary_text(text, True)`(显示 nihao 这类输入串);预编辑:`update_preedit_text_with_mode(text, cursor, True, IBus.PreeditMode.COMPOSITION)`。
- 上屏:`self.commit_text(IBus.Text.new_from_string(s))`。

### 1.4 托盘/属性菜单(借鉴 table.py:2938 do_property_activate)
- `do_property_activate(prop_name, prop_state)` 响应菜单项;engine 创建时构造 `IBus.PropList` + `IBus.Property(key=…, label=…, icon=…, type=IBus.PropType.MENU)` 子菜单(设置/工具/修复/关于),`self.update_property(props)` 挂到状态图标。中英状态用 icon_prop_key=InputMode 的图标属性切换(⟨中/EN⟩ 图标路径由我们提供)。

### 1.5 无 GUI 自动化测试(借鉴 ibus-table 的 MockEngine 模式)
- ibus-table 的做法(table.py `_unit_test` 标志 + MockEngine):把按键逻辑抽成可独立调用的方法,测试里用 Mock 对象替代 IBus.Engine 基类捕获 commit_text/update_* 调用,不经 dbus/daemon。我们的 lyyime.py 采用同样分层:`LyyimeEngine` 的核心 `_feed_key(keyval, keycode, state) -> list[效果]` 不依赖 IBus 类型;IBus 子类只做胶水。tests/unit_ibus_engine.py 直接 import 该模块喢单元键断言效果流。

### 1.6 引擎注册与本机配置事实(实测)
- dconf 路径 `/desktop/ibus/general/preload-engines`,本机当前 `['xkb:us::eng','table:wubi-jidian86','table:wubi-haifeng86']`;engines-order 同源;热键 triggers=['<Control>space','<Super>space']。
- gsettings schema id:`org.freedesktop.ibus.general`(gschema 文件 org.freedesktop.ibus.gschema.xml,path /desktop/ibus/)。**ime-default 实现首选 `gsettings set org.freedesktop.ibus.general preload-engines "[…]"`,schema 不在时降级 `dconf write /desktop/ibus/general/preload-engines "[…]"`**(dconf 直写不依赖 schema)。
- IBus gir 可用:`python3 -c "import gi; gi.require_version('IBus','1.0')"`,typelib 在 /usr/lib64/girepository-1.0/IBus-1.0.typelib,版本 1.5。
- 日志:ibus-table 把引擎日志写 `~/.cache/ibus/table/debug.log`(TimedRotatingFileHandler);我们写 `~/.local/share/lyyime/logs/ibus.log`,同样的 handler 模式。

## 2. X11 独立外挂输入法(主控在本机 Xvfb 实证得出;先例:uim-xim/gcin/yong(小小输入法)/ibus-x11)

### 2.1 实证记录(tests/e2e/x11grab_probe*.c,可复跑)

在 openEuler Xvfb 1.20(与真实 :11 同栈)上,用 C 探针实测了两种"grab 兜住键盘再转发"的方案:

| 实验 | 结果 | 证据 |
|---|---|---|
| `xdotool type "你好abc"`(XChangeKeyboardMapping+XTest) | ✅ 中英文全部上屏,中文走空闲 keycode(如 kc=8→U+4F60) | xev 收到 5 个 KeyPress,keysym 0x1004f60 等 |
| XGrabKey(sync) + `XAllowEvents(ReplayKeyboard)` 放行 | ❌ 探针判定 PASS,xev **收到 0 个事件** | /tmp 消失的回放;两轮复测一致 |
| XGrabKey(sync) + "摘 grab→AsyncKeyboard→XTest 回声→重挂" | ⚠️ 探针自身无循环风暴,但**激活中的 active grab 吞掉了回声键**,焦点窗口仍收不到 | xev 仅见 FocusOut(NotifyGrab)/FocusIn(NotifyUngrab),无 KeyPress |

结论:**被动 grab 的"拦截-转发"在本机 X server 上不可靠,放弃 grab-echo 作为上屏通道**。XChangeKeyboardMapping+XTest 的中文注入本身有效(ibus 模式/其它注入场景可复用)。

### 2.2 Mode B 定稿架构:最小 XIM server(借鉴 uim-xim、gcin、yong、ibus-x11)

XIM(The X Input Method Protocol)是框架无关中文输入的 30 年正统路线,本机 `/usr/libexec/ibus-x11` 就是活证据(ibus 正是用它服务 XIM 客户端)。

- 应用侧把 `XMODIFIERS=@im=lyyime`,GTK 内建 xim immodule / Xlib 应用原生 / Xaw 应用原生 即可接入;**无需任何键盘 grab、无注入、无回放问题**(XIM 的 forward-event/commit 本身就是正确的"拦截与回放")。
- 服务端:IMdkit(C,XIM server 工具库,uim/scim/fcitx 同款;本机未装,**vendor 进仓库**,出处与许可证注释在源码目录)。主二进制 `lyyime-xim` 用 C 写(IMdkit+GTK3 C+dlopen(liblyyime_core.so) 走已定义的 C ABI)。
- 输入样式:root-window style(preedit 与候选都画在我们自己的 GTK3 窗口,类万能五笔外观;on-the-spot 兼容性差,不用)。
- 中英切换 = XIM trigger off/on:中文态=trigger on(应用把按键 forward 给我们);Shift 单击 → trigger off(英文态,应用直接收键)→ 再单击恢复。XIM 协议保证 trigger 外的按键 100% 原样直达应用。
- 覆盖面诚实声明:Mode B 覆盖 GTK3 + Xlib/Xaw + 终端模拟器(XIM 类);**Qt5 应用需 Mode A(ibus)**——Qt5 已无 XIM 支持(本机只有 ibus/compose 插件)。双模式互补正好全覆盖,这也是双模式设计的价值所在。
- 探路石(spike,先行):最小 XIM server 连通 GTK3 Entry(Xvfb),证明 GTK xim immodule 在本机可用;若不可用则升级为自带 GTK immodule 方案(先例:fcitx/gcin 的 gtk immodule)。**此 spike 不通过则不继续铺开 Mode B 全量。**
  - **✅ 实证结果(2026-09-06,xim/spike/run.sh,Xvfb :98)**:xcb-imdkit 开 XIM@lyyime → GTK3 Entry(GTK_IM_MODULE=xim, XMODIFIERS=@im=lyyime)成功连接 → xdotool 按键经 forward-event 到达 server → server commit 的中文出现在 Entry 缓冲。可重复执行。结论:GTK3 3.24.52 的 xim immodule **不是内建,而是独立 RPM `gtk3-immodule-xim`(OS 源有,需安装)**,装后 immodules.cache 出现 "xim" 条目;install-all 需确保该包存在。
  - **✅ 协议细节实证**:XIM 客户端默认只转发 KeyPress;server 端 `xcb_im_create` 必须显式传 `KEY_PRESS|KEY_RELEASE` 才能收到 release(Shift 单击以此为主判据,280ms 时间窗兜底,off→on 后 400ms 组合守护防 Shift+字母 误切)——见 xim/src/xim_server.c。

### 2.3 从死掉的调研中抢救的可靠常识(标注来源,待实现者复核)
- GTK3 候选窗:override-redirect + accept_focus(false) + type_hint,光标跟随用 XQueryPointer(gtk3 文档/常见 IME 实现)。
- 托盘:Xfce4 面板为 XEmbed systray,Gtk.StatusIcon(gtk3)可用;备用 ksni(StatusNotifier)。
- Rust X11 绑定:x11 crate 含 xlib/xrecord/xtest;Mode B 转向 C/IMdkit 后仅在需要时引用。
- auto-repeat:XkbSetDetectableAutoRepeat 区分真重复;XkbMapNotify 后重建抓取(grab 方案已弃,仅留档)。

## 4. 集成期实证补遗(2026-09-06,M6 集成排障)

这些是"单测全绿但真机不工作"级别的坑,均有复现实验佐证:

1. **PyGI 引擎对象必须显式传 `connection=bus.get_connection()`**(ibus 引擎):只传 `object_path` 时引擎实例看似创建(工厂日志有),但对象从未导出到 ibus 私有总线,daemon 代理为死路,按键全部回落 ibus-engine-simple 原始直通。用 busctl 对私有总线 introspect 引擎对象路径即可判定(不存在=此坑)。对照 ibus-table table.py:280-288。
2. **`VAR=x bash -c 'script' VAR=x` 是位置参数不是环境变量**:配合内层 `set -u` 会立刻炸且报错位置误导。必须 export。
3. **测试脚本严禁全局 `pkill ibus-daemon`**:会误杀真实会话的输入法(本项目踩过并已恢复);按 lockfile pid 或会话内 PID 精确清理。
4. **`ibus engine <name>` 设置命令返回码有竞态误报**(设置应答先于引擎工厂完成),以之后的 `ibus engine` 查询为准。
5. **被动 grab 的回放/回声在本机 X server 不可靠**(见 §2.1)——任何"拦截再转发"的输入法设计在本机都应走 XIM。
6. **真实词库(跨源 4 个数量级频差)会击穿加性排序公式**:排序必须层级主导(见 ARCHITECTURE §5 v1.1),fixture 小数据测不出来,只有真库验收能暴露。

## 3. 码表词库资源与排序(调研 Agent 限流牺牲;由 dicttool 实现者自行验证来源,结论回流此处)
