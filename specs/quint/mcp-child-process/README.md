# takt-mcp 子プロセスの開け閉め・打ち切りと鍵なし書き込みの切り離し（[PR #379](https://github.com/j5ik2o/takt-dash/pull/379) / [issue #279](https://github.com/j5ik2o/takt-dash/issues/279)）

[PR #379](https://github.com/j5ik2o/takt-dash/pull/379)（[issue #279](https://github.com/j5ik2o/takt-dash/issues/279) の修正、`main` の `42652fe`）が変更した takt-mcp 子プロセスの起動・所有・往復・打ち切り・破棄と、鍵なし書き込みの切り離し実行を、後追いの Quint 仕様としてモデル化し検査した記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/mcp_client_test.rs` と `backend/tests/write_retry_test.rs` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `childProcess.qnt` | 状態機械、不変条件、進行の条件（witness） |
| `childProcess_test.qnt` | [PR #379](https://github.com/j5ik2o/takt-dash/pull/379) の筋書きを修正前・途中版・現在（修正後）でたどるテスト（15 件） |
| `check_mutations.py` | `childProcess.qnt` の守りを 1 つずつ外した写しを一時ディレクトリに作り、対応する不変条件が違反を出すかを確かめる（元のファイルは書き換えない） |

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
| `completeExchange` / `completeExchangeToolError` | `backend/src/mcp/client.rs` L346-L349（`Ok(result) => { self.process = Some(process); Ok(result) }`）、L239・L420-L430（`extract_tool_text` の `isError`）、`backend/src/grpc/writes.rs` L110-L122 | 一致。正常完了（`completeExchange`）と道具エラー（`completeExchangeToolError`）を分離し、どちらも子プロセスは破棄せず `self.process` へ戻すが、`isError` では `phase = Failed` かつ `writeRecorded = false` として `lane.record_write()` を呼ばない |
| `failExchange` | `backend/src/mcp/client.rs` L350-L353（`Err(error) => Err(error)`、`process` を戻さず drop して `kill_on_drop` で終了） | 一致。通信失敗では `next_spawn_at` を設定しない |
| `lockHolderHasNextStep` / `lockHolderCanProgress` | `backend/src/mcp/client.rs` L268-L355（`start`・`spawn`・`request` の各分岐）、`backend/src/mcp/registry.rs` L402（`shutdown_all` の `client.lock().await`） | 一致。ロックを持つ操作の各段（`Starting`・`Spawning`・`ReadyToCall`・`Exchanging`）に、コードの分岐に当たる遷移が必ずある。`shutdown` はロックを取ってから `take()` するため、ロックを持つ操作の子プロセスを横から閉じない |
| `cancelCaller`（切り離しなし、`Spawning` / `Exchanging`） | `backend/src/mcp/client.rs` L311（`kill_on_drop(true)`）、L342（`self.process.take()`） | 一致。future の drop 時に所有中の `process` が drop され、1 回だけ終了する |
| `shutdownClient` | `backend/src/mcp/client.rs` L248-L252（`if let Some(mut process) = self.process.take() { let _ = process.child.kill().await; }`） | 一致。`take()` により空なら何もしないため、二重終了しない。呼び出し元は `backend/src/mcp/registry.rs` L395-L411（`shutdown_all`） |

### `backend/src/grpc/write_ledger.rs`

| 仕様の要素（`childProcess.qnt`） | コードの対応箇所 | 整合性の確認 |
| --- | --- | --- |
| `OpKind::UnkeyedWrite` + `DETACH_UNKEYED_WRITE = true` の `cancelCaller` | `backend/src/grpc/write_ledger.rs` L141-L146（`let Some(key) = key else { return match tokio::spawn(prepare()?).await { ... }; };`） | 一致。呼び出し側が打ち切られて `JoinHandle` が drop されても、spawn 済みタスクはキャンセルされず最後まで進み、`lane.record_write()`（`backend/src/grpc/writes.rs` L121, L161, L182, L206）まで完了する |
| `OpKind::KeyedWrite` の `cancelCaller` | `backend/src/grpc/write_ledger.rs` L164-L182（`tokio::spawn(async move { ... })` と `wait(receiver).await`） | 一致。呼び出し側の打ち切りは `wait(receiver)` を drop するだけで、裏の書き込みタスクは最後まで進む |

## モデルの範囲

含めているもの:

- 1 つの `McpClient`（排他ロック付き）に対する最大 5 件の操作（`WarmUp`、`ReadCall`、`UnkeyedWrite`、`KeyedWrite`）、最大 2 回の `shutdown`、論理時刻 `0..7`（指数バックオフ `1, 2, 4` の上限 4 単位の満了を含む）。
- `command.spawn()` の即時失敗、`initialize` 往復中の失敗および打ち切り、`request` 往復中の成功・通信失敗・呼び出し側打ち切り、複数回の `shutdown` 呼び出し。
- 子プロセスごとの生死（`Initializing` / `Alive` / `Closed`）、応答の読みかけ状態（`dirty`）、終了回数（`closeCount`）、読みかけ状態での再利用の有無（`usedWhileDirty`）。

意図的に省いているもの:

- `spawn_permits()`（`client.rs` L142, L152-L155）によるプロセス全体の同時起動数制限（上限 3）。単一クライアントの所有・開閉の不変条件には影響しないため省略。
- 実時間の期限値（`INITIALIZE_TIMEOUT = 60s`、`SPAWN_BACKOFF_BASE = 5s`、`SPAWN_BACKOFF_MAX = 300s`、道具別タイムアウト）。抽象的な論理時刻と失敗遷移で代替。
- `WriteLedger` の冪等キー重複排除・不一致検出・保持期限（`specs/quint/decision-write-retry/` が扱う）。
- `prepare()` の入力検証失敗（`write_ledger.rs` L142 の `?` で `tokio::spawn` より前に返るため、takt へ送られない）。
- `WriteSource::connect`（`resolve.rs` L258-L264）と道具呼び出し（`writes.rs` L112-L114）の間でロックを一度手放す細部（道具呼び出し側の `call_tool` が `client.rs` L235 で再度 `ensure_started()` を呼ぶため、途中で `shutdown` が挟まっても起動し直す）。
- JSON-RPC の行分割・サーバー通知・`extract_tool_text` の解析詳細、および `McpClientRegistry` のレーン振り分け。
- `kill_on_drop(true)` による終了指令後の OS 側プロセス回収（`waitpid`）の非同期遅延。モデル上の `Closed` は「`McpClient` が終了指令を出して管理から外した状態」を表し、OS 上での実際の回収完了時刻までは検証しません。

更新するタイミング:

- `backend/src/mcp/client.rs` の子プロセス所有・起動・バックオフ・破棄、または `backend/src/grpc/write_ledger.rs` のタスク実行方式を変更するときは、先にこの仕様を更新して再検査してからコードを変更します。

## 条件

### 不変条件（安全性）

- `noChildRemainsAfterCancel`：起動中・往復中に打ち切られた操作が持っていた子プロセスは必ず `Closed` になり、`client.slot` にも残らない。
- `noOrphanAliveChild`：`Closed` でない子プロセスは、必ず `client.slot` か実行中の操作に所有されており、取り残されない。
- `atMostOneLiveChild`：クライアントが管理する未終了（`Closed` でない）の子プロセスは同時に高々 1 つ（`kill_on_drop` 後の OS 側回収完了時刻は抽象化境界の外）。
- `noDoubleClose`：どの子プロセスも閉じる回数（`closeCount`）は 1 回以下であり、`Closed` であることと `closeCount == 1` が同値（閉じた接続を二度閉じない）。
- `closedChildNotReferenced`：`Closed` になった子プロセスは `client.slot` にも操作の `heldChild` にも残らない。
- `noDirtyChildReused`：読みかけの応答（`dirty == true`）を持った子プロセスが、次の要求に使われない。
- `idleSlotIsClean`：ロックが空いている状態で `client.slot` にある子プロセスは `Alive` かつ `dirty == false`。
- `warmUpFailureDoesNotStartBackoff`：温めの起動失敗は再起動の抑制（`next_spawn_at` / `consecutive_spawn_failures`）を始めない（呼び出し経路の失敗が 0 回なら `nextSpawnAt == NONE`）。
- `callFailureStartsBackoff`：呼び出し経路の起動失敗は必ず `next_spawn_at` を設定し、`consecutiveFailures` を進める。
- `noSpawnDuringBackoff`：抑制期間中（`now < nextSpawnAt`）は起動を試みない。
- `unkeyedWriteSurvivesCallerCancel`：鍵なしの書き込みは、呼び出し側が打ち切っても中断（`Aborted`）されず最後まで進める。
- `keyedWriteSurvivesCallerCancel`：鍵ありの書き込みも、呼び出し側の打ち切りで中断されない。
- `writeRecordedOnlyOnSuccess`：書き込みの記録（`record_write`）は成功完了（`Done`）した書き込みに限り、`isError`・通信失敗・中断では記録しない。
- `sentWriteCanFinish`：takt へ送った書き込みが決着していなければ、往復中（`Exchanging`）で、子プロセスとロックを持ち、往復を終える遷移が取れる。`unkeyedWriteSurvivesCallerCancel`・`keyedWriteSurvivesCallerCancel` が「中断されない」を、この条件が「止まらずに決着まで進める」を受け持つ。
- `lockOwnershipConsistent`：ロック保持中の操作は高々 1 件であり、`lockedBy` とロック保持フェーズが双方向に一致し、`heldChild` の所有も一致する。
- `lockHolderCanProgress`：ロックを持つ操作には、次に取れる遷移がいつも残っている（ロックを持ったまま止まらない）。
- `safety`：上記 16 件の論理積。

次の 3 件は、モデルのガードや定数をそのまま映すため、モデルの中で新しい事実を見つける条件ではありません。守りを外したときに違反が出ることで、ガードがコードの守りと対応していることを確かめます（「守りを外した変異」の節）。

- `noSpawnDuringBackoff`：`canAttemptSpawn` のガード（`client.rs` L272-L277）を外すと違反する。
- `unkeyedWriteSurvivesCallerCancel`：`DETACH_UNKEYED_WRITE`（`write_ledger.rs` L141-L146 の `tokio::spawn`）を外すと違反する。
- `keyedWriteSurvivesCallerCancel`：`isDetached` の鍵ありの枝（`write_ledger.rs` L173-L181 の `tokio::spawn`）を外すと違反する。

### 進行の条件（Witness）

- `warmUpSucceeded`：温めが成功する。
- `warmUpFailedWithoutBackoff`：温めが失敗した時点で抑制が開始されていない（`nextSpawnAt` が変わらず抑制も無効）。
- `callBackedOffAfterFailure`：呼び出し経路の起動失敗後に `Backoff` で拒否される。
- `recoveredAfterBackoff`：抑制の満了後に起動が成功して回復する。
- `cancelledReadDisposedAndRebuilt`：往復中の打ち切りで子プロセスを破棄した**後**に、別の要求が再起動して成功する。
- `cancelledDuringSpawnDisposed`：起動（`initialize`）途中の打ち切りで子プロセスが破棄される。
- `exchangeFailedAndRecovered`：往復の通信失敗で子プロセスを破棄した**後**に、別の要求が回復して成功する。
- `toolErrorReusesCleanChild`：道具エラー（`isError`）で書き込みを記録せずに子プロセスを再利用可能に戻す。
- `unkeyedWriteCompletesAfterCallerCancel`：鍵なしの書き込みが、呼び出し側の打ち切り後も最後まで進み `writeRecorded == true` に到達する。
- `keyedWriteCompletesAfterCallerCancel`：鍵ありの書き込みも、呼び出し側の打ち切り後に完了する。
- `shutdownClosedLiveChild`：`shutdown` が生きた子プロセスを閉じる。
- `shutdownSkippedWhenAlreadyClosed`：既に閉じた子プロセスが存在し `slot` が空の状態で `shutdown` が呼ばれても二度閉じずにスキップする。

## 検査結果

quint 0.32.0（`quint verify` は Apalache 0.56.1）で実行しました。**現在のコード（`current`）に仕様違反・不具合は見つかりませんでした。**

### 型検査と筋書きテスト

```sh
quint typecheck childProcess.qnt
quint typecheck childProcess_test.qnt
quint test childProcess_test.qnt --main=childProcess_test
```

- `quint typecheck`: `childProcess.qnt`・`childProcess_test.qnt` ともに成功。
- `quint test`: 15 件すべて成功（修正前の 3 種の不具合、途中版の鍵なし書き込み中断、修正後の道具エラー再利用と記録分離、3 回連続失敗による上限 4 単位バックオフの満了と回復を固定）。

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
    toolErrorReusesCleanChild unkeyedWriteCompletesAfterCallerCancel keyedWriteCompletesAfterCallerCancel \
    shutdownClosedLiveChild shutdownSkippedWhenAlreadyClosed \
  --max-steps=35 --max-samples=50000 --seed=0x379
```

- 50,000 サンプル、`max-steps=35`、`seed=0x379` で `safety`（16 件）の反例なし（トレース長: 最短 20、最長 36、平均 27.09）。`max-steps` より短いトレースは、5 件の操作がすべて決着し、論理時刻と `shutdown` の回数が上限に達した終端です。
- すべての進行条件（witness 12 件）が 1 件以上のトレースで到達しました。

| Witness | 到達したトレース |
| --- | --- |
| `warmUpSucceeded` | 1616 / 50000（3.23%） |
| `warmUpFailedWithoutBackoff` | 5404 / 50000（10.81%） |
| `callBackedOffAfterFailure` | 31425 / 50000（62.85%） |
| `recoveredAfterBackoff` | 2321 / 50000（4.64%） |
| `cancelledReadDisposedAndRebuilt` | 8 / 50000（0.02%） |
| `cancelledDuringSpawnDisposed` | 2828 / 50000（5.66%） |
| `exchangeFailedAndRecovered` | 537 / 50000（1.07%） |
| `toolErrorReusesCleanChild` | 6943 / 50000（13.89%） |
| `unkeyedWriteCompletesAfterCallerCancel` | 3726 / 50000（7.45%） |
| `keyedWriteCompletesAfterCallerCancel` | 3786 / 50000（7.57%） |
| `shutdownClosedLiveChild` | 1332 / 50000（2.66%） |
| `shutdownSkippedWhenAlreadyClosed` | 3446 / 50000（6.89%） |

### 途中版（`withoutUnkeyedDetach`）：他の不変条件の確認

```sh
for inv in noChildRemainsAfterCancel noDoubleClose noDirtyChildReused warmUpFailureDoesNotStartBackoff callFailureStartsBackoff \
  sentWriteCanFinish lockHolderCanProgress; do
  quint run childProcess.qnt --main=withoutUnkeyedDetach --invariant="$inv" --max-steps=25 --max-samples=10000 --seed=0x379
done
```

上記 7 件はいずれも 10,000 サンプル、`max-steps=25`、`seed=0x379` で反例なし。一方、`unkeyedWriteSurvivesCallerCancel` は反例が出るため、[PR #379](https://github.com/j5ik2o/takt-dash/pull/379) の 2 コミット目（鍵なし書き込みの `tokio::spawn` 切り離し）が必要だったことが仕様上も裏付けられます。

### 現在のコード（`current`）：不変条件ごとの `quint run`

```sh
for inv in \
  noChildRemainsAfterCancel noOrphanAliveChild atMostOneLiveChild \
  noDoubleClose closedChildNotReferenced noDirtyChildReused idleSlotIsClean \
  warmUpFailureDoesNotStartBackoff callFailureStartsBackoff noSpawnDuringBackoff \
  unkeyedWriteSurvivesCallerCancel keyedWriteSurvivesCallerCancel writeRecordedOnlyOnSuccess sentWriteCanFinish \
  lockOwnershipConsistent lockHolderCanProgress; do
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
| `writeRecordedOnlyOnSuccess` | 反例なし |
| `sentWriteCanFinish` | 反例なし |
| `lockOwnershipConsistent` | 反例なし |
| `lockHolderCanProgress` | 反例なし |

### 有界モデル検査（`quint verify`）

```sh
quint verify childProcess.qnt --main=current --invariant=safety --max-steps=6
```

- Apalache 0.56.1 により `safety`（22 個の検証条件）を 6 手まで網羅検査し、`NoError`（反例なし・デッドロックなし、148 秒）。
- `quint verify` の出力先 `_apalache-out/` は `.gitignore` で除外しています。

### 守りを外した変異（`check_mutations.py`）

```sh
python3 check_mutations.py
```

`childProcess.qnt` の守りを 1 つずつ外した写しで、`--main=current --max-steps=35 --max-samples=10000 --seed=0x379` の `quint run` を流しました。19 件すべてで、期待した不変条件が違反しました。変異のない写しは先に `safety` を満たすことを確かめています。`safety` の 16 件は、どれも少なくとも 1 つの変異で違反します。

| 変異 | 外した守り | 期待する違反 | 結果 |
| --- | --- | --- | --- |
| `spawn-cancel-no-kill` | initialize 中の打ち切りで spawn() のローカル process を終了しない（kill_on_drop なし, client.rs:311） | `noChildRemainsAfterCancel` | 違反あり |
| `init-failure-no-kill` | initialize の失敗で process を終了しない（client.rs:329-331 の ? で drop されない） | `noOrphanAliveChild` | 違反あり |
| `exchange-failure-keeps-child` | 往復の失敗で process を self.process に戻す（client.rs:350-353 で捨てない） | `noDirtyChildReused` | 違反あり |
| `exchange-failure-closed-but-kept` | 往復の失敗で子プロセスを終了させたのに、self.process に戻す | `closedChildNotReferenced` | 違反あり |
| `exchange-success-not-returned` | 往復の成功で process を self.process に戻さない（client.rs:347 なし） | `noOrphanAliveChild` | 違反あり |
| `exchange-success-stays-dirty` | 応答を読み終えずに戻す（読みかけのまま self.process へ戻す） | `idleSlotIsClean` | 違反あり |
| `shutdown-without-take` | shutdown が take() せず、process を self.process に残したまま kill する（client.rs:249） | `noDoubleClose` | 違反あり |
| `shutdown-without-lock` | shutdown がクライアントのロックを取らない（registry.rs:402 の client.lock().await なし） | `lockHolderCanProgress` | 違反あり |
| `spawn-while-running` | start が既存の process を確かめずに起動する（client.rs:269-271 なし） | `atMostOneLiveChild` | 違反あり |
| `spawn-ignores-backoff` | start が next_spawn_at を確かめずに起動する（client.rs:272-277 なし） | `noSpawnDuringBackoff` | 違反あり |
| `warm-up-backs-off` | 温めの起動失敗でも抑制を始める（warm_up が OnSpawnFailure::BackOff を使う, client.rs:226） | `warmUpFailureDoesNotStartBackoff` | 違反あり |
| `read-call-retries` | 読み取りの経路の起動失敗で抑制を始めない（ensure_started が Retry を使う, client.rs:265） | `callFailureStartsBackoff` | 違反あり |
| `success-keeps-failure-count` | 起動の成功で連続失敗の回数と next_spawn_at を戻さない（client.rs:280-281 なし） | `callFailureStartsBackoff` | 違反あり |
| `request-borrows-child` | request が take() せず self.process を借りたまま往復する（client.rs:342） | `noChildRemainsAfterCancel` | 違反あり |
| `unkeyed-write-not-spawned` | 鍵なしの書き込みを tokio::spawn せず呼び出しのタスクで待つ（write_ledger.rs:141-146） | `unkeyedWriteSurvivesCallerCancel` | 違反あり |
| `keyed-write-not-spawned` | 鍵ありの書き込みを tokio::spawn せず呼び出しのタスクで待つ（write_ledger.rs:173-181） | `keyedWriteSurvivesCallerCancel` | 違反あり |
| `detached-write-loses-child` | 切り離した書き込みの打ち切りで、往復中の process を手放す | `sentWriteCanFinish` | 違反あり |
| `tool-error-records-write` | 道具エラー（isError）でも record_write() を呼ぶ（writes.rs:110-122 の ? なし） | `writeRecordedOnlyOnSuccess` | 違反あり |
| `cancel-keeps-lock` | 打ち切りでクライアントのロックを手放さない（MutexGuard が drop されない） | `lockOwnershipConsistent` | 違反あり |
