# 切断中の判断待ちの操作（issue #307）

切断中は判断待ちの回答の操作を止め、再接続すると戻すことを、Quint の仕様で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `frontend/web/test/components/decision-actions.test.ts`（全組み合わせの性質のテストを含む）と `e2e/tests/issue-307-disconnected-decisions.spec.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `offlineActions.qnt` | 判断待ち 1 件の操作と、SSE の切断・再接続の状態機械、不変条件 |
| `offlineActions_test.qnt` | issue の筋書きを修正前と修正後でたどるテスト |

| モジュール | `GUARD` | 表すもの |
| --- | --- | --- |
| `current` | false | 修正前。ボタンは送信中だけ無効になる |
| `fixed` | true | 修正後。切断中はトリガーと確定ボタンも無効にし、「やめる」は押せる |

## モデルの範囲

- 確認ダイアログを開く操作（マージの承認）1 件を表します。承認・却下も同じ形です。選択肢・自由記述・作業の報告はダイアログが無いだけで、無効にする条件は同じです（コードは全組み合わせの性質のテストで確かめます）。
- フォーカスは、トリガー、ダイアログの見出し・確定・やめる、body の 5 つです。無効になった要素にあるフォーカスは body へ落ちるとします。
- 送信中に切れた書き込みは、結果が返るまで待ちます（`settle`）。送った分を取り消す話（issue #308）は含めていません。
- 再接続の遅延（1 秒から倍々、上限 30 秒）は含めていません。`reconnect` はいつでも起きうる遷移です。

## 条件

- `noSendWhileDisconnected`：切断中の画面からは書き込みを送らない。issue の受け入れ条件です。
- `focusNotOnDisabled`：無効なボタンにフォーカスが残らない。
- `focusInsideOpenDialog`：ダイアログが開いている間、フォーカスはダイアログの中にある。
- `restoreWaitsOnlyWhileLocked`：閉じたダイアログのフォーカスの復帰を待つのは、トリガーが押せない間だけ。
- `safety`：上の 4 つ。

## 検査

```sh
quint test offlineActions_test.qnt --main offlineActions_test
quint run offlineActions.qnt --main fixed --invariant safety --max-steps 30 --max-samples 20000
quint run offlineActions.qnt --main current --invariant noSendWhileDisconnected --max-steps 30 --max-samples 20000
quint verify offlineActions.qnt --main fixed --invariant safety --max-steps 12
```

結果（quint 0.32.0）：テスト 5 件成功。`fixed` は `safety` を破らない（`run` 30 手 2 万件、`verify` 12 手）。`current` は `noSendWhileDisconnected` を 3 手で破る（切断 → 開く → 確定）。
