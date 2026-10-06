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

## 5. AI 助手(/AI 触发)实现决策(2026-09-06)

1. **协议选型**:不做私有协议,直接实现 OpenAI Chat Completions 兼容
   (`POST {base}/chat/completions`)。DeepSeek/千问(dashscope 兼容模式)/
   智谱/SiliconFlow/Ollama/LM Studio/vLLM 全部兼容,"自定义大模型"因此
   等价于填 base_url + key + model 三个字段。
2. **单实现双宿主**:HTTP 调用只写一份(python 标准库 urllib,
   `ibus-engine/engine/lyyime_ai.py`);Mode A 进程内 import + 线程调用,
   Mode B(C)以 `python3` 子进程调用同一文件——避免 C 里引 HTTP/TLS 依赖
   (AGENTS.MD"不引第三方"),也杜绝两份协议代码漂移。
3. **触发键吞字问题**:`/` 在 core 无中文映射(空缓冲 Pass),触发键必须
   吞下才能拦截 `/ai`;打歪时 Mode B 可协议级补发原事件(xcb_im_forward_event),
   Mode A 的 `return False` 只能放行"当前键",无法补发历史键,故退化为
   **文本上屏 `/`**。普通文本框两者输出一致;仅"以 `/` 为快捷键"的应用
   (如 Firefox 快速查找)在"中文态+AI 已启用+单按 `/`"场景受影响——
   该组合极少见,且未配置 AI 时功能零介入,可接受。
4. **GLib 子进程陷阱**:`g_child_watch_add` 必须配 `G_SPAWN_DO_NOT_REAP_CHILD`,
   否则 wait status 恒为 -1,成功的调用也被判失败(Xvfb e2e 实测踩中)。
5. **配置文件历史缺陷归一**:旧版 xim `config.c` 写行尾注释不带 `#`
   (如 `page_size = 5 候选数 1..9`),不是合法 TOML,python tomllib 直接解析
   失败。新版 split_value 兼容读旧格式,保存时统一写为 ` # 注释`;字符串值
   带引号+转义,并支持 `[ai]` 段(其它段字节级保留)。
6. **Mode B Shift 单击与合同偏差修复**:contract v1.2 是"Shift 按下时有缓冲
   上屏英文原串,随后单击确认切英文"(Mode A 引擎即如此实现);xim 旧代码
   只在空缓冲时挂起单击判定,导致 e2e B3(英文态直通)本来就挂。已修齐:
   无论有无缓冲都挂起,释放/时间窗确认单击 → 切英文。

## 6. 造词模式(Ctrl+=)实现决策(2026-09-06)

- **交互出处**:借鉴极点五笔/万能五笔的「Ctrl+= 自造词」——上屏后按热键把
  刚输入的汉字组成词组入库;方向键增减选字是 lyyIme 的简化(极点用
  Backspace/词长调整,双方向键更直观,两者都保留:←/↓/退格 等效)。
- **取码规则**:五笔86 标准词组编码(2 字词各取前 2 码、3 字词 1+1+2、
  4 字词各 1 码、>4 字取 1/2/3/末 各 1 码),与海峰86 码表自洽——fixture
  中 你(wqiy)+好(vbg)→wqvb 与码表自带「你好=wqvb」一致,真库验证
  「好你→vbwq」造出即可打出且 User 层置顶。
- **存储选择**:user.tsv(learner)只做频次加成、无码位;造词需要
  「词→码」可打,单独落 `user_words.tsv`(word\tcode\tcount),启动并入
  五笔内存索引,与 learner 双轨互不影响。排序词频取码表最大值保证同码
  首位,再叠加 learner ×1.5。
- **两段式纪律的坑**:造词"存词写盘"不能发生在 plan(纯读取)阶段——
  -needed 重试会二次计数。定式:plan 做**幂等可写性预检**(建目录+写模式
  探测打开,不创建文件),效果流里的 Notice 文案在 plan 期定死,apply 才
  真正 add+落盘;落盘竞态失败保 dirty 下次重试。
- **历史只记汉字**:commit 不含汉字(标点/字母直通)时不清 last_run——
  「你好,」的逗号不该让造词起点归零。
