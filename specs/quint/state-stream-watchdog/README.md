# 画面 SSE 状態ストリームの半開き検知と再接続（issue #383）

画面の `openStateStream`（`frontend/web/src/components/state-stream.ts`）が、半開きの SSE 接続（VPN・Wi-Fi の切り替えやスリープからの復帰で `onerror` が出ない無音状態）を無音見張りとスリープ復帰時の鮮度確認で検知し、同時に 1 本だけの再接続で復帰すること、また古い接続からの遅延イベントで状態が戻らないことを Quint で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `frontend/web/test/components/state-stream.test.ts`、`frontend/web/test/components/live-provider-wiring.test.ts`、`e2e/tests/issue-383-half-open-sse.spec.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `stateStreamWatchdog.qnt` | 単調時計・壁時計の 2 時計を持つ状態機械、実状態の 10 の安全不変条件（`safety`）、8 つの到達条件（witnesses） |
| `stateStreamWatchdog_test.qnt` | issue #383 の筋書きを 5 構成でたどる 17 件のテスト |

`stateStreamWatchdog.qnt` は遷移を 5 つの定数で切り替えます。`WAKE_CUTOFF` は復帰時 `checkStale` が切断と判定する壁時計経過のしきい値で、`earlyWakeCutoff` 以外は `STALE_TIMEOUT` と同じ 8 です。

| モジュール | `WATCHDOG` | `STALE_GUARD` | `REARM_ON_WAKE` | `WAKE_CUTOFF` | `ALLOW_CLOSE` | 表すもの |
| --- | --- | --- | --- | --- | --- | --- |
| `current` | false | false | false | 8 | true | 修正前。無音見張りも古い接続の遮断も無い |
| `watchdogOnly` | true | false | true | 8 | true | 無音見張りだけを入れ、古い接続からの遅延 `onmessage` / `onerror` を無視しない変異 |
| `noWakeRearm` | true | true | false | 8 | true | 復帰時 `checkStale` で期限未満のときに残り時間で張り直さない変異 |
| `earlyWakeCutoff` | true | true | true | 2 | true | 復帰時 `checkStale` が 10 秒で切断してしまう変異 |
| `fixed` | true | true | true | 8 | true | 修正案（`closeStream` を含む全遷移） |
| `fixedOpenSession` | true | true | true | 8 | false | 修正案でタブを閉じずに長く開いたまま探索する構成 |

## モデルの範囲と前提

- 時計は 2 本に分けています（1 目盛り = 5 秒）：
  - `monoNow`：`setTimeout` が使う単調時計。通常の `tick` では進むが、OS スリープ（`wakeFromSleep`）では進まない。
  - `wallNow`：`Date.now()` の壁時計。`tick` と `wakeFromSleep` の両方で進み、NTP 等の逆行（`rewindWallClock`）も起こりうる。
- backend の再送間隔（`WATCH_RESEND_INTERVAL` = 15 秒）を `RESEND_INTERVAL = 3`、画面の見張り時間（40 秒）を `STALE_TIMEOUT = 8` として表します。実物では backend の配信間隔の上限は「15 秒＋状態の組み立て・送信時間」であり、40 秒は最大 25 秒の遅延を許容する設定です。
- 状態空間を閉じるため、作成する接続は最大 4 本（`MAX_CONNS = 4`）、時刻は最大 40 目盛り（200 秒、`MAX_TIME = 40`）としています。
- `deliverMessage(id)` と `deliverError(id)` は各 1 本の action にまとめ、実装と同じ判定（`closed` または `id != activeConn` なら無視）で処理します。
- 健全な `Open` 接続であっても期限を越えて黙り続ける経路（`tick` の無制限進行）と、スリープ後も TCP が生きている経路（`Open` のまま `wakeFromSleep` して遅延 `deliverMessage` が届く）をそのまま許しています。猶予の見張りは入れず、期限を越えたら一度切断して閉じ、遅れて届いた受信を捨て、次の接続で復帰する設計を検証します。
- `wakeFromSleep` はスリープ後にブラウザの復帰イベント（`visibilitychange` / `online` / `focus`）で `checkStale()` が呼ばれる前提を表しています。画面が前面のままイベントが出ず OS が `setTimeout` を止めていた場合は、復帰後最大 `STALE_TIMEOUT` で見張りタイマーが発火します。
- 壁時計が前へ飛ぶ場合（NTP の補正など）は `wakeFromSleep` でしか表していません。単調時計が進まないまま壁時計だけが進む点はスリープと同じなので、同じ遷移で代表させています。
- 不正ペイロード（`restoreDashboardState` の検証失敗）は `receiveInvalidMessage` で表し、`lastActivityWall`・`disconnected`・`consecutiveFailures`・見張り期限のいずれも更新しません。
- 次の実装詳細はモデルに含めていません：再接続バックオフの具体的なミリ秒計算（1 秒から倍々、上限 30 秒）、`staleTimeoutMs` の数値検証（`RangeError`）、見捨てられた旧接続のサーバー側負荷（別件 [j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385)）、React Context の再描画とフォーカス復帰（`specs/quint/decision-offline-actions` が扱います）。

## 条件

不変条件（安全性。すべて実状態で記述し、印フラグを使わない）

- `silentConnectionMustDisconnect`：開いている接続で壁時計が見張り時間（`STALE_TIMEOUT`）を過ぎたら必ず切断になる（部分スリープ・長時間スリープを挟んでも成り立つ）。
- `atMostOneUnclosedConn`：未クローズの接続は同時に 1 本まで。
- `atMostOnePendingReconnect`：有効な再接続予約は同時に 1 本まで。
- `noReconnectWhileConnected`：接続が開いている間に再接続を同時に予約しない。
- `connectedImpliesValidActiveConn`：接続中（`not(closed) and not(disconnected)`）なら現在の接続が存在し、再接続後の接続（`activeConn > 0`）ではその接続自身で少なくとも 1 回は受信している。
- `appliedStateMatchesActiveWhenConnected`：接続中に最後に適用した状態は、現在の接続から届いたものだけ。
- `noPrematureWatchdogDisconnect`：期限より前に見張りタイマーでも復帰時確認（`checkStale`）でも切らない。切断のたびに原因（`lastDisconnectCause`）と切った時点の壁時計の経過（`lastDisconnectGap = wallNow - lastActivityWall`）を状態に残し、「原因が `WatchdogTimer` か `WakeCheck` なら `lastDisconnectGap >= STALE_TIMEOUT`」と状態で書いています。
- `watchdogMatchesActiveConn`：見張りタイマーは開いている現在の接続 1 本だけに紐づく。
- `activeConnHasWatchdog`：見張りのある構成では、開いている現在の接続には必ず見張りが付いている。開き直した接続に見張りを張り忘れると、その接続は切断表示のまま期限を過ぎても残り、`silentConnectionMustDisconnect` だけでは捕まえられないため、別に置いています。
- `nothingActiveWhenClosed`：`stream.close()` 後は接続もタイマーも残らない。
- `safety`：上の 10 をすべて満たす。

進行の条件（到達できること。`--witnesses` で数えます）

- `witnessMessageReceived`：現在の接続で有効な状態を受信する。
- `witnessWatchdogDisconnect`：無音見張りタイマーで切断を検知する。
- `witnessWakeDisconnect`：スリープ復帰時の鮮度確認で切断を検知する。
- `witnessReconnectAndRecover`：切断後に再接続し、新しい接続の受信で復帰する。
- `witnessWatchdogTimerRecover`：見張りタイマーで切断 → 開き直し → 新しい接続の受信で戻る一続きを完走する。
- `witnessWakeCheckRecover`：復帰時確認（`checkStale`）で切断 → 開き直し → 新しい接続の受信で戻る一続きを完走する。
- `witnessStaleEventIgnored`：古い接続や `close` 後の遅延イベントを無視する。
- `witnessStreamClosed`：`stream.close()` で正常に終了する。

## 結果

quint 0.32.0 で実行しました（`quint typecheck`、`quint test`、`quint run`）。

### 修正前（`current`）：半開きが切断にならない反例あり

```sh
quint run stateStreamWatchdog.qnt --main=current --invariant=silentConnectionMustDisconnect \
  --max-steps=25 --max-samples=10000 --seed=0x383
