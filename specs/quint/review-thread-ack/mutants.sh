#!/usr/bin/env bash
# reviewThreadAck.qnt の不変条件が恒真でないことを確かめる。
# 規則のガードを 1 つ外した版、ci#43 の残る穴 4 つを 1 つずつ開けた版、遷移を 1 つ変えた版を作り、どれも `fixed` で
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
  "1 つの確認の断片で、別の項目も承認する|    u.ids.contains(i) or u.frags.exists(f => uniqueTo(its, i, f))|    u.ids.contains(i) or u.frags.exists(f => ITEMS.exists(k => uniqueTo(its, k, f)))|noFalseGreen"
  "id の経路を消す（正当な確認が赤になる）|    u.ids.contains(i) or u.frags.exists(f => uniqueTo(its, i, f))|    u.frags.exists(f => uniqueTo(its, i, f))|idAckTurnsGreen"
  "最終編集より後のガードを消す|    isAfter(its.get(i), a),|    true,|noFalseGreen"
  "自動報告を外すガードを消す|    not(a.autoReport),|    true,|noFalseGreen"
  "穴 1：確認の語と参照を単位で結ばない（本文全体で見る）|    a.units.exists(u => polarity(u.word) == Pos and refsUnit(its, i, u))|    a.units.exists(u => polarity(u.word) == Pos) and a.units.exists(u => refsUnit(its, i, u))|noFalseGreen"
  "穴 1：打ち消しの単位による拒否を消す|    not(vetoed(its, i, a)),|    true,|noFalseGreen"
  "穴 1：まだの表明（未対応です）を打ち消しと読まない|    else if (w == Pending) Neg|    else if (w == Pending) Neutral|noFalseGreen"
  "穴 2：引用の中の確認の語を数える|    else if (w == Quoted) Neutral|    else if (w == Quoted) Pos|noFalseGreen"
  "穴 3：英語の助動詞を挟む否定を確認と数える|    else if (w == NegatedEn) Neg|    else if (w == NegatedEn) Pos|noFalseGreen"
  "穴 4：日本語の後ろに付く否定を確認と数える|    else if (w == NegatedJa) Neg|    else if (w == NegatedJa) Pos|noFalseGreen"
  "遷移: 編集で最終編集の時刻を進めない|    items' = items.set(id, { ...items.get(id), updatedAt: clock + 1 }),|    items' = items,|noFalseGreen"
  "遷移: 確認の本文の現れ方の前提を外す|    nondet intended = ITEMS.powerset().filter(s => consistent(items, units, s, pending)).oneOf()|    nondet intended = ITEMS.powerset().oneOf()|noFalseGreen"
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
