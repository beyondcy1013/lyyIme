# lyyIme 测试说明

> 质量红线:每条用户可见功能必须在本机或 Xvfb 实测通过,"编译通过"不算完成。
> 全部命令在项目根 /home/codes/apps/lyyIme 下执行;cargo 一律经 scripts(内置 CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme)。

## 分层测试矩阵

| 层 | 命令 | 依赖 | 断言内容 |
|---|---|---|---|
| core 单测 | `bash scripts/test.sh core` | 无 | 五笔/拼音/混合/标点/翻页/学习/FFI JSON(≥40 test) |
| dicttool | `cargo test -p lyyime-dicttool` + `dicttool verify data/runtime` | 无 | 14 test + 40 项数据校验(行数/排序/抽样) |
| doctor 单测 | `cargo test -p lyyime-doctor` | 无(tempdir 注入) | 60 test:8 检查项、6 修复幂等、ImeManager catalog/保护规则 |
| doctor e2e | `bash tests/e2e/doctor_test.sh` | 真机(root) | 破坏 env→检出→修复→恢复;ime-list 真实枚举;dry-run 预览 |
| ibus 引擎单测 | `python3 tests/unit_ibus_engine.py` | 桩库(gcc 自动编译)或真库 | 效果流:ni hao→候选、选词、Shift 单击、密码框、release 放行 |
| xim 解析器单测 | `make -C xim test` | 无 | effects JSON 迷你解析器(≥8 test) |
| Mode B e2e | `bash tests/e2e/xim_e2e.sh` | Xvfb :98 + 桩库/真库 | XIM 连接、nihao 选词上屏、Shift 切换、退出清理 |
| 全链路 e2e | `bash tests/e2e/run.sh`(M6) | Xvfb + 真库 + 全部安装 | 双模式真实打字断言(M6 集成时补全) |
| 机制探针(留档) | `tests/e2e/x11grab_probe*.c` | Xvfb :99 | grab 回放不可靠/中文注入可行(架构决策证据) |

## 当前状态(2026-09-06)

全部层通过:Rust 172 单测 ✅ / ibus 引擎 26+冒烟 ✅ / xim 单测+e2e ✅ / doctor e2e ✅ /
**双模式全链路 `tests/e2e/run.sh` ✅**(真库+真实词库)。

## 一键

```bash
bash scripts/test.sh          # Rust 三 crate 单测
bash scripts/test.sh all      # + FFI 冒烟 + ibus 引擎单测
bash tests/e2e/doctor_test.sh
bash tests/e2e/xim_e2e.sh
```

## 桩库机制(并行开发解耦)

lyyime-core 的 cdylib 未就绪时,ibus 引擎与 xim 的测试用 `gcc -shared -fPIC` 编译的
**ABI 桩库**(实现 ARCHITECTURE §3 全部导出符号、确定性规则、相同 JSON 格式):

- ibus:`python3 tests/unit_ibus_engine.py`(自动编译桩;`--real-core` 且真库存在时用真库)
- xim:`LYYIME_CORE_LIB=/path/to/stub.so tests/e2e/xim_e2e.sh`

集成(M6)后统一换真库 `liblyyime_core.so`:
`dicttool convert → cargo build -p lyyime-core --release → 安装 /usr/local/lib/lyyime/ → ldconfig`

## E2E 环境约定

- 显示器号:机制探针 :99、xim e2e :98、M6 全链路 :97,互不冲突;Xvfb 用前清理 `/tmp/.X*-lock` 与 `/tmp/.X11-unix/X*` 残留
- e2e 一律自带起停与清理(进程、pidfile、临时 Xvfb),可重复执行
- 真实 :11 用户会话**禁止**注入按键做测试;真机验证仅用 doctor check(只读)与人工步骤
