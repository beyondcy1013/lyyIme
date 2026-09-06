# lyyIme ibus 引擎(Mode A · Rust)

标准 ibus 输入法引擎的 **Rust 实现**(zbus 直连 D-Bus,进程内直连
lyyime-core,无 ctypes/无 python)。在任意 GTK/Qt 应用中获得五笔/拼音/
英文混合输入。行为合同见 `docs/ARCHITECTURE.md` §2/§3/§6/§7、§11(/AI)。

> 项目规则:**主程序一律 Rust,Python 仅可用于测试**。本组件由原 python
> 引擎(lyyime.py/lyyime_ffi.py/lyyime_ai.py,已删除)整体移植而来;
> 配置文件(config.toml)与用户数据完全兼容。

## 目录

```
ibus-engine/
├── lyyime.xml          # 组件注册模板(@占位符由 install.sh 替换)
├── icons/              # lyyime.svg(伍)、lyyime-zh.svg(中)、lyyime-en.svg(EN)
├── lyyime-setup        # 设置入口包装(拉起 lyyime-app,缺失时人话提示)
├── install.sh          # 安装(--system / 用户级 / --enable / --no-restart)
├── uninstall.sh        # 反向清理(幂等,默认保留用户数据,--purge 才删)
└── README.md           # 本文件

引擎实现(crates/):
├── crates/lyyime-ibus/     # 引擎主体:logic(纯逻辑单测)/ zbus 服务 / IBus 报文构建
└── crates/lyyime-ai/       # AI 助手客户端(lib + CLI 二进制,Mode A 进程内 / Mode B 子进程)
```

## 安装

```bash
scripts/build.sh                # 先构建(产出 ibus-engine-lyyime 与 lyyime-ai)
ibus-engine/install.sh          # 用户级(免 root):装到 ~/.local/share/lyyime/ibus
ibus-engine/install.sh --enable # 并追加进 ibus 预载列表(gsettings,只追加不破坏)
```

验证:`ibus list-engine | grep lyyime`;托盘添加 "lyyIme 五笔拼音" 后即可输入。

## 实现要点(与 python 版的行为差异)

- **引擎进程**:zbus 实现 org.freedesktop.IBus.Factory/Engine 接口;报文
  形态(类型名 + a{sv} + 字段的 IBus 序列化)对照 ibus 1.5.29 真机抓包。
- **直连 core**:lyyime_core::Engine 进程内调用,效果流为枚举,不再走
  ctypes 的 effects JSON。
- **托盘图标**:GTK3 的 Gtk.StatusIcon 在 gtk-rs 0.18 不可用,引擎托盘
  属性(InputMode 等)仍照常注册,由 ibus 面板展示。
- **/AI 助手**:由 crates/lyyime-ai(Rust)承担;Mode A 进程内调用,
  Mode B(xim)以 `lyyime-ai --prompt` 子进程调用(CLI 与旧 .py 兼容)。
- **测试**:crates/lyyime-ibus 单测(logic/keysym/ibus_wire)+ tests/e2e
  (Xvfb 隔离会话,真实 ibus-daemon 全链路,含 /AI)。

## 卸载

```bash
ibus-engine/uninstall.sh          # 幂等;默认保留词频/日志,--purge 才删
```
