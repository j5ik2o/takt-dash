# backend の停止の上限（[issue #364](https://github.com/j5ik2o/takt-dash/issues/364) の 1 点目）

backend の停止が、停止の合図から launchd が SIGKILL する前に終わり、その間に、応答する相手への接続（HTTP セッションと stdio の子プロセス）は閉じ終えることを、Quint の仕様で確かめた記録です。

本番の backend は launchd（`~/Library/LaunchAgents/com.ideoplus.taktdash-backend.plist`）で動いています。plist に `ExitTimeOut` が無いので、launchd は SIGTERM の 20 秒後に SIGKILL します。SIGKILL されると、閉じていないセッションは takt-mcp に残ります。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/http_read_test.rs` と `backend/src/mcp/registry.rs` の停止のテスト（性質のテストを含む）と、`backend/src/poller_rebuild_tests.rs` の周期の打ち切りのテストが確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `shutdownDeadline.qnt` | 状態機械、不変条件、進行の条件 |
| `shutdownDeadline_test.qnt` | issue の筋書きを修正前・途中の版・修正案でたどるテスト（11 件） |

`shutdownDeadline.qnt` は遷移を 5 つの定数で切り替えます。

| モジュール | `PARALLEL` | `DEADLINE` | `CANCEL_CYCLE` | `CYCLE_GRACE` | `ABORT_CYCLE_TASKS` | 表すもの |
| --- | --- | --- | --- | --- | --- | --- |
| `current` | false | false | false | － | － | 修正前。周期の途中なら周期を待ち、`shutdown_all` と `Poller::shutdown` が 1 件ずつ順に閉じ、上限が無い |
| `deadlineOnly` | false | true | false | － | － | 上限だけを入れた版 |
| `parallelOnly` | true | false | false | － | － | 同時に閉じるだけの版 |
| `withoutCycleCancel` | true | true | false | － | － | 同時に閉じて上限で打ち切るが、周期の途中なら周期を待つ版（設計の最初の版） |
| `immediateCancel` | true | true | true | 0 | true | 合図で周期をすぐ打ち切る版（設計の 2 番目の版） |
| `withoutTaskAbort` | true | true | true | 2 | false | 2 秒待って周期を打ち切るが、周期が起動した読み取りタスクを中断しない版（PR の最初の版。レビューボットの指摘で直した） |
| `fixed` | true | true | true | 2 | true | 修正案。合図のあと周期を 2 秒だけ待ち、終わらなければ周期と読み取りタスクを打ち切り、すべてを同時に閉じ始め、閉じる段を `SHUTDOWN_DEADLINE`、停止の全体を `STOP_DEADLINE` で打ち切る |

## 仕様とコードの対応

| 仕様の要素 | コード |
| --- | --- |
| `signal` | SIGTERM・SIGINT（`main.rs`）→ 停止の watch |
| `Draining` と `cycleLength` | `poller.rs` の `run_until_stopped` が、合図の前に始めた `read_cycle_once` を待つこと。周期は道具を何回か呼び、上限は 1 回ごと（`READ_TIMEOUT_SECONDS`）なので、周期の全体の長さに上限は無い。モデルでは `CYCLE_BOUND`（30 秒）まで |
| `CANCEL_CYCLE` と `CYCLE_GRACE` | 修正案の `run_until_stopped`（`cycle_until_grace`）は、周期と停止の合図を `select!` で待ち、合図のあと周期を `CYCLE_GRACE`（2 秒）だけ待って、終わらなければ周期の future を捨てる |
| `ABORT_CYCLE_TASKS` と `cycleHeld` | 周期はプロジェクトごとに読み取りタスクを `tokio::spawn` で起動し、タスクは読み取りの間、読み取りハンドル（HTTP）やクライアント（stdio）の鍵を持つ。修正案は `poller.rs` の `ReadTask` で、周期の future を捨てたらタスクも `abort` する。中断しないと、タスクは周期の長さ（`cycleLength`）まで鍵を持ったまま走り、閉じる処理は鍵を待つ |
| `SHORT_CYCLE` | ふだんの周期の長さの見込み（2 秒）。これより短い周期は打ち切らず、その周期の出来事を記録してから止まる |
| `HTTP_ITEMS` | レジストリの常駐サーバーごとの `HttpSession`（`registry.rs` の `http_sessions`）と、定期読み取りのプロジェクトごとの `HttpReadHandle`（`poller.rs` の `reads`） |
| `CLOSE_BOUND` | `http.rs` の `SESSION_CLOSE_BOUND` ＝ `CLOSE_WAIT_FOR_ESTABLISH`（修正案で 5 秒）＋ `CLOSE_TIMEOUT`（5 秒） |
| `STDIO_ITEMS` と `busy` | レジストリの stdio のクライアント。閉じる前に `client.lock().await` で鍵を取る。要求が道具を呼んでいる間は鍵が空かない |
| `OP_BOUND` | 道具の呼び出しの上限（`request_timeout` と道具ごとの上限。値は設定による。モデルでは 30 秒） |
| `SHUTDOWN_DEADLINE` | `registry.rs` の `SHUTDOWN_DEADLINE`（15 秒）。`shutdown_within` が `JoinSet` の全部の終わりを待つ上限 |
| `STOP_DEADLINE` | `main.rs` が停止の合図から gRPC サーバーと定期読み取りの終わりを待つ上限（18 秒） |
| `EXIT_TIMEOUT` | launchd の `ExitTimeOut` の既定（20 秒）。コードの `LAUNCHD_EXIT_TIMEOUT` |
| （モデルの外） | `main.rs` は、`STOP_DEADLINE` のあと、tokio のランタイムを `shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT)`（1 秒）で捨てる。始まった `spawn_blocking` の終わりを待つ時間にも上限を設けるため。コードの `const` の検査は `STOP_DEADLINE + RUNTIME_SHUTDOWN_TIMEOUT < LAUNCHD_EXIT_TIMEOUT` |
| `Abandoned` | 打ち切りで閉じる処理の future を捨てる。stdio の子プロセスは、鍵を持つ要求が終わるか、プロセスの終了で `kill_on_drop` により止まる。HTTP セッションは、サーバーが再起動するまで残る |

時間は 1 単位を 1 秒とします。応答するサーバーの DELETE と、鍵の空いた子プロセスの kill は 1 単位より短く終わるものとし、終わるまで時間を進めません（`tick` の前提）。

## モデルの範囲

- HTTP セッションは 3 件、stdio の子プロセスは 2 件です。各サーバーが応答するか、各子プロセスの鍵が埋まっているか、合図の時点で周期の途中か（途中なら残りの長さ）、周期の読み取りタスクがどの相手の鍵を持っているかは、初期状態で選びます。
- 修正前の順は番号の順（実装は stdio を先、HTTP をあと）です。応答しない相手が先にあると、後ろの相手を待たせます。
- 次のものは含めていません。gRPC サーバーの穏やかな停止の中身（`STOP_DEADLINE` が外から打ち切ります）、閉じる段の途中に届いた要求が置き場を作り直すこと、打ち切った周期の中の個々の処理。周期は `await` の境目で止まります。出来事の記録と、その直後のメモリの更新の間に `await` は無いので、記録が半端に残ることはありません（`backend/src/poller_rebuild_tests.rs` の `a_stop_signal_during_a_long_cycle_cancels_it_and_a_restart_records_each_event_once` が確かめます）。

## 条件

不変条件（安全性）

- `stopBeforeExitTimeout`：launchd が SIGKILL する前（合図から `EXIT_TIMEOUT` 未満）に、プロセスが終わる。
- `closesWithinDeadline`：閉じる段は `SHUTDOWN_DEADLINE` を超えない。
- `respondingSessionsClosed`：応答するサーバーのセッションは、プロセスが終わるまでに閉じる。
- `idleChildrenKilled`：合図の時点で鍵の空いていた子プロセスは、プロセスが終わるまでに kill する。
- `abandonOnlyAtDeadline`：見捨てるのは、上限に達したときだけ。
- `shortCyclesComplete`：ふだんの短い周期（`SHORT_CYCLE` 以下）は打ち切らない。
- `safety`：上の 6 つをすべて満たす。

前提（筋書きテストの `boundsAreOrderedTest` で確かめます）：`CLOSE_BOUND < SHUTDOWN_DEADLINE`、`CYCLE_GRACE + SHUTDOWN_DEADLINE < STOP_DEADLINE < EXIT_TIMEOUT`、`SHORT_CYCLE <= CYCLE_GRACE`。コードでは `const _: () = assert!(...)`（`backend/src/mcp/registry.rs` と `backend/src/poller.rs`）で確かめます。

進行の条件（到達できること。`--witnesses` で数えます）

- `stoppedWithEverythingClosed`：すべて閉じて終える。
- `unresponsiveServerGaveUp`：応答しないサーバーを自分の上限で打ち切る。
- `busyChildAbandoned`：鍵の空かない子プロセスを上限で見捨てる。
- `busyChildKilledAfterOperation`：鍵を持つ要求が終わったあとで、その子プロセスを kill する。
- `stoppedDuringACycle`：周期の途中で合図を受け、応答する相手をすべて閉じて終える。
- `shortCycleCompletedBeforeStop`：短い周期を最後まで走らせてから閉じて終える。
- `longCycleCancelled`：長い周期を打ち切り、応答する相手をすべて閉じて終える。
- `cycleHeldSessionClosedAfterCancel`：周期の読み取りタスクが鍵を持っていた応答する相手を、周期を打ち切ったあとで閉じる。

## 結果

quint 0.32.0 で実行しました。実行したのは `quint typecheck`、`quint run`、`quint test` だけです。

### 修正前と途中の版：反例あり

```sh
quint run shutdownDeadline.qnt --main=current --invariant=stopBeforeExitTimeout \
  --max-steps=60 --max-samples=10000 --seed=0x364
