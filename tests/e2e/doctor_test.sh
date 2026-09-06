#!/usr/bin/env bash
# doctor E2E:破坏环境 → check 检出 → fix 恢复 → 还原
# 另含 ime-list 真实输出与 ime-add/ime-remove/ime-default --dry-run 命令预览断言。
# 无需 sudo(目标机即 root;普通用户跑 env 部分 check 同样成立)。
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$(pwd)"

# 仓库硬性规则:CARGO_TARGET_DIR 固定
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/data/cargo-target/local/lyyIme}"
BIN="$CARGO_TARGET_DIR/debug/lyyime-doctor"

if [ ! -x "$BIN" ]; then
    echo ">> 构建 lyyime-doctor"
    cargo build -p lyyime-doctor
fi

fail() { echo "E2E-FAIL: $*" >&2; exit 1; }
pass() { echo "E2E-OK : $*"; }
PY=python3

# ---------------------------------------------------------------------------
# 0. 备份现场(环境变量 + 将被 fix 触碰的文件)
# ---------------------------------------------------------------------------
ORIG_XMODIFIERS="${XMODIFIERS-}"
ORIG_GTK="${GTK_IM_MODULE-}"
ORIG_QT="${QT_IM_MODULE-}"

XPROF="$HOME/.xprofile"
ENVSH="$HOME/.config/lyyime/env.sh"
CFGDIR="$HOME/.config/lyyime"
TMPBK="$(mktemp -d /tmp/lyyime-doctor-e2e.XXXXXX)"

cleanup() {
    # 还原文件:有备份恢复备份,否则删除 fix 新建的
    if [ -f "$TMPBK/xprofile.bak" ]; then cp "$TMPBK/xprofile.bak" "$XPROF";
    elif [ -f "$XPROF" ] && [ ! -f "$TMPBK/xprofile.orig-existed" ]; then rm -f "$XPROF"; fi
    if [ -f "$TMPBK/envsh.bak" ]; then cp "$TMPBK/envsh.bak" "$ENVSH";
    elif [ -e "$ENVSH" ] && [ ! -f "$TMPBK/envsh.orig-existed" ]; then rm -f "$ENVSH"; rmdir "$CFGDIR" 2>/dev/null || true; fi
    rm -rf "$TMPBK"
    # 还原环境变量
    export XMODIFIERS="$ORIG_XMODIFIERS"
    export GTK_IM_MODULE="$ORIG_GTK"
    export QT_IM_MODULE="$ORIG_QT"
    [ -n "${ORIG_XMODIFIERS:-}" ] || unset XMODIFIERS
    [ -n "${ORIG_GTK:-}" ] || unset GTK_IM_MODULE
    [ -n "${ORIG_QT:-}" ] || unset QT_IM_MODULE
}
trap cleanup EXIT

[ -f "$XPROF" ] && { cp "$XPROF" "$TMPBK/xprofile.bak"; touch "$TMPBK/xprofile.orig-existed"; }
[ -f "$ENVSH" ] && { cp "$ENVSH" "$TMPBK/envsh.bak"; touch "$TMPBK/envsh.orig-existed"; }
if [ -f "$ENVSH" ] || [ -f "$XPROF" ]; then
    md5sum "$ENVSH" 2>/dev/null | awk '{print $1}' > "$TMPBK/envsh.md5" || true
fi
echo ">> 备份完成(xprofile: $([ -f "$TMPBK/xprofile.bak" ] && echo 有 || echo 无), env.sh: $([ -f "$TMPBK/envsh.bak" ] && echo 有 || echo 无))"

# ---------------------------------------------------------------------------
# 1. 故意写错 XMODIFIERS → check --json 必须检出 fail
# ---------------------------------------------------------------------------
export XMODIFIERS="@im=fcitx"      # 本机是 ibus,这是错的
unset GTK_IM_MODULE QT_IM_MODULE 2>/dev/null || true

