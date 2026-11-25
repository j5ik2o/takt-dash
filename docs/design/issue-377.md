# 設計: 人の発言ではないターンを要点だけで見せ、元の入力を折りたたみに入れる（issue #377）

対象: [issue #377](https://github.com/j5ik2o/takt-dash/issues/377)、PR [j5ik2o/takt-dash#382](https://github.com/j5ik2o/takt-dash/pull/382)。
この設計は実装の後に、出来上がった形を書き起こしたものです（2026-11-25 のオーナー決定で、設計の独立レビューが実装の独立レビューに替わったため）。2026-11-26 に設計レビューの指摘7件（すべて「直す」）を反映しました。反映した点は末尾の「設計レビューの指摘と反映」にまとめています。

## 何をどう変えたか

takt は manager の各ターンの入力を、`## このターンの入力` の見出しで始まる包みとして user の行に保存します。包みの中身は、見出し、空行、説明文、JSON の行、Business context の行、応答形式の指示（`次の JSON schema に一致する…` の行から本文の終わりまで）の順に並びます。JSON の行の形でターンの始まり方が分かります。

この環境の opencode.db で、行頭に見出しを持つ manager の user の行の JSON の行を数えた結果（2026-11-26）: patrol 1,947、人の発言 712、watch_observation 658、task_failed 476、task_completed 243、inquiry_answered 190（うち text と report の両方 16）、goal_registered 107、wait_command_completed 91、goal_resumed 15、goal_pr_failed 11、director の依頼 6、kind の無い古い回答 4。

| 始まり方 | JSON の行の形 | 話者 | ふだんの本文 | 折りたたみの中 |
|---|---|---|---|---|
| 人が話しかけた（#111 の対象） | `{"speaker":…,"text":"…"}` | ユーザー | `text` | なし |
| 出来事（patrol、goal_registered、goal_resumed、goal_pr_failed、task_completed、task_failed、watch_observation、wait_command_completed ほか、種類を問わない） | `{"event":{"kind":"…",…},"instruction":"…"}` | システム | 説明文の全文。説明文が無ければ `イベント: <kind>` | `event` の値の JSON（`instruction` は入れない） |
| 保存した質問への回答 | `{"kind":"inquiry_answered","question":"…","answer":{…},…}` | ユーザー | 下の「回答の本文」 | `question` の文と、JSON の行全体 |
| 保存した質問への回答（kind の無い古い形） | `{"questionId":"…","question":"…","answer":{…},…}`（`kind` が無い） | ユーザー | 下の「回答の本文」 | `question` の文と、JSON の行全体 |
| director の依頼 | `{"text":"…","origin":"director","directorConversationId":"…"}` | ディレクター | `text` | JSON の行全体 |
| どれにも当たらない包み | 上のどれでもない | ユーザー | 元の本文から、応答形式の指示と Business context の行を除いたもの | なし |
| 包みの無い行 | — | ユーザー | 元の本文のまま | なし |

1つの JSON の行が複数の形に当たるときは、上の表の順（人の発言 → 出来事 → 回答 → 古い形の回答 → director）で最初に当たった形にします。たとえば `speaker` と `event` を両方持つ行は人の発言です。

### 回答の本文

`answer` の `text` と `report`（作業の依頼への回答で、判断待ちの「実施した」「対応できない」ボタンが保存する値。`backend/src/grpc/mod.rs` の `report_payload`）から決めます。`text` は trim して空でないときだけ「ある」とします（空白だけの回答は無いと同じ）。`report` は空でない文字列のときだけ「ある」とします。

| `text` | `report` | ふだんの本文 |
|---|---|---|
| ある | 無い | `text` |
| ある | ある | `text`、改行、`（回答: <報告の言葉>）` |
| 無い | ある | `<報告の言葉>` |
| 無い | 無い | `回答（本文なし）` |

報告の言葉は、`completed` なら「実施した」、`unable` なら「対応できない」（`frontend/web/src/components/decision-actions-view.tsx` のボタンと同じ言葉）、ほかの値なら `回答: <値>` です。`text` と `report` の両方があるとき、本文の後ろの括弧は `（回答: 実施した）`、`（回答: 対応できない）`、知らない値なら `（回答: <値>）` になります。`answer` が無い、またはオブジェクトでないときも「無い・無い」として扱います。ボタンの言葉は、人が選んだ回答そのものとして扱います（2026-11-26、起動した担当の決定）。

### backend

- `backend/src/conversation.rs`
  - `parse_turn_input(raw) -> Option<TurnInput>` が包みを読みます。行頭の見出しのうち最後のものより後を行ごとに見ます。空行は飛ばし、`{` で始まり `}` で終わらない行を説明文として集めます。JSON の形の行を `turn_input_from_json` に渡し、最初に当たった形で決めます。当たらない JSON の行（解釈できない行も含む）は飛ばします。
  - `TurnInput` は `Speech`、`Event`、`InquiryAnswer`、`Director` の4つです。
  - どれにも当たらない包みは `strip_turn_instructions` が、現在の包みの中の Business context の行と、応答形式の指示の行から後を除きます。どちらも見つからなければ元の本文のままです。
  - `present_body` が話者を決め直します。user の行でも、出来事ならシステム、director の依頼ならディレクターにします。
  - `ConversationMessage` に `detail: Option<ConversationDetail>` を足しました。`ConversationDetail { question: Option<String>, source_json: String }` です。`source_json` は `serde_json::to_string_pretty` で整形します。キーの順は serde_json の既定（アルファベット順）です。
  - `ConversationSpeaker` に `Director` を足しました。
  - 文字化けの印（`garbled_notice`）は、表示する本文の置換文字で数えます。折りたたみの中身は数えません。
- `backend/src/grpc/assembly.rs`、`backend/src/grpc/mapping.rs`: `detail` と `Director` を proto へ写します。

### proto（公開 API）

- `ConversationMessage` に `optional ConversationDetail detail = 6` を足しました。
- `ConversationDetail { optional string question = 1; string source_json = 2; }` を新設しました。
- `ConversationSpeaker` に `CONVERSATION_SPEAKER_DIRECTOR = 4` を足しました。

### web

- `frontend/web/src/state/conversation.ts`: `detail` を `ConversationDetailView`（`question?`、`sourceJson`）へ復元します。話者 4 を「ディレクター」と表示します。
- `frontend/web/src/components/conversation-content.tsx`: `detail` があるときだけ、本文の下に `<details className="conversation-detail">` を置きます。要約は種類によらず「元の内容を表示します。」です。開くと、質問があれば「質問」の見出しと質問の文、「元の JSON」の見出しを出し、`<pre>` に JSON を出します。ふだんは閉じています。開閉はブラウザの `<details>` に任せ、React の状態は持ちません。
- `frontend/web/app/globals.css`: 折りたたみの要約と見出しの見た目を足しました。

### CLI

- `frontend/cli/src/render.rs`: 話者 4 を「ディレクター」と表示します。ほかの描画は変えていません。テキストの出力に折りたたみは無いため、元の JSON は出しません。

## 変えないこと・範囲の外

- 人が話しかけたターン（#111 の対象）と包みの無い行の表示は変えません。
- assistant の fenced JSON の展開と、system の行は変えません。
- 応答形式の指示と Business context は、見つけられればどの形でも、本文にも折りたたみにも出しません。
- 出来事の折りたたみに、JSON の行の `instruction`（manager への指示文）は入れません。
- 会話の取得の範囲（ページング、セッションの選択、スキップの数え方）は変えません。
- 保存済みのデータは読むだけで、書き換えません。
- CLI に折りたたみや元の JSON の表示は足しません。
- 行頭に見出しを持たない行（run のセッションなどで、見出しが文中に引用されているもの）は包みとして扱いません。

## 守る不変条件

1. 現在の包み（行頭の最後の見出しより後）の応答形式の指示と Business context の行は、見つけられた場合、どの形でも、本文にも、折りたたみの質問にも JSON にも現れない。応答形式の指示の行から後は読まない。Business context の行は説明文に入れない。見出しより前（前のターンの本文）は元のまま残す。
2. 出来事のターンの話者はシステム、director の依頼のターンの話者はディレクター、質問への回答と人の発言と当たらない包みのターンの話者はユーザー。
3. 出来事の本文は、説明文の空でない行を trim して改行でつないだもの。説明文が無ければ `イベント: <kind>`。
4. 出来事の折りたたみの JSON は、保存された `event` の値と同じ値に戻る（整形の差だけ）。`instruction` を含まない。
5. 質問への回答（両方の形）の本文は、`(text, report)` の組み合わせで「回答の本文」の表のとおり。`text` があれば、本文は `text` と完全に同じ文字列で始まる。`report` があれば、その言葉が本文に残る。折りたたみの質問は `question` が空でなければその文字列、空か無ければ無し。折りたたみの JSON は JSON の行全体と同じ値に戻る。
6. 人の発言のターンと、行頭に見出しの無い本文は、この変更の前と同じ本文・話者で返り、折りたたみを持たない。
7. 見出しが前のターンの本文に含まれていても、現在の包みは行頭の見出しのうち最後のもの。
8. 文字化けの印は、表示する本文の置換文字が10個以上のときだけ付く。折りたたみの中身は数えない。
9. どの形にも当たらない包みは、応答形式の指示と Business context の行を除いた元の本文を返す。2つとも見つからなければ元の本文のまま。前のターンの本文や説明文は落とさない。
10. 1つの JSON の行が複数の形に当たるときは、人の発言 → 出来事 → 回答 → 古い形の回答 → director の順で最初の形にする。
11. director の依頼の本文は `text` と完全に同じ文字列。折りたたみの JSON は JSON の行全体と同じ値に戻る。

## PBT の性質

`backend/tests/conversation_turn_input_property_test.rs`（proptest、各128件）。入力は実データと同じ並びの包み（`conversation_fixture` の組み立て）で、読み取りは実物の `ConversationReader` と一時 DB を通します。

| 性質 | 生成するもの | 確かめること | 除外 |
|---|---|---|---|
| 出来事 | 種類（任意の空でない文字列）、イベントの欄（任意の文字列のキーと、文字列・数・真偽・null・配列・入れ子のオブジェクトの値）、説明文（改行を含まない任意の行を0〜3行）、Business context の有無、前のターンの本文（任意の文字列） | 不変条件 2、3、4、7 | 包みの構造に関わる説明文の行（下の注） |
| 回答の本文 | 形（kind あり、kind の無い古い形）、`text`（無い、空、空白だけ、任意の文字列）、`report`（無い、空、completed、unable、任意の文字列、文字列でない値）、質問（任意、空も含む）、Business context の有無、前のターンの本文 | 不変条件 2、5、7 | なし |
| director の依頼 | `text`（trim して空でない任意の文字列）、directorConversationId、前のターンの本文 | 不変条件 2、11 | なし |
| 人の発言 | 発言（任意の空でない文字列）、前のターンの本文 | 不変条件 6 | なし |
| 見出しの無い本文 | 任意の空でない文字列 | 不変条件 6 | 行頭に見出しを含む文字列（それは包みなので対象が違う） |
| 当たらない包み | 説明文、どの形にも当たらない JSON の行（判定に使うキーを持たない任意のキーと値）、Business context の有無、応答形式の指示の有無、前のターンの本文 | 不変条件 1、9 | 包みの構造に関わる説明文の行（下の注） |
| 漏れない | 出来事の性質で外した説明文（JSON の形の行、見出しだけの行、`{"speaker":…}` の行、当たらない JSON の行、Business context の行、応答形式の指示の行）を混ぜた説明文 | 不変条件 1 | なし |
| 優先順位 | `speaker` と `event` の両方、`event` と `kind: inquiry_answered` の両方、`kind: inquiry_answered` と `origin: director` の両方を持つ行 | 不変条件 10 | なし |
| 文字化けの印 | 回答・質問・出来事の説明文の置換文字の数（0〜19） | 不変条件 8 | なし |

注: 「包みの構造に関わる説明文の行」は、JSON の形の行（trim して `{` で始まり `}` で終わる）、見出しで始まる行、`Business context (not approval):` で始まる行、`次の JSON schema に一致する` で始まる行です。これらを除くのは、説明文がターンの始まり方の判定と区切りに関わる行だからです。除外した集合でも守るもの（不変条件 1）は「漏れない」の性質で確かめます（JSON の形の行、見出しだけの行、`{"speaker":…}` の行、当たらない JSON の行、Business context の行、応答形式の指示の行を混ぜる）。除外した集合の中での振る舞いは、説明文の JSON の形の行が飛ばされる、`speaker` を持つ行なら人の発言として読まれる、見出しで始まる行ならそこから先が現在の包みになる、です。

## 状態遷移と Quint

状態遷移はありません。変更は、保存された1行の文字列から表示用の値を作る純粋な関数と、その値の描画です。ループの上限、再試行、再開、ロック、キュー、接続の開け閉め、並行して動くものには触れていません。折りたたみの開閉はブラウザの `<details>` の開閉だけで、アプリの状態として持ちません。そのため Quint の仕様は作りません。

## 壊しうる入力

| 種類 | 入力 | 扱い | 確かめ |
|---|---|---|---|
| 境界 | 説明文が無い（wait_command_completed） | `イベント: <kind>` | 例のテスト、PBT |
| 境界 | `event.kind` が空、文字列でない、無い | どの形にも当たらない包み（指示と Business context を除いた元の本文） | 例のテスト |
| 境界 | `answer.text` が空、空白だけ、無い | `report` があれば報告の言葉、無ければ `回答（本文なし）` | 例のテスト、PBT |
| 境界 | `answer.report` が空、文字列でない、知らない値 | 空・文字列でないは無いと同じ。知らない値は `回答: <値>` | 例のテスト、PBT |
| 境界 | `answer` が無い、オブジェクトでない | `回答（本文なし）` | 例のテスト |
| 境界 | `question` が空、無い | 折りたたみに質問を出さない | PBT |
| 境界 | director の `text` が空、空白だけ、`origin` が director でない | director の依頼として扱わない（当たらない包み） | 例のテスト |
| 区切り | 前のターンの本文に見出しがある | 最後の行頭の見出しを使う | 既存の例のテスト、PBT |
| 区切り | 見出しが行頭でない | 見出しとして扱わない | PBT（見出しの無い本文） |
| 区切り | 説明文に JSON の形の行がある | 説明文から外れる。当たる形なら判定に使われる | PBT（漏れない） |
| 区切り | 応答形式の指示の中の `{` だけの行、`}` だけの行 | JSON の形ではない。当たらない包みでは指示ごと除く | PBT（漏れない、当たらない包み） |
| 区切り | 1行が複数の形に当たる | 不変条件 10 の順 | PBT（優先順位） |
| 入れ子 | イベントの値に配列・オブジェクト（patrol の goalIds、task_failed の result など） | そのまま JSON として折りたたみに出す | 例のテスト、PBT |
| 入れ子 | イベントの値や回答に改行・見出し・JSON が入る | JSON の行の中は改行がエスケープされ、1行のまま | PBT |
| 長さ | 実データの JSON の行は数千文字（outputTail、goal_pr_failed の reason など） | 長さで切らない。`<pre>` の高さは CSS の上限（360px）でスクロール | e2e のスクリーンショット |
| Unicode | 置換文字、結合文字、サロゲートの外の文字、制御文字 | そのまま返す。印は表示する本文で数える | PBT |
| 並行・再開 | 該当なし（読み取りだけで状態を持たない） | — | — |

## 影響

- 上流（takt）: 変更なし。takt が書く包みの形を読むだけです。包みの形が変わると、当たらない包みとして、指示と Business context を除いた元の本文が出ます（落ちない）。
- 保存済みのデータ: 読むだけで書き換えません。opencode.db は読み取り専用で開きます（変更前と同じ）。
- 公開 API: proto に、`ConversationMessage` の番号 6 の optional フィールド、新しいメッセージ `ConversationDetail`、`ConversationSpeaker` の値 4（DIRECTOR）を足しました。既存のフィールドと値の番号と型は変えていません。古いクライアントは新しいフィールドを読み飛ばし、値 4 は知らない話者として受け取ります（takt-dash の古い web と CLI は「不明」と表示）。古いクライアントでも、出来事のターンの話者はシステムに、回答の本文は回答だけに、当たらない包みの本文は指示を除いたものに変わります（本文の意味が変わる）。takt-dash の web と CLI は同じリポジトリで更新しています。
- MCP: takt-dash の MCP の道具は会話を返しません（`backend/src/mcp` に会話の読み取りは無い）。
- 守りの仕組み: CI、関門、ルールセットは変えていません。backend の dev-dependencies に `proptest` を足しました。e2e の起動の補助（`e2e/support/stack.ts`）に、一時ホームへ記録を置く `seedHome` を足しました。既定では何もしません。e2e は Node の `node:sqlite` で DB を作ります（CI の e2e は Node 22 の最新で、22.13 以降はフラグ無しで使える）。

## 設計レビューの指摘と反映（2026-11-26）

| # | 指摘 | 扱い | 反映した場所 |
|---|---|---|---|
| 1 | text の無い作業の依頼への回答の表示 | 直す | 「回答の本文」、不変条件 5、PBT「回答の本文」 |
| 2 | 当たらない包み（director の依頼、kind の無い古い回答、ほかの形） | 直す | 表、`Director`、不変条件 1・9・11、PBT「director の依頼」「当たらない包み」 |
| 3 | 要約の言葉を種類によらないものに | 直す | web の要約「元の内容を表示します。」 |
| 4 | text と report の両方があるときに report を残す | 直す | 「回答の本文」の表 |
| 5 | PBT の生成を広げる（配列・オブジェクトの値、空白だけの回答、出来事の文字化け、優先順位） | 直す | PBT の表、不変条件 10 |
| 6 | 出来事の折りたたみに instruction を入れないこと | 直す | 不変条件 4、PBT「出来事」 |
| 7 | patrol、goal_resumed、goal_pr_failed | 直す | 表、例のテストと fixture |
