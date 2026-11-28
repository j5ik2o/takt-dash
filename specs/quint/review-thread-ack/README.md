# 未解決コメントの関門の承認判定（#134）

j5ik2o/ci の `scripts/review-thread-resolution.sh` が、トップレベルコメントとレビュー要約（項目）のどれを「承認済み」とみなすかを Quint で写したモデルです。仕様とコードは機械的にはつながっていません。判定の実装とスモークテストは j5ik2o/ci にあります。設計は `docs/design/issue-134.md` です。

- `reviewThreadAck.qnt`: モデル。`legacy` がいまの v1、`window` が [j5ik2o/ci#43](https://github.com/j5ik2o/ci/pull/43) の 442d679（20 文字の窓）、`fixed` がこの設計の規則です。違いは定数 `RULE` だけで、不変条件は同じです。
- `reviewThreadAck_test.qnt`: 筋書きを固定したテスト。

## モデルの範囲

項目は 2 つです。本文は 20 文字以上の断片の集合に畳みます。`U1` / `U2` はその項目だけが持つ散文、`S` は複数の項目に現れうる定型文（Codex の要約の見出し、利用上限の通知など）、`H` は URL の頭（`https://github.com/j` の 20 文字）です。

確認は、投稿の時点ではどの項目にも結び付けません。確認が持つのは、本文から観測できるもの（承認できる人か、自動報告か、確認の語があるか、含むコメントの id、含む断片）と、作成者が指すつもりだった項目（`intended`）です。規則は `intended` を読まず、観測できるものと項目の本文を突き合わせて、どの確認がどの項目を承認するかを計算します。`intended` を読むのは仕様だけです（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。

確認の本文に何が現れうるかには、次の前提を置きます（`consistent`）。

- コメントの id は、その項目を指すつもりのときにしか貼られない。
- 項目に固有の散文は、その項目を指すつもりのときにしか引用されない。
- 定型文は、それを持つ項目のどれかを指すつもりのときにしか引用されない（どれを指すかは定まらない）。
- URL の頭は、別件の URL を貼るだけで現れる。意図と無関係。

承認できる人か、自動報告か、確認の語があるかは、観測できる真偽値として与えます。文字列の正規化、確認の語の一覧と否定、フェンスの扱い、20 文字の下限は、モデルではなく jq の実装とスモークテストで確かめます。

## 条件

不変条件 `safety` は次の 2 つの論理積です。

- `noFalseGreen`: 関門が緑なら、すべての項目に、その項目を指すつもりの有効な確認がある。1 つの確認で指していない項目が承認されると反例になる。
- `idAckTurnsGreen`: すべての項目に、その項目の id を含む有効な確認があれば緑（正当な確認を赤にしない）。

## 結果（quint 0.32.0）

いまの v1 と ci#43 の規則は反例が出ます。

```
quint run reviewThreadAck.qnt --main legacy --invariant noFalseGreen --max-samples 50000 --max-steps 10 --seed 134
[violation] Found an issue
quint run reviewThreadAck.qnt --main window --invariant noFalseGreen --max-samples 50000 --max-steps 10 --seed 134
[violation] Found an issue
```

`window` の反例は、両方の項目が `H` を持ち、項目 1 を指すつもりで `H` だけを含む確認（別件の URL を貼った「確認しました」）が、項目 2 も承認する形です。[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44) と同じです。

この設計の規則は、同じ不変条件で反例なしです。

```
quint run reviewThreadAck.qnt --main fixed --invariant safety --max-samples 50000 --max-steps 10 --seed 134 --witnesses gateGreen
[ok] No violation found
gateGreen was witnessed in 5038 trace(s) out of 50000 explored (10.08%)
```

`fixed` の規則から、URL の頭を外す条件（`f != URL_HEAD`）か、ほかの項目に無い断片に限る条件のどちらかを消すと、`noFalseGreen` に反例が出ます（手元で確かめました）。

テストは次のとおりです。

```
quint test reviewThreadAck_test.qnt --main reviewThreadAck_test   # 11 passing
```
