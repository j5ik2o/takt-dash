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

### 1. 停止：長い周期を打ち切り、すべてを同時に閉じ始め、閉じる段と停止の全体に上限を設ける

#### 上限の決め方（設計レビューの指摘 1 を受けて決めた）

本番の backend は launchd（`~/Library/LaunchAgents/com.ideoplus.taktdash-backend.plist`）で動いており、plist に `ExitTimeOut` が無い。launchd の既定は、SIGTERM の 20 秒後に SIGKILL。SIGKILL されると後始末が何も走らず、閉じていないセッションは takt-mcp に残る。そこで、停止の全体を合図から 20 秒より前に終える（案 (a)）。plist は変えない（案 (b) は採らない。本番の設定に頼らず、backend だけで上限を守れるため）。

レビューは「合図より前に始まった周期を待つのは最大 15 秒」としていたが、コードを読むと、周期の上限はもっと長い。`READ_TIMEOUT_SECONDS`（15 秒）は道具の呼び出し 1 回ごとの上限で、`read_snapshot` は道具を何回か順に呼ぶ。周期の全体には上限が無い。周期を待ったまま 20 秒に収めることはできないので、**合図のあと周期を `CYCLE_GRACE`（2 秒）だけ待ち、それでも終わらなければ打ち切る**ことにした（実装中の変更。すぐ打ち切ると、ふだんの短い周期で記録するはずの出来事も落とすため）。

| 上限 | 値 | どこで守るか |
| --- | --- | --- |
| 1 件を閉じる（確立の鍵の待ち ＋ DELETE） | 10 秒（`CLOSE_WAIT_FOR_ESTABLISH` を 10 秒から 5 秒に下げる ＋ `CLOSE_TIMEOUT` 5 秒） | `HttpSession::close` |
| 周期を待つ | 2 秒（`CYCLE_GRACE`） | `run_until_stopped` |
| 閉じる段 | 15 秒（`SHUTDOWN_DEADLINE`） | `McpClientRegistry::shutdown_within` |
| 停止の全体（合図から） | 18 秒（`STOP_DEADLINE`） | `main.rs` |
| launchd の SIGKILL | 20 秒（`ExitTimeOut` の既定） | 外側（変えない） |

並びは `const _: () = assert!(...)` で固定する（1 件 < 閉じる段、周期を待つ ＋ 閉じる段 < 停止の全体 < launchd）。launchd の 20 秒は `LAUNCHD_EXIT_TIMEOUT` という定数に、plist の場所と「ExitTimeOut を足すならこの値も直す」という説明をつけて置く。

`CLOSE_WAIT_FOR_ESTABLISH` を使うのは停止の時の `close` だけ（`Drop` の後始末は待たない）。確立は普段なら数ミリ秒で終わるので、5 秒で足りる。確立が 5 秒を超えるのは応答の遅いサーバーで、その時は確立の後にできたセッションが残りうる（今の 10 秒でも同じことが起きうる）。

#### 変えるところ

- `backend/src/mcp/http.rs`：`CLOSE_WAIT_FOR_ESTABLISH` を 5 秒にする。1 件を閉じる上限を `pub(crate) const SESSION_CLOSE_BOUND: Duration`（`CLOSE_WAIT_FOR_ESTABLISH` ＋ `CLOSE_TIMEOUT`、10 秒）として出す。
- `backend/src/mcp/registry.rs`：
  - `pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(15)` と、`SESSION_CLOSE_BOUND < SHUTDOWN_DEADLINE` の `const` の検査を置く。応答しないサーバーも、同時に閉じれば自分の上限（10 秒）で打ち切られ、閉じる段の上限の前に終わる。
  - `pub async fn shutdown_within(&self, deadline: Duration, reads: Vec<Arc<AsyncMutex<HttpReadHandle>>>)` を足す。1 回のロックで stdio のクライアントと HTTP セッションを map から取り出し、`tokio::task::JoinSet` に、クライアントごとの「鍵を取って kill」、セッションごとの `close`、読み取りハンドルごとの「鍵を取って `close`」を同時に入れる。`tokio::time::timeout(deadline, …)` で全部の終わりを待ち、時間切れなら残りの件数を `warn` に出して返る（`JoinSet` を捨てると、残りのタスクは中断される）。
  - `shutdown_all(&self)` は `shutdown_within(SHUTDOWN_DEADLINE, Vec::new())` を呼ぶだけにする（呼び出し元の `backend/tests/mcp_lanes_test.rs` はそのまま）。
