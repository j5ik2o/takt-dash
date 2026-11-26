#!/bin/sh
# run の前に、PATH の takt が task_finish_hook を知る共有 TAKT かを確かめる（make takt-check）。
# 確かめの本体はフォーク（j5ik2o/takt の integration/takt-mcp）の scripts/check-takt-runtime.sh。
# 取り出せないときも、確かめが NG のときも、0 以外で終わる。README「run の前に TAKT の版を確かめる」。
set -u

repo=${TAKT_REPO:-$HOME/Sources/github.com/j5ik2o/takt}
check=$(mktemp) || exit 1
trap 'rm -f "$check"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

git -C "$repo" fetch -q origin integration/takt-mcp &&
  git -C "$repo" show origin/integration/takt-mcp:scripts/check-takt-runtime.sh > "$check" &&
  sh "$check" --key task_finish_hook --repo "$repo" --contains b1acb58ac
