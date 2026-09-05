# lyyIme — Linux 五笔/拼音混合输入法

类搜狗体验的五笔/拼音混打输入法,针对 Linux XFCE + X11(本机 openEuler 24.03)。
**双模式**,互为备份,解决"输入法框架底层总是出问题"的痛点:

| 模式 | 形态 | 依赖 | 适用 |
|---|---|---|---|
| **A · ibus 引擎** | 标准 ibus 输入法(python + Rust FFI) | ibus | 日常桌面,托盘切换 |
| **B · 独立外挂** | 单一可执行程序 `lyyime-xim`(类万能五笔外挂,最小 XIM server + GTK3 候选窗) | 无(只要 X11) | 框架坏掉时的兜底,GTK3/Xlib/终端可用 |

## 功能

- 五笔86 + 全拼/简拼**同一字母空间混打**,候选合并智能排序
- **Shift 单击**切换中/英文(组合键不误触)
- **中英混合**:中文态无匹配时出英文候选;高置信英文词自动直通上屏(可关)
- 用户词学习、简码、词组、翻页、数字选词、中文标点
- **设置界面**(候选数/标点/混合/学习/字体/自启)
- **工具菜单** → "**修复 Linux 中文输入法**"(诊断+一键修复环境变量/自启/引擎注册/缓存)

## 快速开始

```bash
scripts/build.sh                          # 构建(Rust 工作区)
crates/lyyime-dicttool ... dicttool convert && dicttool fetch && dicttool verify
ibus-engine/install.sh                    # 安装 Mode A(ibus 引擎)
scripts/install-app.sh                    # 安装 Mode B(独立外挂,含托盘)
lyyime-doctor check                       # 体检;lyyime-doctor fix --all 修复
```

## 文档

- [AGENTS.MD](AGENTS.MD) — 项目规则与并发协作纪律(开发者必读)
- [docs/PLAN.md](docs/PLAN.md) — 里程碑与验收标准
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — 架构与接口合同
- [docs/RESEARCH.md](docs/RESEARCH.md) — 调研结论
- [docs/TESTING.md](docs/TESTING.md) — 测试说明

## 状态(v0.1.0,已在本机部署)

| 项 | 结果 |
|---|---|
| Rust 单测 | core 97 + dicttool 14 + doctor 61 = 172 全绿 |
| ibus 引擎单测 | 26 条(桩库)+ 真库冒烟 全绿 |
| xim 单元/E2E | effects_json 16 + config 9;Xvfb 全链路 PASS |
| **双模式全链路 E2E** | `tests/e2e/run.sh` ✅(真库+真实词库,nihao→你好/英文直通/Shift 切换) |
| 真机部署 | `scripts/install-all.sh` 已装:引擎已注册、托盘/设置/工具菜单就绪 |

详见 [docs/PLAN.md](docs/PLAN.md)(里程碑全部完成)与 [docs/TESTING.md](docs/TESTING.md)。