- `backend/src/poller.rs`：
  - `run_until_stopped` は、周期（`read_cycle_once`）と停止の合図を `tokio::select!` で待つ。合図が先なら、周期を `CYCLE_GRACE` だけ待ち、終わらなければ周期の future を捨てて、ループを抜けて `shutdown` へ進む。
  - `Poller::shutdown` は、`reads` を取り出してハンドルを `shutdown_within(SHUTDOWN_DEADLINE, handles)` に渡す。読み取りハンドルを閉じる段も、同じ上限に入る。
  - `pub const CYCLE_GRACE`（2 秒）、`STOP_DEADLINE`（18 秒）、`LAUNCHD_EXIT_TIMEOUT`（20 秒）を置き、`CYCLE_GRACE + SHUTDOWN_DEADLINE < STOP_DEADLINE < LAUNCHD_EXIT_TIMEOUT` を `const` で検査する。
- `backend/src/main.rs`：gRPC サーバーと定期読み取りの `join!` を、「停止の合図を待ってから `STOP_DEADLINE` 眠る」future と `select!` する。上限に達したら `warn` を出して `main` から返る（gRPC の穏やかな停止が、長い要求を待ち続けるときの外側の守り）。

#### 周期を打ち切ってよい理由

- 周期の中の読み取りは、プロジェクトごとに `tokio::spawn` したタスクで、future を捨ててもタスクは最後まで走るか、プロセスの終了で捨てられる。タスクが持つクライアントの鍵は、閉じる段では「鍵の埋まった子プロセス」として扱われ、上限で見捨てる（プロセスの終了で `kill_on_drop` により止まる）。
- 周期の中で書くもの（出来事の記録）は 1 回ずつ同期の SQLite の呼び出しで、記録とその直後のメモリの更新（差分基準・接続状態）の間に `await` は無い。打ち切りは記録の前か後でしか起きず、書きかけは残らない。メモリの状態はプロセスの終了で捨てるもので、次の起動は基準化から始まる（今の作り）。そのため次の起動で同じ出来事を二重に記録しない。打ち切った周期で見つけたはずの出来事は記録されないが、これは周期の途中で backend を止めたとき（SIGKILL を含む）と同じで、2 秒に収まるふだんの周期では起きない（テスト `a_stop_signal_during_a_long_cycle_cancels_it_and_a_restart_records_each_event_once` と `a_stop_signal_during_a_short_cycle_lets_it_record_before_stopping`）。
- 打ち切らなくても、launchd が 20 秒で SIGKILL すれば同じ位置かもっと悪い位置で止まる。`await` の境目で止めるほうが安全。
- `gh` の子プロセス（`pull_request.rs`）も `kill_on_drop(true)` で、future を捨てれば止まる。

#### 打ち切ったもの

stdio の子プロセスは、鍵を持つ要求が終わるか、プロセスの終了で `kill_on_drop` により止まる（`McpClient` の今の作り）。HTTP セッションは、応答しないサーバーのもので、サーバーが再起動するまで残る（いまも同じ）。

### 2. 接続先：反映済みの接続先まで照合する

- `RegistryInner.http_servers: BTreeSet<String>` を `http_endpoints: BTreeMap<String, HttpMcpServer>`（反映済みの HTTP の接続先）に替える。`retain_configured` は受け取った map をそのまま残す。
- `http_session(id, server)`：
  - 反映の後（`configured`）で、`http_endpoints.get(id) != Some(server)` なら、その要求だけが使うセッション（`HttpSession::for_server(server.clone())`）を返し、`http_sessions` に触れない。設定から消えたサーバー（`get` が `None`）も同じ枝に入る（いまの挙動と同じ）。
  - それ以外は今のまま（登録済みで接続先が同じなら共有、無ければ作って登録）。反映の後に「接続先が違う登録済みの置き場」があることは、`retain_configured` がそれを外すので無い。反映の前（`configured == false`）は照合する設定が無いので、今のまま、要求の接続先で作り直す。
