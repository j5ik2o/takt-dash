# issue #364 の設計：停止で接続を閉じる段に全体の上限を設けて同時に閉じ、接続先が変わった直後に旧接続先の置き場を登録し直さない

対象の issue：[j5ik2o/takt-dash#364](https://github.com/j5ik2o/takt-dash/issues/364)（[j5ik2o/takt-dash#310](https://github.com/j5ik2o/takt-dash/issues/310) の残り。[PR #357](https://github.com/j5ik2o/takt-dash/pull/357) の再レビューの任意の注記 2 点）

## 現在の main での確認（`020d9ab`）

コードを読んで、2 点とも issue のとおりに起きる作りであることを確かめました。

1. **停止で接続を閉じる段に、全体の上限が無い。**
   - `backend/src/mcp/registry.rs` の `shutdown_all`（395-411 行）は、stdio のクライアントを 1 件ずつ `client.lock().await.shutdown().await` で止め、そのあと HTTP セッションを 1 件ずつ `session.close().await` で閉じる。
   - `HttpSession::close`（`backend/src/mcp/http.rs` 130-135 行）の上限は、確立の鍵の待ち `CLOSE_WAIT_FOR_ESTABLISH`（10 秒）と DELETE の `CLOSE_TIMEOUT`（5 秒）で、1 件 15 秒。
   - `Poller::shutdown`（`backend/src/poller.rs` 723-728 行）は、`shutdown_all` のあとで、プロジェクトごとの読み取りハンドルを 1 件ずつ閉じる。
   - `main.rs` は `run_until_stopped` が返るのを待つだけで、上限を持たない。
   - そのため、応答しないサーバーでは最悪で 15 秒 ×（HTTP サーバーの数 ＋ HTTP のプロジェクトの数）かかる。加えて、issue に無い点として、stdio のクライアントの鍵は画面の読み書きの要求が道具を呼んでいる間は空かず、その上限（`request_timeout` と道具ごとの上限）まで停止が待つ。
2. **接続先が変わった直後、旧設定の要求が旧接続先の置き場を登録し直す。**
   - `retain_configured`（`registry.rs` 332-355 行）は、接続先の map（ID → `HttpMcpServer`）を受け取るが、残すのは ID の集合 `http_servers`（116 行）だけ。
   - `http_session`（`registry.rs` 202-222 行）は `configured && !http_servers.contains(id)` しか見ない。旧設定で解決済みの要求（`backend/src/grpc/resolve.rs` は要求ごとに `load_config` する）が反映のあとに来ると、211-218 行で接続先の違いを「作り直し」と見て、旧接続先の置き場を登録し、新しい接続先の置き場を追い出す。
   - 次の新設定の要求か次の反映で戻る。追い出した置き場は持ち主が消えると `Drop` で閉じるので、たまり続けはしない。

Quint の仕様でも、修正前の遷移で 2 点とも反例が出ます（下の「Quint の仕様」）。

## 何をどう変えるか

### 1. 停止：すべてを同時に閉じ始め、全体を `SHUTDOWN_DEADLINE`（20 秒）で打ち切る

- `backend/src/mcp/http.rs`：1 件を閉じる上限を `pub(crate) const SESSION_CLOSE_BOUND: Duration`（`CLOSE_WAIT_FOR_ESTABLISH` ＋ `CLOSE_TIMEOUT`、15 秒）として出す。`HttpReadHandle` の中身は変えない。
- `backend/src/mcp/registry.rs`：
  - `pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20)` と、`const _: () = assert!(SESSION_CLOSE_BOUND.as_secs() < SHUTDOWN_DEADLINE.as_secs());` を置く。応答しないサーバーも、同時に閉じれば自分の上限（15 秒）で打ち切られ、全体の上限の前に終わる。
  - `pub async fn shutdown_within(&self, deadline: Duration, reads: Vec<Arc<AsyncMutex<HttpReadHandle>>>)` を足す。1 回のロックで stdio のクライアントと HTTP セッションを map から取り出し、`tokio::task::JoinSet` に、クライアントごとの「鍵を取って kill」、セッションごとの `close`、読み取りハンドルごとの「鍵を取って `close`」を同時に入れる。`tokio::time::timeout(deadline, …)` で全部の終わりを待ち、時間切れなら残りの件数を `warn` に出して返る（`JoinSet` を捨てると残りのタスクは中断される）。
  - `shutdown_all(&self)` は `shutdown_within(SHUTDOWN_DEADLINE, Vec::new())` を呼ぶだけにする（呼び出し元の `backend/tests/mcp_lanes_test.rs` はそのまま）。
- `backend/src/poller.rs`：`Poller::shutdown` は、`reads` を取り出してハンドルを `shutdown_within(SHUTDOWN_DEADLINE, handles)` に渡す。読み取りハンドルを閉じる段も、同じ全体の上限に入る。
- 打ち切ったもの：stdio の子プロセスは、鍵を持つ要求が終わるか、プロセスの終了で `kill_on_drop` により止まる（`McpClient` の今の作り）。HTTP セッションは、応答しないサーバーのもので、サーバーが再起動するまで残る（いまも同じ）。

### 2. 接続先：反映済みの接続先まで照合する

- `RegistryInner.http_servers: BTreeSet<String>` を `http_endpoints: BTreeMap<String, HttpMcpServer>`（反映済みの HTTP の接続先）に替える。`retain_configured` は受け取った map をそのまま残す。
- `http_session(id, server)`：
  - 反映の後（`configured`）で、`http_endpoints.get(id) != Some(server)` なら、その要求だけが使うセッション（`HttpSession::for_server(server.clone())`）を返し、`http_sessions` に触れない。設定から消えたサーバー（`get` が `None`）も同じ枝に入る（いまの挙動と同じ）。
  - それ以外は今のまま（登録済みで接続先が同じなら共有、無ければ作って登録）。反映の後に「接続先が違う登録済みの置き場」があることは、`retain_configured` がそれを外すので無い。反映の前（`configured == false`）は照合する設定が無いので、今のまま、要求の接続先で作り直す。
- `client_for_key` の `http_servers.contains(id)` は `http_endpoints.contains_key(id)` に替える（意味は同じ）。

### 変えないこと・範囲の外

- 停止の合図より前に始まった定期読み取りの周期は、今のまま終わるまで待つ（`run_until_stopped` は待機の間だけ停止を見ている）。周期の中の読み取りはプロジェクトごとに `READ_TIMEOUT_SECONDS`（15 秒）で打ち切られる。周期を途中で捨てると出来事の記録の途中で止まりうるので、この issue では触れない。
- gRPC サーバーの穏やかな停止（`serve_with_incoming_shutdown`）と `main.rs` は変えない。
- 閉じる段の途中に届いた画面の要求が、置き場を作り直すこと（`HttpSession::close` の説明にある、今の挙動）は変えない。
- 反映より先に新しい設定で解決した要求は、次の反映まで（定期読み取りの 1 周期まで）、その要求だけのセッションを使う（作って閉じる）。今は新しい要求が置き場を作り直していた。正しさは変わらず、往復が 1 件ごとに 3 回（initialize・initialized・DELETE）増えるだけ。どちらの要求が新しいかを決める世代を要求に持たせることはしない。
- `http.rs` の確立・404・再試行、`CLOSE_TIMEOUT` と `CLOSE_WAIT_FOR_ESTABLISH` の値、proto、保存済みデータ、待ち受け（`0.0.0.0:3000`）と接続元の制限には触れない。

## 守る不変条件

停止（`specs/quint/backend-shutdown-deadline/shutdownDeadline.qnt`）

1. 閉じる段は、停止を始めてから `SHUTDOWN_DEADLINE` を超えない（`stopWithinDeadline`）。
2. 応答するサーバーのセッションは、閉じる段を終えるまでに閉じる（`respondingSessionsClosed`）。応答しないサーバーが前にあっても待たされない。
3. 停止を始めた時点で鍵の空いていた子プロセスは、閉じる段を終えるまでに kill する（`idleChildrenKilled`）。
4. 見捨てるのは、全体の上限に達したときだけ（`abandonOnlyAtDeadline`）。
5. 前提：1 件を閉じる上限は全体の上限より短い（`CLOSE_BOUND < SHUTDOWN_DEADLINE`。コードは `const` の `assert!`）。

接続先（`specs/quint/mcp-http-session-endpoint/sessionEndpoint.qnt`）

6. 反映済みの設定があれば、置き場（slot）は反映済みの接続先のセッションだけ（`slotMatchesReflected`）。
7. takt-mcp に残るセッションは、slot か、置き場を持つ要求のものだけ（`noOrphanSession`）。
8. 置き場を持つ要求は、自分が解決した接続先のセッションを使う（`requestUsesItsEndpoint`）。旧設定の要求を拒まない。
9. slot に登録されたセッションは閉じていない（`slotSessionIsLive`）。

## Quint の仕様

新しく 2 つ足します。既存の `specs/quint/mcp-http-session/` は 1 件の要求の確立・404・再試行・届いたかどうかを扱い、接続先の変更と停止を範囲の外にしています。広げると、そこで確かめている性質と関わらない次元（接続先、時間）が状態に入り、検査も読み手の負担も増えるので、関心ごとに分けます。README には、ほかの仕様と同じ書き方で `quint test`・`quint run` のコマンドと結果を書きました。

| 仕様 | モジュール | 結果（quint 0.32.0、`quint run` と `quint test`） |
| --- | --- | --- |
| `backend-shutdown-deadline/shutdownDeadline.qnt` | `current`（順に、上限なし） | `stopWithinDeadline` に 9 手で違反 |
| | `deadlineOnly` | `respondingSessionsClosed` に 5 手、`idleChildrenKilled` に 6 手で違反 |
| | `parallelOnly` | `stopWithinDeadline` に 9 手で違反（鍵を持つ道具の呼び出しを待つ） |
| | `fixed` | `safety` の反例なし（50000 サンプル、max-steps 30）。witness 4 つとも到達 |
| | `shutdownDeadline_test.qnt` | 6 件成功 |
| `mcp-http-session-endpoint/sessionEndpoint.qnt` | `current` | `slotMatchesReflected` に 9 手で違反 |
| | `fixed` | `safety` の反例なし（50000 サンプル、max-steps 25）。witness 4 つとも到達 |
| | `sessionEndpoint_test.qnt` | 6 件成功 |

上限と同時に閉じることの片方だけでは足りないことを、`deadlineOnly` と `parallelOnly` の反例で示しています。

## PBT の性質（`proptest`、除外なし）

1. **`http_session` の照合（モデルとの比較）**：`backend/src/mcp/registry.rs` のテスト。操作の列（長さ 1〜24）を作る。操作は「反映：ID `a`・`b` ごとに、接続先 {ポート 4100, 4101} × {トークン 2 種} か、設定に無い」と「要求：ID と接続先を選び `http_session` を呼び、返った `HttpSession` を持つ」と「要求の終わり：持っているものを 1 つ捨てる」。各操作のあとで次を確かめる。
   - 反映の後なら、`http_sessions` の各登録の接続先は、反映済みの接続先と同じ（不変条件 6）。
   - 反映の後に、反映済みの接続先と同じ要求は、登録済みの置き場を共有する（無ければ登録する）。
   - 反映の後に、反映済みの接続先と違う要求（接続先が違う、トークンだけが違う、設定に無い）は、登録済みのどれとも共有しない置き場を受け取り、`http_sessions` の中身（置き場の同一性）を変えない。
   - 反映の前は、要求の接続先の置き場が登録される（今の挙動）。
   - 返った `HttpSession` の接続先は、要求の接続先（不変条件 8）。
2. **停止の上限と同時に閉じること**：`backend/tests/http_read_test.rs`（mock の HTTP サーバーと stdio の mock を使う）。HTTP サーバーを 1〜3 台、それぞれ「DELETE に応答しない（新しい振る舞い `hang-delete`）」か「応答する」かを選び、ID の並びも選ぶ。stdio のクライアントを 0〜2 本起動し、それぞれ「テストが鍵を持ったまま」か「空いている」かを選ぶ。プロジェクトの読み取りハンドル（`reads`）も 0〜2 本、同じく選んだサーバーに向けて渡す。`shutdown_within(800ms, reads)` が次を満たす。
   - 返るまでの時間が 800ms ＋ 余裕（500ms）以内（不変条件 1）。
   - 応答するサーバーの生きたセッションが 0（不変条件 2）。応答しないサーバーが ID の並びで前にあっても同じ。
   - 鍵の空いていた子プロセスは止まっている（不変条件 3）。
   - 鍵を持たれていた子プロセスは、テストが鍵を放してハンドルを捨てたあとに止まる（打ち切りでも残らない）。
   - 実時間と子プロセスを使うので、ケース数は 8 に絞る（性質の除外ではなく回数の制限）。

あわせて、修正前に落ちる例のテストを先に書く：`a_stale_request_does_not_evict_the_session_of_the_new_endpoint`（registry.rs）と、`shutdown_does_not_wait_for_an_unresponsive_server_before_closing_the_others`（http_read_test.rs）。

## 壊しうる入力

- 境界：反映の前（`configured == false`）、反映で 0 台になる、ID はそのままで URL だけ・トークンだけが変わる、同じ接続先を 2 つの ID が持つ。
- 並び：応答しない相手が `BTreeMap` の順で前にある（修正前に後ろを待たせる）。
- 並行：閉じる段と同時に画面の要求が来る（範囲の外。今の挙動のまま）。反映と要求の取り合い（レジストリの `std::sync::Mutex` の中で 1 つずつ進む）。
- 時間：確立の途中のセッション（`CLOSE_WAIT_FOR_ESTABLISH` を待つ。既存の `closing_waits_for_an_establishment_in_flight_and_closes_its_session` が固定）、鍵を長く持つ道具の呼び出し。
- 再開：停止は 1 回だけ。打ち切ったあとはプロセスが終わる。

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- 上流（takt-mcp）：送る要求の種類は変わらない。旧設定の要求で、セッションの作成と DELETE が 1 件ずつ増えることがある（上の「範囲の外」）。
- 保存済みデータ・proto：変更なし。
- 公開 API：`McpClientRegistry::shutdown_within` と `SHUTDOWN_DEADLINE` を足す（crate の中とテストだけが使う）。`shutdown_all` と `Poller::shutdown` の形は変えない。
- 守りの仕組み（CI・関門・ルールセット）：変更なし。テスト用の mock（`backend/tests/mock_mcp_http_server.mjs`）に `hang-delete` を足す。