OUT="$("$BIN" check --json)"
echo "$OUT" | $PY -c '
import json, sys
d = json.load(sys.stdin)
assert isinstance(d, dict) and "checks" in d and "summary" in d, "check --json 顶层结构不对"
ids = [c["id"] for c in d["checks"]]
assert ids == ["env","gui-env","session-bus","daemon","engine-register","autostart","immodule","data","logs","locale"], ids
c = {x["id"]: x for x in d["checks"]}
assert c["env"]["status"] == "fail", c["env"]
assert c["env"]["fix_hint"], "env 项应有 fix_hint"
# 新增两项(会话总线/真实进程环境)必须可序列化且带状态
assert c["gui-env"]["status"] in ("ok", "warn", "fail"), c["gui-env"]
assert c["session-bus"]["status"] in ("ok", "warn", "fail"), c["session-bus"]
'
pass "check --json 检出 env=fail(10 项齐全)"

# ---------------------------------------------------------------------------
# 2. fix env --dry-run:只打印,不落盘
# ---------------------------------------------------------------------------
OUT="$("$BIN" fix --issue env --dry-run)"
case "$OUT" in *DRY-RUN*) ;; *) fail "dry-run 输出应含 DRY-RUN 标记: $OUT" ;; esac
if [ -f "$TMPBK/envsh.bak" ]; then
    [ "$(md5sum "$ENVSH" | awk '{print $1}')" = "$(cat "$TMPBK/envsh.md5")" ] || fail "dry-run 改动了 env.sh"
elif [ -e "$ENVSH" ]; then
    fail "dry-run 不应创建 env.sh"
fi
[ -e "$XPROF" ] && [ ! -f "$TMPBK/xprofile.bak" ] && fail "dry-run 不应创建 ~/.xprofile"
pass "fix env --dry-run 无副作用"

# ---------------------------------------------------------------------------
# 3. 真实 fix env → source 后 check 恢复 ok
# ---------------------------------------------------------------------------
OUT="$("$BIN" fix --issue env)"
case "$OUT" in *APPLIED*) ;; *) fail "fix env 应输出 APPLIED: $OUT" ;; esac
[ -f "$ENVSH" ] || fail "fix env 未生成 $ENVSH"
grep -q "export XMODIFIERS=@im=ibus" "$ENVSH" || fail "env.sh 未写入 XMODIFIERS=@im=ibus"
grep -q "export GTK_IM_MODULE=ibus" "$ENVSH" || fail "env.sh 未写入 GTK_IM_MODULE"
[ -f "$XPROF" ] || fail "fix env 未写入 ~/.xprofile"

# shellcheck disable=SC1090
source "$ENVSH"

OUT="$("$BIN" check --json)"
echo "$OUT" | $PY -c '
import json, sys
d = json.load(sys.stdin)
c = {x["id"]: x for x in d["checks"]}
assert c["env"]["status"] == "ok", c["env"]
'
pass "fix env 后 check 恢复 ok(环境变量三件套=@im=ibus)"

# 幂等:再跑一次 fix env,内容不变
MD5A="$(md5sum "$ENVSH" | awk '{print $1}')"
"$BIN" fix --issue env >/dev/null
MD5B="$(md5sum "$ENVSH" | awk '{print $1}')"
[ "$MD5A" = "$MD5B" ] || fail "fix env 不幂等(两次结果不同)"
pass "fix env 幂等(重复执行内容不变)"

# ---------------------------------------------------------------------------
# 3.5 modeb-env:dry-run 预览与实际写入(写后立即用 env 修复还原)
# ---------------------------------------------------------------------------
OUT="$("$BIN" fix --issue modeb-env --dry-run)"
case "$OUT" in *DRY-RUN*@im=lyyime*|*DRY-RUN**) ;; *) fail "modeb-env dry-run 输出异常: $OUT" ;; esac
echo "$OUT" | grep -q "GTK_IM_MODULE=xim" || fail "modeb-env dry-run 未预览 GTK_IM_MODULE=xim"
"$BIN" fix --issue modeb-env >/dev/null
grep -q "export XMODIFIERS=@im=lyyime" "$ENVSH" || fail "modeb-env 未写入 @im=lyyime"
grep -q "export GTK_IM_MODULE=xim" "$ENVSH" || fail "modeb-env 未写入 GTK_IM_MODULE=xim"
pass "modeb-env 写入 Mode B profile(xim/@im=lyyime)"
"$BIN" fix --issue env >/dev/null   # 还原为 ibus(Mode A)