- **热键解析双端同规格**:coin_hotkey 写法 `ctrl+equal`(修饰至少一个,
  纯键热键与打字冲突一律拒绝),Mode A `lyyime.py parse_hotkey` 与
  Mode B `keysym_map.c lyy_hotkey_parse` 各自实现、单测对齐;忽略
  CapsLock/NumLock 位。
- **装机链路坑(2026-09-06 实测)**:ibus 组件模板 lyyime.xml 的 exec 占位
  符从 @ENGINE_DIR@ 换成 @ENGINE_EXEC@ 后(Rust 引擎迁移 WIP),
  install.sh 未跟上,装出带字面 `@ENGINE_EXEC@` 的 XML → daemon 拉起
  失败、窗口静默复用旧引擎实例——真机造词探测两次"看似失效"即此因;
  另:ibus-engine/install.sh 只装引擎不装核心库,新键值(LKEY_COIN=11)
  在旧 liblyyime_core.so 里按 Other 放行, symptoms 相同。装机必须
  scripts/install-all.sh 全链,或引擎+核心库一起更新后 `ibus restart`。

## 7. 上屏后联想(本地续接词尾)实现决策(2026-10)

- **数据源选择**:纯本地静态词表——五笔词表 + 拼音词组 + 推荐词三源合并
  建一张 `BTreeMap<词, 频次>`(同词取最大频次),无任何远程调用;
  联想结果随词库生成,无需联网也无隐私外发面。
- **查询模型**:只取已上屏中文尾串作前缀(最多近 6 个汉字),按
  `BTreeMap::range(prefix..)` 前缀区间扫描,避免逐键全表扫描;上下文
  从长到短匹配,长上下文命中优先生成、尾巴全局去重(同一尾巴只取最长
  上下文档),按频次降序+文本升序输出,封顶 50 条。
- **两段式纪律**:联想页只是普通 Candidates 帧(注释列留空),效果流
  依旧纯函数产出;-needed 重试不落状态,联想选择重试与一次成功逐字节
  一致(ffi_重试纪律_联想触发与联想选择 用例覆盖)。
- **联想态按键语义**:空格=续选首选尾巴、数字=页内选尾、鼠标点选同路;
  普通字母只撤联想行(保留中文上下文,允许分开打词)并开新组合;
  Esc 吞键一次同时清联想行与上下文;标点/回车/退格/方向/Shift/模式切/
  Pass 一律清联想上下文。联想尾巴不写入 learner(半截尾巴不是词)。
- **与菜单触发的共存**:菜单提示与联想行共用候选条位置——同帧最终仍有
  候选行时不得贴提示覆盖(Mode A/B 同口径),否则 Fn 会执行用户看不见
  的动作;AI 采集进入时临时关联想并撤下候选行,退出按配置还原,防止
  空格把联想尾巴吞进提示词。
- **ABI 兼容**:新增 `lyyime_set_next_word_prediction` 为 Mode B 可选
  符号——旧 core 缺符号时无法使用/配置联想(config.toml 键不生效),
  仅记 INFO、保留原输入行为,不整体降级。
- **开关语义**:`next_word_prediction` 默认关(设置→输入 显式开启);
  热关闭立即撤下已展示的联想行并清上下文,不动正在进行的组合缓冲。

## 8. 中文标点切换(Ctrl+.)与配置键归一(2026-10)

- **运行时窄化路径**:Ctrl+. 切换走 `set_chinese_punctuation` /
  `toggle_chinese_punctuation` —— 只改 `cn_punct` 开关并复位引号配对
  状态,**不重装载词表、不动组合缓冲/候选/联想上下文/模式**;
  `set_config` 仅在 cn_punct 实际变化时复位引号配对(新默认下的首个
  引号从头配对)。
- **禁用态先判后映射**:`plan_punct` 必须在 `to_chinese`/QuoteState
  之前判 `!cn_punct` —— ASCII 引号若在禁用态翻转开合状态,会污染
  下一次中文引号对的方向(实测 bug 类)。
- **保留键与锁存**:Ctrl+. 固定保留为中文态标点切换键 —— 干净修饰
  恰为 Ctrl(Caps/NumLock 位已掩掉;Ctrl+Shift/Alt/Super 不命中);
  按住连发由 `punct_key_down` 锁存去抖,配对 release 吞掉,focus/
  trigger 边界清锁存。**纯 Control_L/R 预热键直接放行不进 core** —
  `LKey::Other` 会清组合缓冲,先按 Ctrl 再按 . 不得打断正在输入的
  组合。英文态/密码框不拦截、原样直通。
