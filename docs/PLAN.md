# lyyIme 实施计划

> 执行方式:多 Agent 并行。每个里程碑标注**所有者**与**验收标准**。接口合同见 ARCHITECTURE.md。

## 阶段总览

| 阶段 | 内容 | 所有者 | 依赖 |
|---|---|---|---|
| M0 | 调研 + 文档 + 骨架 | 主控 | - |
| M1 | lyyime-core 引擎核心 + 单测 | Agent-1 | M0(合同已定) |
| M2 | dicttool 词库管线 + runtime 数据生成 | Agent-2 | M0(数据格式合同已定) |
| M3 | lyyime-doctor 诊断修复 lib+bin | Agent-3 | M0 |
| M4 | Mode A:ibus python 引擎 + 注册安装 | Agent-4 | M1(FFI so) |
| M5 | Mode B:lyyime-app 独立外挂 GUI | Agent-5 | M1(FFI so) |
| M6 | 集成、全量测试迭代、E2E、装到本机验证 | 主控 + 全体 | M1–M5 |
| M7 | 文档收尾、git 提交、发版说明 | 主控 | M6 |

## 里程碑验收标准

### M0(已完成)
- [x] 本机环境摸底(XFCE/X11、ibus 1.5.29、海峰86 码表、Xvfb/xdotool 就绪)
- [x] 三路并发调研:ibus 引擎开发 / X11 独立输入法 / 词库资源排序
- [x] AGENTS.MD、PLAN.md、ARCHITECTURE.md、workspace 骨架

### M1 lyyime-core
- [x] `cargo test` 通过:≥40 个单测,覆盖五笔全码/简码/词组、拼音全拼/简拼/模糊分段、中英混合、标点、翻页、学习排序、FFI JSON 效果流
- [x] `scripts/test.sh core` 一键全绿;`lyyime-cli` demo 二进制可 headless 演示(key 序列 → 候选/上屏)
- [x] FFI:`liblyyime_core.so` 导出 ARCHITECTURE.md §4 全部符号,`python3 -c ctypes` 冒烟通过

### M2 dicttool
- [x] `dicttool convert --wubi-db /usr/share/ibus-table/tables/wubi-haifeng86.db --out data/runtime` 生成 wubi.tsv / pinyin_char.tsv / suggestion.tsv
- [x] `dicttool fetch` 下载拼音词组库与英文词频表(缓存 dicts/,可重跑),生成 pinyin_phrase.tsv / english.tsv
- [x] `dicttool verify data/runtime` 行数/格式/抽样断言通过;总词组 ≥10万、单字拼音覆盖 GB2312 常用集、英文 ≥1万词
- [x] 样例查询冒烟:`dicttool query 五笔码 yyyy`、`query 拼音 ni hao`、`query 英文 hello` 输出合理候选

### M3 doctor
- [x] `lyyime-doctor check` 输出结构化诊断(JSON + 人读文本):环境变量三件套、ibus 存活/注册、autostart、GTK/Qt im-module、缓存目录、日志尾部
- [x] `lyyime-doctor fix --issue <id>` 支持自动修复:写 env(Xfce 会话)、装 autostart、注册 lyyime 引擎并 `ibus restart`、清缓存
- [x] **输入法管理**:`ime-list` 枚举本机 ibus/fcitx5/lyyime 全部输入法;`ime-add/ime-remove/ime-default` 增删与设默认(dnf 包级,`--dry-run` 预览 + `--yes` 确认,含 lyyime 自身与最后可用输入法保护)
- [x] `fix` 全程幂等;危险操作(改系统文件、装卸包)默认 `--dry-run` 提示

### M4 ibus 引擎(Mode A)
- [x] `ibus-engine/install.sh` 注册后,ibus 托盘可选 "lyyIme" 引擎,任意 GTK/Qt 应用可输入
- [x] Shift 单击切中英;候选窗数字选词、`-`/`=` 翻页;英文态直通
- [x] 引擎图标右键菜单:设置 / 工具 / 修复输入法 / 关于
- [x] 无 GUI 冒烟:直接 import engine 模块喢单元按键得到预期效果流(测试脚本 `tests/unit_ibus_engine.py`)

### M5 lyyime-xim(Mode B,XIM server 路线,架构见 RESEARCH.md §2)
- [x] **spike 先行**:最小 XIM server(IMdkit)在 Xvfb 连通 GTK3 Entry(gtk xim immodule),失败则回报主控升级方案
- [x] 任意 XIM 接入窗口输入 `nihao` 弹候选窗,数字选词上屏正确中文;Pass 键经 IMForwardEvent 原样直达
- [x] Shift 单击切换(trigger off/on),状态托盘图标显示 中/EN;右键菜单:开关 / 设置 / 工具(含**修复输入法**、**输入法管理**——增删其它输入法与设默认,exec lyyime-doctor CLI,操作带确认) / 退出
- [x] 设置对话框改 config.toml 即时生效(候选数、字体、混合开关)
- [x] Xvfb E2E(`tests/e2e/run.sh`)全自动断言通过

### M6 集成验证
- [x] `scripts/build.sh && scripts/test.sh && tests/e2e/run.sh` 全绿
- [x] 本机(root 会话)实装:Mode A 在 GTK 应用、Mode B 在终端/浏览器各实测一段中文+英文混合输入
- [x] doctor 在故意破坏环境(改错 XMODIFIERS)后能检测并修复

### M7 收尾
- [x] README(安装/使用/截图/架构图)、TESTING.md 更新
- [x] git 提交(分模块多次提交)

## 风险与对策

| 风险 | 对策 |
|---|---|
| X11 grab-echo 回放死循环 | M5 依据调研结论采用"xremap 同款"防护(时间戳/自键屏蔽);E2E 必测 |
| 拼音多音字/简拼歧义 | 优先用带拼音标注的词组库而非逐字反推;排序公式把全拼 > 简拼 |
| ibus 引擎与现有 ibus-table 冲突 | 引擎名独立(lyyime);install 脚本只注册不卸用户表 |
| GTK3 候选窗抢焦点 | override-redirect + accept_focus(false),E2E 断言焦点未变 |
| 并发 Agent 改同一 crate | 严格目录所有权;公共合同改动须主控批准 |
