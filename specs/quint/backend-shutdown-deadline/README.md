# backend の停止で接続を閉じる段の上限（[issue #364](https://github.com/j5ik2o/takt-dash/issues/364) の 1 点目）

backend の停止で takt-mcp への接続（HTTP セッションと stdio の子プロセス）を閉じる段が、閉じる相手の数によらず決まった時間で終わり、応答する相手は閉じ終えることを、Quint の仕様で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/http_read_test.rs` の停止のテスト（性質のテストを含む）が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `shutdownDeadline.qnt` | 状態機械、不変条件、進行の条件 |
| `shutdownDeadline_test.qnt` | issue の筋書きを修正前・片方だけの版・修正案でたどるテスト（6 件） |

`shutdownDeadline.qnt` は遷移を 2 つの定数で切り替えます。

| モジュール | `PARALLEL` | `DEADLINE` | 表すもの |
| --- | --- | --- | --- |
| `current` | false | false | 修正前。`McpClientRegistry::shutdown_all` と `Poller::shutdown` が 1 件ずつ順に閉じ、全体の上限が無い |
| `deadlineOnly` | false | true | 全体の上限だけを入れた版 |
| `parallelOnly` | true | false | 同時に閉じるだけの版 |
| `fixed` | true | true | 修正案。すべてを同時に閉じ始め、停止を始めてから `SHUTDOWN_DEADLINE` で打ち切る |

## 仕様とコードの対応

| 仕様の要素 | コード |
| --- | --- |
| `HTTP_ITEMS` | レジストリの常駐サーバーごとの `HttpSession`（`registry.rs` の `http_sessions`）と、定期読み取りのプロジェクトごとの `HttpReadHandle`（`poller.rs` の `reads`） |
| `CLOSE_BOUND` | `http.rs` の `CLOSE_WAIT_FOR_ESTABLISH`（10 秒）＋ `CLOSE_TIMEOUT`（5 秒） |
| `STDIO_ITEMS` と `busy` | レジストリの stdio のクライアント。閉じる前に `client.lock().await` で鍵を取る。画面の読み書きの要求が道具を呼んでいる間は鍵が空かない |
| `OP_BOUND` | 道具の呼び出しの上限（`request_timeout` と道具ごとの上限。値は設定による。モデルでは 30 秒） |
| `SHUTDOWN_DEADLINE` | `poller.rs` の `SHUTDOWN_DEADLINE`（20 秒） |
| `Abandoned` | 打ち切りで閉じる処理の future を捨てる。stdio の子プロセスは、鍵を持つ要求が終わるか、プロセスの終了で `kill_on_drop` により止まる。HTTP セッションは、サーバーが再起動するまで残る |

時間は 1 単位を 5 秒とします。応答するサーバーの DELETE と、鍵の空いた子プロセスの kill は 1 単位より短く終わるものとし、終わるまで時間を進めません（`tick` の前提）。

## モデルの範囲

- HTTP セッションは 3 件、stdio の子プロセスは 2 件です。各サーバーが応答するか、各子プロセスの鍵が埋まっているかは、初期状態で選びます。
- 修正前の順は番号の順（実装は stdio を先、HTTP をあと）です。応答しない相手が先にあると、後ろの相手を待たせます。
- 次のものは含めていません。停止の合図より前に始まった定期読み取りの周期（終わるまで閉じる段に入りません）、gRPC サーバーの穏やかな停止、閉じる段の途中に届いた要求が置き場を作り直すこと。

## 条件

不変条件（安全性）

- `stopWithinDeadline`：閉じる段は、停止を始めてから `SHUTDOWN_DEADLINE` を超えない。
- `respondingSessionsClosed`：応答するサーバーのセッションは、閉じる段を終えるまでに閉じる。
- `idleChildrenKilled`：停止を始めた時点で鍵の空いていた子プロセスは、閉じる段を終えるまでに kill する。
- `abandonOnlyAtDeadline`：見捨てるのは、全体の上限に達したときだけ。
- `safety`：上の 4 つをすべて満たす。

前提（筋書きテストの `closeBoundFitsInTheDeadlineTest` で確かめます）：`CLOSE_BOUND < SHUTDOWN_DEADLINE`。コードでは `poller.rs` の定数の検査（`const _: () = assert!(...)`）で確かめます。

進行の条件（到達できること。`--witnesses` で数えます）

- `stoppedWithEverythingClosed`：すべて閉じて終える。
- `unresponsiveServerGaveUp`：応答しないサーバーを自分の上限で打ち切る。
- `busyChildAbandoned`：鍵の空かない子プロセスを全体の上限で見捨てる。
- `busyChildKilledAfterOperation`：鍵を持つ要求が終わったあとで、その子プロセスを kill する。

## 結果

quint 0.32.0 で実行しました。実行したのは `quint typecheck`、`quint run`、`quint test` だけです。

### 修正前と片方だけの版：反例あり

```sh
quint run shutdownDeadline.qnt --main=current --invariant=stopWithinDeadline \
  --max-steps=30 --max-samples=10000 --seed=0x364
quint run shutdownDeadline.qnt --main=deadlineOnly --invariant=respondingSessionsClosed \
  --max-steps=30 --max-samples=10000 --seed=0x364
quint run shutdownDeadline.qnt --main=parallelOnly --invariant=stopWithinDeadline \
  --max-steps=30 --max-samples=10000 --seed=0x364
```

| モジュール | 違反する不変条件 | 手数 | 筋書き |
| --- | --- | --- | --- |
| `current` | `stopWithinDeadline` | 9 | 応答しない相手を 1 件ずつ待ち、上限を超える（`sequentialClosesExceedTheDeadlineBeforeFixTest`） |
| `deadlineOnly` | `respondingSessionsClosed` | 5 | 応答しない相手を順に待つうちに上限に達し、後ろの応答するサーバーを閉じずに見捨てる（`deadlineAloneAbandonsARespondingServerTest`） |
| `deadlineOnly` | `idleChildrenKilled` | 6 | 同じ理由で、鍵の空いた子プロセスを kill せずに見捨てる |
| `parallelOnly` | `stopWithinDeadline` | 9 | 鍵を持つ道具の呼び出しを待ち、上限を超える（`parallelAloneWaitsForABusyChildTest`） |

`current` と `parallelOnly` のほかの 3 つ、`deadlineOnly` の `stopWithinDeadline` と `abandonOnlyAtDeadline` は、同じ条件で反例なしです。上限と同時に閉じることの両方が要ります。

### 修正案の遷移：反例なし

```sh
quint run shutdownDeadline.qnt --main=fixed --invariant=safety \
  --witnesses stoppedWithEverythingClosed unresponsiveServerGaveUp busyChildAbandoned \
    busyChildKilledAfterOperation \
  --max-steps=30 --max-samples=50000 --seed=0x364
```

50000 サンプル、max-steps 30、seed 0x364 で `safety` の反例なし。trace の長さは最短 6、最長 13、平均 9.76 です。どの trace も閉じる段を終えて（`Exited`）止まります。

| 条件 | 到達した trace |
| --- | --- |
| `stoppedWithEverythingClosed` | 5544 / 50000（11.09%） |
| `unresponsiveServerGaveUp` | 43733 / 50000（87.47%） |
| `busyChildAbandoned` | 9130 / 50000（18.26%） |
| `busyChildKilledAfterOperation` | 31073 / 50000（62.15%） |

### 筋書きテスト

```sh
quint test shutdownDeadline_test.qnt --main=shutdownDeadline_test
```

6 件すべて成功。
