# lyyime-doctor

诊断/修复 Linux 中文输入法环境 + 输入法管理(lib `lyyime_doctor` + bin `lyyime-doctor`)。
合同:docs/ARCHITECTURE.md §9 / §9.1。

## 分层

| 模块 | 职责 |
|---|---|
| `paths.rs` | `Paths`:全部路径可注入;`Paths::for_home(tempdir)` 供测试收敛到临时目录,单测绝不触碰真实 HOME |
| `system.rs` | `SystemOps` trait(env/进程/命令/kill/detached 拉起)+ `RealSystem` + 测试桩 `FakeSystem`(记录全部副作用调用) |
| `checks.rs` | 8 项检查:env / daemon / engine-register / autostart / immodule / data / logs / locale |
| `fixes.rs` | 6 个幂等修复:env / autostart / engine-register / restart-ibus / clean-cache / reinstall-dict,支持 dry-run |
| `catalog.rs` | 内置输入法目录(id → dnf 包名/ibus 引擎名;包名按 openEuler 24.03) |
| `ime.rs` | `ImeManager`:ime-list / ime-add / ime-remove / ime-default |

## 关键决策

- **依赖只加 serde/serde_json/anyhow**(项目规则)。tempdir、XML 解析、GVariant 列表解析、UTC 时间戳均为手写小实现。
- **安全边界**:
  - 所有写路径由 `Paths` 派生,绝不写 /etc;fix engine-register 只写 /usr/local/share(可写时)与 ~/.local。
  - `restart-ibus` 只 kill `uid == 当前 uid` 的进程。
  - 进程匹配用"首 token 的 basename == 名称"严格规则,而不是子串包含——否则调用方 shell 的参数里恰含 `ibus-daemon` 字样时会被误杀(测试 `wrapper_scripts_mentioning_daemon_are_not_matched` 防回归)。
  - `fix env` 的框架判定以**运行中的框架进程**为准(ibus-daemon/fcitx5),不看 env 三件套——因为 env 本身就是要修的东西(E2E 场景:故意写坏 XMODIFIERS 后 fix 仍能写回正确框架)。
- **env 文件幂等**:`~/.xprofile` 与 `~/.config/lyyime/env.sh` 用 managed block(`>>> lyyime-doctor managed block <<<`)upsert;块外同名 export 冲突行被注释(`# lyyime-doctor: 已停用冲突行`),重复执行内容逐字节不变(有单测)。
- **ime-list**:组件 XML(用户级 → /usr/local → /usr/share)静态引擎 + ibus-table 动态引擎(枚举 `tables/*.db`,engine id 为 `table:<stem>`,与 ibus 1.5.x 行为一致)+ fcitx5(rpm 包探测)+ lyyime 自身 + ibus 框架。`active`/`is_default` 来自 preload-engines 首位。
- **ime-default 的 schema 探测**:合同写 `desktop.ibus.general`,但 ibus 1.5.29 实际 schema 是 `org.freedesktop.ibus.general`;实现按顺序探测两个 schema,dconf 兜底,均不可用时降级 warn + 手动指引。**待主控确认是否回写合同**。
- **安全模型(§9.1)**:ime-add/remove/default 默认 dry-run 只打印命令;`--yes` 才实际执行;包操作仅 root;remove 拒绝默认 lyyime 与"会话唯一中文输入法"(`--force` 强制);操作写审计日志 `~/.local/share/lyyime/logs/ime-manager.log`,实际执行时记录 ime-list 前后差量。
- CLI 对 SIGPIPE(broken pipe)静默退出(141),避免管道接 `head`/`grep -q` 时 panic。

## 测试

- 单测 60 个(`cargo test -p lyyime-doctor`):catalog 纯函数、XML/GVariant 解析、8 项检查矩阵、全部 fix 动作(含幂等/dry-run/uid 过滤)、ImeManager 全流程(stub rpm/gsettings,不真装包)。
- E2E:`tests/e2e/doctor_test.sh` —— 备份现场 → 故意写错 XMODIFIERS → `check --json` 断言 fail → `fix env --dry-run` 断言无副作用 → `fix env` → source 后断言恢复 ok → 幂等复验 → 还原;另断言 ime-list 真实输出与 ime-add/remove/default 的 dry-run 预览。