- `client_for_key` の `http_servers.contains(id)` は `http_endpoints.contains_key(id)` に替える（意味は同じ）。

### 変えないこと・範囲の外

- gRPC サーバーの穏やかな停止（`serve_with_incoming_shutdown`）の中身は変えない。`main.rs` の `STOP_DEADLINE` が外から打ち切るだけにする。
- launchd の plist（`ExitTimeOut`）は変えない。
- 閉じる段の途中に届いた画面の要求が、置き場を作り直すこと（`HttpSession::close` の説明にある、今の挙動）は変えない。
- 反映より先に新しい設定で解決した要求は、次の反映まで（定期読み取りの 1 周期まで）、その要求だけのセッションを使う（作って閉じる）。今は新しい要求が置き場を作り直していた。正しさは変わらず、往復が 1 件ごとに 3 回（initialize・initialized・DELETE）増えるだけ。どちらの要求が新しいかを決める世代を要求に持たせることはしない。
- `http.rs` の確立・404・再試行、`CLOSE_TIMEOUT` と `CLOSE_WAIT_FOR_ESTABLISH` の値、proto、保存済みデータ、待ち受け（`0.0.0.0:3000`）と接続元の制限には触れない。

## 守る不変条件

停止（`specs/quint/backend-shutdown-deadline/shutdownDeadline.qnt`）

0. launchd が SIGKILL する前（合図から 20 秒未満）に、プロセスが終わる（`stopBeforeExitTimeout`）。周期の途中で合図を受けても同じ。
0'. ふだんの短い周期（2 秒以下）は打ち切らない（`shortCyclesComplete`）。
1. 閉じる段は `SHUTDOWN_DEADLINE` を超えない（`closesWithinDeadline`）。
2. 応答するサーバーのセッションは、プロセスが終わるまでに閉じる（`respondingSessionsClosed`）。応答しないサーバーが前にあっても、周期の途中でも待たされない。
3. 合図の時点で鍵の空いていた子プロセスは、プロセスが終わるまでに kill する（`idleChildrenKilled`）。
4. 見捨てるのは、上限に達したときだけ（`abandonOnlyAtDeadline`）。
5. 前提：1 件を閉じる上限 < 閉じる段の上限 < 停止の全体の上限 < launchd の SIGKILL（`boundsAreOrderedTest`。コードは `const` の `assert!`）。

接続先（`specs/quint/mcp-http-session-endpoint/sessionEndpoint.qnt`）

6. 反映済みの設定があれば、置き場（slot）は反映済みの接続先のセッションだけ（`slotMatchesReflected`）。
7. takt-mcp に残るセッションは、slot か、置き場を持つ要求のものだけ（`noOrphanSession`）。
8. 置き場を持つ要求は、自分が解決した接続先のセッションを使う（`requestUsesItsEndpoint`）。旧設定の要求を拒まない。
9. slot に登録されたセッションは閉じていない（`slotSessionIsLive`）。

## Quint の仕様

新しく 2 つ足します。既存の `specs/quint/mcp-http-session/` は 1 件の要求の確立・404・再試行・届いたかどうかを扱い、接続先の変更と停止を範囲の外にしています。広げると、そこで確かめている性質と関わらない次元（接続先、時間）が状態に入り、検査も読み手の負担も増えるので、関心ごとに分けます。README には、ほかの仕様と同じ書き方で `quint test`・`quint run` のコマンドと結果を書きました。

