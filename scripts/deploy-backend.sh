#!/bin/sh
# backend を作り直し、launchd のジョブを bootout と bootstrap で起こし直す。README「backend を反映する」。
#
# kickstart -k で起こし直さない。ログインのときに読まれたジョブには、BTM（Background Task
# Management）が plist の実行ファイルの署名から作った起動の制約（LWCR）が付く。ad-hoc 署名の
# cdhash はビルドのたびに変わるので、作り直したあとの最初の起動を、カーネルが
# OS_REASON_CODESIGNING（Launch Constraint Violation）で止める（issue #402）。
# launchctl bootstrap で読み直したジョブには、この制約が付かない。
#
# 使い方: scripts/deploy-backend.sh --label <launchd のラベル> [--plist <plist>] [--skip-build]
#   --plist       既定は ~/Library/LaunchAgents/<ラベル>.plist
#   --skip-build  ビルドせず、いまのバイナリで起こし直す（前の版へ戻したあとなど）
set -u

die() {
  echo "NG: $*" >&2
  exit 1
}

label=""
plist=""
build=1
while [ $# -gt 0 ]; do
  case "$1" in
    --label) [ $# -ge 2 ] || die "--label にはラベルが必要です。"; label=$2; shift 2 ;;
    --plist) [ $# -ge 2 ] || die "--plist にはパスが必要です。"; plist=$2; shift 2 ;;
    --skip-build) build=0; shift ;;
    *) die "未知の引数です: $1" ;;
  esac
done
[ -n "$label" ] || die "--label を指定してください。"
[ -n "$plist" ] || plist="$HOME/Library/LaunchAgents/$label.plist"
[ -f "$plist" ] || die "plist がありません: $plist"

app_dir=$(cd "$(dirname "$0")/.." && pwd) || exit 1
binary="$app_dir/target/debug/takt-dash-backend"
program=$(plutil -extract ProgramArguments.0 raw -o - "$plist") || die "plist の ProgramArguments を読めません: $plist"
# plist が別のバイナリを指していると、作り直しても起こし直しても新しい版にならない。
[ "$program" = "$binary" ] || die "plist の実行ファイル $program は、このチェックアウトのバイナリ $binary ではありません。"

if [ "$build" = 1 ]; then
  (cd "$app_dir" && cargo build --locked -p takt-dash-backend) || die "ビルドに失敗しました。動いている版はそのままです。"
fi
codesign -v "$binary" || die "バイナリの署名を確かめられません: $binary"
echo "バイナリ: $binary sha256 $(shasum -a 256 "$binary" | cut -d' ' -f1)"

domain="gui/$(id -u)"
service="$domain/$label"
# bootout は、ジョブが終わるまで待ってから戻る（plist の ExitTimeOut が上限）。
if launchctl print "$service" > /dev/null 2>&1; then
  launchctl bootout "$service" || die "bootout に失敗しました: $service"
fi
attempt=0
until launchctl bootstrap "$domain" "$plist"; do
  attempt=$((attempt + 1))
  [ "$attempt" -lt 10 ] || die "bootstrap に失敗しました。launchctl bootstrap $domain $plist を流し直してください。"
  sleep 1
done

# 最初の起動が止められていないことを確かめる。止められると、launchd は数秒おいて起こし直すので、
# pid が変わるか、last exit code に値が入る。
sleep 1
first_pid=$(launchctl print "$service" | awk '$1 == "pid" { print $3 }')
sleep 3
state=$(launchctl print "$service")
pid=$(printf '%s\n' "$state" | awk '$1 == "pid" { print $3 }')
last_exit=$(printf '%s\n' "$state" | awk -F' = ' '$1 ~ /last exit code$/ { print $2 }')
if [ -z "$first_pid" ] || [ "$pid" != "$first_pid" ] || [ "$last_exit" != "(never exited)" ]; then
  printf '%s\n' "$state" | grep -E 'state =|pid =|last exit' >&2
  die "起こし直した backend が動き続けていません（最初の pid ${first_pid:-なし}、いまの pid ${pid:-なし}、last exit code ${last_exit:-なし}）。"
fi
echo "OK: $label を pid $pid で起こし直しました。"
