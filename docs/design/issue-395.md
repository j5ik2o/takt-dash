# issue #395 の設計：本文の無い書き込みの応答を、結果が不明な失敗として扱う

対象の issue：[j5ik2o/takt-dash#395](https://github.com/j5ik2o/takt-dash/issues/395)
出どころ：[j5ik2o/takt-dash#394](https://github.com/j5ik2o/takt-dash/pull/394) のレビュー（`ToolCallError` に `refused` を足した PR）
関連：[j5ik2o/takt-dash#305](https://github.com/j5ik2o/takt-dash/issues/305)（書き込みの台帳）、[j5ik2o/takt-dash#391](https://github.com/j5ik2o/takt-dash/pull/391)（run 詳細の期限）

## 再現の確認（main `54411ce`）

- `backend/src/mcp/client.rs:428-451` の `extract_tool_text` は、`isError` が true でない応答に `content[0].text`（文字列）が無いと、`ToolCallError { refused: false }` を返す。stdio（`client.rs:242`）と HTTP（`http.rs:690`）の両方がこの関数を使う。
- `backend/src/grpc/failure.rs:143-149` の `write_failure` は、`ToolError::Tool(_)` を `refused` によらず `WriteFailure::Failed`（UNAVAILABLE）にする。
- `backend/src/grpc/write_ledger.rs` の `Ticket::record` は `Settled::Failed` の記録を消す。同じ鍵のやり直しは `Begin::Fresh` になり、takt へもう一度送られる。

takt は要求を受け取って処理し、応答を返している。本文が無いだけなので、書き込みは適用済みかもしれない。それを「適用されていない」と扱うのは誤り。

## 何をどう変えるか

1. `write_failure`（`backend/src/grpc/failure.rs`）
   - `ToolError::Tool(error) if error.refused`：いまと同じ `Failed(tool_error_status(..))`（UNAVAILABLE、#394 の文面「takt が操作を受け付けませんでした…takt の理由: …」）。
   - `ToolError::Tool(error)`（`refused == false`）：`Unknown(Status::deadline_exceeded(..))` に変える。文面は固定の文「takt-mcp の応答に本文がなく、操作の結果は不明です。状態を確かめてから、必要ならやり直してください。」。元のエラーは `log_detail` でログへ残す（`Parse` の枝と同じ形）。
2. `registration_write_failure`（`backend/src/grpc/writes.rs:298`）
   - 登録案の食い違いの判定（`is_stale_registration`）を `error.refused` のときだけにする。`refused == false` の文面は backend の固定文なので、いまは当たらない。takt が断っていない失敗を FAILED_PRECONDITION（適用されていない失敗）にしないことを、型の上の条件で固める。
3. mock MCP サーバ（`backend/tests/mock_mcp_server.mjs`）
   - 状態に `"noText": ["<道具名>"]` を足す。指定した道具は、呼び出しを記録したうえで `{ "content": [] }`（`isError` なし）を返す。

### 変えないこと（範囲の外）

- `extract_tool_text` と `ToolCallError` の形。`refused: false` を返すのは正しい（takt は断っていない）。
- 読み取りの境界 `tool_error_status`。本文の無い読み取りは、いまと同じ UNAVAILABLE と「takt-mcp の応答に本文がありませんでした…」。読み取りには台帳も二重書き込みも無い。run 詳細（#391）は読み取りで、`tool_error_status` と `run_detail.rs` の経路だけを使うので変わらない。
- `write_failure` のほかの枝（Timeout、Io、ProcessExited、InvalidJson、Protocol、Parse、Rpc、Http、Spawn、Backoff、NotSent）の振り分けと文面。
- 台帳（`write_ledger.rs`）、Web と CLI のコード。どちらも DEADLINE_EXCEEDED を「結果不明」として扱う仕組みが既にある（下の「公開 API」）。
- proto の定義。
- 台帳の記録が消えたあと（backend の再起動、保持期限の 1 時間）のやり直し。#305 の既知の限界（`fixedWithExpiry` の反例）のまま。

## 守る不変条件

- I1：takt が断った（`refused == true`、JSON-RPC のエラー、HTTP のエラー、送る前の失敗）と分かる失敗だけが `Failed`。
- I2：takt が要求を受け取ったかもしれず、断ったとは分からない失敗は、すべて `Unknown`（DEADLINE_EXCEEDED）。`refused == false` の道具エラーはここに入る。
- I3：`Unknown` の記録は台帳に残り、同じ鍵のやり直しは takt へ送らない（#305 の `noDuplicateWrite`）。
- I4：画面に出す文面は、本文・道具名によらない固定の文で、改行や制御文字を含まない（#306／#394 の P2・P3）。

## PBT の性質（`backend/src/grpc/failure_tests.rs`、除外なし）

- P2・P3 を直す：`expected_write` の `ToolNoText` を `(true, Code::DeadlineExceeded)` に移す。性質は「どの `ToolError` でも、Unknown／Failed の別とコードは表のとおりで、takt の理由を続けない枝の文面は本文によらない」のまま。全 13 の枝を除外なしで生成する。
- P7（新）：任意の道具名・本文の `ToolCallError` について、`write_failure` と `registration_write_failure` は `refused` が false なら `Unknown`（DEADLINE_EXCEEDED）、true なら `Failed`（`registration_write_failure` は本文が食い違いの語を含めば FAILED_PRECONDITION、含まなければ UNAVAILABLE）。本文の生成に `stale_summary` と `does not own the pending registration proposal` を混ぜる。
- P8（新）：`refused == false` の `write_failure` の文面は、本文と道具名によらず同じで、改行や制御文字を含まない。

## 結合テスト（`backend/tests/write_retry_test.rs`、先に書いて失敗を確かめる）

- `a_write_whose_reply_has_no_text_reports_an_unknown_outcome_and_is_not_resent`：mock を `noText: ["takt_answer_goal_question"]` にして鍵つきの回答を送ると DEADLINE_EXCEEDED。mock を正常に戻して同じ鍵でやり直すと、DEADLINE_EXCEEDED（前回の結果が不明）で、mock の呼び出しは 1 回のまま。いまの main では 1 回目が UNAVAILABLE、やり直しで呼び出しが 2 回になって落ちる。
- 承認（`takt_approve_goal_registration`、`registration_write_failure` の経路）でも同じことを確かめる。

## Quint（`specs/quint/decision-write-retry/` を広げる）

新しい仕様は作らず、`writeRetry.qnt` を広げる。#305 の状態機械の、takt が応答する遷移の 1 つを足すだけで、台帳・試行・不変条件は同じものを使うため。

- 定数 `EMPTY_REPLY_UNKNOWN: bool` を足す。
- 遷移 `taktEmptyReply(id)` を足す：試行が `Writing`（takt は適用していない）か `Unacked`（適用済み）のとき、takt が本文の無い応答を返す。試行は `Finished`。
  - `EMPTY_REPLY_UNKNOWN = false`（いまの main）：利用者は `SawRejected`、台帳は `Absent`（記録を消す）。
  - `EMPTY_REPLY_UNKNOWN = true`（直したあと）：利用者は `SawUnknown`、台帳は `Unknown`。
  - 待っていた同じ鍵の試行（`Joined`）にも同じ結果を配る。
- モジュール：`current`（修正前、`DEDUP=false`）、`fixed`（`DEDUP=true, EXPIRY=false, EMPTY_REPLY_UNKNOWN=true`）、`fixedWithExpiry`（`EMPTY_REPLY_UNKNOWN=true`）に加え、`emptyReplyAsRejected`（`DEDUP=true, EXPIRY=false, EMPTY_REPLY_UNKNOWN=false`、#395 の修正前）を置く。
- 不変条件は既存のまま：`noDuplicateWrite`、`rejectedMeansNotWritten`、`okMeansWritten`、`safety`。`emptyReplyAsRejected` では `rejectedMeansNotWritten` と `noDuplicateWrite` に反例が出て、`fixed` では出ないことを確かめる。
- 進行の条件は既存の 3 つ（`retryLearnsSuccess`、`otherOperationProceeds`、`rejectedOperationCanBeRetried`）を `fixed` で到達させる。
- 筋書きテスト（`writeRetry_test.qnt`）に 2 件足す：修正前は本文の無い応答のあと同じ鍵のやり直しで `applied == 2`、直したあとは `applied == 1` でやり直しは `SawUnknown`。
- CI（`specs/quint/ci-specs.json`）は `fixed` の `safety` を検査しているので、広げた遷移もそのまま検査される。README の結果を更新する。

## 壊しうる入力

| 入力 | 扱い |
| --- | --- |
| `content` が無い／配列でない／空の配列 | `refused: false` → 書き込みは Unknown |
| `content[0]` に `text` が無い、`text` が null・数値・オブジェクト | 同上 |
| `content[1]` 以降にだけ本文がある | 同上（`content[0]` しか見ない。変えない） |
| `isError` が `"true"`（文字列）や 1 | true とみなさない（変えない）。本文が無ければ Unknown、あれば解析へ進み、解析の失敗は既存の Parse → Unknown |
| `isError: true` で本文が無い | `refused: true`（既定の文面）→ Failed のまま |
| 本文が `stale_summary` を含む `refused: false` | 起こらない（固定の文面）が、起きても Unknown。P7 で確かめる |
| 同じ鍵の同時要求 | 先の要求の Unknown を待っていた要求にも配る（台帳の既存の仕組み） |
| 応答の後の backend の再起動・保持期限切れ | 記録が消え、やり直しは送られる（#305 の既知の限界） |
| 鍵の無い要求 | 台帳を使わない。コードが DEADLINE_EXCEEDED に変わるだけ |

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- **公開 API（gRPC の状態コード）**：AnswerInquiry、ApproveGoalMerge、ApproveGoalRegistration、RejectGoalRegistration で、takt-mcp が `isError` でない本文の無い応答を返したとき、状態コードが UNAVAILABLE から DEADLINE_EXCEEDED に変わる。proto は変えない。
  - Web の書き込み（`frontend/web/app/api/decisions/_shared.ts` の `runWrite`）：DEADLINE_EXCEEDED は `BackendUnavailableError` の `no-response` になり、HTTP 502 から 504 に変わる。文面は backend の文（上の固定の文）をそのまま出す。画面は成功していない操作のやり直しに同じ鍵を使うので、2 分以内のやり直しは「この操作は前回の結果が不明のため再送していません…」になる（いまの Timeout と同じ見え方）。
  - CLI（`frontend/cli/src/main.rs` の `write_call_error`）：「backend から…に失敗しました（Unavailable）」から「…の結果は不明です。<文面>」に変わる。
  - #394 の文面：takt が断った失敗の文面と、読み取りの文面は変えない。#394 の性質の表（`expected_write`）の `ToolNoText` の行だけを変える。
- **上流（takt、takt-mcp）**：変えない。
- **保存済みデータ**：無い（台帳はメモリだけ）。
- **守りの仕組み（CI・関門・ルールセット）**：変えない。Quint の CI ジョブは既存の `ci-specs.json` の項目で、広げた仕様を検査する。
