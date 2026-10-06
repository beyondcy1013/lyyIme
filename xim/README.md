# lyyIme Mode B — lyyime-xim(XIM 独立外挂)

架构与接口合同见 `docs/ARCHITECTURE.md` §3/§6/§8;调研依据见 `docs/RESEARCH.md` §2
(被动 grab 回放已实证不可靠,XIM 为正统路线)。本文件记录实现层关键决策与验证方法。

## 目录

```
xim/
├── Makefile            libxcb-imdkit.a(直编)/ lyyime-xim / spike / test / e2e / install
├── src/                主程序(C99 + GTK3 + dlopen)
│   ├── main.c          入口:CLI/单实例(pidfile)/信号/装配/GLib 主循环
│   ├── xim_server.c    xcb-imdkit 封装:trigger 键=两个 Shift,root-window 样式
│   ├── core_ffi.c      dlopen liblyyime_core.so(§3 全符号,LYYIME_CORE_LIB 覆盖)
│   ├── effects_json.c  §3 JSON 效果流迷你解析器(固定 schema)
│   ├── keysym_map.c    keysym→LKey 映射(§6)
│   ├── candidate_window.c  GTK3 override-redirect 候选窗(跟随光标 xcb_query_pointer)
│   ├── skin.c           皮肤注册表+CSS 生成器(候选窗与设置画廊共用一份调色板)
│   ├── tray.c          Gtk.StatusIcon 托盘 + 右键菜单(主窗口/设置/工具)
│   ├── mainwin.c       主窗口:状态行 + 输入设置/直输模式/工具箱入口(--mainwin/SIGUSR2 唤起)
│   ├── settings.c      GtkBuilder 设置对话框(保存即生效:重建引擎+刷 CSS)
│   ├── tools.c         工具动作(主窗口与托盘共用;含直输模式窗口 lyyime-float 拉起)
│   ├── config.c        ~/.config/lyyime/config.toml 平面键值 TOML 子集(保留未知行与注释)
│   ├── global_hotkey.c 全局截屏快捷键:XFCE xfconf 事务化登记/移除(§13,GDBus 直连)
│   └── common.c/.h log.c
├── res/                zh/en 托盘 SVG、settings.ui、candidate.css
├── spike/              探路石:最小 XIM server + GTK Entry 连通(run.sh 可复跑)
├── tests/              桩库(stub_lyyime_core.c)、单测、e2e_client
├── install.sh / uninstall.sh
└── vendor/             xcb-imdkit(fcitx,LGPL-2.1+,不改源码)
```

## 关键决策(实现期新增,与文档合同并行有效)

1. **XIM 事件流**:注册 trigger 键(两个 Shift,干净修饰掩码 0xED,忽略
   CapsLock/NumLock)走动态事件流;offKeys 为空,故英文态只有 Shift 按下会被
   客户端转成 `XIM_TRIGGER_NOTIFY(on)`,中文态全部按键转发到 server。
   借鉴:fcitx 触发键"干净修饰"惯例;协议行为以 vendor 库内部实现为准
   (`xcb_im_preedit_start/end` 即 server 侧 trigger on/off 的开关)。
2. **KeyRelease 实证**:Xvfb 实测(Xlib XIM 客户端,gtk3-immodule-xim 3.24.52)
   —— trigger on 时 client 转发 KeyPress;KeyRelease 是否转发依赖
   `SET_EVENT_MASK.forward_event_mask`,server 必须在 `xcb_im_create` 显式传
   `KEY_PRESS|KEY_RELEASE`(默认仅 PRESS)。
3. **Shift 行为**:按下即喂 core——有缓冲立即上屏英文原串,且(默认
   `shift_english=en`,§6)mode 效果即时关 trigger 转英文直通;空缓冲吞键并挂起
   单击判定,优先用 Shift release 确认(到达即判定);若客户端不转发 release,
   退化为 280ms 时间窗(窗内无其它键即单击)。off→on 切换后 400ms 内到达的
   Shift 掩码键判定为组合并回退英文(键事件自带 ShiftMask,应用侧无感)。
4. **Pass 零风险**:停用/降级/解析失败时一切按键 `xcb_im_forward_event` 协议级
   原样回放,绝不吞键(崩溃安全语义,E2E 步骤 B 断言)。
