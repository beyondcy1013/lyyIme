# lyyIme — Linux 五笔/拼音混合输入法

Linux(XFCE + X11)下类似搜狗体验的五笔/拼音混合输入法,采用**双模式**设计:

- **Mode A · ibus 引擎模式**:注册为标准 ibus 输入法引擎(Rust/zbus 直连 lyyime-core),与本机现有 ibus 1.5.29 集成。
- **Mode C · 悬浮窗独立输入(lyyime-float)**:Rust/GTK3 悬浮窗,不依赖 ibus/fcitx,双发送模式(直输/粘贴),支持自定义短语(精确码→任意长度文本,动态变量)。
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
7. **AI 助手**:中文态输入 `/AI`+提示词,回车调用用户自定义大模型(OpenAI 兼容接口),回复直接上屏;配置在 `config.toml` 的 `[ai]` 段/设置窗 AI 页,未配置时功能零介入。交互合同见 docs/ARCHITECTURE.md §11。(2026-09-06 新增:共享客户端 ibus-engine/engine/lyyime_ai.py,双模式同合同;改动触发逻辑必须先读该节并同步更新单测/E2E。)
8. **造词模式(2026-09-06 新增)**:上屏汉字后按造词热键(默认 Ctrl+=,`coin_hotkey` 可配置)把最近上屏的连续汉字组成五笔词组入库;`→`/`↑` 多选一字、`←`/`↓`/退格 少选一字,回车存词、Esc 取消;取码按五笔86 词组规则,存 `~/.local/share/lyyime/user_words.tsv`。交互合同见 docs/ARCHITECTURE.md §12;双模式同合同,改动必须先读该节并同步更新单测/E2E。
9. **词组效率提示(2026-09-06 新增,`phrase_hint` 默认开)**:上屏后最近 2–6 个汉字若存在更省键的五笔词组(词组编码键数 < 实敲字母数),core 在效果流末尾追加 `Effect::Hint`(词 + 编码),双模式在候选条/辅助区展示,**无定时**,保留到下一次输入才清除;刚用同码词组打过的不重复提示。合同见 ARCHITECTURE.md §3/§6。
10. **输入统计(2026-09-06 新增)**:上屏字数按日统计(`~/.local/share/lyyime/stats/`,core `stats` 模块;Mode A 与 Mode C 均写入);悬浮窗 lyyime-float 输入停顿 N 秒(默认 10,可设置)后在状态行显示「今日已输入 X 字 · 约 Y 字/分」,重新输入即还原;速度按活跃时长计算,超过 M 秒(默认 30,可设置)的空隙不计入。设置入口:悬浮窗菜单「输入统计设置…」。数据格式与统计口径见 docs/ARCHITECTURE.md §4。
11. **截屏(2026-09-06 新增)**:快捷键(默认 Ctrl+Alt+A,`shot_hotkey` 可配置)拉起 `lyyime-shot` 框选截屏:拖拽选区(冻结画面、Esc 取消、Enter 确认、双击整屏),PNG 存图片目录 + 复制剪贴板 + 桌面通知;独立 Rust/GTK3 程序,宿主只做热键拦截与拉起。交互合同见 docs/ARCHITECTURE.md §13;双模式同合同,改动必须先读该节并同步更新单测/E2E。
12. **快捷键冲突自动升级(2026-09-06 新增)**:造词/截屏快捷键与其它 lyyime 快捷键占用同一 `修饰+键` 组合时,按 原组合 → +Alt → +Alt+Shift 逐级自动避让(如 ctrl+equal → ctrl+alt+equal → ctrl+alt+shift+equal);设置窗保存时刚改动的一侧让位,生效值直接回写输入框并弹窗说明,写法非法报错还原;配置文件冲突在加载时自愈(截屏让位造词)并写日志;三级全占用人话提示请人工修改。合同见 docs/ARCHITECTURE.md §13.3;规格单源 core `hotkey.rs`,C 镜像 `keysym_map.c`,两端单测覆盖。
13. **候选右键菜单(2026-09-28 新增)**:右键候选行弹操作菜单——「固定首位/取消固定」(`pinned.tsv`,候选带「固」注释)、「删除词组」(`blocked.tsv` 屏蔽,再造该词自动解屏蔽)、「反查英文」(`data/runtime/zh_en.tsv`,命中替换候选页可正常上屏英文)、「自定义查询」(config.toml `custom_query_label`/`custom_query_url` 配置才显示,网址 `{q}` 代入词经 xdg-open 打开,宿主侧动作不经 core);三模式同合同,core `wordops` 单一语义源,Mode A 用候选区操作行替弹层、Mode B 真 GTK 菜单(候选窗已加悬停冻结才能点得到)、Mode C 按钮右键同菜单;功能键候选不弹菜单。交互合同见 docs/ARCHITECTURE.md §15;改动必须先读该节并同步更新单测/E2E。

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
crates/lyyime-shot/       ★截屏助手(合同 §13):GTK3 框选/整屏截屏,存图片目录+剪贴板+通知;被 Mode A/B 热键与托盘菜单拉起。
xim/                      ★Mode B 独立外挂 lyyime-xim(C 语言:IMdkit(vendor)+GTK3+dlopen(liblyyime_core.so);候选窗/托盘/设置窗/工具菜单)。入口 spike:xim/spike/ 最小 XIM server 连通验证。
crates/lyyime-float/      ★Mode C:Rust/GTK3 悬浮窗独立输入(五笔86 码表+自定义短语+动态变量;直输/粘贴双发送;**项目规则:主程序一律 Rust,Python 仅可用于测试**;e2e 见 crates/lyyime-float/e2e_phrases.sh)
ibus-engine/              ★Mode A:component XML、install.sh/uninstall.sh(引擎实现在 crates/lyyime-ibus,AI 客户端在 crates/lyyime-ai;**主程序一律 Rust,Python 仅可用于测试**)。
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