```

4 手で `silentConnectionMustDisconnect` に違反します（`activeConn = 0` が `HalfOpen` になり、`lastActivityWall = 0` のまま `wallNow = 9` まで進んでも `disconnected = false` のまま残る）。

### 見張りのみ（`watchdogOnly`）：古い受信で `disconnected` を戻す変異で `safety` が破れる

```sh
quint run stateStreamWatchdog.qnt --main=watchdogOnly --invariant=safety \
  --max-steps=25 --max-samples=10000 --seed=0x383
```

8 手で `safety` に違反します（切断中で `activeConn = -1`・`conns = Map(0 -> Closed)` の状態に古い接続 `0` から `deliverMessage(0)` が届き、`disconnected` が `false` に戻って状態不変条件 `connectedImpliesValidActiveConn` と `watchdogMatchesActiveConn` に違反）。また、筋書きテスト `staleErrorOpensDuplicateConnectionWithoutGuardTest` では、再接続後に古い接続の `deliverError(0)` が発火すると `noReconnectWhileConnected` と `atMostOneUnclosedConn` に違反することを確かめています。

### 復帰時の張り直し無し（`noWakeRearm`）：部分スリープで期限超過する反例あり

```sh
quint run stateStreamWatchdog.qnt --main=noWakeRearm --invariant=silentConnectionMustDisconnect \
  --max-steps=25 --max-samples=10000 --seed=0x383