5. **vendor 直编清单**:=`src/*.c` + `xlibi18n/lcCT.c lcUTF8.c lcCharSet.c`
   (上游 `src/CMakeLists.txt` 同清单;`xim/vendor/README.md` 所写"仅 src/*.c"
   缺转码器会链接失败;uthash 头在 `xcb-imdkit/uthash/` 而非 `uthash/src/`)。
   vendor 源码不加 `-Werror`(禁改 vendor,红线约束自有代码)。
6. **preedit 清空形态**:core 输出 `{"t":"preedit"}`(无 "s")表示清除预编辑,
   解析器按空串处理(主控 2026-09-05 通报)。
7. **设置保存即生效**:写 config.toml → `lyy_config_apply_autostart`(写/删
   `~/.config/autostart/lyyime-xim.desktop`)→ 重建 core 引擎(等效 set_config)
   → 候选窗 CSS 字体即时刷新。§3 C ABI 无 set_config,重建实例是合同内唯一途径。
8. **单实例唤起**:pidfile `~/.local/share/lyyime/xim.pid`;二次启动 `kill -USR1`
   已存在实例(弹设置窗)后自身退出。`--settings-page N`(0 基页码)先落
   `~/.local/share/lyyime/settings-page.req` 再发 SIGUSR1,实例读到后切到
   对应页签(信号不带参数;`--mainwin` 走 SIGUSR2 不受影响)。
9. **commit 编码**:UTF-8 → COMPOUND_TEXT(`xcb_utf8_to_compound_text`,依赖
   客户端 locale 为 UTF-8;E2E 用 zh_CN.utf8)。
10. **菜单触发(2026-09-30)**:上屏中文文本的连续 CJK 尾串命中菜单功能名
    (目录在 core `menu_trigger.rs`,稳定 id + 别名,可信枚举不落 shell)
    → 候选条持续提示 `匹配了菜单功能「设置」,按 F7 进入该功能`;只有再按
    配置的无修饰确认键(默认 F7,仅 F1–F12)才执行一次。截屏项追加实际
    快捷键 `；也可按 Ctrl+Alt+A`(跟随实际生效的 shot_hotkey;IBus 非法键
    不展示,XIM 非法键显示其回退值 Ctrl+Alt+A),英文项追加
    `；也可单击 Shift 切换中英文`;其它项不发明快捷键。Ctrl/Alt/Super/
    Shift 修饰的 Fn、退格/方向/翻页/Esc 等编辑键、焦点/模式/AI 会话进出、
    引擎与配置重载都是硬边界(清尾串与待执行);打开自身设置窗/主窗口
    同样硬复位(不依赖客户端焦点 UNSET,无 WM 环境也生效);其余普通键
    只取消待执行、尾串保留(「设」「置」分开上屏仍命中)。确认键的 press 被吞后配对的
    release 同样吞掉,不泄漏半个按键给应用。四码顶屏自动起新组合时同帧
    已有新预编辑:提示不抢占且待执行一并取消(不可见动作不可执行),
    尾串保留供后续连续上屏续接。
    - 配置(config.toml,与 Mode A 共用):
      `menu_trigger_enabled`(默认 true)、`menu_trigger_key`(1..=12,默认 7)、
      `menu_trigger_disabled`(逗号分隔目录 id,默认 `fix_ime`;逐项精确匹配,
      未知 id 写回保留)。设置窗新增「菜单触发」页签动态生成勾选行
      (勾选=禁止文字触发;与「快捷键」页的快速功能键 §14 互不相关)。
    - 可选符号组:`lyyime_menu_trigger_*` 全部缺失时(`mt_ok=0` 旧 core 库)
      本特性整体静默关闭,其余功能不受影响。
    - AI 采集态的 commit 进提示词不喂匹配器;`lyy_xim_commit_utf8`
      (AI 回复上屏)不经效果流,同样不喂。
