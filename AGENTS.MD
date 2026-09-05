# lyyIme — Linux 五笔/拼音混合输入法

Linux(XFCE + X11)下类似搜狗体验的五笔/拼音混合输入法,采用**双模式**设计:

- **Mode A · ibus 引擎模式**:注册为标准 ibus 输入法引擎(Python + Rust FFI),与本机现有 ibus 1.5.29 集成。
- **Mode B · 独立外挂模式**:`lyyime-xim` 独立可执行程序(类 Windows 万能五笔外挂),最小 XIM server(IMdkit)+ GTK3 候选窗,**完全不依赖 ibus/fcitx 框架**——框架"底层总是出问题"时的兜底。架构决策与实证记录见 docs/RESEARCH.md §2(结论:被动 grab 回放在本机不可靠,弃用;XIM 为正统路线)。

## 核心需求(验收基准)

1. 五笔86 + 全拼/简拼混打,单一字母空间(万能五笔式),候选合并排序。
2. **Shift 单击**切换中/英文态(按住 Shift 组合键不误触)。
3. **中英混合**:中文态下无中文匹配时,候选给出英文词;直接回车/空格输出原字母;高置信英文词可自动直通上屏(可设置)。
4. 有**设置界面**(候选数、标点、混合开关、学习开关、字体、开机自启等)。
5. 有**工具菜单**,内含:
   - "**修复 Linux 中文输入法**"(诊断/修复环境变量、autostart、ibus 注册与重启、缓存清理);
   - "**输入法管理**":列出本机全部输入法引擎(ibus/fcitx5/lyyime 双模式),**安装(增)、卸载(删)、设为默认**,包级操作走 dnf,全程 dry-run 预览 + 二次确认,绝不误删 lyyime 自身或最后一个可用输入法。
6. 目标系统=本机(openEuler 24.03 XFCE,root 与 beyondcy 双用户,当前 ibus 在跑)。安装路径以 `/usr/local` 与 `~/.local` 为主,不污染系统包管理。

## 目录结构(文件所有权,并发 Agent 不得越界写)

```
AGENTS.MD                 本文件(项目规则,所有人可读,改前需说明)
docs/PLAN.md              里程碑计划与验收标准
docs/ARCHITECTURE.md      架构 + 接口合同(核心 API/FFI/数据格式,实现必须以此为准)
docs/RESEARCH.md          调研结论汇总
docs/EXPERIENCE-INPUT-METHOD.md  输入法排障与实现经验(2026-09-05 xrdp/ibus 故障复盘、码表结构、悬浮外挂设计依据;Mode B 设计前必读)
docs/TESTING.md           测试说明
Cargo.toml                workspace 根(勿动结构)
crates/lyyime-core/       ★纯逻辑引擎:码表加载、五笔/拼音/英文匹配排序、学习、FFI(cdylib)。无 GUI/无 X11 依赖,必须可 headless 测试。
crates/lyyime-dicttool/   ★词典管线 CLI:本地海峰86 sqlite → runtime TSV;下载转换拼音词组/英文词频。
crates/lyyime-doctor/     ★诊断修复:lib(供 GUI 调用)+ bin CLI。检查/修复 IME 环境。
xim/                      ★Mode B 独立外挂 lyyime-xim(C 语言:IMdkit(vendor)+GTK3+dlopen(liblyyime_core.so);候选窗/托盘/设置窗/工具菜单)。入口 spike:xim/spike/ 最小 XIM server 连通验证。
floatapp/                 ★悬浮窗模式参考实现(Python+GTK3,零依赖即用,双发送模式直输/粘贴,已真机验证;经验文档见 docs/EXPERIENCE-INPUT-METHOD.md)
ibus-engine/              ★Mode A:python 引擎(engine/lyyime.py)、component XML、install.sh/uninstall.sh。
dicts/                    原始词典缓存(大文件不入 git)与下载脚本 tools/。
data/runtime/             dicttool 产物(wubi.tsv 等),构建时生成,不入 git。
scripts/                  build.sh / test.sh / e2e.sh / install-all.sh
tests/e2e/                Xvfb + xdotool 端到端测试。
```

