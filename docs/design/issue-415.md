# issue #415 の設計：送信中の二重の確定の守り、使われない answer-report の道、送信の失敗の後のテスト

対象の issue：[j5ik2o/takt-dash#415](https://github.com/j5ik2o/takt-dash/issues/415)（#404 / #308 のマージ後の点検の低の指摘）
関連：`docs/design/issue-308.md`、`specs/quint/decision-offline-actions/`

## 何をどう変えるか

1. 送信中の守りを ref で持つ（`frontend/web/src/components/decision-actions.tsx`）。
   - `DecisionActions` に `submitting = useRef(false)` を足す。`run` の頭で、`action` が無いか切断中なら返す既存の判定の後に、`submitting.current` が true なら返す。false なら、`await` の前に同じ同期の区間で true にしてから `setBusy(true)` する。結果を受け取ったら、`setBusy(false)` と同じ所で false に戻す（成功でも失敗でも戻す）。
   - `handleAction` の入口でも、`submitting.current` が true なら、どの意図も受け付けずに返す。送信中は、画面のボタン（トリガー、確定、やめる、回答する、選択肢）がすべて `disabled={busy …}` なので、受け付けない意図の範囲は画面と同じになる。これで、1つの act（描き直しの前）の中で確定の後に届いた「やめる」や Escape も、送信中に閉じない。
   - `busy` の state は残す。描画（disabled、フォーカスを見出しへ移すこと、Escape の判定）は今のまま `busy` を見る。ref は、描き直しを待たずに見るための入口の守りだけに使う。
2. 使われない `answer-report` の意図を消す（`frontend/web/src/state/decision-actions.ts`）。
   - `DecisionIntent` から `{ type: "answer-report"; report: number }` を除き、`resolveDecisionIntent` の `case "answer-report"` を消す。画面の2択は `DialogActionSection` が `{ type: "open", dialog }` を出しており、この意図を出す所はテストしか無い。
   - テストの呼び出し元を移す。`issue-308-operation-confirm.test.ts` の `openByReport` は、`DIALOG_TRIGGERS.operation` から押したボタンの名前で `dialog` を引き、`{ type: "open", dialog }` で開く（種別の名前をテストに書かない方針は保つ）。`test/state/decision-actions.test.ts` の 2択の確認と、`test/components/decision-actions.test.ts` の切断中の意図の一覧からは、この意図を除く。
   - 型で絞る案（`report: 1 | 2`）は採らない。絞っても、画面が出さない意図と、`open` と同じ働きの分岐が残る。開く道を `open` の1つにすると、読む人が確認なしで送る道を探さずに済む。

## 変えないこと

- 開いた直後の確定を黙って捨てる挙動（issue の「採らないもの」）。確定のボタンは 500ms の間も disabled にせず、見た目も変えない。
- 500ms の守り（`useConfirmAcceptance`）、`decideDialogKey` の Escape（送信中は取り消さない）、フォーカスの封じ込めと復帰。
- 冪等の鍵（`unsettledAttempt`、`decisionAttempt`）の扱い。失敗の後のやり直しは、今のまま同じ鍵を使う。
- 送信の結果で、成功でも失敗でもダイアログを閉じて結果を出すこと。

## 守る不変条件

1. 確定1回につき送信は1回。描き直しの前に確定を2回押しても、書き込みは1回だけ出る。
2. 確認なしで送らない。作業の依頼の2択から送信へ直接つながる意図が、型にも無い。
3. 500ms の守りと Escape の挙動は今のまま。
4. 送信が失敗しても、送信中の守りは解ける。次に開いた確認で、確定は 500ms 後に1回だけ送れる。

## テストで確かめること（テストは別の担当が書く）

`frontend/web/test/components/issue-308-operation-confirm.test.ts` の `mountOperationPage` を使う。

- 1つの `act` の中で確定を2回 `click()` すると、`fetch` は1回だけ呼ばれる。マージとゴール登録の確認でも1回だけ（同じ `run` を通る）。
- 1つの `act` の中で確定の後に「やめる」を押す、または Escape を送ると、ダイアログは送信中のまま開いていて、書き込みは1回。
- `fetch` が失敗する場合（`mountOperationPage` に応答を選ぶ口を足す。例外を投げる場合と `{ ok: false, error }` を返す場合の少なくとも1つ）：
  - 結果が返るとダイアログが閉じ、失敗の文言（例外なら「操作を送信できませんでした。通信を確認してください。」）が結果に出る。
  - フォーカスが押した2択のボタンへ戻る。
  - 同じボタンで開き直すと、開いた直後の確定は送らず、500ms 過ぎた確定は1回だけ送る。送信中の守りが解けている。
- `answer-report` を消した後も、2択から開いた確認の確定で送る report が、押した方の2択と同じ（既存のテストを `open` の意図で通す）。

## Quint の仕様との対応

`offlineActions.qnt` の `confirm` は、`confirmEnabled`（開いていて送信中でない）を見て、送信（`sent' = sent.append(…)`）と `busy' = true` を1手でまとめる。今のコードは、`busy` を state で持つので、描き直しの前の2回目の確定も `busy` が false に見え、この1手の原子性と合わない。`run` の頭の ref の守りは、見ることと立てることを `await` の前の同期の区間で行うので、`confirm` の1手と同じ形になる。

`settle` は、成功と失敗を分けずに、`busy` を戻してダイアログを閉じる。コードも成功と失敗で同じく閉じて守りを解くので、失敗の後のテストは `settle` の後の状態（閉じる、フォーカスの復帰）を確かめることに当たる。

仕様（`.qnt` と `_test.qnt`）は変えない。仕様はもともと `sendOnlyOnConfirm`（送った数と確定の数が等しい）を不変条件に持ち、この直しは、コードを仕様の `confirm` の形に寄せるものだからだ。状態も遷移も増えない。ref は `busy` を描き直しを待たずに読むための写しで、モデルの上では `busy` と同じ変数になる。`answer-report` を消すことは、`CONFIRM_REPORT = true` の `fixed` で `pressReport` が起きないことと同じで、モデルに変わりは無い。

`README.md` の、仕様とコードの対応を述べる段落に、`confirm` の原子性はコードでは `run` の頭の ref の守りに当たることを1文足す。検査の結果は変わらないので、`quint test` と `quint run` / `verify` は CI（`ci-specs.json`）でそのまま通ることを見るだけにする。
