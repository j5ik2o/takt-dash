# issue #383 の設計：半開きの SSE 接続を無音の見張りで検知し、切断として扱って接続を開き直す

対象の issue：[j5ik2o/takt-dash#383](https://github.com/j5ik2o/takt-dash/issues/383)
関連 PR：[j5ik2o/takt-dash#381](https://github.com/j5ik2o/takt-dash/pull/381)、[j5ik2o/takt-dash#378](https://github.com/j5ik2o/takt-dash/pull/378)

## 再現確認（現在の main）

`origin/main`（`75b9eed`）の `frontend/web/src/components/state-stream.ts:65-112` を確認し、不具合が実在することを確かめました。

1. `openStateStream` には最後に状態を受け取ってからの無音時間を見張る仕組みがありません。VPN・Wi-Fi の切り替えやスリープからの復帰で TCP が半開きになり `EventSource.onerror` が発火しないまま受信が止まると、`onError` が呼ばれず `disconnected` は `false` のまま残ります。結果として判断待ちのボタンも押せる状態のままになります。
2. `current.onmessage` に `closed` や「現在の接続（`source === current`）か」の確認がありません。切断後や再接続後に古い接続から遅延して `onmessage` が届くと、`onState` が呼ばれて `disconnected` を `false` に戻してしまいます。
3. `current.onerror` にも `source === current` の確認がありません。再接続後に古い接続から遅延して `onerror` が発火すると、新しい接続を閉じないまま再接続を予約してしまい、再接続の発火時に `EventSource` が同時に 2 本開きます。

## 何をどう変えるか

### 1. 無音見張り時間（`STATE_STREAM_STALE_TIMEOUT_MS = 40_000`）の根拠

- backend の `WatchState` は接続直後に全件を 1 回送り、その後は状態が変わらなくても **15 秒ごと**に再送します（`backend/src/grpc/mod.rs:77,742` の `WATCH_RESEND_INTERVAL`）。
- 見張り時間は **40 秒（`40_000` ms）** とします。
  - 計算根拠：`2 × 15 秒 = 30 秒` を超える長さとして **+10 秒の余裕**（`30 秒 + 10 秒 = 40 秒`）を取ります。
  - 再送 1 回分の遅延や一時的なネットワーク揺らぎ・状態組み立ての遅延があっても誤って切断にしません（35 秒までの遅延を許容）。
  - 2 回続けて再送が届かず 40 秒無音が続けば半開きと判定し、長時間切断の表示しきい値（`BACKEND_UNREACHABLE_LONG_THRESHOLD_MS = 60_000` ms）より早く切断を検知して開き直します。
  - `OpenStateStreamOptions` に任意の `staleTimeoutMs?: number`（既定 `40_000`）を用意し、単体テストでは短い値や仮想タイマーで決定的に検証できるようにします。

### 2. `openStateStream`（`frontend/web/src/components/state-stream.ts`）の変更

- **オプション追加**（すべて省略可能で既存の呼び出しと後方互換）：
  - `staleTimeoutMs?: number`（既定 `STATE_STREAM_STALE_TIMEOUT_MS = 40_000`）。有限の正の数かつ `setTimeout` の 32 bit 上限（`2_147_483_647` ms）以下であることを入口で検証し、範囲外は `RangeError` を投げます（`NaN` やオーバーフローによる 0ms/1ms の無限再接続ループを防ぐ）。
  - `scheduleWatchdog?: (run: () => void, delayMs: number) => () => void`（既定 `scheduleWithTimeout`）。既存の `scheduleReconnect` とは別口にし、既存テストの再接続スケジューラ観測を壊さず、見張りタイマーを独立して差し替えられるようにします。
  - `now?: () => number`（既定 `Date.now`）。スリープ復帰時の経過時間判定に使います。
- **戻り値 `StateStream` への追加**：
  - `checkStale: () => void` を追加します（既存の `close: () => void` は維持）。
- **見張りタイマーの寿命**：
  - `connect()` で新しい `EventSource` を開いた直後に `lastActivityAt = now()` を記録し、40 秒の見張りタイマーを仕掛けます。これにより、初回接続や再接続の直後に 1 件も届かずにハングした半開き接続も検知できます。
  - 有効な `onmessage` を受け取ったら、まず `restoreDashboardState(JSON.parse(event.data))` で検証し、成功した場合だけ `consecutiveFailures = 0`、`lastActivityAt = now()`、見張りタイマーの張り直しを行い、`options.onState(state)` を呼びます。検証に失敗した不正ペイロードでは見張りを張り直しません（例外送出の既存挙動は維持）。
  - `options.onState(state)` の中で同期的に `stream.close()` が呼ばれてもタイマーが残らないよう、見張りの張り直しは `onState` より先に行います。
- **切断処理の一本化と古い接続の遮断（`disconnect(target)`）**：
  - `onerror`・見張りタイマー発火・`checkStale()` の 3 つを共通の `disconnect(target)` にまとめます。
  - `if (closed || source !== target) return;` で、すでに閉じた接続や古い世代の接続からのイベントをすべて無視します。
  - 切断時は順序を固定します：
    1. 見張りタイマーを解除する。
    2. `source = undefined` にして世代を切り離す。
    3. `target.onmessage = null; target.onerror = null;` としてから `target.close()` を呼ぶ（`close()` の中で再入する `onerror` を防ぐ）。
    4. `options.onError?.()` を呼ぶ。
    5. `if (closed) return;`（`onError` の中で同期的に `close()` が呼ばれたら再接続を予約しない）。
    6. 既存のバックオフ計算（1 秒から倍々、上限 30 秒）で `consecutiveFailures += 1` とし、既存の再接続予約があれば解除してから再接続を 1 本だけ予約する。
  - `scheduleWatchdog`・`scheduleReconnect` が同期的にコールバックを実行した場合でも、完了済みの取消関数で新しい取消関数を上書きしないよう `fired` フラグで保護します。
- **スリープ復帰時の鮮度確認（`checkStale`）**：
  - `if (closed || source === undefined) return;`
  - `const currentNow = now();`
  - `if (currentNow < lastActivityAt) { lastActivityAt = currentNow; return; }`（NTP などで壁時計が逆行しても誤切断しない。無音上限は一次の見張りタイマーが担保し続ける）。
  - `if (currentNow - lastActivityAt >= staleTimeoutMs) { disconnect(source); }`
- **`stream.close()`**：
  - `closed = true` にし、見張りタイマーと再接続予約を両方解除し、`source` のハンドラを外して `source.close()` し、`source = undefined` にします。

### 3. `StateStreamProvider`（`frontend/web/src/components/state-provider.tsx`）の変更

- OS のスリープ中に `setTimeout` の単調時計が停止する環境（macOS 等）でも、復帰直後に半開きを即座に検知できるよう、`StateStreamProvider` の `useEffect` で次のイベントに `stream.checkStale()` を配線し、アンマウント時に必ず解除します。
  - `document` の `visibilitychange`（`document.visibilityState === "visible"` のとき）
  - `window` の `online`
  - `window` の `focus`
- 切断の通知は既存の `onError` → `stateStreamHandlers` → `reduceLiveState` の経路をそのまま通します。したがって：
  - [j5ik2o/takt-dash#378](https://github.com/j5ik2o/takt-dash/pull/378)：切断時も最後に読めた `state` と `snapshotLoaded` を保持し、見出しの上に切断バナーを出します。
  - [j5ik2o/takt-dash#381](https://github.com/j5ik2o/takt-dash/pull/381)：`disconnected: true` により判断待ちの回答ボタン・確定ボタンが無効化され、フォーカス移動と再接続時の復帰がそのまま動きます。

### 4. 既存テストへの影響

- `frontend/web/test/components/state-stream.test.ts` の「同じ接続でエラーが複数回発火しても、再接続の予約は1つだけ残り、接続は増えない」では、修正前は閉じた同一接続の 2・3 回目の `onerror` でも再予約していたため `scheduler.scheduled.length === 3` を確認していました。修正後は最初の `onerror` で接続を切り離して 2・3 回目を無視するため、`scheduler.scheduled.length === 1`、有効予約 1 件、`onError` 呼び出し 1 回、接続数 2 本、待機時間 1 秒のままとなります（連続失敗回数の過剰加算も解消）。このテストを新しい契約に合わせて更新します。

### 変えないこと・範囲の外

- backend の `WATCH_RESEND_INTERVAL`（15 秒）、`/api/state` Route Handler、proto 定義は変えません。
- 再接続のバックオフ仕様（1 秒から倍々、上限 30 秒）や、`online` イベント時の再接続待機短縮は入れません（再接続は常に単一の予約経路に保つ）。
- 切断中の表示文面・バナー配置・判断待ちボタンの無効化ロジック（PR #381 / #378）は変えません。
- 待ち受けアドレス（`0.0.0.0:3000`）や接続元制限は一切触りません。
- SSE を使わない 1 秒ポーリングの詳細画面（`frontend/web/src/components/polling.ts`）は範囲外です。

## 守る不変条件

1. **見張り時間を過ぎたら必ず切断になる**：開いている接続で、接続開始または最後の有効な受信から `staleTimeoutMs`（40 秒）以上何も届かなければ、必ず `close()` して `onError()` を呼び、切断状態へ移って再接続を予約する。スリープ復帰時も `now() - lastActivityAt >= staleTimeoutMs` なら即座に切断する。
2. **開き直しは同時に 1 本だけ**：未クローズの `EventSource` は常に 1 本以下、有効な再接続予約は常に 1 本以下、さらに「接続が開いている間は再接続予約が 0 本」を保つ。
3. **古い接続のイベントで状態が戻らない・壊れない**：切断後・再接続後・`stream.close()` 後に古い接続から遅延して届いた `onmessage`・`onerror`・見張り発火はすべて無視し、`disconnected` の復帰・状態の上書き・連続失敗回数のリセット・現行接続の見張りリセット・二重再接続を起こさない。
4. **受信が続いている間は切断にしない**：有効な状態が見張り時間未満の間隔（backend の 15 秒再送を含む）で届き続けている間は、見張りタイマーや `checkStale()` で切断しない。
5. **見張りタイマーは現在の接続 1 本だけに紐づく**：有効な見張りタイマーは最大 1 本であり、`source === undefined` または `closed === true` のときは 0 本。
6. **`stream.close()` 後は接続もタイマーも残らない**：`close()` 呼び出し後は、開いている接続 0 本、有効な再接続予約 0 本、有効な見張りタイマー 0 本、かつ以後のどのイベントもコールバックを発火しない。

## PBT の性質

`frontend/web` には fast-check 等が入っていないため、決定的な PRNG（シード固定の擬似乱数）でランダムな操作列を大量生成する状態機械 PBT を `frontend/web/test/components/state-stream.test.ts` に実装します。除外はつけません。

- **入力空間（除外なし）**：
  - 乱数シード 200 系列 × 各 60 手（計 12,000 遷移）。
  - 各手でランダムに選ぶ操作：時刻進行（0〜50 秒、負の壁時計逆行を含む）、現在の接続での有効メッセージ受信、不正 JSON／スキーマ不正メッセージ受信、現在の接続での `onerror`、過去の任意の接続 ID への遅延 `onmessage`／遅延 `onerror`、見張りタイマー発火、再接続タイマー発火、`checkStale()` 呼び出し、`stream.close()`（閉じた後も遅延イベントやタイマー発火を継続試行）。
  - 境界値（`staleTimeoutMs` ちょうどの時刻、1ms 手前、1ms 後、連続タイムアウト、初回無音、コールバック内 `close()`）も全組み合わせ列挙で網羅します。
- **全ステップで検証する性質**：
  1. 未クローズの `FakeEventSource` 数は常に `<= 1`。
  2. 有効（未取消・未発火）な再接続タイマー数は常に `<= 1`、かつ未クローズ接続が 1 本ある間は `0`。
  3. 有効な見張りタイマー数は未クローズ接続があるとき `1`、無いとき `0`。
  4. 古い接続や `close()` 後のイベントで `onState`・`onError` が一度も呼ばれない。
  5. `now - lastActivityAt < staleTimeoutMs` の間は `checkStale()` や正常受信で見張り切断が起きない。
  6. 半開き（無音）のまま見張り時間が経過した後は必ず `disconnected === true` かつ古い接続が閉じられ、再接続が 1 本予約される。
  7. `stream.close()` 後は未クローズ接続・有効見張りタイマー・有効再接続タイマーがすべて `0`、かつ以後の `onState`・`onError`・新規接続作成が `0`。

## 状態遷移と Quint

`specs/quint/state-stream-watchdog/` に Quint の仕様とテストを置きました。

- `specs/quint/state-stream-watchdog/stateStreamWatchdog.qnt`：状態機械（`current`／`watchdogOnly`／`fixed` の 3 構成）、9 つの安全不変条件（`safety`）、6 つの到達条件（witnesses）
- `specs/quint/state-stream-watchdog/stateStreamWatchdog_test.qnt`：13 件の筋書きテスト
- `specs/quint/state-stream-watchdog/README.md`：モデルの範囲・前提・実行結果

検査結果（quint 0.32.0）：

- `quint typecheck`：`stateStreamWatchdog.qnt`・`stateStreamWatchdog_test.qnt` ともにエラーなし。
- `quint test stateStreamWatchdog_test.qnt --main=stateStreamWatchdog_test`：13 件すべて成功（256ms）。
- `quint run --main=current --invariant=silentConnectionMustDisconnect --max-steps=25 --max-samples=10000 --seed=0x383`：5 手で違反（修正前の不具合を再現）。
- `quint run --main=watchdogOnly --invariant=safety --max-steps=25 --max-samples=10000 --seed=0x383`：6 手で違反（古い接続の遅延受信で `disconnected` が `false` に戻る反例）。
- `quint run --main=fixed --invariant=safety --max-steps=40 --max-samples=50000 --seed=0x383`：50,000 サンプルすべて反例なし、全 6 つの witness が到達（詳細は `specs/quint/state-stream-watchdog/README.md`）。

## 壊しうる入力の一覧

| 分類 | 具体的な入力・状況 | 期待する振る舞い |
| --- | --- | --- |
| 境界 | 最後の受信から `staleTimeoutMs - 1` ms（39,999ms）経過時点で受信 | 切断せず見張りを張り直す |
| 境界 | 最後の受信からちょうど `staleTimeoutMs` ms（40,000ms）経過 | 切断として扱い、接続を閉じて再接続を 1 本予約 |
| 境界 | `staleTimeoutMs` に `0`・負数・`NaN`・`Infinity`・`2_147_483_648` 以上を渡す | `RangeError` を投げて即座に拒否する |
| 境界 | 接続直後に 1 件も受信しないまま 40 秒経過（初回ハング／再接続ハング） | 切断として扱い、接続を閉じて次の再接続を予約する |
| 並行 | 見張り切断の直後（再接続待機中）に古い接続の `onmessage`／`onerror` が届く | 無視する。`disconnected` を `false` に戻さず、再接続予約も増やさない |
| 並行 | 再接続で新しい接続が開いた後に古い接続の `onmessage`／`onerror` が届く | 無視する。新しい接続の見張りや状態を壊さず、二重接続を作らない |
| 並行 | 同一接続で `onerror` が複数回発火する、または `close()` 内で `onerror` が再入する | 最初の 1 回だけ処理し、2 回目以降は無視する |
| 並行 | `onState` または `onError` のコールバック内で同期的に `stream.close()` を呼ぶ | 閉じた後に見張りや再接続を予約せず、タイマー・接続の漏れを残さない |
| 並行 | テスト用の同期スケジューラが `scheduleWatchdog`／`scheduleReconnect` 内で即時実行する | 既に完了した取消関数で新しい予約を上書きしない |
| 再開 | OS スリープ（60 秒以上）から復帰し `visibilitychange`／`online`／`focus` が発火 | `checkStale()` が `now() - lastActivityAt >= 40_000` を検知し即座に切断・再接続予約 |
| 再開 | 短いタブ切替（10 秒）で `visibilitychange`／`focus` が発火 | `now() - lastActivityAt < 40_000` のため切断しない |
| 再開 | NTP 等で壁時計 `now()` が `lastActivityAt` より前へ逆行した状態で `checkStale()` が発火 | 誤切断せず `lastActivityAt` を現在時刻へ詰める（見張りタイマーが上限を担保） |
| 区切り・入れ子・長さ・Unicode | 不正 JSON、必須項目欠落、Unicode や長い文字列を含む正常な `DashboardState` JSON | 不正ペイロードは既存のとおり検証で弾き見張りを更新しない。有効な JSON は従来どおり復元して見張りを更新 |

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- **上流（takt、takt-mcp）**：影響なし。
- **保存済みデータ**：影響なし。
- **公開 API（proto、`/api/*`）**：影響なし。変更は画面内部の `openStateStream`（省略可能なオプションと `checkStale` メソッドの追加）と `StateStreamProvider` のみ。
- **守りの仕組み（CI、関門、ルールセット）**：影響なし。