## 硬性构建规则(继承自 /home/codes/AGENTS.MD)

- **CARGO_TARGET_DIR 必须为 `/data/cargo-target/local/lyyIme`**(目录已建)。一律通过 `scripts/build.sh` / `scripts/test.sh` 执行,脚本已内置该变量;手动 cargo 前自行 export。
- crates.io 走 USTC 镜像(`~/.cargo/config.toml` 已配),不要改镜像配置。
- 新增依赖需在 crate 内 `Cargo.toml` 声明并锁定版本;能用 std/本地库解决的不引第三方。

## 环境事实(已验证,直接采信)

- XFCE + X11,`DISPLAY=:11.0`,无 Wayland。GTK3-devel 3.24.52 已装,无 Qt。
- Rust 1.98 / GCC 12.3 / cmake 4.4 / Python 3.11。
- 本机 ibus 1.5.29 运行中(root 与 beyondcy 各一套),含 ibus-table、ibus-libpinyin、海峰86/极点86 码表。
- `/usr/share/ibus-table/tables/wubi-haifeng86.db`(SQLite,表 ime/goucima/pinyin/suggestion/phrases)是主码表来源。
- Xvfb、xdotool、wmctrl、libXtst-devel(XRecord 所在库)已装,E2E 可全自动。
- 网络可达 github / mirrors.ustc.edu.cn。

## 键盘行为规范(摘要,细则见 ARCHITECTURE.md)

- 字母 a–z 进缓冲;`;` `'` 为二三重码选择;数字 1–9 选候选;Space 顶屏首选;Enter 上屏原始字母;Esc 清空;`-`/`=` 翻页;候选空时 Space/标点直通输出原始字母(中英混合)。
- 中文态空缓冲时输出中文标点(可设置);英文态全部直通。
- Shift 单击(按下后无其它键、随后释放)切换中英;Shift+字母 组合不触发切换。

## 测试政策(必须遵守)

- `lyyime-core` 单测覆盖:五笔码查全/简码/词组、拼音全拼/简拼/分段、中英混合直通、标点、翻页、学习排序回归。跑 `scripts/test.sh` 必须全绿。
- dicttool 转换产物有行数/抽样校验子命令(`dicttool verify`)。
- E2E(`tests/e2e/run.sh`):Xvfb 虚拟屏内启动 lyyime-app,xdotool 模拟按键,断言候选窗内容与上屏文本,不经人工。
- 任何行为改动先补/改测试再改实现。

## 质量红线(用户明确要求:结束前完善、精致,不留后期补工)

- **交付即成品**:实现的功能必须真实可用,禁止 TODO/占位/半成品交付;每个 CLI 参数、每个菜单项、每个设置项都要实际能跑。
- **借鉴注明出处**:动手前先读 docs/RESEARCH.md 调研结论;凡借鉴成熟实现(ibus-table 引擎、xremap、yong/小小输入法、fcitx5/libime、rime、搜狗/万能五笔交互习惯等),在代码注释或 RESEARCH.md 标注"借鉴自 <项目/URL>"。
- **真机验证才算完成**:单测之外,每条用户可见功能必须在本机或 Xvfb 实测通过(留命令输出/xdotool 断言);"编译通过"不算完成。
- **UI 精致**:候选窗/设置窗/托盘菜单对齐 XFCE 风格(跟随系统主题与字体、合理间距),中英文文案统一;候选窗交互对齐主流输入法(序号高亮首选、编码提示注释、翻页指示)。
- **边界完善**:错误处理给"人话"提示;写日志(~/.local/share/lyyime/logs/);异常退出不残留中间状态;每个安装脚本配套 uninstall。

## 并发协作纪律

- 只写自己所有权目录;公共合同(docs/ARCHITECTURE.md、本文件)只读,需变更先在最终汇报中提出理由。
- 每完成一个模块:跑通测试 → 在 docs/RESEARCH.md 或模块 README 补关键决策 → 汇报"做了什么/怎么验证的/遗留问题"。
- 汇报用中文;提交信息格式 `feat(lyyIme): ...` / `fix(lyyIme): ...` / `test(lyyIme): ...`。
