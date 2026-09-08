# lyyime-float — lyyIme 悬浮窗输入(Mode C · Rust/GTK3)

类 Windows 万能五笔"外挂悬浮窗"的 **Rust 实现**。不依赖 ibus/fcitx、不需要重启
任何应用、不切换系统输入法。适合作为框架失效时的兜底输入工具。

> 项目规则:**主程序一律使用 Rust,Python 仅可用于测试**。
> 本 crate 由原 floatapp(Python 参考实现)整体移植而来,配置与数据文件格式
> 完全兼容(`config.json` / `user_freq.json` / `phrase.json` 无缝沿用)。

## 快速开始

```bash
scripts/build.sh                          # 构建(debug)
crates/lyyime-float/install.sh            # 装机:应用菜单/桌面入口 "lyyIme 悬浮窗输入"
crates/lyyime-float/install-autostart.sh  # 开机自启(当前用户)
```

程序为单实例:重复点击图标 = 唤起已有悬浮窗(显示+聚焦),不会开第二个。
卸载:`crates/lyyime-float/uninstall.sh`。

## 使用

1. 先正常点一下你要输入字的应用窗口(状态行显示"目标: xxx")。
2. 点回悬浮窗,直接打五笔86(极点/海峰码表):`wqvb` → 候选「1你好 2您好 …」
   - **空格** 上屏首选;**数字 1–9** 选候选;**`-`/`=`** 翻页;
   - **回车** 原样上屏字母(打英文);**Esc** 清空;
   - **Ctrl+Z** 撤销、**Ctrl+Y**/**Ctrl+Shift+Z** 重做(GTK3 无内建,自实现);
     编码框空缓冲时(刚上屏/刚清空)按 Ctrl+Z/Ctrl+Y 直通目标窗口,
     撤销/重做的就是**目标应用里刚上屏的文本**(与空缓冲退格直通同合同);
   - 候选支持简码(`wq`→你)与词组(`wqvb`→你好),选词自动学习加权。
3. 上屏后焦点自动回到目标窗口完成发送,再自动跳回悬浮窗继续打。

## 不抢焦点模式(光标留在目标窗口)

默认模式下打字要"点目标 → 点回悬浮窗",打字期间光标在悬浮窗里,看不到
文字将落到目标的什么位置。菜单 ☰ → **「光标留在目标窗口(不抢焦点)」**
(或 config.json `"keep_target_focus": true`)可切换到外挂式体验:

- 悬浮窗不再夺取焦点:点一次目标应用后**目标文本框的光标全程保持闪烁**,
  打字、选词、翻页目标窗口都不丢焦;
- 悬浮窗显示期间由它接管全局键盘(菜单/对话框/短语编辑器仍正常输入);
- **代价**:该期间 Alt+Tab、全局快捷键也被悬浮窗接管——想直接操作其它
  应用时,点标题行 **「—」** 隐藏悬浮窗即完全还原,再次唤起自动接管。

## 输入统计(停顿显示今日字数与速度)

打字停顿超过设定秒数(默认 10)后,状态行自动改显「**今日已输入 X 字 · 约
Y 字/分**」;一恢复输入立即还原成目标窗口信息。速度按**活跃打字时长**计算:
相邻两次上屏间隔超过「最长停顿」(默认 30 秒)的空隙(思考、离开)不计入
时长,刷手机一下午不会把均速拉没。

- 设置入口:**lyyIme 设置窗口 → 「输入统计」页**(开关 + 两个秒数)。
  统计是全局配置(config.toml 顶层 `stats_*` 键),由设置窗统一管理;
  悬浮窗对配置文件做监视,**设置窗一保存这里即时生效,无需重启**。
- 统计数据:`~/.local/share/lyyime/stats/日期.tsv`(core `stats` 模块);
  **与 ibus 模式(Mode A)共用同一份数据**,两边打字都计入。
- 计字口径:上屏文本的非空白字符数(汉字/字母/标点都算,空格换行不算)。

## 自定义短语与动态变量

菜单 ☰ → **自定义短语管理…**:自定义编码(1–12 个小写字母)→ 任意长度文本,
精确码命中后短语排候选最前。存储 `~/.config/lyyime/phrase.json`。

文本里可嵌入**动态变量**,候选展示与上屏时实时展开(每次取当前时刻,跨天自动更新):

| 变量 | 展开(以 2026-09-06 15:07:09 星期日为例) |
|---|---|
| `$date(yyyy-MM-dd)` | `2026-09-06` |
| `$date(yyyy年M月d日)` | `2026年9月6日` |
| `$time(HH:mm)` | `15:07` |
| `$week` | `星期日` |
| `$$` | 字面 `$` |

格式 token(办公软件习惯,**补零用双写**):`yyyy` `yy` 年、`MM` `M` 月、
`dd` `d` 日、`HH` `H` 时、`mm` `m` 分、`ss` `s` 秒、`E` 星期几的汉字(可写
`星期E`)。例:`rq` → `今天是 $date(yyyy年M月d日) $week`。未识别的 `$…`
原样上屏,不会报错。编辑器里有一排"插入变量"快捷按钮,列表页有"上屏预览"列。

## 打"日期/时间"直接出动态值(触发词)

不需要定义短语:只要候选里打出 **日期**(五笔 `jjad`)或 **时间**(五笔
`jfuj`),候选里就会在它身后出现动态的今天日期/当前时间,数字选中即上屏。
也可直接敲整拼 `riqi` / `shijian`(悬浮窗只装五笔码表,这里做了直触识别)。

触发时机(避免打扰正常打词):
- 触发词是**候选首选** → 出现(如 `jjad` → 「1日期 2 2026年9月6日 3 2026-09-06」);
- 触发词不是首选,但**候选总数 ≤ 6**(可选少了)→ 也出现;
- 触发词深埋、候选又多 → 不出现。
`riqi`/`shijian` 整拼直触时动态候选追加在候选尾部。自定义短语功能完全不受影响,
两种途径可并存(同码自定义短语仍最优先)。

## 两种发送模式(标题栏下拉切换)

| 模式 | 原理 | 适用 |
|---|---|---|
| **直输** | `xdotool type` keysym 注入 | 绝大多数 GTK/Qt/Electron/终端,速度快 |
| **粘贴** | 写剪贴板 + 模拟 Ctrl+V | 拒收合成键的应用,兼容性最强 |

直输模式下超过 40 字的文本(如长短语)自动改用粘贴上屏(会覆盖剪贴板,
状态栏有提示);`xdotool type` 超时随文本长度伸缩,长文本不误报。

## 文件

- `src/main.rs` 入口(主程序 / `--pad` 打字板 / `--smoke-phrases` / `--smoke-stats` 冒烟)
- `src/ui.rs` 主窗/候选渲染/按键/菜单/短语管理对话框/打字板
- `src/dict.rs` 码表加载:读 `/usr/share/ibus-table/tables/wubi-{极点|海峰}86.db`
  (ibus-table SQLite,`phrases(tabkeys, phrase, freq)`,13.7 万条,内存前缀索引)
- `src/phrases.rs` 自定义短语簿(编码→文本、动态变量展开、"日期/时间"触发词、
  候选合并;纯逻辑可单测)
- `src/undo.rs` 撤销/重做栈与按键拦截(GTK3 Entry/TextView 无内建 undo;
  覆盖编码框/打字板/短语编辑器, 空缓冲直通合同在 ui.rs)
- `src/config.rs` config.json / user_freq.json / pidfile(单实例)
- `src/xtrack.rs` 目标窗口追踪(_NET_ACTIVE_WINDOW,x11rb RustConnection)
- `src/sender.rs` 直输/粘贴发送(xdotool 子进程,超时随长度伸缩)
- `e2e_phrases.sh` 自定义短语端到端(Xvfb 隔离屏 + 临时 HOME,可重复)
- `e2e_no_focus.sh` 不抢焦点模式端到端(同上隔离约定)
- `res/float.svg` 主题图标(应用菜单/桌面入口用,装机名 `lyyime-float`)
- `install.sh` / `uninstall.sh` / `install-autostart.sh` 装机、卸载、自启

## 配置与数据

- `~/.config/lyyime/config.json`:`dict`(jidian86/haifeng86)、`send`(type/paste)、`position`
- `~/.config/lyyime/user_freq.json`:选词学习(重码时高频词前置)
- `~/.config/lyyime/phrase.json`:自定义短语(编码 → 文本列表,支持动态变量)
- `~/.config/lyyime/config.json` 还含 `keep_target_focus`(默认 false):
  不抢焦点模式(见上),菜单可切换
- `~/.config/lyyime/config.toml` 顶层 `stats_enabled` / `stats_pause_secs` /
  `stats_idle_exclude_secs`:输入统计设置(**全局统一配置,设置窗口管理**;
  悬浮窗不再写 config.json 的旧 stats 键,旧键仅作迁移回退)
- `~/.local/share/lyyime/stats/日期.tsv`:输入统计(按天,与 Mode A 共享)
- `~/.local/share/lyyime/float.pid`:单实例 pidfile

## 测试

- Rust 单测(cargo test -p lyyime-float):短语簿读写/校验/增删改、动态变量
  展开(含 `$$` 转义、未知变量容错、补零)、候选合并(置顶/去重/限额)、
  日期/时间触发词(首选触发/候选少触发/深埋不触发/整拼直触/自定义短语不受影响)、
  码表索引与学习加权、撤销栈(往返/重做分支丢弃/回填去重/按词重开/上限截断)。
- `e2e_phrases.sh`:Xvfb 隔离屏全链路(词典回归 / 短语置顶 / 长短语自动粘贴 /
  管理对话框冒烟 / jjad 动态日期真码表触发 / 输入统计冒烟:记录→停顿显示→恢复)。
- `e2e_no_focus.sh`:不抢焦点模式全链路(活动窗口全程=目标 / 抓取收键 /
  上屏解抓重抓后可连续输入 / 空缓冲 Ctrl+Z·Ctrl+Y 直通撤销重做目标文本)。

## 已知边界

- 目标窗口按"最近激活的非自身窗口"识别(250ms 轮询 `_NET_ACTIVE_WINDOW`)。
- 直输模式下目标窗口必须可被 `xdotool windowactivate` 激活;最小化窗口会先被拉起。
- 暂无托盘图标(gtk-rs 0.18 未提供 Gtk.StatusIcon);入口为常驻悬浮窗 ☰ 菜单
  与"再次启动唤起"。
- 不支持在悬浮窗里打拼音(拼音混输由 lyyime-core/dicttool 管线承担)。
