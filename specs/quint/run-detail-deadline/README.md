# run 詳細の取得の期限と、stdio の画面読み取りの切り離し（[issue #59](https://github.com/j5ik2o/takt-dash/issues/59) と [issue #180](https://github.com/j5ik2o/takt-dash/issues/180)）

run 詳細の取得（GetRunDetail）が、backend の総上限と Web の gRPC の deadline の順にそろい、stdio の画面読み取りレーンの起動・初期化・道具の呼び出しが要求の打ち切りで止まらないことを、Quint の仕様で確かめた記録です。設計は `docs/design/issue-59-180.md` にあります。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/run_detail_test.rs` の統合のテストと、`backend/src/grpc/` の性質のテスト（tokio の時間を止めて確かめる）が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `runDetailDeadline.qnt` | 状態機械、不変条件、到達の確認 |
| `runDetailDeadline_test.qnt` | issue の筋書きを修正前・途中の版・修正案でたどるテスト（12 件） |

`runDetailDeadline.qnt` は遷移を 4 つの定数で切り替えます。

| モジュール | `WEB_DEADLINE` | `BUDGET` | `DETACH` | `CHECK_WAITER` | 表すもの |
| --- | --- | --- | --- | --- | --- |
| `current` | 30 | 0（なし） | `none` | － | 修正前。Web の deadline は 30 秒で、backend の総上限は無い。読み取りは要求の future の中にあり、打ち切りで捨てられる |
| `budgetOnly` | 50 | 45 | `none` | － | 期限だけを直した版 |
| `detachInitOnly` | 50 | 45 | `init` | － | 期限を直し、起動と初期化だけを要求から切り離した版 |
| `detachOnly` | 30 | 0 | `call` | true | 読み取り全体を切り離したが、期限を直さない版 |
| `runToEnd` | 50 | 45 | `op` | true | 読み取り全体を切り離し、要求が諦めた後も残りの呼び出しを最後まで流す版（PR の最初の版。Devin Review の指摘） |
| `withoutWaiterCheck` | 50 | 45 | `op` | false | 読み取り全体を切り離したが、鍵を待つ間に要求を確かめない版（設計の最初の版。設計レビューの指摘 1） |
| `fixed` | 50 | 45 | `call` | true | 修正案。要求が諦めたら、進行中の段（初期化か 1 回の呼び出し）だけを終えて止める。Web 50 秒、backend の総上限 45 秒、読み取り全体（鍵の取得、起動と初期化、道具の呼び出し）を要求から切り離す |

## 仕様とコードの対応

| 仕様の要素 | コード |
| --- | --- |
| `WEB_DEADLINE` | `frontend/web/src/grpc/client.ts` の `RUN_DETAIL_DEADLINE_MS`（修正案で 50 秒）。修正前は `DEFAULT_UNARY_DEADLINE_MS`（30 秒）。tonic のサーバーは `grpc-timeout` で要求の future を捨てる |
| `BUDGET` | `backend/src/grpc/mod.rs` の `RUN_DETAIL_BUDGET`（45 秒）。`get_run_detail` の本体を `tokio::time::timeout` で包む |
| `INIT_TIMEOUT` | `backend/src/mcp/client.rs` の `INITIALIZE_TIMEOUT`（60 秒） |
| `CALL_TIMEOUT` | `backend/src/mcp/tools/mod.rs` の `client_config` の `request_timeout`（15 秒）。HTTP は `backend/src/grpc/mod.rs` の `READ_TIMEOUT` |
| `CALLS` | 回り道（`ReadSource::read_remote_run_detail`）の呼び出しの回数。`takt_get_run` の本体、view instructions、view task の 3 回。stepLogs のページはモデルに入れない |
| `op` と `OpInit`・`OpCalling` | `ReadSource::Stdio` の読み取り。鍵（`ClientHandle` の `lock`）を持ち、`call_tool` の中の `ensure_started` → `spawn`（起動と initialize）と、`request`（道具の往復）を進める |
| `DETACH = "none"` の打ち切り | 修正前。要求の future を捨てると、`spawn` の中の子プロセスは `kill_on_drop` で止まり、`request` で外していた子プロセスも捨てられる（`client.rs` の `request` の説明） |
| `DETACH = "call"` と `opOrphan` | 修正案。`resolve.rs` の `ReadSource::Stdio` の各読み取りを、`detached_read.rs` の `detached` が `tokio::spawn` したタスクで行い、要求は結果を待つだけにする。要求が終わると、`AwaitedCaller` が新しい呼び出しを断る（初期化は先に済ませる）。進行中の初期化と呼び出しは最後まで進む |
| `orphanCalls` | 要求に諦められた op が新しく始めた呼び出しの数。`DETACH = "op"` で増える |
| `reqQueued` | 切り離した読み取りが鍵を持っている間に来た要求は、`lock_owned().await` で鍵を待つ |
| `waitingTasks` と `CHECK_WAITER` | 鍵を待つタスクの数。修正案のタスクは、`tokio::select!` で鍵の取得と `oneshot` の送り手の `closed()` を待ち、要求が諦めたら鍵を取らずに終える。鍵の待ちは先着順（tokio の `Mutex` は公平） |
| `abandonedDequeue` と `abandonedOps` | 要求を確かめない版で、諦められたタスクが鍵を取って読み取りを流すこと |
| `budgetExpires` の `BackendDeadline` | `Status::deadline_exceeded`。Web は `BackendUnavailableError("no-response")` にして「backend が応答しません。読み取りが遅れている可能性があります。」と前に読めた内容を出す |
| `webDeadlineExpires` の `WebDeadline` | Web 側の deadline の超過（同じ表示）。修正案では起きない |

時間は 1 単位を 1 秒とします。成功する初期化は 60 秒未満、成功する呼び出しは 15 秒未満で終わります。

## モデルの範囲

- 画面は 1 つで、要求は前の要求が終わってから出します（画面の自動更新と再読み込み）。要求は最大 3 件です。
- 読み取りレーンは 1 本、子プロセスは 1 つです。
- 初期化と呼び出しの失敗（期限切れ、輸送エラー、道具のエラー）は入れていません。失敗の扱い（子プロセスを捨てる、バックオフ）は変えないためです。
- 停止は入れていません。切り離した読み取りが鍵を持っている間の停止は、[issue #364](https://github.com/j5ik2o/takt-dash/issues/364) の仕様（`specs/quint/backend-shutdown-deadline/`）で、鍵の埋まった stdio のクライアント（`busy`）として扱われています。閉じる段は上限（15 秒）で見捨て、ランタイムを捨てると、タスクの中の子プロセスは `kill_on_drop` で止まります。停止の全体の上限（18 秒）は変わりません。
- HTTP の接続先は入れていません。セッションの確立はすでに別のタスクで走り（`http.rs` の `ensure_session`）、要求を打ち切ってもセッションを捨てないためです。

## 条件

不変条件（安全性）

- `backendAnswersFirst`：backend が Web の deadline より先に答える（Web の deadline で打ち切られる要求は無い）。
- `readyReadsWithinCallTimeoutsSucceed`：子プロセスが使え、レーンも空いていた要求は、どの呼び出しも 1 回の期限内に終わる限り成功する（issue #59）。
- `noKillByCancel`：要求の打ち切りで子プロセスを捨てない（issue #180）。
- `spawnsOnce`：初期化が失敗しないモデルなので、子プロセスの起動は 1 回だけ。
- `requestsAreBounded`：要求は、backend の総上限（無ければ Web の deadline）を超えて待たない。
- `waitingTasksBoundedByWaiters`：鍵を待つタスクの数は、待っている要求の数を超えない。
- `noReadsForAbandonedWaits`：要求に諦められた後で、鍵の待ちから読み取りを始めるタスクは無い。
- `noCallsAfterAbandon`：要求に諦められた op は、新しい道具の呼び出しを始めない。
- `safety`：上の 8 つ。

到達の確認（`quint run` で違反として見つかれば、その筋書きに到達できる）

- `recoveryAfterInitTimeoutUnreachable`：初期化の途中で総上限で返った要求のあとで、別の要求が子プロセスを起動し直さずに成功する。
- `slowSuccessUnreachable`：30 秒を超えて成功する取得。

## 検査の結果（quint 0.32.0）

```sh
cd specs/quint/run-detail-deadline
quint typecheck runDetailDeadline.qnt
quint typecheck runDetailDeadline_test.qnt
quint test runDetailDeadline_test.qnt --main=runDetailDeadline_test
for m in current budgetOnly detachInitOnly detachOnly runToEnd withoutWaiterCheck fixed; do
  for inv in backendAnswersFirst readyReadsWithinCallTimeoutsSucceed noKillByCancel spawnsOnce requestsAreBounded \
      waitingTasksBoundedByWaiters noReadsForAbandonedWaits noCallsAfterAbandon; do
    quint run runDetailDeadline.qnt --main=$m --invariant=$inv --max-samples=20000 --max-steps=40
  done
