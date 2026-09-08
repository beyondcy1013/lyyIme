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
│   ├── tray.c          Gtk.StatusIcon 托盘 + 右键菜单(主窗口/设置/工具)
│   ├── mainwin.c       主窗口:状态行 + 输入设置/直输模式/工具箱入口(--mainwin/SIGUSR2 唤起)
│   ├── settings.c      GtkBuilder 设置对话框(保存即生效:重建引擎+刷 CSS)
│   ├── tools.c         工具动作(主窗口与托盘共用;含直输模式窗口 lyyime-float 拉起)
│   ├── config.c        ~/.config/lyyime/config.toml 平面键值 TOML 子集(保留未知行与注释)
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
3. **Shift 行为**:按下即喂 core——有缓冲立即上屏英文原串;空缓冲吞键并挂起
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
   已存在实例(弹设置窗)后自身退出。
9. **commit 编码**:UTF-8 → COMPOUND_TEXT(`xcb_utf8_to_compound_text`,依赖
   客户端 locale 为 UTF-8;E2E 用 zh_CN.utf8)。

## 验证

```bash
make -C xim spike spike-run   # spike:Xvfb :98,server commit「你好尖兵」进 Entry
make -C xim test              # 单测:effects_json(16 例)+ config(9 例)+ 桩库
bash tests/e2e/xim_e2e.sh     # 全链路:数字选词/Shift 切换/英文直通/空格顶屏
make -C xim e2e               # 同上(make 入口)
```

## 已知局限

- 覆盖面:GTK3(XIM immodule)/Xlib/Xaw 类应用;Qt5 无 XIM 支持,走 Mode A
  (docs/RESEARCH.md §2.2)。
- trigger 状态随新焦点重置为中文态(XIM per-IC 语义);英文态偏好不跨应用记忆。
- 大写字母直通不进组词缓冲(§6 未定义,取主流行为)。
- 托盘需 XEmbed systray 宿主(xfce4-panel);无宿主时仅无图标,功能不受影响。
- 时间窗单击判定(280ms)在"按住 Shift 超 280ms 再单点输入法"场景会误判为
  单击——但该场景随后必有字母键取消,仅纯按住 Shift 不动时不切换,影响可忽略。