- 字母 a–z 进缓冲;`;` `'` 为二三重码选择;数字 1–9/0 选候选(0 = 第 10 个,页不足 10 条时吞键;每页条数 `page_size` 1–10 默认 10);Space 顶屏首选;Enter 上屏原始字母;Esc 清空;`-`/`=` 翻页;候选空时 Space/标点直通输出原始字母(中英混合);造词热键与方向键见 ARCHITECTURE.md §12/核心需求 #8。**四码首选上屏(默认开启,`commit_first_at_four` 可关)**:恰好四码且首选是五笔命中时免空格直接上屏首选,有重码也上屏第一个;首选是拼音/英文或缓冲命中功能键触发词时不触发。**四码唯一上屏(默认开启,`commit_unique_four` 可关)**:`commit_first_at_four` 关闭后回退判定——恰好四码且有中文命中的唯一候选时免空格上屏(英文候选不触发,§3/§6);四码顶屏(`commit_on_extra_after_four`)默认关闭。**死码保护**:有中文命中时,字母把缓冲推进完全无候选的死胡同则吞键、原样保留候选(功能键触发词前缀例外)。**词组效率提示(默认开启,`phrase_hint` 可关)**:上屏后最近几字有更省键的五笔词组时候选条提示词组与编码,保留到下一次输入(§3/§6)。
- **CapsLock 大写态字母直通英文,不进组词缓冲**(ARCHITECTURE.md §6):无 Shift 输出大写字母;大写态下 Shift+字母 由应用翻译输出小写字母;数字/标点不受影响。
- **全大写输入候选(2026-09-28 新增)**:中文态 Shift+字母 进组词缓冲(core 内部小写化组词,preedit 保留敲入大小写);全大写敲入时候选固定为 原样大写 → 首字母大写 → 全小写 → 中文翻译(候选 4 起,常用在前、5/6 后为其它常用翻译;`data/runtime/en_trans.tsv`,由 `dicttool entrans` 从 ECDICT 生成 + 内置缩略语人工校对表兜底),不混五笔/拼音;混合大小写回退小写普通通道。合同见 ARCHITECTURE.md §2/§3/§4/§6;双模式(Mode A/B)同合同,改动须同步单测(caps_test/ffi_test)与 E2E(run.sh B4b/B4c)。
- 中文态空缓冲时输出中文标点(可设置);英文态全部直通。
- Shift 单击(按下后无其它键、随后释放)切换中英;Shift+字母 组合不触发切换。
- **英文上屏去向(`enter_english` / `shift_english`,一处两键同配置,§6;XIM 设置窗「输入」页)**:回车默认**临时**(`temp`,上屏单个英文词后保持中文模式),可配 `en` 上屏并切英文模式(长久);组合中 Shift 默认**切英文模式**(`en`,上屏原串并转英文直通,再 Shift 单击切回),可配 `temp` 恢复"仅上屏"旧版行为。
- **精确单字按词频排位(默认开启,`exact_char_freq_rank` 可关,§5)**:四码/简码精确命中的单字不再无条件恒居首位——精确层内单字与词组同档、由词频定次序(不常用单字不再压高频词组),低频/生僻字再按频率降档让位高频词组;关闭恢复"单字恒居首位"旧行为(设置窗「输入」页勾选项)。
- **截屏热键(默认 Ctrl+Alt+A,`shot_hotkey` 可配置)**:命中即吞键拉起 `lyyime-shot`(ARCHITECTURE.md §13),不进组词缓冲;Mode A 中英态同效,Mode B 英文直通态不可见(托盘菜单兜底)。
- **快捷键冲突自动升级(核心需求 #12)**:造词/截屏快捷键被占用时按 原组合 → +Alt → +Alt+Shift 逐级避让(§13.3);设置窗保存回显生效值,加载时截屏让位造词。
- **候选右键(核心需求 #13,§15)**:右键候选行 → 固定首位/删除词组/反查英文/自定义查询(config.toml `custom_query_url` 已配置才显示第 4 项,`{q}` 代入词);Mode A 把候选区换成操作行用数字 1–4 或点选执行(Esc 还原),Mode B/C 为真弹出菜单;固定/屏蔽持久化在 `pinned.tsv`/`blocked.tsv`。

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
