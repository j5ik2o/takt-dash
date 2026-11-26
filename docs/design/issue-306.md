# issue #306 の設計：backend のエラー文に、設定のパスや takt-mcp の生の応答を入れない

対象の issue：[j5ik2o/takt-dash#306](https://github.com/j5ik2o/takt-dash/issues/306)
関連：[j5ik2o/takt-dash#391](https://github.com/j5ik2o/takt-dash/pull/391)（GetRunDetail の DEADLINE_EXCEEDED と UNAVAILABLE）、[j5ik2o/takt-dash#305](https://github.com/j5ik2o/takt-dash/issues/305)（結果不明の 504）

## 現在の main での確認（`decbe89`）

コードを読んで確かめました。

1. **画面に backend の文面が届くのは、書き込み（判断待ちの操作）だけ。** `frontend/web/app/api/decisions/_shared.ts` の `runWrite` は、backend が返した gRPC の `details` を、そのまま JSON の `error` として返す。DEADLINE_EXCEEDED は 504、応答の中の UNAVAILABLE は 502、INVALID_ARGUMENT は 400、FAILED_PRECONDITION は 409、NOT_FOUND は 404。画面（`src/state/decision-actions.ts:251`）はその `error` を結果の欄に出す。
   読み取り（run 詳細、ゴール詳細、タスク詳細、会話、本文）は、UNAVAILABLE と DEADLINE_EXCEEDED を `BackendUnavailableError` に置き換え、固定の文面だけを出す（`src/grpc/unary.ts`、`app/api/error-response.ts`）。NOT_FOUND は固定の「見つかりません」になる。それ以外のコードは Next の 500 になり、本文は画面に出ない。
2. **書き込みの失敗の文面に、takt-mcp の生の応答が入る。**
   - `backend/src/grpc/mod.rs` の `tool_error_status` は、`ToolError` の表示（道具のエラーの本文、JSON-RPC のエラーの本文、輸送のエラーの本文）をそのまま埋める。書き込みの接続の失敗（`grpc/lanes.rs:82`）と、`write_failure` の「適用されていない」枝が使う。
   - `write_failure` の「結果が不明」の枝は、`（{error}）`、`（{message}）` で輸送のエラーと解析のエラーの本文を埋める。
   - `TransportError::InvalidJson` は、takt-mcp の応答の本文を上限なしで持つ（`mcp/client.rs:382` の 1 行全体、`mcp/http.rs:557` の本文全体）。結果不明の 504 で、これがそのまま画面に出る。
   - `grpc/writes.rs` の `registration_write_failure` は、登録案の食い違いの FAILED_PRECONDITION に、takt-mcp の道具のエラーの本文を `（…）` で埋める。
3. **書き込みの失敗の文面に、設定ファイルのパスが入る。** `grpc/resolve.rs:90` の「設定を読み込めませんでした: {error}」は、`ConfigError` の表示（`設定ファイル {path} を読み込めませんでした: …`、`HTTP 接続先 {url} …`）を埋める。書き込みの宛先の解決（`resolve_write_target`）がこれを通るので、502 で画面に出る。`grpc/mod.rs:575`、`:624` も同じ形。
4. **書き込みの失敗は、backend のログに残っていない。** `grpc/` の書き込みの経路に `tracing` の記録が無い。文面から詳細を外すと、詳細はどこにも残らなくなる。

Authorization の値は、どの文面にも入らない（`mcp/http.rs:272` の `InvalidHeaderValue` の表示は値を含まない）。これは issue の記載と同じ。

## 何をどう変えるか

方針：**gRPC の Status の文面は、失敗の種類ごとに backend が決めた文だけにする。** 詳細（元のエラーの表示）は、Status を作る境界で backend のログ（`tracing::warn!`）に残す。gRPC の状態コードは変えない。

画面（`_shared.ts`）は変えない。backend の文面を「そのまま出せる文」として扱う今の約束（#305、#319）を保ち、その約束を backend の側で守る。種類を知っているのは backend の境界なので、そこで決める。

### 1. `tool_error_status`（`backend/src/grpc/mod.rs` から新しい `backend/src/grpc/failure.rs` へ移す）

元のエラーを `tracing::warn!` に残し、種類ごとに次の文を返す。コードは今と同じ。

| 種類 | コード（変えない） | 文面（案） |
| --- | --- | --- |
| `ToolError::Tool`（道具が isError を返した。`refused` が true） | UNAVAILABLE | takt が操作を受け付けませんでした。画面を読み込み直して状態を確かめ、必要ならもう一度操作してください。詳しい理由は backend のログにあります。 |
| `TransportError::Rpc`（JSON-RPC のエラー） | UNAVAILABLE | 同上 |
| `ToolError::Tool`（isError ではない応答に本文が無かった。`refused` が false） | UNAVAILABLE | takt-mcp の応答に本文がありませんでした。takt と takt-dash の版の組み合わせを確かめてください。詳しい理由は backend のログにあります。 |
| `TransportError::Backoff(s)` | UNAVAILABLE | takt-mcp を起動し直すまで待っています。{s} 秒ほどおいてから、もう一度操作してください。 |
| `TransportError::Timeout(s)` | UNAVAILABLE | takt-mcp から {s} 秒以内に応答がありませんでした。しばらくおいてから、もう一度操作してください。 |
| そのほかの `TransportError`（Spawn、Io、NotSent、Http、Protocol、InvalidJson、ProcessExited） | UNAVAILABLE | takt-mcp とやり取りできませんでした。takt-mcp が動いているかと、takt-dash の接続先の設定を確かめてから、もう一度操作してください。詳しい理由は backend のログにあります。 |
| `ToolError::Parse` | INTERNAL | takt-mcp の応答を読み取れませんでした。takt と takt-dash の版の組み合わせを確かめてください。詳しい理由は backend のログにあります。 |

`{s}` は秒数（数字だけ）なので、入れても漏れない。

**実装中の変更（PR のレビューの指摘、Devin Review）。** `extract_tool_text`（`mcp/client.rs`）は、isError ではない応答に `content[0].text` が無いときも `ToolCallError` を返す。takt が断ったわけではないのに「takt が操作を受け付けませんでした」と出さないよう、`ToolCallError` に `refused`（isError で断ったときだけ true）を足し、false のときは上の表の別の文にする。コードと Unknown／Failed の別は変えない（本文の無い書き込みの応答を結果不明として扱うかは別件）。

「詳しい理由は backend のログにあります。」は、ログの場所を添えて「詳しい理由は backend のログ（配置先の data/backend-launchd.err.log）にあります。」にする（以下の表の文も同じ）。本番の backend は `tracing` を標準エラーに書き（`backend/src/main.rs`）、launchd の plist（`~/Library/LaunchAgents/com.ideoplus.taktdash-backend.plist`）の `StandardErrorPath` が `<配置先>/data/backend-launchd.err.log` を指す（`tools/prod-check/prod_log.py` も同じ場所を読む）。配置先の絶対パス（利用者のホームを含む）は画面に出さないので、配置先からの相対で書く。

**takt の理由を残す（設計レビューの決定、2026-11-26）。** `ToolError::Tool` と `TransportError::Rpc` の文は、固定の文のあとに「takt の理由: <本文>」を続ける。
- 本文は、道具のエラーの `ToolCallError.message` と、JSON-RPC のエラーの `message` だけ。`ToolCallError` の表示（「takt-mcp の道具 {tool} がエラーを返しました: …」）は使わない。
- 改行と制御文字は空白 1 つにし、前後の空白を落とし、先頭 200 文字で切る。切ったら「…」を付ける。空になったら「takt の理由: 」ごと付けない。
- 輸送のエラーの本文、`InvalidJson` の応答の本文、`ConfigError` の本文は、この欄にも出さない。
- 理由：takt の理由（例：「ゴールが待ちの状態ではない」）は、利用者が次に何をするかを決める材料で、消すと画面からは「受け付けませんでした」しか分からない。画面は LAN と VPN の中で使う前提（オーナーの決定）なので、takt の理由を出しても困らない。issue の主な問題は、設定のパスと生の応答が出て読みにくいことで、それは消す。
- 3 の登録案の食い違い（`registration_write_failure`）も、同じ形で「takt の理由: …」を続ける（今は括弧で本文を全部付けている）。

### 2. `write_failure`（`backend/src/grpc/mod.rs`）

- 結果不明の枝から `（{error}）`、`（{message}）` を外す。元のエラーはログへ。
  - Timeout(s)：今のまま（「takt-mcp から {s} 秒以内に応答がなく、操作の結果は不明です。状態を確かめてから、必要ならやり直してください。」）。
  - Io、ProcessExited、InvalidJson、Protocol：「takt-mcp の応答を受け取れず、操作の結果は不明です。状態を確かめてから、必要ならやり直してください。」
  - Parse：「takt-mcp の応答を読み取れず、操作の結果は不明です。状態を確かめてから、必要ならやり直してください。」
- 「適用されていない」枝は 1 の `tool_error_status` を通す（今と同じ）。
- 枝の分け方（どの失敗を Unknown にし、どれを Failed にするか）とコードは変えない。

### 3. `registration_write_failure`（`backend/src/grpc/writes.rs`）

FAILED_PRECONDITION の文の `（{error.message}）` を、1 の「takt の理由: …」（改行を空白にし、200 文字で切ったもの）に替える。「ゴール登録案は、作り直されたか、すでに承認・却下されています。画面を読み込み直してから、もう一度操作してください。takt の理由: …」元の本文はログへ。食い違いの判定（`is_stale_registration`）は変えない。

### 4. 設定の読み込みの失敗（`grpc/resolve.rs:90`、`grpc/mod.rs:575`、`:624`）

3 か所を 1 つの関数 `config_load_status(ConfigError) -> Status` にまとめる。元のエラー（パスを含む）はログへ。コードは UNAVAILABLE のまま。文面：「takt-dash の設定ファイルを読み込めませんでした。設定ファイルを直してから、もう一度操作してください。詳しい理由は backend のログにあります。」（設定は要求のたびに読み直すので、backend の起動し直しは要らない。）

### 5. 本文の抜き出しに上限をつける（`backend/src/mcp/`）

`TransportError::InvalidJson` に入れる takt-mcp の応答の本文を、先頭 200 文字（文字の境界で切る）までにし、切ったときは「…（ほか N 文字）」を付ける。`mcp/http.rs` の SSE の枝がすでに 200 文字で切っているので、上限をそろえ、1 つの関数（`response_excerpt`）を 3 か所で使う。

- 対象：`mcp/client.rs:382`（stdio の 1 行）、`mcp/http.rs:550`（SSE）、`mcp/http.rs:557`（JSON の本文）。
- serde_json の解析エラーの文も、型の合わない文字列を全文で含むことがある（PR のレビューの指摘、CodeRabbit）。`mcp/client.rs:382` と `mcp/http.rs:557` では、解析エラーの文も同じ 200 文字で切る。
- ログに残す元のエラーは 2000 文字で切る（登録案の食い違いの枝も同じ関数 `log_detail` を通す）。
- 道具のエラーの本文（`ToolCallError.message`）と JSON-RPC のエラーの本文（`Rpc.message`）は、値としては切らない。`is_stale_registration` が本文の中の語で判定するので、値を変えない。画面へは 1 と 3 の「takt の理由」として、200 文字で切って出す。

## 変えないこと・範囲の外

- **gRPC の状態コード。** どの失敗も、今と同じコードを返す。公開 API（proto）は変えない。変わるのは Status の文面だけ。
- **画面（frontend）。** `_shared.ts` の HTTP ステータスの対応と、`details` をそのまま出す作りは変えない。
- **#391 の GetRunDetail。** `grpc/run_detail.rs` の `within_budget`（DEADLINE_EXCEEDED）と `remote_failure_status`（DEADLINE_EXCEEDED、UNAVAILABLE、NOT_FOUND の振り分け）は、コードも文面も変えない。`tool_error_status` は使っていない。
- **読み取りの文面（範囲の外）。** run 詳細の `（保存場所: {local}）`、本文の `ContentRead::Unreadable`、`RunReadFailure` の表示、SQLite の INTERNAL は、画面に届かない（確認 1）ので今回は変えない。ゴール詳細・タスク詳細の読み取りは `tool_error_status` を通るので、1 の文面に変わる（コードは同じで、画面は今も固定の文面を出す）。
- **入力の誤りの文面。** INVALID_ARGUMENT の「goal_id が不正です: …」などは、利用者が送った値を返すだけで、設定や takt-mcp の応答を含まないので変えない。`resolve_project_in` の「プロジェクト {name} の cwd が設定にありません。」なども、画面に出ている名前だけなので変えない。

## 守る不変条件

1. 書き込み RPC が返す Status の文面は、輸送のエラーの本文、解析の失敗の本文、`InvalidJson` の応答の本文と、設定のエラーの本文（パス、接続先の URL）を含まない。
2. 1〜4 の境界が返す文面は、失敗の種類（と Timeout・Backoff の秒数、道具のエラーと JSON-RPC のエラーの本文を整えた「takt の理由」）だけで決まる。
2a. 「takt の理由」は、改行と制御文字を含まず、200 文字と「…」を超えない。
3. gRPC の状態コードは、どの失敗でも変更の前と同じ。書き込みの Unknown／Failed の振り分けも同じ（#305 の結果不明の 504、#391 のコードを保つ）。
4. 文面から外した詳細は、同じ失敗で backend のログに残る。
5. `InvalidJson` に入る応答の本文は、先頭 200 文字までと省略の印だけ。

## PBT の性質（`proptest`、除外なし）

`grpc/` の単体テストに置く。文字列は任意の Unicode（空、改行、`{`、`}`、`（`、制御文字を含む）。

- **P1 文面は本文によらない**：`Tool`（`refused` が true）と `Rpc` を除く `ToolError` のすべての枝（`TransportError` の 9 種、`refused` が false の `Tool`、`Parse`）について、任意の本文 `a`、`b` で作った 2 つの失敗の `tool_error_status` の文面は等しい。`Timeout`・`Backoff` は同じ秒数で比べる。
- **P1a takt の理由**：`Tool`（任意の道具名と本文）と `Rpc`（任意のコードと本文）の文面は、固定の文と `takt_reason(本文)` だけで決まる（道具名とコードによらない）。`takt_reason(s)` は改行と制御文字を含まず、長さは 200 文字＋「…」以下で、`s` を整えたものが 200 文字以下なら、それと等しい。
- **P2 書き込みの文面も本文によらない**：P1 と同じ組で、`write_failure` が返す Status の文面と、Unknown／Failed の別が等しい。
- **P3 コードの表**：`tool_error_status` と `write_failure` のコードと Unknown／Failed の別が、表（変更の前のコードの対応を書き写したもの）と一致する。
- **P4 登録案の食い違い**：`stale_summary` を含む任意の本文で、`registration_write_failure` の文面が 3 の固定の文と `takt_reason(本文)` だけで決まり、コードは FAILED_PRECONDITION。
- **P5 設定**：`ConfigError` のすべての枝（任意のパスと本文）で、`config_load_status` の文面が等しく、コードは UNAVAILABLE。
- **P6 抜き出し**：任意の文字列 `s` で、`response_excerpt(s)` は (a) `s` が 200 文字以下なら `s` と等しい、(b) それより長ければ `s` の先頭 200 文字で始まり、全体は 200 文字＋省略の印の長さ以下。文字の境界で切る（パニックしない）。

P1、P2、P5 は「本文が文面に入らない」より強い（どんな本文でも文面が変わらない）ので、本文が偶然に固定の文の一部と一致する場合を除外する必要がない。

## 不具合を捕らえるテスト（先に書いて、失敗を確かめる）

- `backend/tests/write_grpc_test.rs`：偽の takt-mcp が、複数行で長い道具のエラーを返すとき、書き込み RPC の Status の文面は固定の文で始まり、「takt の理由: 」に 1 行目が続き、改行を含まず、長さに上限がある。修正前は道具名つきの本文が改行ごと全部入るので失敗する。
- 同：設定ファイルを壊して（存在しないパス）書き込み RPC を呼ぶと、文面に設定のパスが入らない。修正前は入る。
- 上の PBT の P1〜P6。

## 状態遷移と Quint

ありません。変えるのは、失敗の値から文面を作る純粋な対応だけで、再試行、台帳（`write_ledger`）、接続の開け閉めの遷移は変えません。Unknown／Failed の振り分けも変えないので、台帳の扱い（結果不明の鍵の再送を断る）も同じです。Quint の仕様は追加しません。

## 壊しうる入力

- 本文：空、とても長い（MB 単位）、多バイト文字（切る位置が文字の途中に来る）、改行、制御文字、`{}`（format の書式と誤らないこと。値は引数で渡すので書式にならない）。
- `Timeout(0)`、`Backoff(u64::MAX)`：数字の表示だけ。
- 登録案の本文：`stale_summary` が先頭、途中、末尾。大文字（判定は今と同じく大文字小文字を区別する）。
- 設定：存在しないパス、JSON の誤り、検証の誤り（`Invalid`）。

## 影響

- **上流（takt、takt-mcp）**：なし。
- **保存済みのデータ**：なし。状態ストアにも書き込みの台帳にも、文面は保存しない（台帳はメモリの中で Status を持つだけ）。
- **公開 API**：proto と gRPC の状態コードは変えない。Status の文面は変わる。文面に頼るのは画面（そのまま出すだけ）と、backend のテストの `contains` の確認。テストの確認は新しい文面に合わせて直す（「読み込み直」「結果は不明」「再送していません」「接続先の設定が変わる前」は残る）。e2e の `e2e/tests/flow-8-write-failure-recovery.spec.ts` は、旧い文（「takt-mcp の道具が失敗しました」）を待っていたので、新しい文（「takt が操作を受け付けませんでした。」）に直す。
- **守りの仕組み**：CI、関門、ルールセットは変えない。
- **運用**：書き込みの失敗が backend のログ（launchd の標準エラー）に `warn` で 1 行ずつ増える。書き込みは人の操作の回数だけなので、量は小さい。
