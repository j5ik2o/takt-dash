# 未解決コメントの関門の承認判定（#134）

j5ik2o/ci の `scripts/review-thread-resolution.sh` が、トップレベルコメントとレビュー要約（項目）のどれを「承認済み」とみなすかを Quint で写したモデルです。仕様とコードは機械的にはつながっていません。判定の実装とスモークテストは j5ik2o/ci にあります。設計は `docs/design/issue-134.md` です。

- `reviewThreadAck.qnt`: モデル。`legacy` がいまの v1、`window` が [j5ik2o/ci#43](https://github.com/j5ik2o/ci/pull/43) の 442d679・86a443c（20 文字の窓）、`unique` が設計 b808a29（一意に指す値。ci#43 のレビューで見つかった穴 4 つが残る）、`fixed` がこの設計の規則（2 回目の直しの単位と極性、3 回目の直しの既定の除外、4 回目の直しの URL と HTML コメントの中の語）です。違いは定数 `RULE` だけで、不変条件は同じです。Codex の利用上限の通知を既定で外すのは `fixed` だけです（v2 の 3 回目の直し。ほかの 3 つの規則には既定の除外がありません）。`RULE` が 3 つの値のどれでもないと関門は緑にならず、witness の `gateGreen` が 0 件になります。
- `reviewThreadAck_test.qnt`: 筋書きを固定したテスト。名前の頭がどの規則で流すかを表します。`v1*` は `legacy`、`window*` は `window`、`unique*` は b808a29 の `unique`（穴 4 つが開いていることを固定する）、`scoped*` はこの設計の `fixed` です。
- `mutants.sh`: 不変条件が恒真でないことを確かめる、わざと壊した版の一覧と実行（下の「確かめ (a)」）。

## モデルの範囲

項目は 2 つです。本文は 20 文字以上の断片の集合に畳みます。`Own(i)` は項目 i だけが持つ散文、`Shared` は複数の項目に現れうる定型文（Codex の要約の見出し、利用上限の通知など）、`UrlHead` は URL の頭（`https://github.com/j` の 20 文字）です。

項目には真の種類（`Kind`）があります。`Plain`（ふつうの項目）、`CodexNotice`（Codex の利用上限の通知）、`CodexOther`（Codex のほかのコメント。本物のレビュー・要約・指摘）、`HeadByOther`（ほかの作者の、通知の文で始まるコメント）の 4 つです。規則が読めるのは、種類から決まる 2 つの観測、作者が Codex か（`byCodex`）と本文が通知の頭で始まるか（`noticeHead`）だけです。`fixed` は両方が真の項目だけを外します（`defaultExcluded`）。仕様は種類を読みます。観測が種類から決まること（作者が Codex で本文が通知の頭で始まるのは通知だけ）は環境の前提で、`mutants.sh` で外すと反例が出ます。本文の頭の当て方（先頭の空白、大小文字）は文字列の水準なので、モデルでは見ません（設計のテストの観点 30）。外した通知の断片は、断片がその項目だけのものかを決める「ほかの項目」に数えます（`uniqueTo` は外した項目も含めて見る）。

確認は、投稿の時点ではどの項目にも結び付けません。確認が持つのは、本文から観測できるもの（承認できる人か、自動報告か、1 つか 2 つの単位）と、作成者が承認するつもりだった項目（`intended`）、「まだ」と書いた項目（`pending`）です。単位は段落・リストの項目・節で、確認の語の現れ方（`Word`）、含むコメントの id、含む断片を持ちます。

| `Word` | 意味 | 例 |
| --- | --- | --- |
| `Affirmed` | 作成者が自分の文で書いた、打ち消されない確認の語 | `#10 を確認しました` |
| `Quoted` | 確認の語が引用・項目の写しの中にだけある（穴 2） | `> Once this is fixed, …` ＋ `Which tests should I run?` |
| `NegatedEn` | 英語の語が、助動詞を挟む否定・条件・未来で打ち消されている（穴 3） | `This has not been fixed.` |
| `NegatedJa` | 日本語の語が、後ろに付く否定で打ち消されている（穴 4） | `確認済みではありません` |
| `Pending` | 確認の語が無く、まだだと書いている | `#11 は未対応です` |
| `NoWord` | どれも無い | `対象: <URL>` |
| `MaskedWord` | 確認の語が URL か HTML コメントの中にだけある（穴 5・7。4 回目の直し） | `<!-- fixed --> #10`、`https://github.com/acme/fixed/pull/7#issuecomment-10` |
| `AffirmedMaskedCue` | 作成者の打ち消されない確認の語があり、打ち消し・まだ・条件の語は URL か HTML コメントの中にだけある（穴 6。4 回目の直し） | `https://github.com/acme/pending/pull/7#issuecomment-10 を確認しました`、`<!-- review-state: pending --> #10 を確認しました` |

規則は `intended`・`pending` を読まず、観測できるものと項目の本文を突き合わせて、どの確認がどの項目を承認するかを計算します。仕様は `intended` と編集の記録 `editLog` を読み、規則が読む `items` の `updatedAt` とは別に、最後の編集の時刻を求めます（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。どの文字列がどの `Word` になるかは文字列の水準の判定（設計の jq の `clauses` と `polarity`）で、モデルはその結果を観測として受け取ります。その判定の正しさは、設計のテストの観点 25〜29 の表で確かめます。

規則 `scoped`（`fixed`）は、単位の極性 `polarity`（`Affirmed`・`AffirmedMaskedCue` は `Pos`、`NegatedEn`・`NegatedJa`・`Pending` は `Neg`、ほかは `Neutral`）を使い、`Pos` の単位の中で確認の語と一意の参照（id か、その項目だけが持つ断片）がそろい、同じ確認の `Neg` の単位がその項目に触れていない（id か、URL の頭を除く、その項目が持つ断片）ときだけ承認します。`unique`（b808a29）と `window`（ci#43）は、語を本文の全体で見て（`NoWord`・`Pending` 以外のどれか 1 つでよい）、参照も本文の全体で探します。

確認の本文に何が現れうるかには、次の前提を置きます（`consistent`・`pendingOk`）。前提は規則の `polarity` を使わず、`Word` を直接読みます（規則を壊しても前提が一緒に動かないように）。

- 作成者の語の単位（`Affirmed`・`AffirmedMaskedCue`。仕様の `authorWord`）の id と固有の散文は、承認するつもりの項目か、同じ確認で「まだ」と書いた項目のもの。
- 打ち消しの単位（`NegatedEn`・`NegatedJa`・`Pending`）の id と固有の散文は、「まだ」と書いた項目のもの。「まだ」と書いた項目は、打ち消しの単位のどれかで触れている。
- 「まだ」と書いた項目は、承認するつもりの項目ではない。
- 定型文は、それを持つ項目がある PR にしか現れない。`Affirmed` の単位の定型文は、承認するつもりの項目か「まだ」と書いた項目のどれかが持つ。
- `Quoted`・`MaskedWord`・`NoWord` の単位の id と断片は、意図と無関係に現れうる（指摘を引用して質問する、別件として触れる）。URL の頭は、どの単位でも意図と無関係に現れる。

この前提を外すと、`fixed` でも `noFalseGreen` に反例が出ます（`mutants.sh` の最後の版）。結論はこの前提に依存します。

## 条件

不変条件 `safety` は次の 2 つの論理積です。

- `noFalseGreen`: 関門が緑なら、通知（`CodexNotice`）を除くすべての項目に、その項目を承認するつもりで、最後の編集より後に書かれた、自動報告でない、作成者が自分の文で確認の語を書いた（`Affirmed` か `AffirmedMaskedCue` の単位を持つ）確認がある。1 つの確認で指していない項目や「まだ」と書いた項目が承認されると反例になる。Codex のほかのコメントや、ほかの作者が通知の文で始めたコメントを外しても反例になる。
- `idAckTurnsGreen`: 通知を除くすべての項目に、その項目の id を作成者の語の単位（`Affirmed` か `AffirmedMaskedCue`）に含み、同じ確認の打ち消しの単位がその項目に触れていない有効な確認があれば緑（正当な確認を赤にしない）。打ち消しの単位で同じ定型文に触れた確認は、この条件の外です（どの項目のことか定まらないので、規則は赤の側に倒します）。通知に確認を求めないので、通知が外れずに残って赤になれば反例になります。

## 結果（quint 0.32.0）

いまの v1、ci#43 の規則、設計 b808a29 の規則は反例が出ます。

```
quint run reviewThreadAck.qnt --main legacy --invariant noFalseGreen --max-samples 50000 --max-steps 10 --seed 134
[violation] Found an issue
quint run reviewThreadAck.qnt --main window --invariant noFalseGreen --max-samples 50000 --max-steps 10 --seed 134
[violation] Found an issue
quint run reviewThreadAck.qnt --main unique --invariant noFalseGreen --max-samples 50000 --max-steps 10 --seed 134
[violation] Found an issue
```

`window` の反例には、両方の項目が `UrlHead` を持ち、`UrlHead` だけを含む確認（別件の URL を貼った「確認しました」）が項目を承認する形があります。[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44) と同じです。`unique` の反例の形は、テストの `uniqueScopeLeaksTest`・`uniqueQuotedWordTest`・`uniqueNegatedEnTest`・`uniqueNegatedJaTest` に 1 つずつ固定しています（ci#43 のレビューの穴 4 つ）。

この設計の規則は、同じ不変条件で反例なしです（50000 の標本を調べた範囲で。証明ではありません）。手元で 3 分 49 秒かかります（2026-11-28、4 回目の直しのあと）。

```
quint run reviewThreadAck.qnt --main fixed --invariant safety --max-samples 50000 --max-steps 10 --seed 134 --witnesses postedAck editedItem editInvalidatedAck approvedByQuoteOnly greenWithSharedText greenWithUrlHeadAck approvedByMixedAck vetoFired greenWithQuotedWordAck greenWithNegatedAck approvedByTwoUnitAck greenWithNoticeExcluded greenWithCodexOtherItem greenWithHeadByOtherItem greenWithMaskedWordAck approvedByMaskedCue gateGreen
[ok] No violation found
postedAck was witnessed in 49956 trace(s) out of 50000 explored (99.91%)
editedItem was witnessed in 49959 trace(s) out of 50000 explored (99.92%)
editInvalidatedAck was witnessed in 13063 trace(s) out of 50000 explored (26.13%)
approvedByQuoteOnly was witnessed in 5895 trace(s) out of 50000 explored (11.79%)
greenWithSharedText was witnessed in 9803 trace(s) out of 50000 explored (19.61%)
greenWithUrlHeadAck was witnessed in 11630 trace(s) out of 50000 explored (23.26%)
approvedByMixedAck was witnessed in 1543 trace(s) out of 50000 explored (3.09%)
vetoFired was witnessed in 4743 trace(s) out of 50000 explored (9.49%)
greenWithQuotedWordAck was witnessed in 6654 trace(s) out of 50000 explored (13.31%)
greenWithNegatedAck was witnessed in 9058 trace(s) out of 50000 explored (18.12%)
approvedByTwoUnitAck was witnessed in 8201 trace(s) out of 50000 explored (16.40%)
greenWithNoticeExcluded was witnessed in 4320 trace(s) out of 50000 explored (8.64%)
greenWithCodexOtherItem was witnessed in 4312 trace(s) out of 50000 explored (8.62%)
greenWithHeadByOtherItem was witnessed in 4368 trace(s) out of 50000 explored (8.74%)
greenWithMaskedWordAck was witnessed in 6853 trace(s) out of 50000 explored (13.71%)
approvedByMaskedCue was witnessed in 8014 trace(s) out of 50000 explored (16.03%)
gateGreen was witnessed in 12666 trace(s) out of 50000 explored (25.33%)
```

テストは次のとおりです。

```
quint test reviewThreadAck_test.qnt --main reviewThreadAck_test   # 25 passing
```

## 確かめ (a)：不変条件が恒真でない

`specs/quint/review-thread-ack/mutants.sh`（リポジトリの根から実行）は、原本のほかに 21 の壊した版を作り、それぞれ `fixed` で決めた不変条件に違反が出ることを確かめます。原本は `safety` で違反なしです。手元で全体 5 分 29 秒でした（2026-11-28、4 回目の直しのあと、21 版すべて OK）。

| 壊し方 | 違反が出る不変条件 |
| --- | --- |
| **穴 1**：確認の語と参照を単位で結ばない（`scopedRef` を「どれかの単位が `Pos`、どれかの単位が参照」に変える） | `noFalseGreen` |
| 穴 1：打ち消しの単位による拒否（`vetoed`）を消す | `noFalseGreen` |
| 穴 1：`Pending`（`未対応です`）を打ち消しと読まない | `noFalseGreen` |
| **穴 2**：`Quoted` を `Pos` と読む（引用の中の語を数える） | `noFalseGreen` |
| **穴 3**：`NegatedEn` を `Pos` と読む（`has not been fixed` を数える） | `noFalseGreen` |
| **穴 4**：`NegatedJa` を `Pos` と読む（`確認済みではありません` を数える） | `noFalseGreen` |
| **穴 5**：`MaskedWord` を `Pos` と読む（HTML コメントか URL の中だけの語を数える。06770ed の jq） | `noFalseGreen` |
| **穴 6**：`AffirmedMaskedCue` を `Neg` と読む（URL かボットの印の中の `pending` を打ち消しと読む。06770ed の jq） | `idAckTurnsGreen` |
| URL の頭を外すガード（`f != UrlHead`）を消す | `noFalseGreen`（URL の頭だけで当たる版） |
| ほかの項目に無い断片に限るガードを消す | `noFalseGreen` |
| 1 つの確認の断片で、別の項目も承認する（`uniqueTo(its, i, f)` を「どれかの項目に固有」に変える） | `noFalseGreen`（1 つの確認で別の項目が承認される版） |
| id の経路を消す | `idAckTurnsGreen` |
| 最終編集より後のガード（`isAfter`）を消す | `noFalseGreen` |
| 自動報告を外すガードを消す | `noFalseGreen` |
| 既定の除外を作者だけで当てる（Codex のほかのコメントも外す） | `noFalseGreen` |
| 既定の除外を本文の頭だけで当てる（ほかの作者の、通知の文で始まるコメントも外す） | `noFalseGreen` |
| 既定の除外を消す（通知が項目に残る） | `idAckTurnsGreen` |
| 外した通知を「ほかの項目」に数えない（`uniqueTo` が外した項目を見ない） | `noFalseGreen` |
| 遷移：編集で `updatedAt` を進めない | `noFalseGreen` |
| 遷移：観測の前提を外す（どの項目も作者が Codex に見える） | `noFalseGreen` |
| 遷移：確認の本文の現れ方の前提（`consistent`）を外す | `noFalseGreen` |

穴 4 つを開けたままの規則は、b808a29 の `unique` そのものです。上の「結果」のとおり、`unique` も `noFalseGreen` に違反が出ます。

## 確かめ (b)：遷移に届く

`step` の 2 つの action（`postAnyAck`、`editItem`）は、`postedAck`（99.91%）と `editedItem`（99.92%）で届きます。2 回目・3 回目・4 回目の直しで足した形にも届きます（数は 4 回目の直しのあとの走り）。

| witness | 意味 | 割合 |
| --- | --- | --- |
| `greenWithMaskedWordAck` | URL か HTML コメントの中だけの語の単位を含む確認がある PR で緑（4 回目） | 13.71% |
| `approvedByMaskedCue` | 打ち消しの語が URL か HTML コメントの中にだけある単位が、項目を承認する（4 回目） | 16.03% |
| `greenWithNoticeExcluded` | 既定で外した通知があり、ほかの項目の承認で緑（3 回目） | 8.64% |
| `greenWithCodexOtherItem` | Codex のほかのコメント（作者だけ一致）が項目に残り、確認で承認されて緑（3 回目） | 8.62% |
| `greenWithHeadByOtherItem` | ほかの作者の、通知の文で始まるコメント（本文だけ一致）が項目に残り、確認で承認されて緑（3 回目） | 8.74% |
| `approvedByMixedAck` | 「まだ」と書いた単位を含む確認（まとめ返信）が、別の項目を承認する | 3.09% |
| `vetoFired` | 肯定の単位が指した項目を、同じ確認の打ち消しの単位が拒否する | 9.49% |
| `greenWithQuotedWordAck` | 引用の中だけの語の単位を含む確認がある PR で緑 | 13.31% |
| `greenWithNegatedAck` | 打ち消された語（英語・日本語）の単位を含む確認がある PR で緑 | 18.12% |
| `approvedByTwoUnitAck` | 2 つの単位を持つ確認が項目を承認する | 16.40% |
| `editInvalidatedAck` | 確認が編集で無効になる（I4） | 26.13% |
| `approvedByQuoteOnly` | 引用だけで承認する | 11.79% |
| `greenWithSharedText` | 定型文がある PR で緑 | 19.61% |
| `greenWithUrlHeadAck` | URL の頭を含む確認がある PR で緑 | 23.26% |
| `gateGreen` | 緑（項目が 2 つとも通知で外れる、何も確かめない緑を含む） | 25.33% |

規則がいつも赤を返して違反を避けているのではありません。まとめ返信・打ち消し・引用・URL と HTML コメントの中だけの語の単位があっても、Codex のほかのコメントや通知の文で始まるコメントが項目に残っても、緑と承認に届きます。

## 確かめ (c)：実物との対応

行番号は j5ik2o/ci#43 の 86a443c の `scripts/review-thread-resolution.sh`（`filter_unacknowledged_comments` は 440 行目から）。判定の部分は 442d679 から変わっておらず（86a443c までの差分はテストとフィクスチャだけ）、行番号も同じです。

| モデル | 実物 |
| --- | --- |
| `items`（項目） | `$candidates[] \| select(is_item)`（601–602 行目。`is_item` の定義は 576 行目）。設計の jq では `$its` |
| `Item.updatedAt` | `$comment.updatedAt // $comment.createdAt`（607 行目） |
| `Item.frags` | 項目の `norm_text` の 20 文字の窓（545・556・575 行目）。設計では `quote_text`（URL を外してから正規化） |
| `ItemId`・`Unit.ids` | 項目の `.url` の `anchor_of`（558 行目）と `id_match`（562 行目） |
| `acks`（確認） | `$candidates[] \| select(is_acknowledger)`（605–606 行目。`is_acknowledger` の定義は 587 行目） |
| `Ack.at` | 確認の `.createdAt`（607 行目） |
| `Ack.eligible` | `is_acknowledger`（587 行目） |
| `Ack.autoReport` | `is_auto_report`（定義 522 行目）。実物は `$candidates` を作る時点（599 行目）で外す。モデルは確認の側の旗で持つ。項目の側の自動報告は、既定で外す Codex の利用上限の通知（`defaultExcluded`）だけを入れ、ほかは項目になる前に外れるのでモデルに入れない |
| `Ack.units`・`Unit` | 実物に無い。実物は本文を 1 つとして見る（`ack_phrase` も `references` も本文の全体）。設計の jq の `line_kinds`・`units`・`attach_targets`・`lead_of`・`clauses` |
| `Unit.word` | 実物では `ack_phrase`（534 行目）が真か偽かだけ。`strip_fences`（508 行目）はフェンスだけを外し、行頭 `>` の引用を外さない（穴 2）。`negated`（529 行目）は語の直前 12 文字と直後 3 文字だけを見る（穴 3・4）。設計の jq では `clauses` の masked と `polarity`・`negated2`・`pending_cue` |
| `Unit.frags` | 確認の `norm_text` の 20 文字の窓（568–575 行目）。設計では単位ごとの `refs_in` |
| `bodyAckWord`（`window`・`unique`） | `ack_phrase`（534 行目）。`Affirmed`・`Quoted`・`NegatedEn`・`NegatedJa` のどれか 1 つでも真 |
| `bodyIds`・`bodyFrags` | `references`（568 行目）が本文の全体を見ること |
| `refsWindow` | `references`（568 行目） |
| `refsUnique` | 実物に無い。設計 b808a29 の jq の `references($target; $ack; $others)` |
| `polarity`・`refsUnit`・`scopedRef`・`vetoed`・`approvesScoped` | 実物に無い（この設計で入れる）。設計の jq の `polarity`・`refs_in`・`mentions_in`・`acked_keys` |
| `approves` | `select(.createdAt > …) \| select(ack_phrase and references(…))`（607–608 行目）。設計の jq では `$acks` と `acked_keys` |
| `Item.byCodex`・`Item.noticeHead` | 実物に無い（v2 で入れる）。設計の jq の `default_ignored_notice` の `report_author_login`（作者が `chatgpt-codex-connector`）と `startswith`（本文が `You have reached your Codex usage limits` で始まる） |
| `defaultExcluded` | 実物に無い。設計の jq の `default_ignored_notice`。`is_auto_report` に入り、`$candidates` を作る時点で外れる |
| `uniqueTo` が外した項目も「ほかの項目」に数えること | 実物に無い。設計の jq の `$excl`（`refs_in` の `$others` に、外した通知の `quote_text` を足す） |
| `Word` の `MaskedWord`・`AffirmedMaskedCue`（4 回目） | 実物に無い。設計の jq の `acked_keys` の頭の `html_comment_lines`（フェンスの外の行全体の HTML コメントを除き、ほかの `<!--` があれば何も承認しない）、`clauses` の `$url`（`url_flags`。URL を masked で空白にし、節を切らない）。06770ed の jq は、どちらも masked に残していた |
| `gateGreen` | `filter_unacknowledged_comments` が空を返す（未解決スレッドの判定は範囲外）。`fixed` では既定で外した項目を待たない |
| `RULE = "v1"` | v1（2ead62a）の `filter_unacknowledged_comments` |
| `postAck`・`editItem` | GitHub でのコメントの投稿と編集（スクリプトの外のイベント） |

対応の無いもの：

- `Item.kind`：項目の真の種類。スクリプトは作者と本文しか読めない。仕様が「承認を待たなくてよい項目か」を規則と別に決めるためと、観測を決める `mkItem` のためだけに持つ。
- `intended`・`pending`：作成者の頭の中にあり、スクリプトは読めない。仕様と環境の前提が「承認するつもりだった項目」「まだと書いた項目」と突き合わせるためだけに持つ。
- `editLog`：GitHub の編集の履歴。スクリプトは `updatedAt` しか読まない。仕様が規則と別の経路で最後の編集を求めるためだけに持つ。
- `clock`：GitHub の時刻。順序だけを写す。
- `consistent`・`pendingOk`：環境の前提（上の「モデルの範囲」）。
- `lastStep`：witness のためだけ。
- `trusted_acknowledgement`（541 行目）による Codex の確認の項目からの除外、作成者のコメントを項目にしない規則、作成者が Bot のときの OWNER 等、未解決スレッド（248・275 行目）：この変更の承認の判定の外にあるか、v1 と同じなのでモデルに入れない。

## 確かめ (d)：検査の大きさと、見られないもの

- 大きさ：項目 2 つ、項目の種類 4 種、断片 4 種（固有 2・定型文 1・URL の頭 1）、確認の単位は 2 つまで、語の現れ方 8 種、ステップ 10（確認と編集で合わせて 10 回まで）、`quint run` の標本 50000（seed 134）。網羅ではない（標本を調べた範囲で反例が無い、という意味）。
- 見られないもの：
  - 項目が 3 つ以上のとき（3 つのうち 2 つだけが持つ定型文など）。規則はそのまま広がるが、モデルでは調べていない。
  - 単位が 3 つ以上の確認。前置きに続くリストの項目（モデルでは、前置きの語が効いた項目を `Affirmed` の単位として与える）。
  - 定型文が 2 種類以上あるとき、引用が固有の文と定型文にまたがるとき（モデルの断片は分けられない 1 つの記号）。
  - 文字列の水準：単位と節への分け方、引用・フェンスの付く先、確認の語と打ち消し・「まだ」の表明の一覧、項目の写しの印、正規化、URL の外し方の正規表現、フェンスの境界、20 文字の下限。設計の jq とスモークテスト（設計のテストの観点 25〜29 の表）で確かめる。
  - 時刻が同じ秒に並ぶとき。モデルの時刻は 1 ずつ進むので、確認と編集が同じ時刻になることがない。実物の `createdAt > updatedAt` は厳密な比較なので、編集と同じ秒の確認は数えない。
  - 前提（`consistent`・`pendingOk`）が崩れる入力。別の PR の項目の文を偶然 20 文字以上写した確認、作成者が「確認しました」と書きながら承認するつもりがない確認などは、モデルの外。
  - 未解決スレッドの判定、項目の選び方（作成者のコメント、Codex の承知）、ページ取得の順。
  - 既定の除外の文字列の水準：本文の頭の当て方（先頭の空白・改行、大小文字、通知の 2 つの文言）と、作者の login の比べ方（`[bot]` の有無）。設計のテストの観点 30 で確かめる。同じ login を騙ったアカウント、Codex が通知の文言を変えたときも、モデルの外。
  - HTML コメントと URL の文字列の水準（4 回目）：HTML コメントの境界（閉じない `<!--`、コードの中の `<!--`）、URL の範囲（ASCII の文字、終わりの句読点、scheme の無いホスト・パス）、HTML コメントの中の参照を除くこと。境界が決まらない確認を何も承認しないことも、モデルでは確かめない（どの単位にも語の無い確認と同じ形になる）。設計のテストの観点 34・35 の表で確かめる。
  - 既定で外す通知が 2 種類以上あるとき、v1 の既存の自動投稿の除外（CodeRabbit の要約、`Codex Review` の見出し）。どちらもモデルでは項目にならないものとして扱わない（項目の側の自動報告は、項目になる前に外れるので入れない。既定の除外だけは、片方だけの一致を外さないことを確かめるために入れた）。