11. **全局截屏快捷键(2026-10-02,合同 §13 修订)**:截屏热键不再依赖
    XIM 内按键转发——`global_hotkey.c` 经 GDBus 直连 `org.xfce.Xfconf`
    (channel `xfce4-keyboard-shortcuts`),把 `shot_hotkey` 登记为
    `/commands/custom/<GTK accelerator>`(命令固定
    `/usr/local/bin/lyyime-shot`)。xfsettingsd 全局抓取,所有输入法/
    无输入法环境可用,IM 停止后依然有效。事务化:全量快照 → 归一化
    `(keyval,mods)` 冲突检查(commands 与 xfwm4 有效分支;<Control>≡
    <Primary> 等别名判等)→ 先登记目标、再撤其余自有绑定;中途失败
    逆序回滚。三条入口:正常启动(gtk_init 后自动登记,失败仅告警)、
    设置窗保存(apply → 落盘 → commit,任一步失败回滚且不假报成功)、
    CLI `--sync-shot-hotkey` / `--remove-shot-hotkey`(安装/卸载脚本调用,
    不碰 pidfile 不拉起引擎)。仅支持 XFCE(显式非 XFCE 的
    XDG_CURRENT_DESKTOP 直接报错);override 未启用时报人话错误,
    绝不代开自定义组。
12. **CapsLock 联动解锁(2026-10-06)**:英文态锁存时 Shift 单击确认回中文
    即解除系统大写锁。`combo_guard` 作待确认标记:触发按下不解锁(窗内
    Shift+字母回退会先误清锁);release 到达或防护窗到期兜底即确认并解锁,
    到期路径同时清掉本事件 `state` 的陈旧 LockMask。中→英/组合回退/焦点
    切换不动锁。解锁走专用短命 Xlib 连接 `XkbLockModifiers`(mask-only,
    失败仅记日志);ibus 端同合同,由 logic 回调宿主执行。

## 验证

```bash
make -C xim spike spike-run   # spike:Xvfb :98,server commit「你好尖兵」进 Entry
make -C xim test              # 单测:effects_json + config + 桩库
bash tests/e2e/xim_e2e.sh     # 全链路:数字选词/Shift 切换/英文直通/空格顶屏/
                              # 造词/CapsLock 直通+联动解锁/截屏热键/右键菜单
bash tests/e2e/xim_e2e.sh --caps-only --keep
                              # 聚焦 CapsLock 套件:A–F 前置 + 场景 H,跳过 G
make -C xim e2e               # 同上(make 入口)
bash tests/e2e/menu_trigger_e2e.sh  # 菜单触发聚焦:可见提示/Fn 确认/设置页改键+
                                    # 黑名单即时生效/续接与边界取消(隔离 Xvfb)
make -C xim skin-test          # 皮肤单测(xvfb-run):注册表/CSS 解析/真窗色值/
                              # 双预览不透染/跟随系统明暗/显式皮肤稳定
bash tests/e2e/global_shot_hotkey_e2e.sh  # 全局截屏快捷键(§13):隔离
                              # dbus+Xvfb+真 xfsettingsd;登记/幂等/改键清旧/
                              # 别名与 xfwm4 冲突拒绝/总线缺失/remove 只摘自有
bash tests/e2e/skin_e2e.sh     # 皮肤 E2E:皮肤页打开/点选保存/取消不落盘/
                              # 重启持久化/候选窗换肤截图(隔离 xvfb-run,
                              # 截图留证 /tmp/lyyime-skins-review/)
```

> 已知失败(未定位):xim_e2e.sh 场景 G(候选右键菜单)按旧硬编码行几何
> 点选,当前桩链路下 G3 未弹出菜单(2026-10-06 运行现场
> /tmp/lyyime-e2e.tgmP79,失败点「右键第 1 行未弹出菜单」)。
> 聚焦验证用 `--caps-only` 跳过 G;候选菜单行为的现代覆盖见
> `tests/e2e/candidate_menu_e2e.sh`(真 core 链路)。

## 皮肤(候选窗主题)

- **用法**:设置窗「皮肤」页(第 7 页,--settings-page 6)卡片画廊选皮肤,
  「确定」保存并即时生效;「取消」/关窗只丢弃草稿。配置键 `config.toml`
  顶层 `skin = "<id>"`(默认 `system`)。卡片预览与真实候选窗共用
  `skin.c` 的同一份调色板与 CSS 模板,所见即所得;每卡 provider 仅挂本
  预览子树(APPLICATION 优先级,不走 screen 级注入),与候选窗及邻卡
  互不透染。