- **提示通道**:空缓冲+无候选行+非 AI 采集时,用 `show_hint`(候选条
  预编辑行,**无定时**)亮当前状态(中文标点:,.?! / 英文标点:,.?!);
  组合中或候选/联想行在时静默切换。不走 notice —— 定时清除会连带
  撤下候选行。
- **配置键归一**:core `ConfigToml` 同认规范键 `chinese_punct`(XIM
  设置窗写盘键名)与旧别名 `cn_punct`,规范键优先(与书写顺序无关);
  XIM `config.c` 两键同归注册表键 3,保存统一写 `chinese_punct` 一行,
  别名/重复行只消费不写出,其它行与注释原样保留。
- **保留键冲突自愈**:造词/截屏热键配成 `ctrl+period`(含 `Ctrl + .`
  等别名写法)时,加载路径自动让位 +alt → +alt+shift(保留键自身计入
  占用,否则让位会原样返回保留键);设置窗保存路径直接拒绝并保持
  输入框可编辑,不静默改写。
- **ABI 兼容**:`lyyime_set/toggle_chinese_punctuation` 为 Mode B 可选
  符号组(`punct_ok`);旧 core 缺符号时无法切换/配置运行时标点,
  仅记 INFO,原输入行为不变。
- **运行时态归属**:Ctrl+. 翻转只落在引擎运行时开关,不写 config.toml;
  设置窗复选框仍反映保存的默认值,保存后经 `lyy_engine_reload` 重建
  引擎即生效,无需重启。

## 拼音前缀候选:部分消费量(缺词兜底,不猜字)

- **语义**:`Candidate.consumed` 表示该候选消费编码缓冲开头的 ASCII
  字节数;`0` 消费整个缓冲,非零只消费前缀(如 `jieping` 选「截」
  消费 3,`ping` 留在缓冲里继续拼)。值取自分词边界,
  不经显示注释/字数/来源推断,学习改 kind、固定、排序、克隆均不改写;
  并列同分时消费量大的候选优先。
- **生成边界**:整词/整音节候选维持 consumed=0。删除「只认最后一个
  音节」的两处来源(多音节输入的 `seg_last` 单字循环、不完整音节
  单字段)——它们丢弃前面音节,是「后缀顶替整词」的来源;新增
  前缀兜底段只对首段完整音节 + 后缀语法可续(`can_continue`)
  时产出 consumed=k 候选,档级固定 `TIER_PINYIN_ABBREV - 1.0`
  恒低于一切完整候选,词库补齐词组后自动让位。
- **选词分支**:`plan_select` 对 `0 < consumed < buf.len()` 先上屏
  前缀文本,清空组合后再放回未选后缀并重算候选 —— 学习只记已选
  前缀(键数成本同样按前缀计),联想/提示不盖剩余组合,
  后缀照旧可退格/改打/再选;
  `select_candidate` 直选走同一路径,FFI 两段式重试纪律不变。
- **边界键不丢字**:Enter/Shift 照旧提交原始编码;标点/Coin 收尾走
  `finish_text` —— 首选为前缀候选时提交「前缀文本+原始后缀原文」,
  绝不对未选后缀猜字;`learn_worthy` 只认 consumed=0 的首候选,
  「截ping」这类拼接串不入学习表。前缀候选不触发四码唯一顶屏,
  `plan_wubi_hit` 仅认 consumed=0 候选。
- **死码判定放松**:空候选+`cn_hit` 时仅当 `can_continue` 失败(当前
  串既不是音节前缀、也无法在完整音节边界后合法续打)才吞键 ——
  「jie」首音节字全屏蔽后 `jiep` 仍应正常续打;当前音节表不支持的
  后缀(测试夹具无 q 起头音节时的 `jieq`)照旧吞键 —— 真死码与否
  相对词库音节表判定,真实词库含 qi* 时 `jieq` 属合法续拼。
  五笔等既有死码测试语义不变。