| 仕様 | モジュール | 結果（quint 0.32.0、`quint run` と `quint test`） |
| --- | --- | --- |
| `backend-shutdown-deadline/shutdownDeadline.qnt` | `current`（周期を待ち、順に、上限なし） | `stopBeforeExitTimeout` に 22 手、`closesWithinDeadline` に 42 手で違反 |
| | `deadlineOnly` | `respondingSessionsClosed` に 20 手、`idleChildrenKilled` に 20 手で違反 |
| | `parallelOnly` | `stopBeforeExitTimeout` に 22 手で違反（鍵を持つ道具の呼び出しを待つ） |
| | `withoutCycleCancel`（設計の最初の版） | `respondingSessionsClosed` に 20 手、`idleChildrenKilled` に 19 手で違反（周期を待つうちに全体の上限に達する） |
| | `immediateCancel`（設計の 2 番目の版） | `shortCyclesComplete` に 2 手で違反（ふだんの短い周期も打ち切る） |
| | `fixed` | `safety` の反例なし（50000 サンプル、max-steps 60）。witness 7 つとも到達 |
| | `shutdownDeadline_test.qnt` | 9 件成功 |
| `mcp-http-session-endpoint/sessionEndpoint.qnt` | `current` | `slotMatchesReflected` に 9 手で違反 |
| | `fixed` | `safety` の反例なし（50000 サンプル、max-steps 25）。witness 4 つとも到達 |
| | `sessionEndpoint_test.qnt` | 6 件成功 |

同時に閉じること・上限・長い周期の打ち切り・短い周期を待つことの 4 つがすべて要ることを、`deadlineOnly`・`parallelOnly`・`withoutCycleCancel`・`immediateCancel` の反例で示しています。外側の上限（launchd の 20 秒）は `EXIT_TIMEOUT` として仕様に入れ、不変条件 0 で確かめます。

## PBT の性質（`proptest`、除外なし）

1. **`http_session` の照合（モデルとの比較）**：`backend/src/mcp/registry.rs` のテスト。操作の列（長さ 1〜24）を作る。操作は「反映：ID `a`・`b` ごとに、接続先 {ポート 4100, 4101} × {トークン 2 種} か、設定に無い」と「要求：ID と接続先を選び `http_session` を呼び、返った `HttpSession` を持つ」と「要求の終わり：持っているものを 1 つ捨てる」。各操作のあとで次を確かめる。
   - 反映の後なら、`http_sessions` の各登録の接続先は、反映済みの接続先と同じ（不変条件 6）。
   - 反映の後に、反映済みの接続先と同じ要求は、登録済みの置き場を共有する（無ければ登録する）。
   - 反映の後に、反映済みの接続先と違う要求（接続先が違う、トークンだけが違う、設定に無い）は、登録済みのどれとも共有しない置き場を受け取り、`http_sessions` の中身（置き場の同一性）を変えない。
   - 反映の前は、要求の接続先の置き場が登録される（今の挙動）。
   - 返った `HttpSession` の接続先は、要求の接続先（不変条件 8）。
2. **閉じる段の上限（tokio の時間を止めて確かめる）**：`backend/src/mcp/registry.rs` のテスト（`#[tokio::test(start_paused = true)]`、実際の入出力を使わない）。stdio のクライアントを 0〜3 本（子プロセスは起動しない）、読み取りハンドルを 0〜3 本（セッションは確立しない）作り、それぞれ「テストが鍵を持ったまま」か「空いている」かを選ぶ。登録済みの HTTP セッション（確立しない）も 0〜2 件。`shutdown_within(deadline, reads)`（`deadline` は 1〜30 秒から選ぶ）が次を満たす。
   - 鍵を持たれたものが 1 つでもあれば、止めた時計でちょうど `deadline` で返る。無ければ、時計を進めずに返る（不変条件 1、4）。
   - 返ったあと、レジストリの stdio のクライアントと HTTP セッションの map は空。
   - 空いていたクライアント・読み取りハンドルの鍵を、返ったあとにテストがすぐ取れる（閉じる処理が鍵を持ち続けていない）。