- **作用域**:仅 lyyIme 自绘候选窗(GTK XIM 模式),**不影响 IBus 系统
  面板/其它输入法界面**。
- **调色板**(id — 名称,分类):`system` 跟随系统(经典,随桌面明暗
  自动切换;暗色=深灰底亮字)、`business-navy` 深海蓝金(商务)、
  `porcelain` 云瓷简白(商务)、`sakura` 樱花奶糖(可爱)、
  `peach` 蜜桃软糖(可爱)、`bamboo` 青岚竹影(自然)、
  `lavender` 暮紫星河(梦幻)、`cyber` 霓虹夜航(科技)、
  `contrast` 曜石明晰(高对比)。新增皮肤只需在 `src/skin.c` 的
  `g_skins[]` 表尾追加一条,画廊与候选窗自动收录。
- **安全回退**:id 缺失/未知/空一律回退 `system`;自定义
  `candidate.css` 损坏时丢弃该文件改用内置布局,程序生成的皮肤层 CSS
  不参与降级;再失败保留上一份有效样式,绝不空窗。显式皮肤不做定时
  CSS 重解析;`system` 由 5s 低频定时器复查 GTK 明暗,实际变化才重建。

## 已知局限

- 覆盖面:GTK3(XIM immodule)/Xlib/Xaw 类应用;Qt5 无 XIM 支持,走 Mode A
  (docs/RESEARCH.md §2.2)。
- trigger 状态随新焦点重置为中文态(XIM per-IC 语义);英文态偏好不跨应用记忆。
- 大写字母直通不进组词缓冲(§6 未定义,取主流行为)。
- 托盘需 XEmbed systray 宿主(xfce4-panel);无宿主时仅无图标,功能不受影响。
- 时间窗单击判定(280ms)在"按住 Shift 超 280ms 再单点输入法"场景会误判为
  单击——但该场景随后必有字母键取消,仅纯按住 Shift 不动时不切换,影响可忽略。
- 菜单触发只在 core 库含 `lyyime_menu_trigger_*` 符号组时生效
  (旧库优雅降级为关闭);目录为静态白名单,暂不支持用户自定义条目;
  提示文本走候选条预编辑行通道,英文直通态(trigger off)按键不经 server,
  不产生匹配。
- 安装级部署(仅装工件+受控重启,不构建不测试):
  `scripts/deploy-menu-trigger.sh`,必传权威哈希
  `EXPECT_XIM_SHA/EXPECT_CORE_SHA/EXPECT_IBUS_SHA`;
  源取 `xim/build/bin/lyyime-xim` 与
  `$CARGO_TARGET_DIR/release/{liblyyime_core.so,ibus-engine-lyyime}`,
  原子 .new+mv 落位、.bak 保底、systemctl 同 unit 复会话环境重启 XIM、
  ibus restart 恢复原引擎、IBus 候选面板健康核验(须在总线上连续 5s
  持有 IBus.Panel;无主/不稳则以独立 lyyime-ibus-panel.service 补齐,
  该 unit 带 Restart=on-failure,已有稳定面板零改动)、config.toml
  字节级不动、失败自动回滚。
  现役 XIM 优先取 `lyyime-xim-sample.service` 的 MainPID;无托管时须显式
  `LYYIME_EXISTING_XIM_PID=<pid>` 指定桌面自动启动进程——经 exe 路径/
  cgroup 无 .service/uid/父进程 xfce4-session 四重核验才接受,由其它
  service 托管的 PID 一律拒绝直停;部署后该进程即被同名 transient unit
  接管(自动启动→托管化,后续部署无需再指定)。
- 本进程自身窗口(设置窗)内的输入框强制 `gtk-im-context-simple`:
  若沿用 xim 模块,realize 时会对自己的 XIM server 同步 `XOpenIM` 而
  主循环自锁(2026-09-30 gdb 栈实证)。因此设置窗输入框不能用本输入法
  打字(中文文本可粘贴);该限制仅作用本进程,不影响外部应用与其
  拉起的工具进程。
