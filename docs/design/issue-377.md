# 設計: 人の発言ではないターンを要点だけで見せ、元の入力を折りたたみに入れる（issue #377）

対象: [issue #377](https://github.com/j5ik2o/takt-dash/issues/377)、PR [j5ik2o/takt-dash#382](https://github.com/j5ik2o/takt-dash/pull/382)。
この設計は実装の後に、出来上がった形を書き起こしたものです（2026-11-25 のオーナー決定で、設計の独立レビューが実装の独立レビューに替わったため）。

## 何をどう変えたか

takt は manager の各ターンの入力を、`## このターンの入力` の見出しで始まる包みとして user の行に保存します。包みの中身は、見出し、空行、説明文、JSON の行、Business context の行、応答形式の指示（JSON schema）の順に並びます。JSON の行の形でターンの始まり方が分かります。

| 始まり方 | JSON の行の形 | 話者 | ふだんの本文 | 折りたたみの中 |
|---|---|---|---|---|
| 人が話しかけた（#111 の対象） | `{"speaker":…,"text":"…"}` | ユーザー | `text` | なし |
| 出来事 | `{"event":{"kind":"…",…},"instruction":"…"}` | システム | 説明文の全文。説明文が無ければ `イベント: <kind>` | `event` の値の JSON |
| 保存した質問への回答 | `{"kind":"inquiry_answered","question":"…","answer":{"text":"…",…},…}` | ユーザー | `answer.text` | `question` の文と、JSON の行全体 |
| どれにも当たらない | 上のどれでもない | ユーザー | 元の本文のまま | なし |

### backend

- `backend/src/conversation.rs`
  - `parse_turn_input(raw) -> Option<TurnInput>` が包みを読みます。行頭の見出しのうち最後のものより後を行ごとに見ます。空行は飛ばし、`{` で始まり `}` で終わらない行を説明文として集めます。JSON の形の行を `turn_input_from_json` に渡し、最初に当たった形で決めます。当たらない JSON の行（解釈できない行も含む）は飛ばします。
  - `TurnInput` は `Speech { text }`、`Event { description, kind, event }`、`InquiryAnswer { answer, question, source }` の3つです。
  - `present_body` が話者を決め直します。user の行でも、出来事なら話者をシステムにします。
  - `ConversationMessage` に `detail: Option<ConversationDetail>` を足しました。`ConversationDetail { question: Option<String>, source_json: String }` です。`source_json` は `serde_json::to_string_pretty` で整形します。キーの順は serde_json の既定（アルファベット順）です。
  - 文字化けの印（`garbled_notice`）は、表示する本文（説明文または回答）の置換文字で数えます。折りたたみの中身は数えません。
- `backend/src/grpc/assembly.rs`: `detail` を proto の `ConversationDetail` へ写します。

### proto（公開 API）

- `ConversationMessage` に `optional ConversationDetail detail = 6` を足しました。
- `ConversationDetail { optional string question = 1; string source_json = 2; }` を新設しました。

### web

- `frontend/web/src/state/conversation.ts`: `detail` を `ConversationDetailView`（`question?`、`sourceJson`）へ復元します。
- `frontend/web/src/components/conversation-content.tsx`: `detail` があるときだけ、本文の下に `<details className="conversation-detail">` を置きます。要約は、質問があれば「質問と元の JSON を表示します。」、無ければ「イベントの JSON を表示します。」です。開くと、質問があれば「質問」の見出しと質問の文、「元の JSON」の見出しを出し、`<pre>` に JSON を出します。ふだんは閉じています。開閉はブラウザの `<details>` に任せ、React の状態は持ちません。
- `frontend/web/app/globals.css`: 折りたたみの要約と見出しの見た目を足しました。

### CLI

- `frontend/cli` の描画は変えていません。新しい話者と要点の本文はそのまま出ます。テキストの出力に折りたたみは無いため、元の JSON は出しません。fixture に `detail: None` を足しただけです。

## 変えないこと・範囲の外

- 人が話しかけたターン（#111 の対象）と包みの無い行の表示は変えません。
- assistant の fenced JSON の展開と、system の行は変えません。
- 応答形式の指示と Business context は、本文にも折りたたみにも出しません。
- 出来事の折りたたみに、JSON の行の `instruction`（manager への指示文）は入れません。`event` の値だけです。
- 会話の取得の範囲（ページング、セッションの選択、スキップの数え方）は変えません。
- 保存済みのデータは読むだけで、書き換えません。
- CLI に折りたたみや元の JSON の表示は足しません。

## 未決（起動した担当に確認中）

- 作業の依頼（operation_request）への回答は、takt-dash の「実施した」「対応できない」ボタンから `answer: {"report":"completed"}` のように `text` 無しで保存されうる（`backend/src/grpc/mod.rs` の `report_payload`）。この環境の実データ16件はすべて `answer.text` を持ち、1件は `text` と `report` の両方を持っていた。`text` が無い回答は、いまの実装ではどの形にも当たらず、包みのまま全文が出る。案は、本文を「実施した」または「対応できない」（判断待ちの画面のボタンと同じ言葉）にし、折りたたみは質問と元の JSON にすること。Devin Review の指摘（PR のスレッド）もこの点。

## 守る不変条件

1. 包みの中の応答形式の指示と Business context の行は、本文にも、折りたたみの質問にも JSON にも現れない。
2. 出来事のターンの話者はシステム、質問への回答と人の発言のターンの話者はユーザー。
3. 出来事の本文は、説明文の空でない行を trim して改行でつないだもの。説明文が無ければ `イベント: <kind>`。
4. 出来事の折りたたみの JSON は、保存された `event` の値と同じ値に戻る（整形の差だけ）。
5. 質問への回答の本文は `answer.text` と完全に同じ文字列。折りたたみの質問は `question` が空でなければその文字列、空なら無し。折りたたみの JSON は JSON の行全体と同じ値に戻る。
6. 人の発言のターンと、行頭に見出しの無い本文は、この変更の前と同じ本文・話者で返り、折りたたみを持たない。
7. 見出しが前のターンの本文に含まれていても、現在の包みは行頭の見出しのうち最後のもの。
8. 文字化けの印は、表示する本文の置換文字が10個以上のときだけ付く。折りたたみの中身は数えない。
9. 包みを解釈できないときは、元の本文をそのまま返す（情報を落とさない）。

## PBT の性質

`backend/tests/conversation_turn_input_property_test.rs`（proptest、各128件）。入力は実データと同じ並びの包み（`conversation_fixture` の組み立て）で、読み取りは実物の `ConversationReader` と一時 DB を通します。

| 性質 | 生成するもの | 確かめること | 除外 |
|---|---|---|---|
| 出来事 | 種類（任意の空でない文字列）、イベントの欄（任意の文字列のキーと値）、説明文（改行を含まない任意の行を0〜3行）、Business context の有無、前のターンの本文（任意の文字列） | 不変条件 2、3、4、7 | 説明文の行が JSON の形（trim して `{` で始まり `}` で終わる）か、見出しで始まる行 |
| 質問への回答 | 質問（任意、空も含む）、回答（任意の空でない文字列、改行や制御文字を含む）、Business context の有無、前のターンの本文 | 不変条件 2、5、7 | なし |
| 人の発言 | 発言（任意の空でない文字列）、前のターンの本文 | 不変条件 6 | なし |
| 見出しの無い本文 | 任意の空でない文字列 | 不変条件 6 | 行頭に見出しを含む文字列（それは包みなので対象が違う） |
| 漏れない | 出来事の性質で外した説明文（JSON の形の行、見出しだけの行、`{"speaker":…}` の行）を混ぜた説明文 | 不変条件 1 | なし |
| 文字化けの印 | 回答と質問の置換文字の数（0〜19） | 不変条件 8 | なし |

出来事の性質の除外は、説明文がターンの始まり方の判定と区切りに関わる行だからです。除外した集合でも守るもの（不変条件 1）は「漏れない」の性質で確かめます。除外した集合の中での振る舞いは、説明文の JSON の形の行が飛ばされる、`speaker` を持つ行なら人の発言として読まれる、見出しで始まる行ならそこから先が現在の包みになる、です。

## 状態遷移と Quint

状態遷移はありません。変更は、保存された1行の文字列から表示用の値を作る純粋な関数と、その値の描画です。ループの上限、再試行、再開、ロック、キュー、接続の開け閉め、並行して動くものには触れていません。折りたたみの開閉はブラウザの `<details>` の開閉だけで、アプリの状態として持ちません。そのため Quint の仕様は作りません。

## 壊しうる入力

| 種類 | 入力 | 扱い | 確かめ |
|---|---|---|---|
| 境界 | 説明文が無い（wait_command_completed） | `イベント: <kind>` | 例のテスト、PBT |
| 境界 | `event.kind` が空、文字列でない、無い | どの形にも当たらず元の本文 | 例のテスト |
| 境界 | `answer.text` が空、無い（report だけ） | どの形にも当たらず元の本文（未決の節） | （未決） |
| 境界 | `question` が空、無い | 折りたたみに質問を出さない | PBT |
| 区切り | 前のターンの本文に見出しがある | 最後の行頭の見出しを使う | 既存の例のテスト、PBT |
| 区切り | 見出しが行頭でない | 見出しとして扱わない | PBT（見出しの無い本文） |
| 区切り | 説明文に JSON の形の行がある | 説明文から外れる。当たる形なら判定に使われる | PBT（漏れない） |
| 区切り | 応答形式の指示の中の `{` だけの行、`}` だけの行 | JSON の形ではないので説明文の候補になるが、JSON の行より後なので読まない | PBT（漏れない） |
| 入れ子 | イベントの値や回答に改行・見出し・JSON が入る | JSON の行の中は改行がエスケープされ、1行のまま | PBT |
| 長さ | 実データの JSON の行は数千文字（outputTail など） | 長さで切らない。`<pre>` の高さは CSS の上限（360px）でスクロール | e2e のスクリーンショット |
| Unicode | 置換文字、結合文字、サロゲートの外の文字、制御文字 | そのまま返す。印は表示する本文で数える | PBT |
| 並行・再開 | 該当なし（読み取りだけで状態を持たない） | — | — |

## 影響

- 上流（takt）: 変更なし。takt が書く包みの形を読むだけです。包みの形が変わると、当たらない行は元の本文のまま出ます（いまの表示に戻るだけで、落ちない）。
- 保存済みのデータ: 読むだけで書き換えません。opencode.db は読み取り専用で開きます（変更前と同じ）。
- 公開 API: proto に、番号 6 の optional フィールドと新しいメッセージを足しました。既存のフィールドの番号と型は変えていません。古いクライアントは新しいフィールドを読み飛ばします。ただし、古いクライアントでも出来事のターンの話者はシステムに、質問への回答の本文は回答だけに変わります（本文の意味が変わる）。takt-dash の web と CLI は同じリポジトリで更新しています。
- MCP: takt-dash の MCP の道具は会話を返しません（`backend/src/mcp` に会話の読み取りは無い）。
- 守りの仕組み: CI、関門、ルールセットは変えていません。backend の dev-dependencies に `proptest` を足しました。e2e の起動の補助（`e2e/support/stack.ts`）に、一時ホームへ記録を置く `seedHome` を足しました。既定では何もしません。e2e は Node の `node:sqlite` で DB を作ります（CI の e2e は Node 22 の最新で、22.13 以降はフラグ無しで使える）。