# 还原环境变量到破坏态,供 cleanup 统一恢复
export XMODIFIERS="$ORIG_XMODIFIERS"
export GTK_IM_MODULE="$ORIG_GTK"
export QT_IM_MODULE="$ORIG_QT"
[ -n "${ORIG_XMODIFIERS:-}" ] || unset XMODIFIERS
[ -n "${ORIG_GTK:-}" ] || unset GTK_IM_MODULE
[ -n "${ORIG_QT:-}" ] || unset QT_IM_MODULE

# ---------------------------------------------------------------------------
# 4. ime-list 真实输出断言
# ---------------------------------------------------------------------------
"$BIN" ime-list --json | $PY -c '
import json, sys
d = json.load(sys.stdin)
assert isinstance(d, list) and len(d) >= 3, f"ime-list 条目过少: {len(d)}"
ids = [e["id"] for e in d]
assert "table:wubi-haifeng86" in ids, f"缺海峰86: {ids[:10]}"
assert "libpinyin" in ids, "缺 ibus-libpinyin"
kinds = {e["id"]: e["kind"] for e in d}
assert kinds["table:wubi-haifeng86"] == "ibus-engine"
assert any(k == "lyyime" for k in kinds.values()), "缺 lyyime 条目"
assert "fcitx5" in ids, "应有 fcitx5 占位条目(未安装)"
assert "ibus" in ids and kinds["ibus"] == "other"
e = {x["id"]: x for x in d}
assert e["table:wubi-haifeng86"]["installed"] is True
assert e["table:wubi-haifeng86"]["package"] == "ibus-table-chinese-wubi-haifeng"
assert e["fcitx5"]["installed"] is False
assert set(d[0].keys()) >= {"id","name","kind","installed","active","is_default","package"}, "字段不全"
'
pass "ime-list 真实输出:海峰86/libpinyin/lyyime/fcitx5/ibus 条目与字段齐全"

# ---------------------------------------------------------------------------
# 5. ime-add / ime-remove / ime-default 的 dry-run 命令预览(不真装包)
# ---------------------------------------------------------------------------
OUT="$("$BIN" ime-add pinyin)"
case "$OUT" in *"dnf install -y ibus-libpinyin"*) ;; *) fail "ime-add pinyin 预览缺 dnf install: $OUT" ;; esac
case "$OUT" in *DRY-RUN*) ;; *) fail "ime-add 默认必须 dry-run" ;; esac

OUT="$("$BIN" ime-remove table:wubi-jidian86)"
case "$OUT" in *"dnf remove -y ibus-table-chinese-wubi-jidian"*) ;; *) fail "ime-remove wubi-jidian86 预览缺 dnf remove: $OUT" ;; esac

OUT="$("$BIN" ime-default table:wubi-haifeng86)"
case "$OUT" in *"preload-engines ['table:wubi-haifeng86"*) ;; *) fail "ime-default 预览未把目标引擎放首位: $OUT" ;; esac

if "$BIN" ime-remove no-such-ime >/dev/null 2>&1; then
    fail "ime-remove 未知 id 应失败"
fi
if "$BIN" ime-add sogou-job >/dev/null 2>&1; then
    fail "ime-add 未知 id 应失败"
fi
pass "ime-add/remove/default dry-run 预览与保护性失败均符合预期"

# 审计日志已生成
[ -f "$HOME/.local/share/lyyime/logs/ime-manager.log" ] || fail "ime-manager.log 未生成"
pass "ime-manager 审计日志已写入"

echo ""
echo "doctor_test.sh 全部通过 ✅"
