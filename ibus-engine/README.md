# lyyIme ibus 引擎(Mode A)

标准 ibus 输入法引擎:Python + ctypes FFI 对接 Rust 核心 `liblyyime_core.so`,
在任意 GTK/Qt 应用中获得五笔/拼音/英文混合输入。行为合同见
`docs/ARCHITECTURE.md` §3(FFI)/§6(按键)/§7(Mode A);实现依据
`docs/RESEARCH.md` §1(ibus 引擎一手调研)。

## 目录

```
ibus-engine/
├── engine/
│   ├── lyyime.py       # 引擎主体:EngineLogic(纯逻辑,可无 dbus 单测)+ IBus 胶水 + factory + main
│   └── lyyime_ffi.py   # ctypes 封装:库探测 / -needed 重试 / effects JSON 解析
├── icons/              # lyyime.svg(伍)、lyyime-zh.svg(中)、lyyime-en.svg(EN)
├── lyyime.xml          # 组件注册模板(@占位符由 install.sh 替换)
├── lyyime-setup        # 设置入口包装(拉起 lyyime-app,缺失时人话提示)
├── install.sh          # 安装(--system / 用户级 / --enable / --core-lib …)
├── uninstall.sh        # 反向清理(幂等,默认保留用户数据,--purge 才删)
└── README.md           # 本文件
```

## 安装

```bash
# 用户级(免 root,推荐):装到 ~/.local/share/lyyime/ibus + ~/.local/share/ibus/component
./install.sh

# 并把 lyyime 追加进 ibus 预载列表(gsettings,只追加不破坏现有条目)
./install.sh --enable

# 系统级(需 root):/usr/local/share/lyyime + /usr/local/share/ibus/component + /usr/local/lib/lyyime
sudo ./install.sh --system --enable
```

安装脚本会自动:

1. 安装 FFI 核心库 `liblyyime_core.so`(默认取 `/data/cargo-target/local/lyyIme/release/`
   的构建产物;可用 `--core-lib <路径>` 指定、`--no-core-lib` 跳过)。用户级安装会把库放在
   `~/.local/share/lyyime/lib/` 并提示在 `~/.xprofile` 里 `export LYYIME_CORE_LIB=…`;
   系统级放 `/usr/local/lib/lyyime/` 并执行 `ldconfig`。
2. 生成组件 XML 到合同位置(用户级 `~/.local/share/ibus/component/`、系统级
   `/usr/local/share/ibus/component/`),**并桥接一份到 `/usr/share/ibus/component/`**——
   实证本机 ibus 1.5.29(源码 `src/ibusregistry.c`):daemon 只扫 `IBUS_DATA_DIR/component`,
   用户目录支持被上游 `#if 0` 禁用;无 root 时可改用
   `export IBUS_COMPONENT_PATH=~/.local/share/ibus/component:/usr/share/ibus/component`
   (该变量会整体替换默认搜索路径)。
3. 执行 `ibus write-cache`;默认重启 ibus(`--no-restart` 可跳过),托盘即可选
   **lyyIme 五笔拼音**(symbol:伍)。
4. 引擎运行时按以下顺序探测核心库(找不到时报含修复指引的清晰异常):
   `$LYYIME_CORE_LIB` → `/usr/local/lib/lyyime/` → `/usr/local/lib` → `/usr/lib64`
   → `/data/cargo-target/local/lyyIme/release`(及 `$CARGO_TARGET_DIR`)→ 仓库 `target/release`。

卸载:`./uninstall.sh`(自动清理用户级/系统级注册与核心库、从预载列表移除;
用户词频与日志默认保留,确认不要加 `--purge`)。

## 使用

| 按键/操作 | 行为 |
|---|---|
| a–z | 进缓冲,预编辑+候选实时更新(数字/`-``=` 由候选窗承载) |
| 1–9 | 选候选上屏;空缓冲时数字原样放行 |
| Space | 有缓冲顶屏;无缓冲放行空格 |
| Enter | 有缓冲上屏原始字母(中英混合直通);无缓冲放行 |
| Esc / Backspace | 清缓冲 / 删尾 |
| `-` / `=`、PageUp/PageDown | 翻页(边界钳制,不循环) |
| 标点 | 中文态出中文标点;有缓冲先顶首选;英文态放行 |
| **Shift 单击** | 切换中/英(按下后无其它键即释放才算单击;带 Ctrl/Alt/Super 不算) |
| 英文态 | 全部直通;密码框(InputPurpose PASSWORD/PIN)永远全放行 |

