<div align="center">

# lyyIme — Linux 五笔拼音混输输入法

**Wubi 86 + Pinyin Mixed-Chinese Input Method for Linux · IBus / XIM / Floating Window**

类搜狗手感的**五笔拼音混输** · 纯 Rust 核心 · 三种形态互为备份 · 内置 AI 助手 / 截屏 / 打字统计 · 一键修复中文输入环境

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Core: Rust](https://img.shields.io/badge/Core-Rust%20%2B%20C-orange.svg)](Cargo.toml)
[![Platform](https://img.shields.io/badge/Platform-Linux%20X11%20(XFCE%2FGNOME)-blue.svg)](docs/ARCHITECTURE.md)
[![Tests](https://img.shields.io/badge/Tests-319%20Rust%20%2B%20C%20passing-brightgreen.svg)](docs/TESTING.md)

[GitHub](https://github.com/beyondcy1013/lyyIme) · [Gitee 镜像](https://gitee.com/beyondcy1013/lyyIme) · [English](#english)

</div>

## 这是什么

lyyIme 是一个为 Linux(X11)打造的**五笔86 与拼音混输**输入法。五笔、全拼、简拼、英文共用同一个字母缓冲区,**不用切换输入方案、不打模式开关**,候选智能合并排序,接近搜狗/万能五笔在 Windows 上的手感。

它同时解决 Linux 中文输入法的老大难问题——**输入法框架底层总出坏**:

| 形态 | 是什么 | 依赖 | 适用场景 |
|---|---|---|---|
| **A · ibus 引擎** | 标准 ibus 输入法(Rust/zbus 直连核心) | ibus | 日常桌面,托盘一键切换 |
| **B · XIM 独立外挂** | 单一可执行程序 `lyyime-xim`(最小 XIM server + GTK3 候选窗) | 只要有 X11 | ibus/fcitx 坏掉时的兜底,GTK3/Xlib/终端可用 |
| **C · 悬浮窗输入** | `lyyime-float` 独立悬浮窗(类万能五笔外挂) | 无框架依赖 | 完全绕开系统输入法,点哪打哪,焦点自动回传 |

三种形态共用同一套 Rust 核心与词库,一种坏了换另一种,**永远有办法打出中文**。

## 功能亮点

### 输入核心:拼音五笔真正混输

- **五笔86 + 全拼/简拼同一字母空间混打**——`wqvb` 出"你好",`nihao` 也出"你好",候选按「五笔精确 → 拼音完整切分 → 简拼/前缀 → 用户词学习 → 英文兜底」合并排序
- **拼音反查五笔码**:候选注释显示五笔编码,拼音用户边打边学五笔
- **四码唯一上屏**:五笔四码打满且候选唯一时免空格直接上屏(默认开,可关);另有"四码顶屏"(满四码后继续输入先上屏当前词)可选
- **中英混合输入**:中文态无匹配时出英文候选;高置信英文词自动直通上屏(可关)
- **Shift 单击**切换中/英文——组合键里的 Shift 不误触
- 用户词学习、简码、词组、翻页、数字选词、中文标点(默认开,输出 ，。？！;中文态 `Ctrl+.` 临时切换中英文标点不改默认值,设置→常规 可关)

### 效率功能

- **造词模式**:上屏后按 `Ctrl+=` 进入造词,方向键多选/少选汉字,回车存入用户词库并**自动按五笔86 词组规则编码**,之后直接打该编码
- **动态日期/时间词**:输入 `riqi`/`shijian` 直接触发当天日期、当前时间候选
- **词组效率提示**:上屏后若最近几字有更省键的五笔词组,候选条提示词组与编码
- **上屏后联想**:中文上屏后,候选条立刻给出本地词表里"接下来可输入的词尾"(如打完"你"提示"好/们"),空格/数字/鼠标续选只上屏尾巴;纯本地词表,无网络调用,默认关闭(设置→输入 可开启)
- **打字统计**:按天记录字数,停顿时显示「今日已输入 N 字 · 约 M 字/分」;速度按**活跃打字时长**计算,思考/离开的空隙不计入
- **快捷键冲突自动升级**:造词/截屏热键与系统或其它应用冲突时,自动 +Shift 逐级让位,并给出说明

### AI 助手(OpenAI 兼容)

- 中文态输入 `/AI` + 提示词,回车调用你自定义的大模型(DeepSeek / 通义千问 / 智谱 GLM / Kimi / Ollama 本地模型等任何 OpenAI 兼容接口),**回复直接上屏**
- 提示词里可以正常打中文(组词结果进提示词);设置窗可视化配置 API 地址/密钥/模型并测试连接

### 快速功能键(候选条里的快捷入口)

- 输入 `peizhi` 选候选即**打开设置**、输入 `bangzhu` 打开**帮助**;还可自定义最多 8 个触发词,绑定内置功能或任意 shell 命令
- 功能候选紧跟首选显示,不影响正常打字;数字/鼠标/空格确认才执行

### 工具箱

- **截屏助手**:`Ctrl+Alt+A` 框选截屏——冻结画面拖拽选区,Enter 确认、Esc 取消、双击整屏;PNG 自动存图 + 复制到剪贴板 + 桌面通知
- **修复 Linux 中文输入法**(托盘菜单):10 项诊断(环境变量三件套/会话总线/引擎注册/自启/缓存)+ 一键修复——Linux 中文输入法环境的"120 救援车"
- **输入法管理**:图形化增删 ibus 输入法、设默认
- **设置界面**:候选数/标点/混合模式/学习开关/字体/自启,改完即时生效
- 候选窗自适应系统**明暗主题**

## 与常见方案对比

| | 五笔拼音混输 | 框架坏了能兜底 | 一键修复环境 | AI 直通上屏 | 技术栈 |
|---|---|---|---|---|---|
| **lyyIme** | ✅ 同缓冲混打 | ✅ A/B/C 三形态 | ✅ lyyime-doctor | ✅ `/AI` | Rust 核心 + C/XIM |
| ibus 自带五笔/拼音 | ❌ 单方案二选一 | ❌ 依赖 ibus | ❌ | ❌ | C |
| fcitx5 + rime | 需自行定制方案 | ❌ 依赖 fcitx | ❌ | ❌ | C++ |
| 搜狗/百度输入法 Linux | ✅ | ❌ | ❌ | ❌ | 闭源商业,依赖特定桌面 |

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

# 4) 一键安装:构建 → 词库 → 核心库 → Mode A + Mode B → 体检
scripts/install-all.sh          # 用户级;--system 系统级;--no-xim/--no-ibus 可选装

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

| 项 | 结果 |
|---|---|
| Rust 单测 | **319 项全绿**(core/ibus/dicttool/doctor/float/ai) |
| xim C 单测 | effects_json 18 + config 47(含快捷键冲突自动升级/快速功能键),全绿 |
| 双模式全链路 E2E | `tests/e2e/run.sh` ✅(真库+真实词库,Xvfb 全链路) |
| 真机部署 | openEuler 24.03 + XFCE + X11 日常使用中 |

## English

**lyyIme** is a mixed **Wubi-86 + Pinyin** input method for Linux (X11), aiming for a Sogou-like experience. Wubi, full pinyin, abbreviated pinyin and English share one letter buffer — no mode switching, candidates merged and ranked intelligently. Ships in **three interchangeable shapes**: a standard **IBus engine**, a dependency-free **standalone XIM server** (`lyyime-xim`, works when your input-method framework breaks), and a **floating-window** IME (`lyyime-float`).

Highlights: Wubi↔Pinyin mixed typing with reverse Wubi-code hints · unique-4-key auto-commit · Chinese/English mixing with auto English pass-through · single-Shift CN/EN toggle · word-coining (auto Wubi-86 phrase encoding) · dynamic date/time words · typing statistics (chars/day, active-time WPM) · built-in **AI assistant** (`/AI` prompt → any OpenAI-compatible LLM, answer committed in place) · configurable quick actions in the candidate bar (`peizhi`/`bangzhu` or your own shell commands) · region screenshot tool · **lyyime-doctor**: 10-point diagnose-and-repair for broken Linux Chinese input environments. Core engine in pure Rust (C-ABI FFI), candidate UI in GTK3, dark/light theme aware. 319 Rust unit tests + C unit tests + end-to-end Xvfb suite, deployed on openEuler 24.03 (XFCE).

```bash
scripts/build.sh && scripts/install-all.sh   # build + install (IBus engine + XIM standalone)
lyyime-doctor check && lyyime-doctor fix --all
```

Keywords: Linux Chinese input method, Wubi 86, Wubi-Pinyin mixed input, IME, IBus engine, XIM, GTK3, Rust, FFI, openEuler, XFCE, 五笔输入法, 拼音输入法, 五笔拼音混输, 中文输入法.

## License

[MIT](LICENSE) · Gitee 镜像:[https://gitee.com/beyondcy1013/lyyIme](https://gitee.com/beyondcy1013/lyyIme)