done
quint run runDetailDeadline.qnt --main=fixed --invariant=safety --max-samples=100000 --max-steps=60
quint run runDetailDeadline.qnt --main=fixed --invariant=recoveryAfterInitTimeoutUnreachable --max-samples=5000 --max-steps=40
quint run runDetailDeadline.qnt --main=fixed --invariant=slowSuccessUnreachable --max-samples=5000 --max-steps=40
```

- `typecheck`：2 ファイルともエラーなし。
- `quint test`：12 件すべて成功。
- 不変条件ごとの `quint run`：

| モジュール | backendAnswersFirst | readyReads… | noKillByCancel | spawnsOnce | requestsAreBounded | waitingTasksBounded… | noReadsForAbandoned… | noCallsAfterAbandon |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `current` | 違反 | 違反 | 違反 | 違反 | 成立 | 成立 | 成立 | 成立 |
| `budgetOnly` | 成立 | 成立 | 違反 | 違反 | 成立 | 成立 | 成立 | 成立 |
| `detachInitOnly` | 成立 | 成立 | 違反 | 違反 | 成立 | 成立 | 成立 | 成立 |
| `detachOnly` | 違反 | 違反 | 成立 | 成立 | 成立 | 成立 | 成立 | 成立 |
| `runToEnd` | 成立 | 成立 | 成立 | 成立 | 成立 | 成立 | 成立 | 違反 |
| `withoutWaiterCheck` | 成立 | 成立 | 成立 | 成立 | 成立 | 違反 | 違反 | 違反 |
| `fixed` | 成立 | 成立 | 成立 | 成立 | 成立 | 成立 | 成立 | 成立 |

- `fixed` の `safety`（10 万件、60 手）：違反なし。
- `fixed` の到達の確認 2 つ：どちらも違反として見つかる（到達できる）。

## 更新するタイミング

- `RUN_DETAIL_BUDGET`、`RUN_DETAIL_DEADLINE_MS`、`INITIALIZE_TIMEOUT`、読み取りの 1 回の期限、回り道の呼び出しの回数、`ReadSource::Stdio` の読み取りの実行のしかたを変えるときは、先にこの仕様を直して検査し直します。
