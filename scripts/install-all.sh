#!/usr/bin/env bash
# lyyIme 总装脚本:构建 → 词库 → 核心库 → 双模式安装 → 体检
#   scripts/install-all.sh            # 用户级安装(推荐;ibus 组件装到 ~/.local/share)
#   scripts/install-all.sh --system   # 系统级(ibus 组件装到 /usr/local/share/ibus/component)
#   scripts/install-all.sh --no-xim   # 跳过 Mode B
#   scripts/install-all.sh --no-ibus  # 跳过 Mode A
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
export CARGO_TARGET_DIR=/data/cargo-target/local/lyyIme
PREFIX=/usr/local
LIBDIR=$PREFIX/lib/lyyime
DATADIR=$PREFIX/share/lyyime/data
SYSTEM=0; DO_XIM=1; DO_IBUS=1
for a in "$@"; do
  case "$a" in
    --system) SYSTEM=1 ;;
    --no-xim) DO_XIM=0 ;;
    --no-ibus) DO_IBUS=0 ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
    *) echo "未知参数: $a"; exit 1 ;;
  esac
done

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }

step "1/6 构建 Rust 工作区(release)"
cargo build --workspace --release

step "2/6 词库:生成/校验 data/runtime"
DICTTOOL=$CARGO_TARGET_DIR/release/dicttool
if [ ! -f data/runtime/meta.json ]; then
  "$DICTTOOL" convert --wubi-db /usr/share/ibus-table/tables/wubi-haifeng86.db --out data/runtime
  "$DICTTOOL" fetch --out data/runtime
fi
# 单字分档表(GB2312 常用字档,生僻字沉底依据):旧数据目录补齐
if [ ! -f data/runtime/char_tier.tsv ]; then
  "$DICTTOOL" tier --out data/runtime
fi
# 大写输入中文翻译表(`dicttool entrans` 产物,verify 必检项):缺失才重建
if [ ! -f data/runtime/en_trans.tsv ] && [ -f dicts/cache/stardict.db ]; then
  "$DICTTOOL" entrans --out data/runtime
fi
# 中英反查表(§15 候选右键「反查英文」):ECDICT 反生成 zh_en.tsv;
# 大表生成耗时,缺失才重建;stardict.db 不在时跳过(反查菜单给"无结果"提示)
if [ ! -f data/runtime/zh_en.tsv ] && [ -f dicts/cache/stardict.db ]; then
  "$DICTTOOL" zhen --out data/runtime
fi
"$DICTTOOL" verify data/runtime

step "3/6 安装核心库、词库、CLI 工具到 $PREFIX"
install -d "$LIBDIR" "$DATADIR" "$PREFIX/bin"
install -m 0755 "$CARGO_TARGET_DIR/release/liblyyime_core.so" "$LIBDIR/"
install -m 0644 data/runtime/*.tsv data/runtime/meta.json "$DATADIR/"
install -m 0755 "$CARGO_TARGET_DIR/release/lyyime-cli" "$CARGO_TARGET_DIR/release/lyyime-doctor" "$CARGO_TARGET_DIR/release/dicttool" "$CARGO_TARGET_DIR/release/lyyime-shot" "$PREFIX/bin/"
echo "$LIBDIR" > /etc/ld.so.conf.d/lyyime.conf 2>/dev/null && ldconfig || echo "  (非 root:跳过 ldconfig,宿主将按 LYYIME_CORE_LIB/路径探测加载)"

if [ "$DO_IBUS" = 1 ]; then
  step "4/6 Mode A:安装 ibus 引擎"
  if [ "$SYSTEM" = 1 ]; then bash ibus-engine/install.sh --system; else bash ibus-engine/install.sh; fi
fi

if [ "$DO_XIM" = 1 ]; then
  step "5/6 Mode B:构建并安装 lyyime-xim"
  make -C xim
  bash xim/install.sh
fi

if [ "${DO_FLOAT:-1}" = 1 ]; then
  step "5.5/6 Mode C:安装 lyyime-float 悬浮窗(Rust)"
  bash crates/lyyime-float/install.sh
fi

step "6/6 体检"
"$PREFIX/bin/lyyime-doctor" check || true
cat <<EOF

安装完成。
  Mode A:ibus 托盘选择 "lyyIme 五笔拼音"(如未出现:ibus restart)
  Mode B:运行 lyyime-xim,并让应用使用 XMODIFIERS=@im=lyyime GTK_IM_MODULE=xim(lyyime-doctor 可写入会话环境)
  Mode C:应用菜单 "lyyIme 悬浮窗输入"(lyyime-float, 兜底/免框架悬浮输入,支持自定义短语)
  截屏:lyyime-shot(热键默认 Ctrl+Alt+A,shot_hotkey 可配置;托盘菜单亦有入口)
  修复/管理:lyyime-doctor check | fix --all | ime-list | ime-add <id>
EOF
