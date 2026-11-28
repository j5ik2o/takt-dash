#!/usr/bin/env bash
# refreshDemand.qnt の不変条件が恒真でないことを確かめる（設計 docs/design/issue-40.md の「Quint」の (a)）。
# 変異のモジュール（11 の定数のうち 1 つだけを false にしたもの）ごとに、違反が出るはずの不変条件を渡して
# quint run を流し、どれも違反が出ることを確かめる。対照として、原本（fixed）の safety は違反なし。
# 無作為の探索では見つからない resetAttempts の latestNeverRejected は、決まった手順の
# resetAttemptsRejectsLatestTest を quint test で流して確かめる。
# --matrix を付けると、変異 11 × 不変条件 10 の 110 回を流し、違反した組が下の表と過不足なく一致することも
# 確かめる（表に無い違反が出ても失敗する。違反しない組は 20,000 本を流し切るので時間がかかる。所要は README）。
# リポジトリの根から実行する: specs/quint/refresh-demand/mutants.sh [--matrix]
# 手数・サンプル数・seed は STEPS・SAMPLES・SEED で変えられる（既定は README と設計の表の値）。
set -euo pipefail

dir=$(cd "$(dirname "$0")" && pwd)
spec="$dir/refreshDemand.qnt"
test_spec="$dir/refreshDemand_test.qnt"
quint=${QUINT:-./node_modules/.bin/quint}
if [ ! -x "$quint" ]; then
  quint=$(command -v quint)
fi
steps=${STEPS:-150}
samples=${SAMPLES:-20000}
seed=${SEED:-0x40}
matrix=0
case "${1:-}" in
  --matrix) matrix=1 ;;
  "") ;;
  *)
    echo "使い方: $0 [--matrix]" >&2
    exit 2
    ;;
esac
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

invariants=(
  liveConnsRegistered
  registryPointsToOpen
  supersededOnlyByNewer
  latestNeverRejected
  noOverdueEntry
  noExpiryBeforeFirstSend
  appliedTracksFloor
  pollerNotLate
  closedReleasedGuard
  noSendAfterExpiry
)

# 変異のモジュール|違反が出るはずの不変条件（空白区切り）。README の表と同じ。
mutants=(
  "acquireEarly|noExpiryBeforeFirstSend liveConnsRegistered"
  "noRejectStale|supersededOnlyByNewer"
  "keepSuperseded|liveConnsRegistered"
  "noOwnerCheck|liveConnsRegistered"
  "leakOnClose|registryPointsToOpen closedReleasedGuard"
  "lazyExpiry|noOverdueEntry"
  "expiryKeepsTask|liveConnsRegistered"
  "noPostAssemblyCheck|noSendAfterExpiry pollerNotLate"
  "fixedAtStart|appliedTracksFloor"
  "noRewake|pollerNotLate"
  "resetAttempts|supersededOnlyByNewer"
)

# 無作為の探索で出ても出なくてもよい組（本当の違反だが、探索では見つけにくい。決まった手順のテストで確かめる）。
optional=("resetAttempts latestNeverRejected")

# 1 回流して violation・none・error のどれかを出す。
run_one() {
  local main=$1 inv=$2
  if "$quint" run "$spec" --main="$main" --invariant="$inv" --max-steps="$steps" --max-samples="$samples" \
    --seed="$seed" >"$work/log" 2>&1; then
    echo none
  elif grep -q '\[violation\]' "$work/log"; then
    echo violation
  else
    echo error
  fi
}

# 違反の跡を再現する seed（quint の「Use --seed=… to reproduce」の行）。
seed_of() {
  sed -n 's/.*Use --seed=\(0x[0-9a-f]*\).*/再現の seed \1/p' "$1" | head -1
}

failed=0

result=$(run_one fixed safety)
if [ "$result" = none ]; then
  echo "OK（原本の safety は違反なし）"
else
  echo "NG（原本で $result）"
  tail -5 "$work/log"
  failed=1
fi

# --match が何も選ばなくても quint test は 0 で終わるので、そのテストが 1 件だけ流れたことも確かめる。
if "$quint" test "$test_spec" --main=refreshDemand_test --match='^resetAttemptsRejectsLatestTest$' >"$work/log" 2>&1 \
  && grep -q 'ok resetAttemptsRejectsLatestTest passed' "$work/log" && grep -q '^ *1 passing' "$work/log"; then
  echo "OK（resetAttemptsRejectsLatestTest：resetAttempts は latestNeverRejected に違反する）"
else
  echo "NG（resetAttemptsRejectsLatestTest が失敗した）"
  tail -5 "$work/log"
  failed=1
fi

contains() {
  local needle=$1
  shift
  local item
  for item in "$@"; do
    [ "$item" = "$needle" ] && return 0
  done
  return 1
}

for m in "${mutants[@]}"; do
  IFS='|' read -r main expected_raw <<<"$m"
  read -r -a expected <<<"$expected_raw"
  if [ "$matrix" = 1 ]; then
    targets=("${invariants[@]}")
  else
    targets=("${expected[@]}")
  fi
  for inv in "${targets[@]}"; do
    result=$(run_one "$main" "$inv")
    if contains "$inv" "${expected[@]}"; then
      if [ "$result" = violation ]; then
        echo "OK（$main は $inv に違反する。$(seed_of "$work/log")）"
      else
        echo "NG（$main は $inv に違反するはずが $result）"
        [ "$result" = error ] && tail -5 "$work/log"
        failed=1
      fi
    elif contains "$main $inv" "${optional[@]}"; then
      echo "OK（$main の $inv は探索で $result。決まった手順のテストで確かめる）"
    else
      case "$result" in
        none) echo "OK（$main は $inv に違反しない）" ;;
        violation)
          echo "NG（表に無い違反：$main が $inv に違反する。表を直すか変異を見直す）"
          failed=1
          ;;
        *)
          echo "NG（$main の $inv で quint が失敗した）"
          tail -5 "$work/log"
          failed=1
          ;;
      esac
    fi
  done
done
exit "$failed"
