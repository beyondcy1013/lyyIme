# floatapp — lyyIme 悬浮窗输入（类万能五笔外挂·即装即用版）

不依赖 ibus/fcitx、不需要重启任何应用、不切换系统输入法。适合作为框架失效时的
兜底输入工具，也是 Mode B"悬浮窗形态"的参考实现（生产版 XIM 路线见 `xim/`，
架构结论见 `docs/RESEARCH.md` §2）。

## 快速开始

```bash
python3 floatapp/lyyime_float.py          # 直接跑
# 开机自启（当前用户）:
bash floatapp/install-autostart.sh
```

## 使用

1. 先正常点一下你要输入字的应用窗口（面板会显示"目标: xxx"）。
2. 点回悬浮窗，直接打五笔86：`wqvb` → 候选「1你好 2您好 …」
   - **空格** 上屏首选；**数字 1–9** 选候选；**`-`/`=`** 翻页；
   - **回车** 原样上屏字母（打英文）；**Esc** 清空；
   - 候选窗支持简码（`wq`→你）与词组（`wqvb`→你好），选词自动学习加权。
3. 上屏后焦点自动回到目标窗口完成发送，再自动跳回悬浮窗继续打。

## 两种发送模式（标题栏下拉切换）

| 模式 | 原理 | 适用 |
|---|---|---|
| **直输** | `xdotool type` keysym 注入（XChangeKeyboardMapping 空闲 keycode） | 绝大多数 GTK/Qt/Electron/终端，速度快 |
| **粘贴** | 写剪贴板 + 模拟 Ctrl+V | 拒收合成键的应用，兼容性最强 |

两种模式均已真机端到端验证（见 `floatapp/e2e_floatapp.sh`，可复跑）。

## 文件

- `lyyime_float.py` 主程序（GTK3 + python-xlib + xdotool）
- `dictload.py` 码表加载：直接读 `/usr/share/ibus-table/tables/wubi-{极点|海峰}86.db`
  （ibus-table SQLite，`phrases(tabkeys, phrase, freq)`，13.7 万条，内存前缀索引）
- `chinese_pad.py` 简易打字板（菜单里可拉起，复制中转用）
- `e2e_floatapp.sh` 端到端自测脚本
- `install-autostart.sh` 写入 `~/.config/autostart/lyyime-float.desktop`

## 配置与数据

- `~/.config/lyyime/config.json`：`dict`(jidian86/haifeng86)、`send`(type/paste)、`position`
- `~/.config/lyyime/user_freq.json`：选词学习（重码时高频词前置）

## 已知边界

- 目标窗口按"最近激活的非自身窗口"识别（250ms 轮询 `_NET_ACTIVE_WINDOW`）。
- 直输模式下目标窗口必须可被 `xdotool windowactivate` 激活；最小化窗口会先被拉起。
- 不支持在悬浮窗里打拼音（拼音混输由 lyyime-core/dicttool 管线承担）。