3. **応答するサーバーを待たせないこと（実時間）**：`backend/tests/http_read_test.rs`（mock の HTTP サーバーと stdio の mock を使う）。HTTP サーバーを 1〜3 台、それぞれ「DELETE に応答しない（新しい振る舞い `hang-delete`）」か「応答する」かを選び、ID の並びも選ぶ。起動済みで鍵の空いた stdio のクライアントを 0〜2 本。プロジェクトの読み取りハンドルも 0〜2 本、選んだサーバーに向けて渡す。`shutdown_within(2 秒, reads)` が次を満たす。
   - 応答するサーバーの生きたセッションが 0（不変条件 2）。応答しないサーバーが ID の並びで前にあっても同じ。
   - 起動済みの子プロセスは止まっている（不変条件 3）。
   - 時間は「上限を大きく超えなかったか」だけを見る（2 秒 ＋ 余裕 2.5 秒未満。`hang-delete` は 1 件 5 秒待たせるので、順に閉じる作りに戻れば超える）。高負荷のホストでの不安定さ（[j5ik2o/takt-dash#261](https://github.com/j5ik2o/takt-dash/issues/261) と同じ型）を避けるため、ちょうどの時間は 2 で確かめる。
   - 実時間と子プロセスを使うので、ケース数は 6 に絞る（性質の除外ではなく回数の制限）。
4. **停止の全体の上限（tokio の時間を止めて確かめる）**：`main.rs` の打ち切りを、`poller.rs` の小さな関数 `run_until_stop_deadline(run, stop, deadline)`（`run` が先に終わればその結果、合図から `deadline` が過ぎれば `None`）に切り出し、`start_paused` で確かめる。`run` の長さ（0〜60 秒）、合図の時刻（0〜60 秒、または合図なし）、`deadline`（1〜30 秒）を選び、返る時刻が `min(run の終わり, 合図 ＋ deadline)` と等しいこと。

あわせて、修正前に落ちる例のテストを先に書く：
- `a_stale_request_does_not_evict_the_session_of_the_new_endpoint`（registry.rs）
- `shutdown_does_not_wait_for_an_unresponsive_server_before_closing_the_others`（http_read_test.rs）
- `a_stop_signal_during_a_long_cycle_cancels_it_and_a_restart_records_each_event_once`（`poller_rebuild_tests.rs`。周期を `ReadBarrier` で止めたまま合図を送り、`run_until_stopped` が `CYCLE_GRACE` のあと周期の再開を待たずに返ること。打ち切った周期が出来事を残さず、次の起動で二重に記録しないこと）
- `a_stop_signal_during_a_short_cycle_lets_it_record_before_stopping`（合図のあと 2 秒以内に終わる周期は、出来事を 1 件記録してから止まり、次の起動で記録し直さないこと）
- `dropping_a_session_during_an_establishment_longer_than_the_close_wait_closes_it_afterwards`（http_read_test.rs。確立が 5 秒を超えても、ふだんの close（`Drop`）は確立のあとで DELETE を送ること）

## 壊しうる入力

- 境界：反映の前（`configured == false`）、反映で 0 台になる、ID はそのままで URL だけ・トークンだけが変わる、同じ接続先を 2 つの ID が持つ。
- 並び：応答しない相手が `BTreeMap` の順で前にある（修正前に後ろを待たせる）。
- 並行：閉じる段と同時に画面の要求が来る（範囲の外。今の挙動のまま）。反映と要求の取り合い（レジストリの `std::sync::Mutex` の中で 1 つずつ進む）。
- 時間：確立の途中のセッション（`CLOSE_WAIT_FOR_ESTABLISH` を待つ。既存の `closing_waits_for_an_establishment_in_flight_and_closes_its_session` が固定）、鍵を長く持つ道具の呼び出し、合図の時点で周期の途中（読み取りのタスクが鍵を持っている）、合図が周期の始まりと同時。
- 再開：停止は 1 回だけ。打ち切ったあとはプロセスが終わる。

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- 上流（takt-mcp）：送る要求の種類は変わらない。旧設定の要求で、セッションの作成と DELETE が 1 件ずつ増えることがある（上の「範囲の外」）。
- 保存済みデータ・proto：変更なし。
- 公開 API：`McpClientRegistry::shutdown_within`、`SHUTDOWN_DEADLINE`、`STOP_DEADLINE`、`LAUNCHD_EXIT_TIMEOUT`、`run_until_stop_deadline` を足す（crate の中、`main.rs`、テストだけが使う）。`shutdown_all` と `Poller::shutdown` の形は変えない。`run_until_stopped` は、合図のあと 2 秒を超える周期を打ち切るようになる。
- 本番の運用：停止は合図から 18 秒以内に終わる（今は上限が無く、launchd が 20 秒で SIGKILL していた）。plist は変えない。
- 守りの仕組み（CI・関門・ルールセット）：変更なし。テスト用の mock（`backend/tests/mock_mcp_http_server.mjs`）に `hang-delete` を足す。

## 設計レビューの指摘の扱い（43c138d への指摘）

| 指摘 | 扱い |
| --- | --- |
| 1（中）全体の上限と launchd の関係 | 直した。案 (a) を採り、停止の合図で周期を打ち切ること、1 件の上限を 10 秒に下げること、`main.rs` の `STOP_DEADLINE`（18 秒）を足した。指摘の前提「周期を待つのは最大 15 秒」は、15 秒が道具の呼び出し 1 回ごとの上限なので当たらず、周期を待ったまま 20 秒に収める案はとれなかった。Quint に外側の上限 `EXIT_TIMEOUT` と不変条件 `stopBeforeExitTimeout`、周期を待つ段（`Draining`）と `withoutCycleCancel` の版を足した |
| 2（低）PBT の実時間 | 直した。上限ちょうどの時間は、tokio の時間を止めて確かめる（性質 2 と 4）。実時間が要る性質 3 は、上限を大きく超えなかったかだけを見る |

## 実装中の変更（設計レビューの後）

- 合図で周期をすぐ打ち切る案を、2 秒だけ待ってから打ち切る案に替えた。すぐ打ち切ると、ふだんの短い周期で見つけた出来事も記録せずに止まり、次の起動は基準化から始まるので、その出来事を記録する機会がなくなる。Quint に `CYCLE_GRACE` と `immediateCancel` の版、不変条件 `shortCyclesComplete` を足した。
- `CLOSE_WAIT_FOR_ESTABLISH` を 5 秒に下げた影響：使うのは `HttpSession::close`（停止のとき）だけで、設定の反映で置き場を外すとき（`Drop`）は確立を待たない。確立はレジストリと別のタスクで最後まで走り、終わってから最後の持ち主が消えた時点で DELETE を送るので、確立に 5 秒以上かかってもセッションは残らない。停止のときに確立が 5 秒を超えると、確立のあとのセッションは、確立のタスクが終わる前にプロセスが終われば残る（10 秒のときも同じことが 10 秒で起きていた）。
- 停止の上限をちょうどの時間で確かめるため、dev-dependencies の `tokio` に `test-util`（`start_paused`）を足した。

## レビューボットの指摘で変えたこと（[PR #390](https://github.com/j5ik2o/takt-dash/pull/390)）

- 周期を打ち切っても、周期がプロジェクトごとに起動した読み取りタスク（`tokio::spawn`）は走り続け、読み取りハンドルやクライアントの鍵を持ったままだった。閉じる段が鍵を取れず、応答するサーバーのセッションも閉じられなかった（Devin Review と CodeRabbit の指摘）。`ReadTask`（捨てたら `abort` する `JoinHandle` の包み）を足し、周期の future を捨てたらタスクも中断する。Quint に `ABORT_CYCLE_TASKS`・`cycleHeld`・`withoutTaskAbort` の版を足した（反例あり。修正案は反例なし）。テスト `a_cancelled_cycle_stops_its_http_read_so_the_session_is_closed`（中断を外すと 10 秒待って落ちる）。
- `STOP_DEADLINE` で打ち切っても、tokio のランタイムを捨てるときに始まった `spawn_blocking` の終わりを待つ（Devin Review の指摘）。`main.rs` でランタイムを自分で作り、`shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT)`（1 秒）で捨てる。`STOP_DEADLINE + RUNTIME_SHUTDOWN_TIMEOUT < LAUNCHD_EXIT_TIMEOUT` を `const` で検査する。
- 接続先の仕様の到達条件 `staleRequestServed` と `oldSlotClosedOnReflect` が、意図しない筋書きも数えていた（CodeRabbit と Devin Review の指摘）。置き場を求めた時点の不一致と、反映が手放したセッションを記録して数えるように直し、意図しない筋書きを数えないことのテストを足した。
- aislop の `complexity/file-too-large`（`poller.rs` が 1100 行を超えた）を受け、停止に関わるもの（上限の定数、`ReadTask`、`run_until_stopped` の周り、`run_until_stop_deadline` とその性質のテスト）を `backend/src/poller_stop.rs` に分けた。`poller` からの公開の名前は変えていない。
