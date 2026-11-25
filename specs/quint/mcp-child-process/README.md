# takt-mcp 子プロセスの開け閉め・打ち切りと鍵なし書き込みの切り離し（[PR #379](https://github.com/j5ik2o/takt-dash/pull/379) / [issue #279](https://github.com/j5ik2o/takt-dash/issues/279)）

[PR #379](https://github.com/j5ik2o/takt-dash/pull/379)（[issue #279](https://github.com/j5ik2o/takt-dash/issues/279) の修正、`main` の `42652fe`）が変更した takt-mcp 子プロセスの起動・所有・往復・打ち切り・破棄と、鍵なし書き込みの切り離し実行を、後追いの Quint 仕様としてモデル化し検査した記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/mcp_client_test.rs` と `backend/tests/write_retry_test.rs` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `childProcess.qnt` | 状態機械、不変条件、進行の条件（witness） |
| `childProcess_test.qnt` | [PR #379](https://github.com/j5ik2o/takt-dash/pull/379) の筋書きを修正前・途中版・現在（修正後）でたどるテスト（13 件） |

`childProcess.qnt` は遷移を 3 つの定数で切り替えます。

| モジュール | `TAKE_DURING_REQUEST` | `WARMUP_RETRY` | `DETACH_UNKEYED_WRITE` | 表すもの |
| --- | --- | --- | --- | --- |
| `beforePr379` | false | false | false | [PR #379](https://github.com/j5ik2o/takt-dash/pull/379) 以前。`request` が `self.process.as_mut()` で借りたまま往復し、`warm_up` が `ensure_started` を呼び、鍵なし書き込みを呼び出しタスク内で待つ |
| `withoutUnkeyedDetach` | true | true | false | [PR #379](https://github.com/j5ik2o/takt-dash/pull/379) の最初のコミット相当。`request` の `take()` と `warm_up` の `OnSpawnFailure::Retry` を入れたが、鍵なし書き込みをまだ `tokio::spawn` していない版 |
| `current` | true | true | true | 現在の `main`（[PR #379](https://github.com/j5ik2o/takt-dash/pull/379) 適用後）。上記に加えて鍵なし書き込みも `tokio::spawn` で呼び出しから切り離す |

## 仕様と現在のコードの対応（行番号）

確認対象コミット: `75b9eed`（[PR #379](https://github.com/j5ik2o/takt-dash/pull/379) の `42652fe` を含む `main`）

### `backend/src/mcp/client.rs`

| 仕様の要素（`childProcess.qnt`） | コードの対応箇所 | 整合性の確認 |
| --- | --- | --- |
| `ClientState`（`slot`, `consecutiveFailures`, `nextSpawnAt`） | `backend/src/mcp/client.rs` L171-L183（`McpClient` の `process`, `consecutive_spawn_failures`, `next_spawn_at`） | 一致。`lockedBy` は `backend/src/mcp/registry.rs` L87 の `ClientHandle = Arc<AsyncMutex<McpClient>>` による排他取得を表す |
| `shouldBackoffOnFailure` / `WARMUP_RETRY` | `backend/src/mcp/client.rs` L186-L192（`OnSpawnFailure::{BackOff, Retry}`）、L287-L290 | 一致。`BackOff` のときだけ `consecutive_spawn_failures` を増やし `next_spawn_at` を設定する |
| `backoffDelay` | `backend/src/mcp/client.rs` L147-L148, L158-L164（`spawn_backoff`） | 抽象化して一致。連続失敗ごとに倍増し上限で止まる構造を `1, 2, 4` の論理時刻で表現 |
| `OpKind::WarmUp` | `backend/src/mcp/client.rs` L225-L227（`McpClient::warm_up` → `self.start(OnSpawnFailure::Retry)`） | 一致。呼び出し元は `backend/src/poller.rs` L787-L796 |
| `OpKind::{ReadCall, UnkeyedWrite, KeyedWrite}` の起動経路 | `backend/src/mcp/client.rs` L234-L240（`call_tool` → `ensure_started`）、L264-L266（`ensure_started` → `self.start(OnSpawnFailure::BackOff)`） | 一致。書き込み経路の事前接続（`backend/src/grpc/resolve.rs` L258-L264 の `WriteSource::connect`）も `ensure_started` を呼ぶ |
| `startAlreadyRunning` | `backend/src/mcp/client.rs` L269-L271（`if self.process.is_some() { return Ok(()); }`） | 一致 |
| `startBlockedByBackoff` | `backend/src/mcp/client.rs` L272-L277（`next_spawn_at` 未満なら `TransportError::Backoff` を返して起動しない） | 一致。`warm_up` も抑制期間中は起動せず `Backoff` を返し、失敗回数を増やさない |
| `failSpawnImmediate` / `beginSpawn` / `completeSpawn` / `failSpawnInit` | `backend/src/mcp/client.rs` L278-L294（`start` の `match self.spawn().await`）、L297-L333（`spawn`: L311 `kill_on_drop(true)`、L312 `command.spawn()`、L329-L331 `initialize`・`notifications/initialized`） | 一致。初期化完了まで `self.process` には格納せず、`spawn` 内のローカル変数 `process` が所有するため、失敗時も打ち切り時も `kill_on_drop(true)` で破棄される。成功時は L280-L282 で `consecutive_spawn_failures = 0`、`next_spawn_at = None` に戻す |
| `beginRequest` | `backend/src/mcp/client.rs` L340-L345（`let Some(mut process) = self.process.take()` → `process.exchange`） | 一致。往復の間は `self.process` を `None` にして子プロセスを `request` のフレームが所有する |
| `completeExchange` | `backend/src/mcp/client.rs` L346-L349（`Ok(result) => { self.process = Some(process); Ok(result) }`）、L239（`extract_tool_text` の `isError` は子プロセスを破棄しない） | 一致 |
| `failExchange` | `backend/src/mcp/client.rs` L350-L353（`Err(error) => Err(error)`、`process` を戻さず drop して `kill_on_drop` で終了） | 一致。通信失敗では `next_spawn_at` を設定しない |
| `cancelCaller`（切り離しなし、`Spawning` / `Exchanging`） | `backend/src/mcp/client.rs` L311（`kill_on_drop(true)`）、L342（`self.process.take()`） | 一致。future の drop 時に所有中の `process` が drop され、1 回だけ終了する |
| `shutdownClient` | `backend/src/mcp/client.rs` L248-L252（`if let Some(mut process) = self.process.take() { let _ = process.child.kill().await; }`） | 一致。`take()` により空なら何もしないため、二重終了しない。呼び出し元は `backend/src/mcp/registry.rs` L395-L411（`shutdown_all`） |

### `backend/src/grpc/write_ledger.rs`

| 仕様の要素（`childProcess.qnt`） | コードの対応箇所 | 整合性の確認 |
| --- | --- | --- |
| `OpKind::UnkeyedWrite` + `DETACH_UNKEYED_WRITE = true` の `cancelCaller` | `backend/src/grpc/write_ledger.rs` L141-L146（`let Some(key) = key else { return match tokio::spawn(prepare()?).await { ... }; };`） | 一致。呼び出し側が打ち切られて `JoinHandle` が drop されても、spawn 済みタスクはキャンセルされず最後まで進み、`lane.record_write()`（`backend/src/grpc/writes.rs` L121, L161, L182, L206）まで完了する |
| `OpKind::KeyedWrite` の `cancelCaller` | `backend/src/grpc/write_ledger.rs` L164-L182（`tokio::spawn(async move { ... })` と `wait(receiver).await`） | 一致。呼び出し側の打ち切りは `wait(receiver)` を drop するだけで、裏の書き込みタスクは最後まで進む |

## モデルの範囲

含めているもの:

- 1 つの `McpClient`（排他ロック付き）に対する最大 5 件の操作（`WarmUp`、`ReadCall`、`UnkeyedWrite`、`KeyedWrite`）、最大 2 回の `shutdown`、論理時刻 `0..6`（指数バックオフ `1, 2, 4` の満了を含む）。
- `command.spawn()` の即時失敗、`initialize` 往復中の失敗および打ち切り、`request` 往復中の成功・通信失敗・呼び出し側打ち切り、複数回の `shutdown` 呼び出し。
- 子プロセスごとの生死（`Initializing` / `Alive` / `Closed`）、応答の読みかけ状態（`dirty`）、終了回数（`closeCount`）、読みかけ状態での再利用の有無（`usedWhileDirty`）。

意図的に省いているもの:

- `spawn_permits()`（`client.rs` L142, L152-L155）によるプロセス全体の同時起動数制限（上限 3）。単一クライアントの所有・開閉の不変条件には影響しないため省略。
- 実時間の期限値（`INITIALIZE_TIMEOUT = 60s`、`SPAWN_BACKOFF_BASE = 5s`、`SPAWN_BACKOFF_MAX = 300s`、道具別タイムアウト）。抽象的な論理時刻と失敗遷移で代替。
- `WriteLedger` の冪等キー重複排除・不一致検出・保持期限（`specs/quint/decision-write-retry/` が扱う）。
- `prepare()` の入力検証失敗（`write_ledger.rs` L142 の `?` で `tokio::spawn` より前に返るため、takt へ送られない）。
- `WriteSource::connect`（`resolve.rs` L258-L264）と道具呼び出し（`writes.rs` L112-L114）の間でロックを一度手放す細部（道具呼び出し側の `call_tool` が `client.rs` L235 で再度 `ensure_started()` を呼ぶため、途中で `shutdown` が挟まっても起動し直す）。
- JSON-RPC の行分割・サーバー通知・`extract_tool_text` の解析詳細、および `McpClientRegistry` のレーン振り分け。

更新するタイミング:

- `backend/src/mcp/client.rs` の子プロセス所有・起動・バックオフ・破棄、または `backend/src/grpc/write_ledger.rs` のタスク実行方式を変更するときは、先にこの仕様を更新して再検査してからコードを変更します。

## 条件

### 不変条件（安全性）

- `noChildRemainsAfterCancel`：起動中・往復中に打ち切られた操作が持っていた子プロセスは必ず `Closed` になり、`client.slot` にも残らない。
- `noOrphanAliveChild`：`Closed` でない子プロセスは、必ず `client.slot` か実行中の操作に所有されており、取り残されない。
- `atMostOneLiveChild`：同時に生きている子プロセスは高々 1 つ。
- `noDoubleClose`：どの子プロセスも閉じる回数（`closeCount`）は 1 回以下であり、`Closed` であることと `closeCount == 1` が同値（閉じた接続を二度閉じない）。
- `closedChildNotReferenced`：`Closed` になった子プロセスは `client.slot` にも操作の `heldChild` にも残らない。
- `noDirtyChildReused`：読みかけの応答（`dirty == true`）を持った子プロセスが、次の要求に使われない。
- `idleSlotIsClean`：ロックが空いている状態で `client.slot` にある子プロセスは `Alive` かつ `dirty == false`。
- `warmUpFailureDoesNotStartBackoff`：温めの起動失敗は再起動の抑制（`next_spawn_at` / `consecutive_spawn_failures`）を始めない（呼び出し経路の失敗が 0 回なら `nextSpawnAt == NONE`）。
- `callFailureStartsBackoff`：呼び出し経路の起動失敗は必ず `next_spawn_at` を設定し、`consecutiveFailures` を進める。
- `noSpawnDuringBackoff`：抑制期間中（`now < nextSpawnAt`）は起動を試みない。
- `unkeyedWriteSurvivesCallerCancel`：鍵なしの書き込みは、呼び出し側が打ち切っても中断（`Aborted`）されず最後まで進める。
- `keyedWriteSurvivesCallerCancel`：鍵ありの書き込みも、呼び出し側の打ち切りで中断されない。
- `lockOwnershipConsistent`：ロック保持中の操作は高々 1 件であり、`lockedBy` と `heldChild` の所有が一致する。
- `safety`：上記 13 件の論理積。

### 進行の条件（Witness）

- `warmUpSucceeded`：温めが成功する。
- `warmUpFailedWithoutBackoff`：温めが失敗しても `nextSpawnAt == NONE` のままである。
- `callBackedOffAfterFailure`：呼び出し経路の起動失敗後に `Backoff` で拒否される。
- `recoveredAfterBackoff`：抑制の満了後に起動が成功して回復する。
- `cancelledReadDisposedAndRebuilt`：往復中の読み取りを打ち切って子プロセスを破棄したあと、次の読み取りが新しい子プロセスで成功する。
- `cancelledDuringSpawnDisposed`：起動（`initialize`）途中の打ち切りで子プロセスが破棄される。
- `exchangeFailedAndRecovered`：往復の通信失敗で子プロセスを破棄したあと、次の呼び出しが回復する。
- `unkeyedWriteCompletesAfterCallerCancel`：鍵なしの書き込みが、呼び出し側の打ち切り後も最後まで進み `writeRecorded == true` に到達する。
- `keyedWriteCompletesAfterCallerCancel`：鍵ありの書き込みも、呼び出し側の打ち切り後に完了する。
- `shutdownClosedLiveChild`：`shutdown` が生きた子プロセスを閉じる。
- `shutdownSkippedWhenAlreadyClosed`：既に空（閉じた後など）の状態で `shutdown` が呼ばれても二度閉じずにスキップする。

## 検査結果

quint 0.32.0 で実行しました。**現在のコード（`current`）に仕様違反・不具合は見つかりませんでした。**

### 型検査と筋書きテスト

```sh
quint typecheck childProcess.qnt
quint typecheck childProcess_test.qnt
quint test childProcess_test.qnt --main=childProcess_test
```

- `quint typecheck`: `childProcess.qnt`・`childProcess_test.qnt` ともに成功。
- `quint test`: 13 件すべて成功（修正前の 3 種の不具合と途中版の鍵なし書き込み中断が再現し、修正後の `current` ではすべて防げることを固定）。

### 修正前（`beforePr379`）と途中版（`withoutUnkeyedDetach`）：期待どおり反例あり

```sh
quint run childProcess.qnt --main=beforePr379 --invariant=noChildRemainsAfterCancel --max-steps=25 --max-samples=10000 --seed=0x379
quint run childProcess.qnt --main=beforePr379 --invariant=noDirtyChildReused --max-steps=25 --max-samples=10000 --seed=0x379
quint run childProcess.qnt --main=beforePr379 --invariant=warmUpFailureDoesNotStartBackoff --max-steps=25 --max-samples=10000 --seed=0x379
quint run childProcess.qnt --main=beforePr379 --invariant=unkeyedWriteSurvivesCallerCancel --max-steps=25 --max-samples=10000 --seed=0x379
quint run childProcess.qnt --main=withoutUnkeyedDetach --invariant=unkeyedWriteSurvivesCallerCancel --max-steps=25 --max-samples=10000 --seed=0x379
```

| モジュール | 不変条件 | 結果 | 要点 |
| --- | --- | --- | --- |
| `beforePr379` | `noChildRemainsAfterCancel` | 反例あり | 往復中に打ち切られた読み取りの子プロセスが `Alive` のまま `slot` に残る |
| `beforePr379` | `noDirtyChildReused` | 反例あり | 上記の読みかけの子プロセスを次の要求が使い回す |
| `beforePr379` | `warmUpFailureDoesNotStartBackoff` | 反例あり | 温めの起動失敗が `nextSpawnAt` と `consecutiveFailures` を設定する |
| `beforePr379` | `unkeyedWriteSurvivesCallerCancel` | 反例あり | 鍵なし書き込みが呼び出し側の打ち切りで `Aborted` になる |
| `withoutUnkeyedDetach` | `unkeyedWriteSurvivesCallerCancel` | 反例あり | `request` の `take()` だけでは、呼び出し側の打ち切りで鍵なし書き込みと子プロセスが止まる |

### 現在のコード（`current`）：`safety` + 全 witness（50,000 サンプル）

```sh
quint run childProcess.qnt --main=current --invariant=safety \
  --witnesses warmUpSucceeded warmUpFailedWithoutBackoff callBackedOffAfterFailure recoveredAfterBackoff \
    cancelledReadDisposedAndRebuilt cancelledDuringSpawnDisposed exchangeFailedAndRecovered \
    unkeyedWriteCompletesAfterCallerCancel keyedWriteCompletesAfterCallerCancel \
    shutdownClosedLiveChild shutdownSkippedWhenAlreadyClosed \
  --max-steps=35 --max-samples=50000 --seed=0x379
```

- 50,000 サンプル、`max-steps=35`、`seed=0x379` で `safety` の反例なし（トレース長: 最短 19、最長 36、平均 25.99）。
- すべての進行条件（witness）が 1 件以上のトレースで到達しました。

| Witness | 到達したトレース |
| --- | --- |
| `warmUpSucceeded` | 1451 / 50000（2.90%） |
| `warmUpFailedWithoutBackoff` | 5105 / 50000（10.21%） |
| `callBackedOffAfterFailure` | 33164 / 50000（66.33%） |
| `recoveredAfterBackoff` | 1755 / 50000（3.51%） |
| `cancelledReadDisposedAndRebuilt` | 1 / 50000（0.00%） |
| `cancelledDuringSpawnDisposed` | 2721 / 50000（5.44%） |
| `exchangeFailedAndRecovered` | 4495 / 50000（8.99%） |
| `unkeyedWriteCompletesAfterCallerCancel` | 4614 / 50000（9.23%） |
| `keyedWriteCompletesAfterCallerCancel` | 4512 / 50000（9.02%） |
| `shutdownClosedLiveChild` | 979 / 50000（1.96%） |
| `shutdownSkippedWhenAlreadyClosed` | 49983 / 50000（99.97%） |

### 途中版（`withoutUnkeyedDetach`）：他の不変条件の確認

```sh
for inv in noChildRemainsAfterCancel noDoubleClose noDirtyChildReused warmUpFailureDoesNotStartBackoff callFailureStartsBackoff; do
  quint run childProcess.qnt --main=withoutUnkeyedDetach --invariant="$inv" --max-steps=25 --max-samples=10000 --seed=0x379
done
```

上記 5 件はいずれも 10,000 サンプル、`max-steps=25`、`seed=0x379` で反例なし。一方、`unkeyedWriteSurvivesCallerCancel` は反例が出るため、[PR #379](https://github.com/j5ik2o/takt-dash/pull/379) の 2 コミット目（鍵なし書き込みの `tokio::spawn` 切り離し）が必要だったことが仕様上も裏付けられます。

### 現在のコード（`current`）：不変条件ごとの `quint run`

```sh
for inv in \
  noChildRemainsAfterCancel noOrphanAliveChild atMostOneLiveChild \
  noDoubleClose closedChildNotReferenced noDirtyChildReused idleSlotIsClean \
  warmUpFailureDoesNotStartBackoff callFailureStartsBackoff noSpawnDuringBackoff \
  unkeyedWriteSurvivesCallerCancel keyedWriteSurvivesCallerCancel lockOwnershipConsistent; do
  quint run childProcess.qnt --main=current --invariant="$inv" --max-steps=35 --max-samples=10000 --seed=0x379
done
```

| 不変条件 | 結果（`max-steps=35`, `max-samples=10000`, `seed=0x379`） |
| --- | --- |
| `noChildRemainsAfterCancel` | 反例なし |
| `noOrphanAliveChild` | 反例なし |
| `atMostOneLiveChild` | 反例なし |
| `noDoubleClose` | 反例なし |
| `closedChildNotReferenced` | 反例なし |
| `noDirtyChildReused` | 反例なし |
| `idleSlotIsClean` | 反例なし |
| `warmUpFailureDoesNotStartBackoff` | 反例なし |
| `callFailureStartsBackoff` | 反例なし |
| `noSpawnDuringBackoff` | 反例なし |
| `unkeyedWriteSurvivesCallerCancel` | 反例なし |
| `keyedWriteSurvivesCallerCancel` | 反例なし |
| `lockOwnershipConsistent` | 反例なし |

### 有界モデル検査（`quint verify`）

```sh
quint verify childProcess.qnt --main=current --invariant=safety --max-steps=6
```

- Apalache 0.56.1 により `safety`（18 個の検証条件）を 6 手まで網羅検査し、`NoError`（反例なし・デッドロックなし、103 秒）。
- `quint verify` の出力先 `_apalache-out/` は `.gitignore` で除外しています。
