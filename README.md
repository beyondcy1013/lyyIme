<div align="center">

# lyyIme — Linux 五笔拼音混输输入法

**Wubi 86 + Pinyin Mixed-Chinese Input Method for Linux · IBus / XIM / Floating Window**

> 🐧 **为 Linux 而生,在 openEuler 24.03 + XFCE + X11 上实战打磨** —— 类搜狗手感的五笔拼音混输,框架异常还有兜底形态!

类搜狗手感的**五笔拼音混输** · 纯 Rust 核心 · 三种形态互为备份 · 候选窗皮肤 / 上屏联想 / AI 直通上屏 · 内置截屏 / 打字统计 · 一键修复中文输入环境

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Core: Rust](https://img.shields.io/badge/Core-Rust%20%2B%20C-orange.svg)](Cargo.toml)
[![Platform](https://img.shields.io/badge/Platform-Linux%20X11%20%C2%B7%20openEuler%2024.03-294172.svg)](docs/ARCHITECTURE.md)
[![Tests](https://img.shields.io/badge/Tests-Rust%20%2B%20C%20unit%20%C2%B7%20Xvfb%20E2E-lightgrey.svg)](docs/TESTING.md)

[GitHub](https://github.com/beyondcy1013/lyyIme) · [Gitee 镜像](https://gitee.com/beyondcy1013/lyyIme) · [English](#english)

</div>

## 为什么值得试试

- **同缓冲混输** — 五笔86、全拼、简拼、英文共用一个字母缓冲区,不切输入方案;也可设 `pinyin_only` 只用纯拼音
- **三形态互为备份** — ibus 引擎 / XIM 外挂 / 悬浮窗输入共用同一 Rust 核心与词库,框架异常时换形态继续打
- **日常打字全本地** — 正常输入零网络调用;AI 助手与在线词库下载均为独立可选功能,不开不影响打字
- **可调可管** — 候选右键固定/删除/反查、5 套内置皮肤、标点/学习/混输开关即时生效
- **键盘行为考究** — Shift 单击切中英不误伤组合键;英文态锁存 CapsLock 时单击确认回中文联动解除系统大写锁(已实现,隔离 Xvfb GUI 验证通过后方部署)
- **工具箱内建** — 截屏助手(XFCE 桌面级热键)/ 造词 / 打字统计 / 输入法环境一键修复

## 这是什么

lyyIme 是一个为 Linux 打造的**五笔86 与拼音混输**输入法,支持基线为 **openEuler 24.03 + XFCE + X11**(其它发行版/桌面未验证,不承诺 Wayland 原生支持)。五笔、全拼、简拼、英文共用同一个字母缓冲区,**不用切换输入方案、不打模式开关**,候选智能合并排序,接近搜狗/万能五笔在 Windows 上的手感。

除了日常输入,它也为**输入法框架异常时的应急输入**提供备选形态:

| 形态 | 是什么 | 依赖 | 适用场景 |
|---|---|---|---|
| **A · ibus 引擎** | 标准 ibus 输入法(Rust/zbus 直连核心) | ibus | 日常桌面,托盘一键切换 |
| **B · XIM 独立外挂** | 单一可执行程序 `lyyime-xim`(最小 XIM server + GTK3 候选窗) | X11 + GTK3/XIM 兼容应用 | ibus/fcitx 异常时的兜底 |
| **C · 悬浮窗输入** | `lyyime-float` 独立悬浮窗(类万能五笔外挂) | 目标应用可接收键盘输入/剪贴板 | 绕开系统输入法框架 |

三种形态共用同一套 Rust 核心与词库,一种不可用时换另一种兜底。

## 功能亮点

### 🆕 最新亮点(近期更新)

- **候选窗皮肤画廊** — 5 套内置皮肤:跟随系统 / 樱花奶糖 🌸 / 青岚竹影 🎋 / 暮紫星河 🌌 / 霓虹夜航 🌃,设置窗卡片预览所见即所得,自动跟随系统明暗
- **上屏后联想** — 打完"你"立刻提示"好/们",空格续选只上屏尾巴;纯本地词表,零网络调用
- **全大写输入** — 中文态按住 Shift 敲 `WHO`,候选给出 原样大写 / 首字母大写 / 全小写 / **中文翻译**(WHO→世界卫生组织、CPU→中央处理器,ECDICT 词义库驱动)
- **悬浮窗「模拟内置」模式** — 候选窗贴着目标应用光标走(无障碍树光标追踪),还提供「不抢焦点」开关:目标窗口全程不丢焦,外挂输入法也有内置般体验
- **悬浮窗撤销/重做** — `Ctrl+Z` / `Ctrl+Y`,空缓冲时直通目标窗口,**撤回的正是刚上屏的文本**
- **常用字优先** — 五笔单字按 **GB2312 分档**排序(一级常用 3755 字 > 二级次常用 3008 字),生僻字/繁体沉底但翻页仍可选
- **候选右键菜单** — 固定首位 / 删除词组 / 中英反查,词库越用越顺手
- **菜单触发** — 上屏文本命中功能名即持续提示,显式功能键一键执行,误打不误触
- **输入状态联动** — 悬浮窗实时显示内置输入法状态(内置:中 / EN / 停用 / 未运行),三形态配合一目了然
- **IBus 自持 GTK 候选菜单** — 候选条右键即置顶/删除/反查/通用操作,不依赖 ibus 面板菜单
- **纯拼音模式** — `pinyin_only` 一开五笔候选整体退场,只走全拼/简拼
- **`Ctrl+.` 标点即时切换** — 中文态临时切中英文标点,不改默认配置
- **Shift 单击 CapsLock 联动** — 英文态锁存大写锁时,单击确认回中文联动解除系统锁;组合键与反向切换不动锁(已实现,GUI 验证后方部署)
- **截屏热键 XFCE 全局化** — `Ctrl+Alt+A` 登记为桌面级绑定,换输入法/停用 lyyIme 仍有效;与系统快捷键冲突时提示且不覆盖

### 输入核心:拼音五笔真正混输

- **五笔86 + 全拼/简拼同一字母空间混打**——`wqvb` 出"你好",`nihao` 也出"你好",候选按「五笔精确 → 拼音完整切分 → 简拼/前缀 → 用户词学习 → 英文兜底」合并排序
- **拼音反查五笔码**:候选注释显示五笔编码,拼音用户边打边学五笔
- **四码唯一上屏**:五笔四码打满且候选唯一时免空格直接上屏(默认开,可关);另有"四码顶屏"(满四码后继续输入先上屏当前词)可选
- **中英混合输入**:中文态无匹配时出英文候选;高置信英文词自动直通上屏(可关)
- **Shift 单击**切换中/英文——组合键里的 Shift 不误触
- 用户词学习、简码、词组、翻页、数字选词、中文标点(默认开,输出 ，。？！)

### 效率功能

- **造词模式**:上屏后按 `Ctrl+=` 进入造词,方向键多选/少选汉字,回车存入用户词库并**自动按五笔86 词组规则编码**,之后直接打该编码
- **动态日期/时间词**:输入 `riqi`/`shijian` 直接触发当天日期、当前时间候选
- **词组效率提示**:上屏后若最近几字有更省键的五笔词组,候选条提示词组与编码
- **上屏后联想**:中文上屏后,候选条立刻给出本地词表里"接下来可输入的词尾"(如打完"你"提示"好/们"),空格/数字/鼠标续选只上屏尾巴;纯本地词表,无网络调用,默认关闭(设置→输入 可开启)
- **打字统计**:按天记录字数,停顿时显示「今日已输入 N 字 · 约 M 字/分」;速度按**活跃打字时长**计算,思考/离开的空隙不计入
- **快捷键冲突自动让位**:造词/截屏等 lyyime 内部热键互撞时,按 原组合 → +Alt → +Alt+Shift 逐级让位并给出说明;XFCE 全局截屏绑定与系统快捷键冲突时提示、不覆盖

### AI 助手(OpenAI 兼容)

- 中文态输入 `/AI` + 提示词,回车调用你自定义的大模型(任何 OpenAI 兼容接口,API 地址/密钥/模型自行配置,不承诺特定厂商兼容性),**回复直接上屏**
- 提示词里可以正常打中文(组词结果进提示词);设置窗可视化配置 API 地址/密钥/模型并测试连接

### 快速功能键(候选条里的快捷入口)

- 输入 `peizhi` 选候选即**打开设置**、`bangzhu` 打开**帮助**、`jietu` 直接**唤起截屏**(选词即提示热键)
- 还可自定义最多 8 个触发词,绑定内置功能或任意 shell 命令;功能候选紧跟首选显示,不影响正常打字,数字/鼠标/空格确认才执行

### 工具箱

- **截屏助手**:`Ctrl+Alt+A` 框选截屏——冻结画面拖拽选区,Enter 确认、Esc 取消、双击整屏;
  PNG 自动存图 + 复制到剪贴板 + 桌面通知。快捷键登记为 XFCE 桌面级全局绑定:
  所有输入法及英文键盘下均可使用,换用其它输入法或停用 lyyIme 后仍然有效;
  设置→快捷键页可改(写法同造词键,如 `ctrl+alt+a`),保存即替换旧绑定,
  与系统快捷键冲突时提示不覆盖。作用域:XFCE(X11)
- **修复 Linux 中文输入法**(托盘菜单):10 项诊断(环境变量三件套/会话总线/引擎注册/自启/缓存)+ 一键修复——Linux 中文输入法环境的"120 救援车"
- **输入法管理**:图形化增删 ibus 输入法、设默认
- **设置界面**:候选数/标点/混合模式/学习开关/字体/自启,改完即时生效
- **候选窗皮肤画廊**:多套内置主题卡片预览,所见即所得,支持跟随系统明暗
- 候选窗自适应系统**明暗主题**;XIM **主窗口**提供实时状态行 + 设置/直输/工具箱一键入口

## 适合哪些使用场景

| 场景 | 推荐形态 / 功能 |
|---|---|
| 日常桌面中文输入,想要搜狗式五笔拼音混输 | Mode A · ibus 引擎 |
| ibus/fcitx 框架异常时的应急兜底 | Mode B · XIM 独立外挂(X11 + GTK3/XIM 兼容应用) |
| 想绕开系统输入法框架 | Mode C · 悬浮窗输入(目标应用可收键/剪贴板) |
| 五笔为主、拼音反查边打边学 | 同缓冲混输 + 候选注释五笔码 |
| 只用拼音、不要五笔候选 | `pinyin_only` 纯拼音模式 |
| 中文输入环境损坏需要诊断修复 | lyyime-doctor 一键体检/修复 |
| 本地离线为主、偶尔接大模型 | 本地引擎 + `/AI` 自选 OpenAI 兼容接口 |

## 快速开始

```bash
# 1) 依赖:Rust(2021 edition)、GTK3、ibus(可选,Mode A 用);五笔码表来自
#    ibus-table-wubi(海峰86):/usr/share/ibus-table/tables/wubi-haifeng86.db

# 2) 构建(Rust 工作区 + xim C)
scripts/build.sh

# 3) 生成并校验词库(ibus 码表 → TSV,联网补充拼音词组/英文词频)
dicttool convert && dicttool fetch && dicttool verify
#    另有 data/pinyin_supplement.tsv:少量人工校对词组随核心内嵌发布,
#    仅当词组文件非空且主词库缺该词时补入(权重为人工值,非语料频次)。

# 4) 安装:构建 → 词库 → 核心库 → Mode A + Mode B → 体检
#    注意:脚本按 openEuler 环境假定 /usr/local 与 /data/cargo-target 等
#    路径,需要相应写权限;部署类脚本还要求与运行中输入法会话一致的
#    环境(DISPLAY/会话总线)。其它发行版的路径/权限需自行适配。
scripts/install-all.sh          # IBus 默认用户级注册;核心/工具仍写 /usr/local;--no-xim/--no-ibus 可选装

# 5) Mode C 悬浮窗(可选)
crates/lyyime-float/install.sh  # 应用菜单出现 "lyyIme 悬浮窗输入"

# 6) 体检/修复
lyyime-doctor check             # 10 项诊断
lyyime-doctor fix --all         # 一键修复(环境变量/总线/引擎注册/缓存)
```

安装后:托盘出现"中/EN"图标,`Shift` 单击切中英;托盘右键菜单有设置/截屏/造词/修复工具。

## 架构一览

| 组件 | 语言 | 说明 |
|---|---|---|
| `lyyime-core` | Rust | 输入引擎核心(混输排序/词库/用户词/AI/造词/统计),C ABI FFI |
| `lyyime-ibus` | Rust | ibus 引擎(zbus 直连,Mode A) |
| `xim/` | C | 独立 XIM server 外挂 `lyyime-xim`(xcb-imdkit + GTK3 候选窗,Mode B) |
| `lyyime-float` | Rust | 悬浮窗输入(单实例,焦点自动回传,Mode C;中文标点直上,空缓冲退格/删除直通) |
| `lyyime-ai` / `lyyime-shot` | Rust | AI 助手 / 截屏助手 |
| `lyyime-doctor` | Rust | 诊断修复 CLI(输入法管理/环境修复/真屏探测) |
| `lyyime-dicttool` | Rust | 词库工具(ibus 码表转换/联网抓取/校验/查询) |

详见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)(架构与接口合同)。

## 文档

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — 架构与接口合同(14 节)
- [docs/PLAN.md](docs/PLAN.md) — 里程碑与验收标准
- [docs/RESEARCH.md](docs/RESEARCH.md) — 调研结论与踩坑实录
- [docs/TESTING.md](docs/TESTING.md) — 测试说明
- [AGENTS.MD](AGENTS.MD) — 开发者协作纪律

## 质量

项目提供 Rust/C 单元测试与隔离 Xvfb 输入验证。以下列出覆盖范围和复现入口;最新结果以对应运行日志为准:

| 覆盖 | 命令 |
|---|---|
| Rust 单测(core/ibus/dicttool/doctor/float/shot) | `scripts/test.sh` |
| xim C 单测(effects_json/config/快捷键让位/功能键) | `make -C xim test` |
| XIM 全链路 E2E,聚焦 CapsLock 套件(隔离 Xvfb) | `tests/e2e/xim_e2e.sh --caps-only --keep` |
| IBus 候选窗/菜单 E2E(隔离 Xvfb) | `tests/e2e/ibus_candwin_e2e.sh --keep` |
| 候选菜单真核心 E2E(隔离 Xvfb) | `tests/e2e/candidate_menu_e2e.sh --keep` |
| 真机使用 | openEuler 24.03 + XFCE + X11 日常使用中 |

## English

**lyyIme** is a mixed **Wubi-86 + Pinyin** input method for Linux, developed and used daily on **openEuler 24.03 + XFCE + X11** (other distros/desktops unverified; no native Wayland support promised). Sogou-like experience: Wubi, full pinyin, abbreviated pinyin and English share one letter buffer — no scheme switching, candidates merged and ranked intelligently; an optional `pinyin_only` mode hides Wubi candidates entirely. Ships in **three interchangeable shapes** sharing one Rust core and dictionary: a standard **IBus engine**, a **standalone XIM server** (`lyyime-xim`, needs X11 plus GTK3/XIM-compatible apps — a fallback when your IM framework breaks), and a **floating-window IME** (`lyyime-float`, needs the target app to accept synthetic keys/clipboard) — if one shape is unavailable, switch to another.

Highlights: Wubi↔Pinyin mixed typing with reverse Wubi-code hints · optional `pinyin_only` pure-pinyin mode · unique-4-key auto-commit & 4-key top-commit · **GB2312-tiered character ranking** (common-first) · **post-commit word prediction** (local, offline) · **ALL-CAPS input** with candidates for UPPER / Capitalized / lower / **Chinese translation** (`WHO` → 世界卫生组织) · candidate **right-click menu** (pin top / delete phrase / CN↔EN lookup), including self-owned GTK menus on the IBus frontend · **candidate-window skins** (5 built-in themes, WYSIWYG gallery, follows dark/light) · Chinese/English mixing with auto English pass-through · single-Shift CN/EN toggle plus CapsLock auto-unlock on confirmed EN→CN switch (implemented, deployed only after isolated-Xvfb GUI validation) · `Ctrl+.` instant punctuation toggle · word-coining (auto Wubi-86 phrase encoding) · dynamic date/time words · typing statistics (chars/day, active-time WPM) · floating-window **caret-following "built-in-like" mode** with optional **no-focus-stealing** and **undo/redo that reverts committed text in the target app** · built-in **AI assistant** (`/AI` prompt → any OpenAI-compatible endpoint you configure, answer committed in place) · configurable quick actions in the candidate bar (`peizhi`/`bangzhu`/`jietu` or your own shell commands) · region screenshot tool with an **XFCE desktop-level hotkey binding** (conflicts are reported, never overwritten) · **lyyime-doctor**: 10-point diagnose-and-repair for broken Linux Chinese input environments. Core engine in pure Rust (C-ABI FFI), candidate UI in GTK3. Rust + C unit tests plus Xvfb end-to-end suites — see docs/TESTING.md for coverage and commands.

```bash
# assumes openEuler paths (/usr/local, /data/cargo-target), matching
# session env, and the required write permissions
scripts/build.sh && scripts/install-all.sh   # build + install (IBus engine + XIM standalone)
lyyime-doctor check && lyyime-doctor fix --all
```

Keywords: Linux Chinese input method, openEuler 24.03, Wubi 86, Wubi-Pinyin mixed input, IME, IBus engine, XIM server, floating window IME, GTK3, Rust, FFI, XFCE, candidate skins, word prediction, 五笔输入法, 拼音输入法, 五笔拼音混输, 中文输入法, 输入法皮肤, 上屏联想, openEuler 输入法.

## License

[MIT](LICENSE) · Gitee 镜像:[https://gitee.com/beyondcy1013/lyyIme](https://gitee.com/beyondcy1013/lyyIme)
