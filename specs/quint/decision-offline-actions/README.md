# 切断中の判断待ちの操作（issue #307、#308）

切断中は判断待ちの回答の操作を止め、再接続すると戻すこと（#307）と、作業の依頼の2択も確認ダイアログで確定したときだけ送ること（#308）を、Quint の仕様で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `frontend/web/test/components/decision-actions.test.ts`（全組み合わせの性質のテストを含む）、`frontend/web/test/components/issue-308-operation-confirm.test.ts`（任意の操作の列の性質のテストを含む）、`e2e/tests/issue-307-disconnected-decisions.spec.ts`、`e2e/tests/issue-308-operation-confirm.spec.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `offlineActions.qnt` | 判断待ち 1 件の操作と、SSE の切断・再接続の状態機械、不変条件 |
| `offlineActions_test.qnt` | issue の筋書きを修正前と修正後でたどるテスト |

| モジュール | `GUARD` | `FOCUS_GUARD` | `CONFIRM_REPORT` | 表すもの |
| --- | --- | --- | --- | --- |
| `current` | false | false | false | #307 の修正前。ボタンは送信中だけ無効になる |
| `withoutReportConfirm` | true | true | false | main 54b10a6。#307 の修正は入り、作業の依頼の2択は押すとすぐに送る（#308 の修正前） |
| `fixed` | true | true | true | 修正後。切断中はトリガーと確定ボタンも無効にし、操作の入口でも止める。「やめる」と Escape は送信中だけ取り消せない。フォーカスの復帰は、フォーカスが body にあるときだけ行う。作業の依頼の2択も確認を開き、確定したときだけ送る |
| `fixedWithoutFocusGuard` | true | false | true | 修正後で、フォーカスの復帰を無条件に行うと仮定したもの |

## モデルの範囲

- 判断待ち 1 件を表します。種類（`kind`）は、マージの承認（`merge`）か作業の依頼の2択（`report`）で、初めに決まります。承認・却下はマージと同じ形です。`report` は、押した方の2択のボタンを1つのトリガーとして表します。`CONFIRM_REPORT = false` では、`report` のトリガーは確認を開かずにすぐ送ります（`pressReport`）。選択肢と自由記述はダイアログが無いだけで、無効にする条件は同じです（コードは全組み合わせの性質のテストで確かめます）。
- フォーカスは、トリガー、ダイアログの見出し・確定・やめる・リンク、項目の外の要素（`elsewhere`）、body の 7 つです。リンク（プルリクエストを開く）はマージの確認にだけあり、作業の依頼の確認には無いので、ダイアログの中の移動（`focusInDialogTo`）でリンクへ移れるのはマージのときだけです。ダイアログの中にあったフォーカスは、閉じると body へ落ちます。ダイアログの外で無効になったトリガーのフォーカスは、コードが外して body にそろえます（落とすブラウザも残すブラウザも同じになる）。
- 項目の外へのフォーカス移動（`focusElsewhere`）は、ダイアログが閉じているときだけ起きます。開いている間はダイアログが閉じ込めます。
- 送信中に切れた書き込みは、結果が返るまで待ちます（`settle`）。送った分を取り消す話は含めていません（#308 は、送る前に確定を挟む形で扱います）。
- 再接続の遅延（1 秒から倍々、上限 30 秒）は含めていません。`reconnect` はいつでも起きうる遷移です。
- 時間は含めていません。開いてから 500ms の間の確定を受け付けないこと（#308 の設計の「開いた直後の確定」）は、`issue-308-operation-confirm.test.ts` と `issue-308-operation-confirm.spec.ts` が確かめます。

## 条件

- `noSendWhileDisconnected`：切断中の画面からは書き込みを送らない。issue の受け入れ条件です。
- `focusNotOnDisabled`：無効なボタンにフォーカスが残らない。無いリンク（作業の依頼の確認）にもフォーカスは無い。
- `focusInsideOpenDialog`：ダイアログが開いている間、フォーカスはダイアログの中（見出し・確定・やめる・リンク）にある。
- `restoreWaitsOnlyWhileLocked`：閉じたダイアログのフォーカスの復帰を待つのは、トリガーが押せない間だけ。
- `noFocusOverwrite`：復帰は、利用者が項目の外に置いたフォーカスを上書きしない（履歴の変数 `stolen` で見る）。
- `sendOnlyOnConfirm`：書き込みは確定を押したときだけ出て、1 回の確定で 1 回だけ出る（送った数と、履歴の変数 `confirms` が等しい）。#308 の受け入れ条件です。
- `safety`：上の 6 つ。

## 検査

```sh
quint test offlineActions_test.qnt --main offlineActions_test
quint run offlineActions.qnt --main fixed --invariant safety --max-steps 30 --max-samples 20000
quint run offlineActions.qnt --main current --invariant noSendWhileDisconnected --max-steps 30 --max-samples 20000
quint run offlineActions.qnt --main fixedWithoutFocusGuard --invariant noFocusOverwrite --max-steps 30 --max-samples 20000
quint run offlineActions.qnt --main withoutReportConfirm --invariant sendOnlyOnConfirm --max-steps 30 --max-samples 20000
quint verify offlineActions.qnt --main fixed --invariant safety --max-steps 12
```

結果（quint 0.32.0）：

- テスト 17 件成功（修正前に切断中の送信が起きる筋書き、切断中に閉じて再接続でトリガーへ戻る筋書き、よそのフォーカスを奪わない筋書き、無条件の復帰なら奪う筋書き、送信中は取り消せない筋書き、#308 の前は作業の依頼が確定なしで送られる筋書き、#308 の後は作業の依頼も確認を開き、やめれば送らず、確定は 1 回だけ送る筋書き、マージの確認のリンクのフォーカスは切断で動かず、作業の依頼の確認にはリンクが無い筋書き、など）。
- `fixed` は `safety` を破らない（`run` 30 手 2 万件、`verify` 12 手）。
- `current` は `noSendWhileDisconnected` を破る（切断 → 開く → 確定）。
- `fixedWithoutFocusGuard` は `noFocusOverwrite` を破る（開く → 切断 → やめる → よそへ → 再接続）。
- `withoutReportConfirm` は `sendOnlyOnConfirm` を破る（作業の依頼で始まる → 2択を押す）。いまの main のコードに当たり、#308 の red です。

`quint verify` の出力先 `_apalache-out/` は `.gitignore` に入れています。
