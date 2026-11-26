# issue #308 の設計：作業の依頼の2択に確認ダイアログを足す

対象の issue：[j5ik2o/takt-dash#308](https://github.com/j5ik2o/takt-dash/issues/308)（決定は issue のコメント、2026-11-27）
関連：[j5ik2o/takt-dash#381](https://github.com/j5ik2o/takt-dash/pull/381)（切断中の操作、`specs/quint/decision-offline-actions/`）

## 何をどう変えるか

- 作業の依頼（operation_request）の「実施した」「対応できない」は、押してもすぐには送らず、マージやゴール登録と同じ `ConfirmDialog`（`frontend/web/src/components/decision-actions-view.tsx`）を開く。確定したときだけ、押した方の report で回答を送る。
- 確認の種別を足して、`DialogActionSection` と `ConfirmDialog` を使い回す。種別の名前と数（2択で1つか2つか）は実装が決める。テストは名前を書かず、2択の意図から開いた状態をそのまま使う。
- 2択のボタンが出す意図 `{ type: "answer-report", report }` は、作業の依頼では送信（`submit`）ではなく、ダイアログを開く（`resolveDecisionIntent` は `open`、`decideDecisionTransition` は開いた状態）に解決する。ボタンが `{ type: "open", dialog }` を出す形に変えてもよいが、`answer-report` も送らずに開くこと。

## 守る不変条件

1. 確定しなければ送らない。2択を押しただけ、やめる、Escape では書き込みが出ない。
2. 1回の確定で1回だけ送る。送信中は確定を押せず、二重に押しても増えない。送る report は、ダイアログに出ていた2択と同じ。
3. 切断中は送らない。開く・確定は止め、やめるだけは押せる（#307 と同じ規則）。
4. フォーカスの戻り方は既存のダイアログと同じ。開くと見出しへ移り、閉じると押したボタンへ戻る。切断中に閉じたら、押せるようになってから戻す（`decideDialogRestore`）。

## 文言の案

| 押したボタン | 見出し | 本文 | 確定のボタン |
| --- | --- | --- | --- |
| 実施した | 作業の依頼への回答（実施した） | 作業の依頼に「実施した」と回答します。よろしいですか。 | 実施した |
| 対応できない | 作業の依頼への回答（対応できない） | 作業の依頼に「対応できない」と回答します。よろしいですか。 | 対応できない |

対象の行には「プロジェクト」とプロジェクト名を出す。

理由：

- 見出しは「対象の操作」（「ゴール登録の承認」）にそろえた。2択の区別は、一覧の種別名「問い合わせ（作業の依頼）」と同じ括弧の形で添えた。見出しはダイアログの読み上げ名になるので、2択で別の名前にする。
- 本文は、ほかの確認と同じ「〜します。よろしいですか。」の形にした。
- 確定のボタンは、ゴール登録（「承認」で開き、「承認」で確定する）と同じく、押したボタンと同じ言葉にした。
- 一覧には、別のプロジェクトの作業の依頼が同じボタン名で並ぶ。取り違えを防ぐため、マージの確認（#304）と同じくプロジェクトを出す。

## 変えないこと

- 「回答する」（自由記述）と、選択肢の回答。押すとすぐに送る。
- マージとゴール登録の確認の文言と動き。
- 送信の中身（`/api/decisions/answer` へ送る report の値、冪等の鍵）、backend、proto。

## 実装への注意

- `decideDialogRestore` の `triggers` は、既存のテスト（`test/components/decision-actions.test.ts`）が3つの鍵（merge と登録の2つ）だけで組む。足した種別の鍵が無くても型が通る形（`Partial` など）にし、無い鍵は `null` と同じに扱う。テストは変えない。
- 一覧のボタン名と確定のボタン名が同じになる。テストは、一覧のボタンを `[role="dialog"]` の外で探す。

## 仕様（Quint）

`specs/quint/decision-offline-actions/` に足した。項目の種類（マージか作業の依頼か）と、確定の回数（`confirms`）を持たせ、不変条件 `sendOnlyOnConfirm`（送った数 = 確定の回数）を `safety` に入れた。いまの main に当たる `withoutReportConfirm` は「作業の依頼で始まる → 2択を押す」の2手で破り、修正後の `fixed` は破らない。くわしくは同じディレクトリの README。

## テスト

- `frontend/web/test/components/issue-308-operation-confirm.test.ts`：遷移の単体、描画（案の文言）、送信中と切断の全組み合わせの性質、ページをマウントして押す描画、任意の操作の列の性質（種つきの乱数で 150 列、除外なし）。
- `frontend/web/test/state/decision-actions.test.ts`：作業の依頼の意図がすぐ送る、という前提の1件を、確認を開く、に直した。
- `e2e/tests/issue-308-operation-confirm.spec.ts`：押す → ダイアログ → 確定で1回だけ届く。二重に押してもやめれば届かず、フォーカスは押したボタンへ戻る。
