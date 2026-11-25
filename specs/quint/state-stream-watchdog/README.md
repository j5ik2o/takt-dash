# 画面 SSE 状態ストリームの半開き検知と再接続（issue #383）

画面の `openStateStream`（`frontend/web/src/components/state-stream.ts`）が、半開きの SSE 接続（VPN・Wi-Fi の切り替えやスリープからの復帰で `onerror` が出ない無音状態）を無音見張りとスリープ復帰時の鮮度確認で検知し、同時に 1 本だけの再接続で復帰すること、また古い接続からの遅延イベントで状態が戻らないことを Quint で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `frontend/web/test/components/state-stream.test.ts`、`frontend/web/test/components/live-provider-wiring.test.ts`、`e2e/tests/issue-383-half-open-sse.spec.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `stateStreamWatchdog.qnt` | 状態機械、9 つの安全不変条件（`safety`）、6 つの到達条件（witnesses） |
| `stateStreamWatchdog_test.qnt` | issue #383 の筋書きを修正前・見張りのみ・修正案でたどる 13 件のテスト |

`stateStreamWatchdog.qnt` は遷移を 2 つの定数で切り替えます。

| モジュール | `WATCHDOG` | `STALE_GUARD` | 表すもの |
| --- | --- | --- | --- |
| `current` | false | false | 修正前。無音見張りも古い接続の遮断も無い |
| `watchdogOnly` | true | false | 無音見張りだけを入れ、古い接続からの遅延 `onmessage` / `onerror` を無視しない版 |
| `fixed` | true | true | 修正案。無音見張り・スリープ復帰確認に加え、現在の接続以外からのイベントをすべて無視する |

## モデルの範囲

- 時刻は 1 目盛り = 5 秒の離散時刻です。backend の再送間隔（`WATCH_RESEND_INTERVAL` = 15 秒）を `RESEND_INTERVAL = 3`、画面の見張り時間（40 秒 = `2 × 15 秒 + 10 秒`）を `STALE_TIMEOUT = 8` として表します。
- 状態空間を閉じるため、作成する接続は最大 4 本（`MAX_CONNS = 4`）、時刻は最大 32 目盛り（160 秒、`MAX_TIME = 32`）としています。
- 健全な `Open` 接続では `STALE_TIMEOUT` 未満の間隔で受信が続きます。VPN・Wi-Fi の切り替えやハングで半開きになった状態は `goHalfOpen`（`Open` → `HalfOpen`）で表します。初回接続直後に 1 件も受信せず `HalfOpen` になる経路も含みます。
- スリープからの復帰は `wakeFromSleep(jump)`（`jump ∈ {2, 8, 12}` = 10 秒・40 秒・60 秒）で表します。
- 不正ペイロード（`restoreDashboardState` の検証失敗）は `receiveInvalidMessage` で表し、`lastActivity`・`disconnected`・`consecutiveFailures`・見張り期限のいずれも更新しません。
- 次の実装詳細はモデルに含めていません：再接続バックオフの具体的なミリ秒計算（1 秒から倍々、上限 30 秒）、`staleTimeoutMs` の数値検証（`RangeError`）、壁時計の逆行クランプ、React Context の再描画とフォーカス復帰（フォーカス復帰は `specs/quint/decision-offline-actions` が扱います）。

## 条件

不変条件（安全性）

- `silentConnectionMustDisconnect`：開いている接続で見張り時間（`STALE_TIMEOUT`）を過ぎたら必ず切断になる。
- `atMostOneUnclosedConn`：未クローズの接続は同時に 1 本まで。
- `atMostOnePendingReconnect`：有効な再接続予約は同時に 1 本まで。
- `noReconnectWhileConnected`：接続が開いている間に再接続を同時に予約しない。
- `noStaleStateRestore`：切断後・再接続後・`close` 後に、古い接続の `onmessage` で状態が戻らない。
- `noStaleErrorInterference`：古い接続の `onerror` で現在の接続や再接続予約を壊さない。
- `noPrematureWatchdogDisconnect`：見張り時間未満で受信が続いている間は見張り切断を起こさない。
- `watchdogMatchesActiveConn`：見張りタイマーは開いている現在の接続 1 本だけに紐づく。
- `nothingActiveWhenClosed`：`stream.close()` 後は接続もタイマーも残らない。
- `safety`：上の 9 つをすべて満たす。

進行の条件（到達できること。`--witnesses` で数えます）

- `witnessMessageReceived`：現在の接続で有効な状態を受信する。
- `witnessWatchdogDisconnect`：無音見張りタイマーで切断を検知する。
- `witnessWakeDisconnect`：スリープ復帰時の鮮度確認で切断を検知する。
- `witnessReconnectAndRecover`：切断後に再接続し、新しい接続の受信で復帰する。
- `witnessStaleEventIgnored`：古い接続や `close` 後の遅延イベントを無視する。
- `witnessStreamClosed`：`stream.close()` で正常に終了する。

## 結果

quint 0.32.0 で実行しました（`quint typecheck`、`quint test`、`quint run`）。

### 修正前（`current`）：半開きが切断にならない反例あり

```sh
quint run stateStreamWatchdog.qnt --main=current --invariant=silentConnectionMustDisconnect \
  --max-steps=25 --max-samples=10000 --seed=0x383
```

5 手で `silentConnectionMustDisconnect` に違反します（`activeConn = 0` が `HalfOpen` になり、`lastActivity = 1` のまま `now = 14` まで進んでも `disconnected = false` のまま残る）。

### 見張りのみ（`watchdogOnly`）：古い接続のイベントで状態が戻る・二重接続になる反例あり

```sh
quint run stateStreamWatchdog.qnt --main=watchdogOnly --invariant=safety \
  --max-steps=25 --max-samples=10000 --seed=0x383
```

6 手で `safety` に違反します（切断後に古い接続 `0` から遅延 `onmessage` が届き、`disconnected` が `false` に戻って `staleStateApplied = true` になる）。また、筋書きテスト `staleErrorOpensDuplicateConnectionWithoutGuardTest` では、再接続後に古い接続の `onerror` が発火すると `noReconnectWhileConnected` と `atMostOneUnclosedConn` に違反することを確かめています。

### 修正案（`fixed`）：反例なし

```sh
quint run stateStreamWatchdog.qnt --main=fixed --invariant=safety \
  --witnesses witnessMessageReceived witnessWatchdogDisconnect witnessWakeDisconnect \
    witnessReconnectAndRecover witnessStaleEventIgnored witnessStreamClosed \
  --max-steps=40 --max-samples=50000 --seed=0x383
```

50,000 サンプル、max-steps 40、seed `0x383` で `safety` の反例なし（trace 長は全件 41、デッドロックなし）。

| 条件 | 到達した trace |
| --- | --- |
| `witnessMessageReceived` | 15,524 / 50,000（31.05%） |
| `witnessWatchdogDisconnect` | 19 / 50,000（0.04%） |
| `witnessWakeDisconnect` | 5,387 / 50,000（10.77%） |
| `witnessReconnectAndRecover` | 4,279 / 50,000（8.56%） |
| `witnessStaleEventIgnored` | 50,000 / 50,000（100.00%） |
| `witnessStreamClosed` | 49,996 / 50,000（99.99%） |

### 筋書きテスト

```sh
quint test stateStreamWatchdog_test.qnt --main=stateStreamWatchdog_test
```

13 件すべて成功（256ms）。

## 更新のタイミング

`openStateStream` や `StateStreamProvider` の接続開閉・見張り・再接続の状態遷移を変える前に、この仕様を更新して `quint typecheck`・`quint test`・`quint run` を再実行します。