```

6 手で `silentConnectionMustDisconnect` に違反します（期限未満の部分スリープで単調時計 `monoNow` が遅れたまま見張りタイマーを残り時間で張り直さないため、`wallNow = 9 > lastActivityWall + 8` になっても `monoNow = 4 < watchdogDeadlineMono = 8` のまま `disconnected = false` が残る）。

### 復帰時確認が早すぎる（`earlyWakeCutoff`）：期限前の切断で `safety` が破れる

```sh
quint run stateStreamWatchdog.qnt --main=earlyWakeCutoff --invariant=safety \
  --max-steps=25 --max-samples=10000 --seed=0x383
```

4 手で `safety` に違反します（最後の受信から壁時計で 25 秒（5 目盛り）の `wakeFromSleep(5)` で `checkStale` が切断し、`lastDisconnectCause = WakeCheck`・`lastDisconnectGap = 5 < 8` となって `noPrematureWatchdogDisconnect` に違反）。前の版では復帰時の切断の枝が不変条件 4 の観測に入っておらず、この変異でも `safety` が通っていました。

### 開き直した接続に見張りを張らない変異（写しで確認）：`safety` が破れる

`reconnectFire` で `watchdogArmed' = false`・`watchdogConn' = NONE` とした写しを作り、`--main=fixed` で走らせました。

- `--invariant=silentConnectionMustDisconnect --max-steps=40 --max-samples=20000 --seed=0x383`：反例なし。開き直した接続は切断表示のままなので、この条件だけでは見張りの張り忘れを捕まえられません。
- `--invariant=safety --max-steps=25 --max-samples=10000 --seed=0x383`：9 手で違反します（`activeConnHasWatchdog`）。

### 修正案（`fixed`）：反例なし

```sh
quint run stateStreamWatchdog.qnt --main=fixed --invariant=safety \
  --witnesses witnessMessageReceived witnessWatchdogDisconnect witnessWakeDisconnect \
    witnessReconnectAndRecover witnessWatchdogTimerRecover witnessWakeCheckRecover \
    witnessStaleEventIgnored witnessStreamClosed \
  --max-steps=40 --max-samples=20000 --seed=0x383
```

20,000 サンプル、max-steps 40、seed `0x383` で `safety` の反例なし（trace 長は全件 41、デッドロックなし）。

| 条件 | 到達した trace |
| --- | --- |
| `witnessMessageReceived` | 3,350 / 20,000（16.75%） |
| `witnessWatchdogDisconnect` | 31 / 20,000（0.15%） |
| `witnessWakeDisconnect` | 5,794 / 20,000（28.97%） |
| `witnessReconnectAndRecover` | 536 / 20,000（2.68%） |
| `witnessWatchdogTimerRecover` | 2 / 20,000（0.01%） |
| `witnessWakeCheckRecover` | 276 / 20,000（1.38%） |
| `witnessStaleEventIgnored` | 20,000 / 20,000（100.00%） |
| `witnessStreamClosed` | 19,991 / 20,000（99.95%） |

### 修正案・長時間セッション（`fixedOpenSession`）：反例なし

`ALLOW_CLOSE = false` でタブを閉じない長期セッションを探索しました。`fixed` では見張りタイマーで切って戻る一続きが 2 件しか通らないため、この構成で 36 件まで増やしています。見張りタイマーの経路は 1 件あたりの手数が長く、復帰時確認の経路（2,985 件）に比べると薄いままです。筋書きテスト `halfOpenDisconnectsAndRecoversTest`・`silentHealthyConnDisconnectsAndRecoversTest` が、この経路を決まった手順で通します。

```sh
quint run stateStreamWatchdog.qnt --main=fixedOpenSession --invariant=safety \
  --witnesses witnessMessageReceived witnessWatchdogDisconnect witnessWakeDisconnect \
    witnessReconnectAndRecover witnessWatchdogTimerRecover witnessWakeCheckRecover \
    witnessStaleEventIgnored \
  --max-steps=60 --max-samples=20000 --seed=0x383
```

20,000 サンプル、max-steps 60、seed `0x383` で `safety` の反例なし（trace 長は全件 61、デッドロックなし）。

| 条件 | 到達した trace |
| --- | --- |
| `witnessMessageReceived` | 8,097 / 20,000（40.48%） |
| `witnessWatchdogDisconnect` | 662 / 20,000（3.31%） |
| `witnessWakeDisconnect` | 19,554 / 20,000（97.77%） |
| `witnessReconnectAndRecover` | 5,196 / 20,000（25.98%） |
| `witnessWatchdogTimerRecover` | 36 / 20,000（0.18%） |
| `witnessWakeCheckRecover` | 2,985 / 20,000（14.92%） |
| `witnessStaleEventIgnored` | 19,999 / 20,000（100.00%） |

### 筋書きテスト

```sh
quint test stateStreamWatchdog_test.qnt --main=stateStreamWatchdog_test
```

17 件すべて成功（992ms）。

## 更新のタイミング

`openStateStream` や `StateStreamProvider` の接続開閉・見張り・再接続の状態遷移を変える前に、この仕様を更新して `quint typecheck`・`quint test`・`quint run` を再実行します。
