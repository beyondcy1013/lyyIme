#!/usr/bin/env bash
# 一键把 lyyIme(apps/lyyIme 子目录)以真实提交历史同步发布到 GitHub + Gitee。
# 用法:
#   scripts/publish-dual.sh            # 同步到两平台的 main
#   scripts/publish-dual.sh main       # 指定远端分支
# 前提:GitHub 推送走 gh 凭据(gh auth login && gh auth setup-git);
#       Gitee 推送走 SSH key(git@gitee.com)。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MONOREPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"   # apps/lyyIme/scripts -> /home/codes
BRANCH="${1:-main}"
GITHUB_URL="${GITHUB_URL:-https://github.com/beyondcy1013/lyyIme.git}"
GITEE_URL="${GITEE_URL:-git@gitee.com:beyondcy1013/lyyIme.git}"

cd "$MONOREPO_ROOT"
if ! git diff --quiet -- apps/lyyIme || ! git diff --cached --quiet -- apps/lyyIme; then
    echo "❌ apps/lyyIme 有未提交改动,先 commit 再发布" >&2
    exit 1
fi

echo "==> git subtree split apps/lyyIme -> lyyime-export"
git subtree split -P apps/lyyIme -b lyyime-export >/dev/null
trap 'git branch -D lyyime-export >/dev/null 2>&1 || true' EXIT

echo "==> push -> GitHub ($GITHUB_URL)"
git push -f "$GITHUB_URL" lyyime-export:"$BRANCH"

echo "==> push -> Gitee ($GITEE_URL)"
GIT_SSH_COMMAND="ssh -o StrictHostKeyChecking=accept-new" \
    git push -f "$GITEE_URL" lyyime-export:"$BRANCH"

echo "✅ 双平台已同步: $BRANCH @ $(git rev-parse --short lyyime-export)"
