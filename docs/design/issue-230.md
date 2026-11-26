# issue #230 の設計：切り詰めの印を takt の応答から取り、どの表示でも黙って途中で終わらせない

対象の issue：[j5ik2o/takt-dash#230](https://github.com/j5ik2o/takt-dash/issues/230)
前段：[j5ik2o/takt-dash#229](https://github.com/j5ik2o/takt-dash/pull/229)（表示側の推定）、[j5ik2o/takt-dash#380](https://github.com/j5ik2o/takt-dash/pull/380)（登録案の長い項目を全文で読む）

## 再現の確認（main `63d3c11`）

takt（`j5ik2o/takt` の `origin/integration/takt-mcp` `94e7318ae`）の応答の形：

- `takt_list_goals` の各要素は `goalSummary`（`src/features/mcp/goalResponse.ts:43`）。`objective` は UTF-8 の 256 バイトで切った preview で、要素ごとに `omissions.objective.totalBytes`（元の文のバイト数）を必ず持つ。要素の `truncated` は常に true で、印には使えない。
- 一覧の道具（`takt_list_goal_questions`、`takt_list_standalone_inquiries`、`takt_list_goal_work_log`、`takt_get_goal` の `path=notifications`）は全文を返す。`pagedResult`（`response.ts:122`）は 64 KiB に収まるまで要素の数を減らし、1 件でも収まらないときだけ `jsonResult` が文字列を短くする。そのとき応答の最上位に `truncated: true` と `omissions: [{ path: "<配列名>[].<項目>", ... }]`、`omittedDetails` が付く。この場合、ページの要素は 1 件。

takt-dash で印が消える場所：

| 表示 | 出どころ | いまの main |
| --- | --- | --- |
| ゴール一覧・プロジェクト・概要のゴール名 | `goal.rs:136` `parse_goal` が `objective` だけ読む → `assembly.rs:98` `Goal.name` | 印を捨てる。画面は `goal-summary.ts:60-77` でバイト長と文末から推定 |
| 判断待ちのマージ待ちの本文 | `store.rs:493` `goal.objective` | 印なし。`decisions-content.tsx:148` がそのまま描く。QA の「…プルリク」で途切れる現象 |
| 判断待ちの問い合わせの本文 | `goal.rs` `parse_inquiry` の `body` | 64 KiB を超える 1 件だけ短くなる。印を捨てる |
| アクティビティの本文 | `events.rs:104,267,293`（目的・問い合わせ本文・通知本文を「」に入れて文を作る） | 印なし。SQLite に保存 |
| run の要約（一覧・run 詳細） | `run_progress.rs:215` `task_summary` が 1 行目を 80 文字で切る | takt-dash が切って、印なし |
| run 詳細のログ | `run_log.rs:896` `sanitize` が文字列を 512 文字で切る、`run_log.rs:739` の道具名も 512 文字 | takt-dash が切って、印なし |
| ゴール詳細の質問・回答・作業記録 | `mod.rs:781,763` の一覧 | 64 KiB を超える 1 件だけ短くなる。印を捨てる |
| ゴール詳細の目的 | `read_goal_text`（`textOffset` で全文） | 全文。変えない |
| 登録案の本文 | #380 が全文を読み、読み切れないときは注記を付ける | 変えない |

## 何をどう変えるか

方針：表示側が文の形から何かを作る項目（ゴール名からの見出し）と、issue が proto に載せると決めた項目は、proto に印の項目を足す。backend が文を組み立てる項目と、backend 自身が切る項目は、backend が文の末尾に `…` を付ける（印は正確で、受け取り側に変更が要らない）。

### 1. backend の型（`backend/src/model.rs`）

- 新しい値の型 `PreviewText { text: String, truncated: bool }` を置く。`truncated` は「takt の応答の印が、この文は途中で切ってあると言っている」。`display()` は `truncated` なら末尾に `…` を付けた文字列を返す。
- `GoalEntry.objective` と `InquiryEntry.body`（と解析中の `ParsedInquiry.body`）をこの型にする。文字列のまま `bool` を並べると、印を渡し忘れても型で気づけないため。
- 判断待ち（`PendingDecision`）に `body_truncated: bool` を足す。マージ待ちは `goal.objective.truncated`、問い合わせは `inquiry.body.truncated`、登録案は false。

### 2. 応答の印を読む（`backend/src/mcp/tools/parse.rs`、`goal.rs`）

- `Page` に、応答の最上位の `omissions` から `<配列名>[].` で始まる path の残り（`objective`、`body`、`answer.text`、`summary` など）を集めた `shortened: BTreeSet<String>` と、`omittedDetails > 0` かを持たせる。`omittedDetails > 0` のときは、どの項目が短くなったか takt が書き切れていないので、そのページの文の項目はすべて切ってあるとみなす（印に基づく保守側）。
- `parse_goal`：`truncated = omissions.objective.totalBytes > objective のバイト数 || page が objective を短くした`。`omissions.objective.totalBytes` が無い（古い takt-mcp）なら、ページの印だけで決める。
- `parse_inquiry`（質問・ゴールに属さない問い合わせ）：`body.truncated = page が body を短くした`。
- ゴール詳細（`GoalQuestionEntry` の `body`・`answer_text`、`GoalWorkLogEntry.summary`）：ページの印で短くなった項目は、解析のときに末尾へ `…` を付ける。proto は変えない。
- 通知（`NotificationEntry.body`、`items[].body`）：同じく、短くなっていれば末尾に `…`。

### 3. proto（`proto/taktdash.proto`）

- `Goal` に `bool name_truncated = 12;`（name は takt が切った preview で、続きがある）。
- `PendingDecision` に `bool body_truncated = 14;`（body は takt が切った文で、続きがある）。
- 既存の番号と型は変えない。`make proto-gen` で `frontend/web/src/proto` を作り直す。

### 4. backend の射影

- `grpc/assembly.rs`：`Goal.name = objective.text`、`name_truncated = objective.truncated`。
- `store.rs`：判断待ちの `body` と `body_truncated`。`grpc` の写像で `PendingDecision.body_truncated` へ。
- `events.rs`：「」に入れる目的と問い合わせ本文を `display()`（切れていれば `…`）にする。通知本文は 2. で `…` が付いている。
- `run_progress.rs` `task_summary`：1 行目が 80 文字を超えて切ったときだけ、末尾に `…` を付ける（80 文字＋`…`）。改行で 1 行目だけにするのは要約で、切り詰めではないので印を付けない。
- `run_log.rs`：`sanitize` と `tool_use_name` で 512 文字を超えて切ったときだけ、末尾に `…` を付ける（512 文字＋`…`）。上限の値は変えない。

### 5. Web（`frontend/web`）

- `restore-state.ts`：`nameTruncated`、`bodyTruncated` を `=== true` で復元する（欠けていれば false）。
- `goal-summary.ts`：推定（`PREVIEW_BYTE_LIMIT`、`goalNameIsTruncatedPreview`、`endsComplete`）を消す。`goalDisplayName(goal)` は `nameTruncated` なら `…` を付ける。`goalListHeading(goal)` は、最初の文が文末記号で閉じていない（`goalHeading` が文末を見つけられず全体を返した）かつ `nameTruncated` のときだけ `…` を付ける。preview がちょうど文末で切れたときは、最初の文は完結しているので印を付けない。
- `run-view.ts`・`overview-content.tsx`：run のゴール名にも印を運ぶ（`goalNameTruncated`）。
- 判断待ち：`decisionDisplayBody(decision)` を置き、`bodyTruncated` なら最後の段落の末尾に `…` を付ける。`decisions-content.tsx` の本文と、`decision-labels.ts` の読み上げ用の文の両方で使う。

### 6. CLI（`frontend/cli/src/render.rs`）

- ゴールの行（`render_goal_line`、ゴール一覧の詳細行）は `name_truncated` なら `…` を付ける。判断待ちの行（`render_pending_detail`、判断待ちの一覧）は `body_truncated` なら `…` を付ける。

### 変えないこと（範囲の外）

- takt 側の preview の長さ（256 バイト）と応答の形。takt-dash 側の印に絞る。
- ゴール名・判断待ちの本文の全文を読む経路は作らない（全文はゴール詳細で読める）。
- run 要約の 80 文字・ログの 512 文字の上限そのもの。
- 保存済みのアクティビティ（SQLite の `events`）。直す前に記録した文はそのまま。新しく記録する文から `…` が付く。
- 登録案（#380）、ゴール詳細の目的、run のレポート・指示書（`GetRunContentResponse.truncated` は既にある）。
- Quint：状態遷移ではない（応答の印を値へ写して表示するだけで、ループ・再試行・遷移・並行に触れない）ため、Quint の仕様は書かない。

## 守る不変条件

- I1：表示する文が takt の印で切ってあると分かるとき、ゴール名・判断待ちの本文・アクティビティ・ゴール詳細の質問と作業記録・通知は、末尾に `…` を出す（Web と CLI の両方）。
- I2：takt の印が切っていないと言う文には、`…` を付けない（推定で付けない）。`totalBytes == バイト数` のゴール名は、256 バイトちょうどでも文末記号が無くても印なし。
- I3：backend が自分で切った文（run 要約 80 文字、ログ 512 文字）は、切ったときだけ末尾に `…` が付き、切らなかった文は変わらない。
- I4：`text` は takt の preview のまま変えない（印は別の項目）。ゴール名・判断待ちの本文の `text` に `…` を混ぜない。
- I5：見出しは、最初の文が preview の中で完結していれば印を付けず、完結していないときだけ付ける。

## PBT の性質（除外なし）

backend（proptest、`backend/src/mcp/tools/goal_tests.rs` ほか）：

- P1：任意の文字列 `s`（Unicode、空、改行、絵文字、URL を混ぜる）と任意のバイト上限 `n`（0〜300）について、takt と同じく文字の境界で `n` バイト以下に切った preview と `totalBytes = len(s)` の要素を `parse_goal` に通すと、`truncated == (preview != s)`、`text == preview`。
- P2：任意の要素とページの `omissions`（`goals[].objective`、`questions[].body`、関係ない path、`omittedDetails` 0／正）について、`truncated` はページの印か要素の印のどちらかが立つときだけ true。
- P3：`task_summary` は任意の文字列について、1 行目の文字数が 80 以下なら 1 行目そのもの、超えれば先頭 80 文字＋`…`。結果の文字数は 81 以下。
- P4：`sanitize` は任意の JSON（入れ子、配列、長い文字列）について、512 文字以下の文字列を変えず、超える文字列を先頭 512 文字＋`…` にする。キーの除外（`PROMPT_FIELDS`）は今のまま。
- P5：`PreviewText::display()` は `truncated` のときだけ `text + "…"`、そうでなければ `text`。

Web（`frontend/web/test/state/goal-summary.test.ts` ほか。生成のループで多数の入力を流す）：

- P6：任意の名前と `nameTruncated` について、`goalDisplayName` は `nameTruncated` のときだけ `…` で終わる（名前の長さ・文末記号によらない）。
- P7：`goalListHeading` は `nameTruncated` が false なら `goalHeading(name)` と同じ。true なら、最初の文が文末記号で閉じているときは同じ、閉じていないときだけ `…` を足す。
- P8：`decisionDisplayBody` は `bodyTruncated` のときだけ末尾が `…`、段落の数は変わらない。
- P9：`restore-state` は `nameTruncated` / `bodyTruncated` を、true だけ true、欠落・false・それ以外の値は false に復元する。

## 先に書いて落ちることを確かめるテスト

- backend：`omissions.objective.totalBytes` が preview より大きいゴールの一覧から、`Goal.name_truncated == true`、マージ待ちの判断待ちの `body_truncated == true`、アクティビティの「ゴール「…」が登録されました。」に `…` が入る（mock MCP のゴールに `omissions` を足す）。いまの main では項目が無く落ちる。
- backend：ちょうど 256 バイトで終わり、`totalBytes` も 256 のゴールは印なし。
- backend：`task_summary` の 81 文字の 1 行目、`sanitize` の 513 文字の文字列。
- Web：推定で付いていた「256 バイトで文末記号の無い名前」は、`nameTruncated: false` なら `…` を付けない。`nameTruncated: true` の短い名前には付ける。
- CLI：`name_truncated` / `body_truncated` の行に `…`。

## 壊しうる入力

| 入力 | 扱い |
| --- | --- |
| `omissions` が無い・`objective` が無い・`totalBytes` が数でない（古い takt-mcp） | 要素の印なし。ページの印だけで決める |
| `totalBytes` が preview のバイト数より小さい（壊れた応答） | 印なし（切っていない） |
| preview がちょうど 256 バイト、元も 256 バイト | 印なし |
| preview が多バイト文字の境界で 253〜256 バイト | `totalBytes` との比較だけで決まる |
| preview が文末記号・閉じ括弧・URL の途中で終わる | 文の形を見ない。印だけで決まる |
| 空の目的 | 印なし（`totalBytes` 0） |
| ページの `omissions` の path に番号が残る（`questions.0.body`） | takt は `[]` へ置き換えるので来ないが、来たら一致しないので印なし。性質 P2 に入れる |
| `omittedDetails > 0` | そのページの文の項目をすべて切ってあるとみなす |
| 64 KiB を超える 1 件の質問 | ページの印で `body.truncated` |
| 1 行目が 80 文字ちょうど・81 文字・改行だけ・前後の空白 | 80 以下は変えない、81 以上は 80＋`…` |
| ログの文字列が 512 文字ちょうど・513 文字・BMP 外の文字 | 文字（スカラー値）で数える。今と同じ |
| 新しい Web と古い backend | 項目が無いので false。推定を消したので印は出ない（同じリポジトリから一緒に出すので、組み合わせは起きない前提） |
| 古い Web・CLI と新しい backend | 知らない項目は読み飛ばす。今と同じ表示（古い Web は推定のまま） |

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- **公開 API（proto）**：`Goal.name_truncated = 12`、`PendingDecision.body_truncated = 14` を足す。既存の番号・型・意味は変えない。proto3 の bool は無ければ false なので、古いクライアントは読み飛ばし、古いサーバーの応答は「印なし」と読める。`Goal.name` と `PendingDecision.body` の文字列は今と同じ（`…` を混ぜない）。`ActivityEvent.body`、`Run.summary`、`GetRunDetailResponse.summary`、run のログ行、`GoalQuestion.body`／`answer_text`、`GoalWorkLog.summary` は型を変えず、切ってあるときだけ末尾に `…` が付く。
- **上流（takt、takt-mcp）**：変えない。`omissions.objective.totalBytes` と、ページの `omissions`／`omittedDetails` を読むだけ。
- **保存済みデータ**：アクティビティの SQLite は形を変えない。古い行は書き換えない。
- **守りの仕組み（CI・関門・ルールセット）**：変えない。`make proto-check` は生成し直したコードで通す。