托盘图标右键菜单:**中英切换**(图标随中/EN 变化)、**设置**(拉起 `lyyime-app`,
未安装时给提示)、**工具**(修复输入法 = `lyyime-doctor fix --all --dry-run` 终端预览;
输入法管理 = `lyyime-doctor ime-list`;重载词库;打开日志目录)、**关于**。

日志:`~/.local/share/lyyime/logs/ibus.log`,按天轮转保留 7 份。
健壮性:FFI 任何异常 → 该键放行并降级英文直通 + 记日志,绝不卡死按键。

## 测试

```bash
# 桩库模式(默认):gcc 编译 tests/stub_lyyime_core.c 为 ABI 桩库,
# 不经 dbus 直接喢单元键断言效果流,26 条用例
python3 tests/unit_ibus_engine.py

# 真库集成冒烟:LYYIME_CORE_LIB 指向真 so + LYYIME_DATA_DIR 指向词典目录
LYYIME_CORE_LIB=/data/cargo-target/local/lyyIme/release/liblyyime_core.so \
LYYIME_DATA_DIR=$PWD/data/runtime \
    python3 tests/unit_ibus_engine.py --real-core
```

无显示环境冒烟(验证 gi import 顺序,借鉴 ibus-table main.py 对 --xml 的处理经验):

```bash
env -u DISPLAY python3 -c "import sys; sys.path.insert(0,'ibus-engine/engine'); import lyyime, lyyime_ffi; print('ok')"
```

## 借鉴出处(依 AGENTS.MD 红线标注)

- **进程/注册模型、factory 写法**:逐行借鉴 ibus-table 1.17.2 的
  `factory.py` / `main.py`(本机 `/usr/share/ibus-table/engine/`),见 RESEARCH §1.1。
- **按键处理与放行策略**:`return False` 常规路径(Qt5 的 ibus 模块不实现
  `forward_key_event`)、release 放行、密码框全放行,借鉴 `table.py:3816-3940`,
  见 RESEARCH §1.2。
- **候选窗/辅助区/预编辑**:`IBus.LookupTable`、`update_auxiliary_text`、
  `update_preedit_text_with_mode`,借鉴 `table.py`(RESEARCH §1.3)。
- **属性菜单**:`IBus.Property(sub_props=…)` 子菜单构造,借鉴 `table.py`
  `_init_or_update_property_menu`(RESEARCH §1.4)。
- **无 GUI 单测分层(MockEngine 模式)**:核心逻辑与 IBus 类型解耦,借鉴
  ibus-table `_unit_test` 思路(RESEARCH §1.5)。
- **日志轮转**:`TimedRotatingFileHandler` 模式借鉴 ibus-table(RESEARCH §1.6)。
- **设置入口包装**:借鉴 ibus-table 的 `ibus-setup-table` 入口模式。

## 已知局限

- 组件发现依赖桥接:ibus 1.5.29 只扫 `/usr/share/ibus/component`(源码实证),
  桥接文件由 install/uninstall 幂等维护;上游若启用用户目录扫描可移除桥接。
- 候选注释直接拼在候选文本后(ibus 面板无富文本候选注释通道)。
- Shift 单击无按压时长阈值(长按无其它键也判单击),与 ibus-libpinyin 一致。
- “重载词库”通过重建 FFI Engine 实现核心侧全量重载;core 若日后提供增量
  reload C ABI 可平滑替换。
- 工具菜单的 doctor 输出需终端展示(xfce4-terminal/xterm 缺失时退化为
  前台运行 + 写日志 + 辅助区提示)。
- 引擎与 ibus-daemon 同生命周期;ibus 异常退出由 ibus 自身拉起机制恢复。
