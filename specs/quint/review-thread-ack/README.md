# 未解決コメントの関門の承認判定（#134）

j5ik2o/ci の `scripts/review-thread-resolution.sh` が、トップレベルコメントとレビュー要約（項目）のどれを「承認済み」とみなすかを Quint で写したモデルです。仕様とコードは機械的にはつながっていません。判定の実装とスモークテストは j5ik2o/ci にあります。設計は `docs/design/issue-134.md` です。

- `reviewThreadAck.qnt`: モデル。`legacy` がいまの v1、`window` が [j5ik2o/ci#43](https://github.com/j5ik2o/ci/pull/43) の 442d679（20 文字の窓）、`fixed` がこの設計の規則です。違いは定数 `RULE` だけで、不変条件は同じです。`RULE` が 3 つの値のどれでもないと関門は緑にならず、witness の `gateGreen` が 0 件になります。
- `reviewThreadAck_test.qnt`: 筋書きを固定したテスト。
- `mutants.sh`: 不変条件が恒真でないことを確かめる、わざと壊した版の一覧と実行（下の「確かめ (a)」）。

## モデルの範囲

項目は 2 つです。本文は 20 文字以上の断片の集合に畳みます。`Own(i)` は項目 i だけが持つ散文、`Shared` は複数の項目に現れうる定型文（Codex の要約の見出し、利用上限の通知など）、`UrlHead` は URL の頭（`https://github.com/j` の 20 文字）です。

確認は、投稿の時点ではどの項目にも結び付けません。確認が持つのは、本文から観測できるもの（承認できる人か、自動報告か、確認の語があるか、含むコメントの id、含む断片）と、作成者が指すつもりだった項目（`intended`）です。規則は `intended` を読まず、観測できるものと項目の本文を突き合わせて、どの確認がどの項目を承認するかを計算します。仕様は `intended` と編集の記録 `editLog` を読み、規則が読む `items` の `updatedAt` とは別に、最後の編集の時刻を求めます（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。

確認の本文に何が現れうるかには、次の前提を置きます（`consistent`）。

- コメントの id は、その項目を指すつもりのときにしか貼られない。
- 項目に固有の散文は、その項目を指すつもりのときにしか引用されない。
- 定型文は、それを持つ項目のどれかを指すつもりのときにしか引用されない（どれを指すかは定まらない）。
- URL の頭は、別件の URL を貼るだけで現れる。意図と無関係。

この前提を外すと、`fixed` でも `noFalseGreen` に反例が出ます（`mutants.sh` の最後の版）。結論はこの前提に依存します。

## 条件

不変条件 `safety` は次の 2 つの論理積です。

- `noFalseGreen`: 関門が緑なら、すべての項目に、その項目を指すつもりで、最後の編集より後に書かれた、自動報告でない確認の語つきの確認がある。1 つの確認で指していない項目が承認されると反例になる。
- `idAckTurnsGreen`: すべての項目に、その項目の id を含む有効な確認があれば緑（正当な確認を赤にしない）。

## 結果（quint 0.32.0）

いまの v1 と ci#43 の規則は反例が出ます。

```
quint run reviewThreadAck.qnt --main legacy --invariant noFalseGreen --max-samples 50000 --max-steps 10 --seed 134
[violation] Found an issue
quint run reviewThreadAck.qnt --main window --invariant noFalseGreen --max-samples 50000 --max-steps 10 --seed 134
[violation] Found an issue
```

`window` の反例は、両方の項目が `UrlHead` を持ち、`UrlHead` だけを含む確認（別件の URL を貼った「確認しました」）が項目を承認する形です。[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44) と同じです。

この設計の規則は、同じ不変条件で反例なしです（50000 の標本を調べた範囲で。証明ではありません）。

```
quint run reviewThreadAck.qnt --main fixed --invariant safety --max-samples 50000 --max-steps 10 --seed 134 --witnesses postedAck editedItem editInvalidatedAck approvedByQuoteOnly greenWithSharedText greenWithUrlHeadAck gateGreen
[ok] No violation found
postedAck was witnessed in 49944 trace(s) out of 50000 explored (99.89%)
editedItem was witnessed in 49949 trace(s) out of 50000 explored (99.90%)
editInvalidatedAck was witnessed in 20024 trace(s) out of 50000 explored (40.05%)
approvedByQuoteOnly was witnessed in 7620 trace(s) out of 50000 explored (15.24%)
greenWithSharedText was witnessed in 3973 trace(s) out of 50000 explored (7.95%)
greenWithUrlHeadAck was witnessed in 4528 trace(s) out of 50000 explored (9.06%)
gateGreen was witnessed in 5125 trace(s) out of 50000 explored (10.25%)
```

テストは次のとおりです。

```
quint test reviewThreadAck_test.qnt --main reviewThreadAck_test   # 11 passing
```

## 確かめ (a)：不変条件が恒真でない

`specs/quint/review-thread-ack/mutants.sh`（リポジトリの根から実行）は、原本のほかに 8 つの壊した版を作り、それぞれ `fixed` で決めた不変条件に違反が出ることを確かめます。原本は `safety` で違反なしです。

| 壊し方 | 違反が出る不変条件 |
| --- | --- |
| URL の頭を外すガード（`f != UrlHead`）を消す | `noFalseGreen`（URL の頭だけで当たる版） |
| ほかの項目に無い断片に限るガードを消す | `noFalseGreen` |
| 1 つの確認の断片で、別の項目も承認する（`uniqueTo(its, i, f)` を「どれかの項目に固有」に変える） | `noFalseGreen`（1 つの確認で別の項目が承認される版） |
| id の経路を消す | `idAckTurnsGreen` |
| 最終編集より後のガード（`isAfter`）を消す | `noFalseGreen` |
| 自動報告を外すガードを消す | `noFalseGreen` |
| 遷移：編集で `updatedAt` を進めない | `noFalseGreen` |
| 遷移：確認の本文の現れ方の前提（`consistent`）を外す | `noFalseGreen` |

## 確かめ (b)：遷移に届く

`step` の 2 つの action（`postAnyAck`、`editItem`）は、`postedAck`（99.89%）と `editedItem`（99.90%）で届きます。確認が編集で無効になる遷移（`editInvalidatedAck`、40.05%）、引用だけでの承認（`approvedByQuoteOnly`、15.24%）、定型文がある PR での緑（`greenWithSharedText`、7.95%）、URL の頭を含む確認がある PR での緑（`greenWithUrlHeadAck`、9.06%）、緑（`gateGreen`、10.25%）にも届きます。規則がいつも赤を返して違反を避けているのではありません。

## 確かめ (c)：実物との対応

行番号は j5ik2o/ci#43 の 442d679 の `scripts/review-thread-resolution.sh`（`filter_unacknowledged_comments` は 440 行目から）。

| モデル | 実物 |
| --- | --- |
| `items`（項目） | `$candidates[] \| select(is_item)`（601–602 行目。`is_item` の定義は 576 行目）。設計の jq では `$its` |
| `Item.updatedAt` | `$comment.updatedAt // $comment.createdAt`（607 行目） |
| `Item.frags` | 項目の `norm_text` の 20 文字の窓（545・556・575 行目）。設計では `quote_text`（URL を外してから正規化） |
| `ItemId`・`Ack.ids` | 項目の `.url` の `anchor_of`（558 行目）と `id_match`（562 行目） |
| `acks`（確認） | `$candidates[] \| select(is_acknowledger)`（605–606 行目。`is_acknowledger` の定義は 587 行目） |
| `Ack.at` | 確認の `.createdAt`（607 行目） |
| `Ack.eligible` | `is_acknowledger`（587 行目） |
| `Ack.autoReport` | `is_auto_report`（定義 522 行目）。実物は `$candidates` を作る時点（599 行目）で外す。モデルは確認の側の旗で持つ。項目の側の自動報告は、項目になる前に外れるのでモデルに入れない |
| `Ack.ackWord` | `ack_phrase`（534 行目） |
| `Ack.frags` | 確認の `norm_text` の 20 文字の窓（568–575 行目） |
| `refsWindow` | `references`（568 行目） |
| `refsUnique` | 実物に無い（この設計で入れる）。設計の jq の `references($target; $ack; $others)` |
| `approves` | `select(.createdAt > …) \| select(ack_phrase and references(…))`（607–608 行目） |
| `gateGreen` | `filter_unacknowledged_comments` が空を返す（未解決スレッドの判定は範囲外） |
| `RULE = "v1"` | v1（2ead62a）の `filter_unacknowledged_comments` |
| `postAck`・`editItem` | GitHub でのコメントの投稿と編集（スクリプトの外のイベント） |

対応の無いもの：

- `intended`：作成者の頭の中にあり、スクリプトは読めない。仕様が「指すつもりだった項目」と突き合わせるためだけに持つ。
- `editLog`：GitHub の編集の履歴。スクリプトは `updatedAt` しか読まない。仕様が規則と別の経路で最後の編集を求めるためだけに持つ。
- `clock`：GitHub の時刻。順序だけを写す。
- `consistent`：環境の前提（上の「モデルの範囲」）。
- `lastStep`：witness のためだけ。
- `trusted_acknowledgement`（541 行目）による Codex の確認の項目からの除外、作成者のコメントを項目にしない規則、作成者が Bot のときの OWNER 等、未解決スレッド（248・275 行目）：この変更の承認の判定の外にあるか、v1 と同じなのでモデルに入れない。

## 確かめ (d)：検査の大きさと、見られないもの

- 大きさ：項目 2 つ、断片 4 種（固有 2・定型文 1・URL の頭 1）、ステップ 10（確認と編集で合わせて 10 回まで）、`quint run` の標本 50000（seed 134）。網羅ではない（標本を調べた範囲で反例が無い、という意味）。
- 見られないもの：
  - 項目が 3 つ以上のとき（3 つのうち 2 つだけが持つ定型文など）。規則はそのまま広がるが、モデルでは調べていない。
  - 定型文が 2 種類以上あるとき、引用が固有の文と定型文にまたがるとき（モデルの断片は分けられない 1 つの記号）。
  - 文字列の水準：正規化、URL の外し方の正規表現、フェンスの境界、確認の語と否定、20 文字の下限。jq の実装とスモークテストで確かめる。
  - 時刻が同じ秒に並ぶとき。モデルの時刻は 1 ずつ進むので、確認と編集が同じ時刻になることがない。実物の `createdAt > updatedAt` は厳密な比較なので、編集と同じ秒の確認は数えない。
  - 前提（`consistent`）が崩れる入力。別の PR の項目の文を偶然 20 文字以上写した確認などは、モデルの外。
  - 未解決スレッドの判定、項目の選び方（作成者のコメント、Codex の承知）、ページ取得の順。
