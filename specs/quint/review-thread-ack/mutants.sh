#!/usr/bin/env bash
# reviewThreadAck.qnt の不変条件が恒真でないことを確かめる。
# 規則のガードを 1 つ外した版と、遷移を 1 つ変えた版を作り、どれも `fixed` で
# 違反が出ることを確かめる（どの不変条件に出るかも決めておく）。違反が出ない版が 1 つでもあれば失敗する。
# リポジトリの根から実行する: specs/quint/review-thread-ack/mutants.sh
set -euo pipefail

dir=$(cd "$(dirname "$0")" && pwd)
spec="$dir/reviewThreadAck.qnt"
quint=${QUINT:-./node_modules/.bin/quint}
if [ ! -x "$quint" ]; then
  quint=$(command -v quint)
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# 名前|置き換える前の行|置き換えた後の行|違反が出るはずの不変条件
mutants=(
  "URL の頭を外すガードを消す|    f != UrlHead|    true|noFalseGreen"
  "ほかの項目に無い断片に限るガードを消す|      and ITEMS.forall(j => j != i implies not(its.get(j).frags.contains(f)))|      and true|noFalseGreen"
  "1 つの確認の断片で、別の項目も承認する|    a.ids.contains(i) or a.frags.exists(f => uniqueTo(its, i, f))|    a.ids.contains(i) or a.frags.exists(f => ITEMS.exists(k => uniqueTo(its, k, f)))|noFalseGreen"
  "id の経路を消す（正当な確認が赤になる）|    a.ids.contains(i) or a.frags.exists(f => uniqueTo(its, i, f))|    a.frags.exists(f => uniqueTo(its, i, f))|idAckTurnsGreen"
  "最終編集より後のガードを消す|    else if (RULE == \"unique\") candidate(a) and isAfter(its.get(i), a) and refsUnique(its, i, a)|    else if (RULE == \"unique\") candidate(a) and refsUnique(its, i, a)|noFalseGreen"
  "自動報告を外すガードを消す|  pure def candidate(a: Ack): bool = a.eligible and not(a.autoReport) and a.ackWord|  pure def candidate(a: Ack): bool = a.eligible and a.ackWord|noFalseGreen"
  "遷移: 編集で最終編集の時刻を進めない|    items' = items.set(id, { ...items.get(id), updatedAt: clock + 1 }),|    items' = items,|noFalseGreen"
  "遷移: 確認の本文の現れ方の前提を外す|    nondet fs = FRAGS.powerset().filter(fs => consistent(items, intended, ids, fs)).oneOf()|    nondet fs = FRAGS.powerset().oneOf()|noFalseGreen"
)

failed=0
# 対照：壊していない原本では違反が出ない（どの版でも違反と出る道具でないことを確かめる）。
if "$quint" run "$spec" --main=fixed --invariant=safety --max-steps=10 --max-samples=50000 --seed=134 >"$work/log" 2>&1; then
  echo "OK（原本は違反なし）"
else
  echo "NG（原本で違反か失敗）"
  tail -5 "$work/log"
  failed=1
fi
for m in "${mutants[@]}"; do
  IFS='|' read -r name before after inv <<<"$m"
  out="$work/mutant.qnt"
  count=$(grep -cxF -- "$before" "$spec" || true)
  if [ "$count" != 1 ]; then
    echo "NG（置き換える行が $count 行）: $name"
    failed=1
    continue
  fi
  BEFORE="$before" AFTER="$after" awk '$0 == ENVIRON["BEFORE"] { print ENVIRON["AFTER"]; next } { print }' "$spec" >"$out"
  if "$quint" run "$out" --main=fixed --invariant="$inv" --max-steps=10 --max-samples=50000 --seed=134 >"$work/log" 2>&1; then
    echo "NG（違反が出ない）: $name"
    failed=1
  elif grep -q '\[violation\]' "$work/log"; then
    echo "OK（$inv に違反が出る）: $name"
  else
    echo "NG（quint が失敗した）: $name"
    tail -5 "$work/log"
    failed=1
  fi
done
exit "$failed"