- **内嵌补充词表**:`data/pinyin_supplement.tsv` 随核心编译内嵌
  (`include_str!`),`load_pinyin_phrase` 在 base 词组文件**非空**、
  主词库无该词、且全部标注音节已存在时才补入 —— 仅"同名词条"
  让主词库优先(不覆盖其读音/权重),并非压低其它补充词排位;
  缺词/空词的词库保持降级形态(前缀兜底),补充表不会把小词库
  扩展到它不支持的音节。首条补充词「截屏 jie ping」的 16 是
  **人工排序权重**(与既有「截图」档相当),不是测得的语料频次;
  拼音标注依据 phrase-pinyin-data large_pinyin.txt。
- **验证(2026-10-01,webClx)**:请求 `200749-…b037` 全量
  `scripts/test.sh` 套件、release 工作区构建、`make -C xim all test`
  全部通过;唯一失败是新 E2E 的初始空缓冲断言(探针客户端只在
  changed 时写盘,预置空文件修复)。修正夹具后请求
  `201010-…b038` 的隔离 Xvfb E2E 通过:jieping 经 截→屏 两次独立
  上屏、nihao 整词一次上屏、逗号无损拼接「截ping，」逐字节命中。
  部署请求 `201051-…b039` 通过:核心/IBus 工件 SHA 校验、
  config.toml 未变、XIM/IBus/面板稳定;回滚走既有部署脚本生成的
  core/ibus/xim `.bak` 备份。

## 全局截屏快捷键:XFCE xfconf 桌面级登记(2026-10-02,§13 修订)

- **问题**:此前 Ctrl+Alt+A 只在 lyyime-xim 的 XIM 事件流内拦截,焦点在
  未接入本输入法的窗口(英文键盘、其它 IM、桌面)时截屏不可用。
- **设计**:把 `shot_hotkey` 登记为 XFCE 的桌面级命令绑定——
  `xfce4-keyboard-shortcuts` 频道 `/commands/custom/<accelerator>`,
  由 xfsettingsd 在根窗口做被动 grab。绑定存在 xfconf,与输入法进程
  无关:换 IM/停用 lyyime/输入法引擎崩溃后快捷键仍可用,且不需要常驻
  守护进程(桌面设置本身就持久)。
- **服务面**(实机 `gdbus introspect` 验证):总线名 `org.xfce.Xfconf`,
  对象 `/org/xfce/Xfconf`,接口 `org.xfce.Xfconf`,方法
  `GetAllProperties(s,s)->a{sv}`(property_base="" 返回全频道)、
  `GetProperty(s,s)->v`、`SetProperty(s,s,v)`、`ResetProperty(s,s,b)`、
  `PropertyExists(s,s)->b`;实现走 GDBus 直连(无 libxfconf 依赖、
  无子进程解析),调用超时 3000ms。
- **属性形态**:`/commands/custom/` 与 `/commands/default/` 是命令绑定
  两组分支,`/xfwm4/{custom,default}/` 是窗口管理器键两组;直接子叶
  才是键,`override`(bool,选 custom 或 default 生效)、`startup-notify`
  与 `<Super>r/XF86Audio*` 这类嵌套路径不算绑定。`type="empty"` 的
  xfconf 属性经 GetAllProperties/GetProperty 均不可见(等同不存在)。
  加速键名为 GTK accelerator 写法(`<Primary><Alt>a`;Primary≡Control),
  比较一律归一化 (keyval, 修饰位),别名/大小写/修饰序差异同判冲突。
- **事务纪律**:快照 → 冲突检查(commands+xfwm4 有效分支,值非空且
  命令非 `/usr/local/bin/lyyime-shot` 的等效组合即拒绝并点名)→
  Set 目标 → 逐个 Reset 其余自有绑定(删除前重读现值仍属本方),
  任何一步 DBus 失败逆序回滚;设置保存失败由调用方显式回滚。
  `commands/custom/override` 未启用时报错请用户先开,不代为打开
  (那会连带激活全部自定义绑定)。
- **验证**:`tests/e2e/global_shot_hotkey_e2e.sh` —— xvfb-run 隔离显示
  + dbus-run-session 独立总线 + 临时 HOME/XDG_* + 真 xfsettingsd,
  xdotool 按键实弹拉起/取消覆盖窗;另含别名占用、xfwm4 占用、总线
  缺失、`--remove-shot-hotkey` 只摘自有绑定等用例。
