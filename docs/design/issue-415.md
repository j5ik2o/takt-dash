# issue #415 の設計：送信中の二重の確定の守り、使われない answer-report の道、送信の失敗の後のテスト

対象の issue：[j5ik2o/takt-dash#415](https://github.com/j5ik2o/takt-dash/issues/415)（#404 / #308 のマージ後の点検の低の指摘）
関連：`docs/design/issue-308.md`、`specs/quint/decision-offline-actions/`

## 何をどう変えるか

1. 送信中の守りを ref で持つ（`frontend/web/src/components/decision-actions.tsx`）。
   - `DecisionActions` に `submitting = useRef(false)` を足す。`run` の頭で、`action` が無いか切断中なら返す既存の判定の後に、`submitting.current` が true なら返す。false なら、`await` の前に同じ同期の区間で true にしてから `setBusy(true)` する。
   - 守りを戻す処理（`submitting.current = false`）は `try … finally` の `finally` に置く。`decisionAttempt`、`submitDecisionAction`（`json` が関数でない応答など）、`onResult` で例外が出ても ref が true のまま残らない。残ると、下の入口の守りが「やめる」まで捨てるので、ダイアログを閉じられなくなる。
   - `handleAction` の入口でも、`submitting.current` が true なら、どの意図も受け付けずに返す。送信中は、画面のボタン（トリガー、確定、やめる、回答する、選択肢）がすべて `disabled={busy …}` なので、受け付けない意図の範囲は画面と同じになる。これで、1つの act（描き直しの前）の中で確定の後に届いた「やめる」や Escape も、送信中に閉じない。
   - `handleAction` は、意図を受け付けたかを boolean で返す（`onAction: (intent) => boolean`。`DecisionActionsView`、`DialogActionSection`、`ConfirmDialog` の型も合わせる）。`acceptsDecisionIntent` で止めたときと、送信中で止めたときは false を返す。
   - `busy` の state は残す。描画（disabled、フォーカスを見出しへ移すこと、Escape の判定）は今のまま `busy` を見る。ref は、描き直しを待たずに見るための入口の守りだけに使う。
2. `ConfirmDialog` の閉じる途中の扱いを直す（`frontend/web/src/components/confirm-dialog.tsx`）。
   - `cancel` は、先に `released.current = true` にせず、`onAction({ type: "cancel" })` が true を返したときだけ立てる。今は先に立てるので、送信中に「やめる」や Escape を押すと、`handleAction` が捨ててダイアログは開いたままなのに、フォーカスの封じ込め（`handleKeyDown`、`handleFocusOut`、`handleFocusIn`）が送信の終わりまで外れる。Escape も、`busyRef` が `useEffect` で写るので描き直しの前は false に見え、同じ道を通る。
   - `confirm` は、`released.current` が true なら返す。同じ描き直しの区間で「やめる」の後に確定を押すと、`handleAction` は描画の時点の `dialogState`（開いたまま）で解決するので、閉じたはずの確認から送ってしまう。送信中の ref はこの場合を止めない（送信はまだ始まっていない）。
3. 使われない `answer-report` の意図を消す（`frontend/web/src/state/decision-actions.ts`）。
   - `DecisionIntent` から `{ type: "answer-report"; report: number }` を除き、`resolveDecisionIntent` の `case "answer-report"` を消す。画面の2択は `DialogActionSection` が `{ type: "open", dialog }` を出しており、この意図を出す所はテストしか無い（src の外からの参照が無いことは確かめた）。
   - 型で絞る案（`report: 1 | 2`）は採らない。絞っても、画面が出さない意図と、`open` と同じ働きの分岐が残る。開く道を `open` の1つにすると、読む人が確認なしで送る道を探さずに済む。
   - 波及する所：
     - `test/state/decision-actions.test.ts:491` のテスト名（「作業の依頼の意図は送らずに確認を開く」）と `:499-504` の2択のループ（説明の行を含む）。ループを消し、テスト名を自由記述だけに絞るか、`open` の意図で開くことを確かめる形にする。
     - `test/components/issue-308-operation-confirm.test.ts:1-9` の冒頭の説明（「2択の意図から開いた状態をそのまま使い」）と `:84` の `openByReport` の説明、`:86` の呼び出し。`openByReport` は `DIALOG_TRIGGERS.operation` から押したボタンの名前で `dialog` を引き、`{ type: "open", dialog }` で開く（種別の名前をテストに書かない方針は保つ）。
     - `test/components/decision-actions.test.ts:989` の切断中の意図の一覧から、この意図を除く。
     - `docs/design/issue-308.md:10`。過去の設計なので書き換えず、#415 で `answer-report` を消したことを注記するだけにする。

## 変えないこと

- 開いた直後の確定を黙って捨てる挙動（issue の「採らないもの」）。確定のボタンは 500ms の間も disabled にせず、見た目も変えない。
- 500ms の守り（`useConfirmAcceptance`）、`decideDialogKey` の Escape（送信中は取り消さない）、フォーカスの封じ込めと復帰。
- 冪等の鍵（`unsettledAttempt`、`decisionAttempt`）の扱い。失敗の後のやり直しは、今のまま同じ鍵を使う。
- 送信の結果で、成功でも失敗でもダイアログを閉じて結果を出すこと。

## 守る不変条件

1. 確定1回につき送信は1回。描き直しの前に確定を2回押しても、書き込みは1回だけ出る。
2. 確認なしで送らない。作業の依頼の2択から送信へ直接つながる意図が、型にも無い。やめた後の確定も送らない。
3. 500ms の守りと Escape の挙動は今のまま。送信中に捨てた「やめる」と Escape は、フォーカスの封じ込めも外さない。
4. 送信が失敗しても、例外が出ても、送信中の守りは解ける。次に開いた確認で、確定は 500ms 後に1回だけ送れる。

## テストで確かめること（テストは別の担当が書く）

`frontend/web/test/components/issue-308-operation-confirm.test.ts` の `mountOperationPage` を使う。

- 1つの `act` の中で確定を2回 `click()` すると、`fetch` は1回だけ呼ばれる。マージとゴール登録の確認でも1回だけ（同じ `run` を通る）。
  - このテストが直す前のコード（origin/main）で落ちることを、テストの担当が確かめる。実装を戻して走らせ、`fetch` が2回になることを見る。
- 1つの `act` の中で確定の後に「やめる」を押す、または Escape を送ると、ダイアログは送信中のまま開いていて、書き込みは1回。その後の Tab（と Shift+Tab）で、フォーカスがダイアログの中に留まる。
- 1つの `act` の中で「やめる」の後に確定を押すと、書き込みは出ず、ダイアログは閉じる。
- `fetch` が失敗する場合（`mountOperationPage` に応答を選ぶ口を足す。例外を投げる場合と `{ ok: false, error }` を返す場合の少なくとも1つ）：
  - 結果が返るとダイアログが閉じ、失敗の文言（例外なら「操作を送信できませんでした。通信を確認してください。」）が結果に出る。
  - フォーカスが押した2択のボタンへ戻る。開き直す前に、2択のボタンの `disabled` が false に戻っている。
  - 同じボタンで開き直すと、開いた直後の確定は送らず、500ms 過ぎた確定は1回だけ送る。送信中の守りが解けている。
  - 開き直して送った書き込みの `body.idempotencyKey` が、1回目と同じ。
- `answer-report` を消した後も、2択から開いた確認の確定で送る report が、押した方の2択と同じ（既存のテストを `open` の意図で通す）。
- 任意：自由記述の「回答する」と選択肢のボタンを、1つの `act` の中で2回押しても、書き込みは1回。

## Quint の仕様との対応

`offlineActions.qnt` の `confirm` は、`confirmEnabled`（開いていて送信中でない）を見て、送信（`sent' = sent.append(…)`）と `busy' = true` を1手でまとめる。今のコードは、`busy` を state で持つので、描き直しの前の2回目の確定も `busy` が false に見え、この1手の原子性と合わない。`run` の頭の ref の守りは、`busy` を見ることと立てることを `await` の前の同期の区間で行うので、`busy` の分については `confirm` の1手と同じ形になる。`confirmEnabled` のもう一方の条件（`dialog`、開いていること）は、ref では守らない。同じ描き直しの区間の「やめる」の後の確定は、`ConfirmDialog` の `released` の守りが止める。

`settle` は、成功と失敗を分けずに、`busy` を戻してダイアログを閉じる。コードも成功と失敗で同じく閉じて守りを解くので、失敗の後のテストは `settle` の後の状態（閉じる、フォーカスの復帰）を確かめることに当たる。

仕様（`.qnt` と `_test.qnt`）は変えない。仕様はもともと `sendOnlyOnConfirm`（送った数と確定の数が等しい）と `focusInsideOpenDialog` を不変条件に持ち、この直しは、コードを仕様の `confirm` と `cancel`（`cancelEnabled` が false なら起きない）の形に寄せるものだからだ。状態も遷移も増えない。ref は `busy` を描き直しを待たずに読むための写しで、モデルの上では `busy` と同じ変数になる。`answer-report` を消すことは、`CONFIRM_REPORT = true` の `fixed` で `pressReport` が起きないことと同じで、モデルに変わりは無い。

`README.md` の、仕様とコードの対応を述べる段落に、`confirm` の `busy` の原子性はコードでは `run` の頭の ref の守りに、`dialog` の条件は `ConfirmDialog` の `released` の守りに当たることを1文足す（実装の担当が足す）。検査の結果は変わらないので、`quint test` と `quint run` / `verify` は CI（`ci-specs.json`）でそのまま通ることを見るだけにする。