quint run shutdownDeadline.qnt --main=deadlineOnly --invariant=respondingSessionsClosed \
  --max-steps=60 --max-samples=10000 --seed=0x364
quint run shutdownDeadline.qnt --main=parallelOnly --invariant=stopBeforeExitTimeout \
  --max-steps=60 --max-samples=10000 --seed=0x364
quint run shutdownDeadline.qnt --main=withoutCycleCancel --invariant=respondingSessionsClosed \
  --max-steps=60 --max-samples=10000 --seed=0x364
quint run shutdownDeadline.qnt --main=immediateCancel --invariant=shortCyclesComplete \
  --max-steps=60 --max-samples=10000 --seed=0x364
quint run shutdownDeadline.qnt --main=withoutTaskAbort --invariant=respondingSessionsClosed \
  --max-steps=60 --max-samples=10000 --seed=0x364
```

| モジュール | 違反する不変条件（手数） | 筋書き |
| --- | --- | --- |
| `current` | `stopBeforeExitTimeout`（22）、`closesWithinDeadline`（42） | 周期と、応答しない相手を 1 件ずつ待ち、SIGKILL を超える（`sequentialClosesExceedTheExitTimeoutBeforeFixTest`） |
| `deadlineOnly` | `respondingSessionsClosed`（20）、`idleChildrenKilled`（22） | 応答しない相手を順に待つうちに上限に達し、後ろの応答する相手を見捨てる（`deadlineAloneAbandonsARespondingServerTest`） |
| `parallelOnly` | `stopBeforeExitTimeout`（22）、`closesWithinDeadline`（29） | 鍵を持つ道具の呼び出しを待ち、SIGKILL を超える（`parallelAloneWaitsForABusyChildTest`） |
| `withoutCycleCancel` | `respondingSessionsClosed`（20）、`idleChildrenKilled`（19） | 周期を待つうちに停止の全体の上限に達し、閉じ始める前にすべて見捨てる（`waitingForACycleAbandonsEverythingTest`） |
| `immediateCancel` | `shortCyclesComplete`（2） | ふだんの短い周期も打ち切り、その周期の出来事を記録せずに止まる（`immediateCancelDropsAShortCycleTest`） |
| `withoutTaskAbort` | `respondingSessionsClosed`（20）、`idleChildrenKilled`（21） | 周期を打ち切っても読み取りタスクが鍵を持ったまま走り、応答する相手も閉じられずに見捨てる（`cycleTaskKeepsTheLockWithoutAbortTest`） |

同じ条件（10000 サンプル、max-steps 60、seed 0x364）で、6 つの不変条件のうち表に無い組み合わせは反例なしです。同時に閉じること、上限、長い周期の打ち切り、短い周期を待つこと、周期の読み取りタスクの中断の 5 つがすべて要ります。

### 修正案の遷移：反例なし

```sh
quint run shutdownDeadline.qnt --main=fixed --invariant=safety \
  --witnesses stoppedWithEverythingClosed unresponsiveServerGaveUp busyChildAbandoned \
    busyChildKilledAfterOperation stoppedDuringACycle shortCycleCompletedBeforeStop longCycleCancelled \
    cycleHeldSessionClosedAfterCancel \
  --max-steps=60 --max-samples=50000 --seed=0x364
```

50000 サンプル、max-steps 60、seed 0x364 で `safety` の反例なし。trace の長さは最短 8、最長 27、平均 19.37 です。どの trace もプロセスが終わって（`Exited`）止まります。

| 条件 | 到達した trace |
| --- | --- |
| `stoppedWithEverythingClosed` | 5949 / 50000（11.90%） |
| `unresponsiveServerGaveUp` | 43879 / 50000（87.76%） |
| `busyChildAbandoned` | 2202 / 50000（4.40%） |
| `busyChildKilledAfterOperation` | 36143 / 50000（72.29%） |
| `stoppedDuringACycle` | 48382 / 50000（96.76%） |
| `shortCycleCompletedBeforeStop` | 3183 / 50000（6.37%） |
| `longCycleCancelled` | 45199 / 50000（90.40%） |
| `cycleHeldSessionClosedAfterCancel` | 18426 / 50000（36.85%） |

### 筋書きテスト

```sh
quint test shutdownDeadline_test.qnt --main=shutdownDeadline_test
```

11 件すべて成功。
