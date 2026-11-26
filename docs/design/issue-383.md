# issue #383 の設計：半開きの SSE 接続を無音の見張りで検知し、切断として扱って接続を開き直す

対象の issue：[j5ik2o/takt-dash#383](https://github.com/j5ik2o/takt-dash/issues/383)
関連 PR：[j5ik2o/takt-dash#381](https://github.com/j5ik2o/takt-dash/pull/381)、[j5ik2o/takt-dash#378](https://github.com/j5ik2o/takt-dash/pull/378)
関連別件：[j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385)（見捨てられた接続のサーバー側負荷）

## 再現確認（現在の main）

`origin/main`（`75b9eed`）の `frontend/web/src/components/state-stream.ts:65-112` を確認し、不具合が実在することを確かめました。

1. `openStateStream` には最後に状態を受け取ってからの無音時間を見張る仕組みがありません。VPN・Wi-Fi の切り替えやスリープからの復帰で TCP が半開きになり `EventSource.onerror` が発火しないまま受信が止まると、`onError` が呼ばれず `disconnected` は `false` のまま残ります。結果として判断待ちのボタンも押せる状態のままになります。
2. `current.onmessage` に `closed` や「現在の接続（`source === current`）か」の確認がありません。切断後や再接続後に古い接続から遅延して `onmessage` が届くと、`onState` が呼ばれて `disconnected` を `false` に戻してしまいます。
3. `current.onerror` にも `source === current` の確認がありません。再接続後に古い接続から遅延して `onerror` が発火すると、新しい接続を閉じないまま再接続を予約してしまい、再接続の発火時に `EventSource` が同時に 2 本開きます。

## 何をどう変えるか

### 1. 無音見張り時間（`STATE_STREAM_STALE_TIMEOUT_MS = 40_000`）の根拠

- backend の `WatchState` は接続直後に全件を 1 回送り、その後は状態が変わらなくても **15 秒ごと**（`WATCH_RESEND_INTERVAL`、`backend/src/grpc/mod.rs:77,742`）に `assemble_state_for` を実行して再送します。
- 実際の受信間隔の上限は **「15 秒 ＋ 状態の組み立て・送信時間」** です。
- 見張り時間は **40 秒（`40_000` ms）** とします。
  - 計算根拠：15 秒の再送間隔に対して **25 秒の余裕** があり、**状態の組み立てやネットワーク遅延が最大 25 秒遅れても誤検知しません**。
  - 一方で、本当に半開きで無音になった場合は最後の受信から 40 秒で切断を検知でき、長時間切断の文面しきい値（`BACKEND_UNREACHABLE_LONG_THRESHOLD_MS = 60_000` ms）より早く開き直しに入れます。
- **タブを裏に回したときの間引き・凍結からの復帰**：
  - ブラウザはバックグラウンドタブの `setTimeout` を間引くことがあります（例：数秒〜最大 1 分単位）。裏にいる間は見張りタイマーの発火が遅れることがありますが、その間は利用者が画面のボタンを押すことはありません。
  - タブを前面に戻した瞬間（または凍結 freeze から復帰して表に出た瞬間）に `visibilitychange`（`visible`）・`focus`・`online` が発火し、`stream.checkStale()` が `now() - lastActivityAt` を壁時計で即座に検査します。40 秒以上経っていれば利用者が操作する前に即座に切断・再接続へ移り、40 秒未満なら残り時間で見張りタイマーを張り直します。
  - 裏にいる間も正常に SSE を受信できていれば `lastActivityAt` が更新されるため、表に戻っても誤切断しません。凍結中に SSE 受信自体がブラウザに止められていた場合は、40 秒以上凍結していれば復帰直後に一度切断→再接続となります。
- **健全な接続が期限を越えて黙った場合・スリープ後も TCP が生きている場合の扱い**：
  - 猶予の見張り（追加の待ち時間）は入れません。40 秒以上無音が続いた場合や、スリープ復帰時点で 40 秒以上経っていた場合は、たとえ TCP が裏でまだ生きていても一度切断として扱い、古い接続を閉じます。
  - このとき一瞬だけ切断バナーの表示とボタンの無効化が起きますが、その一瞬の切断は受け入れます。遅れて届いた旧接続の受信は捨て、約 1 秒後の再接続と次の受信で復帰します。

### 2. `openStateStream`（`frontend/web/src/components/state-stream.ts`）の変更

- **オプション追加**（すべて省略可能で既存の呼び出しと後方互換）：
  - `staleTimeoutMs?: number`（既定 `STATE_STREAM_STALE_TIMEOUT_MS = 40_000`）。有限の正の数かつ `setTimeout` の 32 bit 上限（`2_147_483_647` ms）以下であることを入口で検証し、範囲外は `RangeError` を投げます（`NaN` やオーバーフローによる 0ms/1ms の無限再接続ループを防ぐ）。
  - `scheduleWatchdog?: (run: () => void, delayMs: number) => () => void`（既定 `scheduleWithTimeout`）。既存の `scheduleReconnect` とは別口にし、既存テストの再接続スケジューラ観測を壊さず、見張りタイマーを独立して差し替えられるようにします。
  - `now?: () => number`（既定 `Date.now`）。見張り発火時と `checkStale()` の経過時間判定に使います。
- **戻り値 `StateStream` への追加**：
  - `checkStale: () => void` を追加します（既存の `close: () => void` は維持）。
- **見張りタイマーの寿命と残り時間での張り直し**：
  - `connect()` で新しい `EventSource` を開いた直後に `lastActivityAt = now()` を記録し、`staleTimeoutMs`（40 秒）で見張りタイマーを仕掛けます。これにより、初回接続や再接続の直後に 1 件も届かずにハングした半開き接続も検知できます。
  - 有効な `onmessage` を受け取ったら、まず `restoreDashboardState(JSON.parse(event.data))` で検証し、成功した場合だけ `consecutiveFailures = 0`、`lastActivityAt = now()`、見張りタイマーの張り直し（`staleTimeoutMs`）を行い、`options.onState(state)` を呼びます。検証に失敗した不正ペイロードでは見張り・`lastActivityAt`・`consecutiveFailures` を更新しません（例外送出の既存挙動は維持）。
  - `options.onState(state)` の中で同期的に `stream.close()` が呼ばれてもタイマーが残らないよう、見張りの張り直しは `onState` より先に行います。
  - **見張りタイマー発火時と `checkStale()` での共通鮮度判定（`evaluateFreshness(target)`）**：
    1. `if (closed || source !== target) return;`
    2. `const currentNow = now();`
    3. 壁時計が逆行していた場合（`currentNow < lastActivityAt`）：誤切断せず `lastActivityAt = currentNow` へ詰めて `staleTimeoutMs` で見張りタイマーを張り直す。
    4. 経過時間 `elapsedMs = currentNow - lastActivityAt` が `staleTimeoutMs` 未満の場合：切断せず、**残り時間 `remainingMs = staleTimeoutMs - elapsedMs` で見張りタイマーを張り直す**。これにより、タイマーの早期発火や、単調時計が止まった部分スリープ（例：25 秒スリープ後に復帰）のあとでも、壁時計基準の期限（40 秒）どおりに切断できます。
    5. `elapsedMs >= staleTimeoutMs` の場合：`disconnect(target)` を呼んで切断し、再接続を 1 本予約する。
- **切断処理の一本化と古い接続の遮断（`disconnect(target)`）**：
  - `onerror` と `evaluateFreshness(target)`（見張りタイマー発火・`checkStale()`）を共通の `disconnect(target)` にまとめます。
  - `if (closed || source !== target) return;` で、すでに閉じた接続や古い世代の接続からのイベントをすべて無視します。
  - 切断時は順序を固定します：
    1. 見張りタイマーを解除する。
    2. `source = undefined` にして世代を切り離す。
    3. `target.onmessage = null; target.onerror = null;` としてから `target.close()` を呼ぶ（`close()` の中で再入する `onerror` を防ぐ）。
    4. `options.onError?.()` を呼ぶ。
    5. `if (closed) return;`（`onError` の中で同期的に `close()` が呼ばれたら再接続を予約しない）。
    6. 既存のバックオフ計算（1 秒から倍々、上限 30 秒）で `consecutiveFailures += 1` とし、既存の再接続予約があれば解除してから再接続を 1 本だけ予約する。
  - `scheduleWatchdog`・`scheduleReconnect` が同期的にコールバックを実行した場合でも、完了済みの取消関数で新しい取消関数を上書きしないよう `fired` フラグで保護します。
- **`stream.close()`**：
  - `closed = true` にし、見張りタイマーと再接続予約を両方解除し、`source` のハンドラを外して `source.close()` し、`source = undefined` にします。

### 3. `StateStreamProvider`（`frontend/web/src/components/state-provider.tsx`）の変更

- OS スリープ中に `setTimeout` の単調時計が停止する環境やバックグラウンドタブ間引き・凍結からの復帰に備え、`StateStreamProvider` の `useEffect` で次のイベントに `stream.checkStale()` を配線し、アンマウント時に必ず解除します。
  - `document` の `visibilitychange`（`document.visibilityState === "visible"` のとき）
  - `window` の `online`
  - `window` の `focus`
- 切断の通知は既存の `onError` → `stateStreamHandlers` → `reduceLiveState` の経路をそのまま通します。したがって：
  - [j5ik2o/takt-dash#378](https://github.com/j5ik2o/takt-dash/pull/378)：切断時も最後に読めた `state` と `snapshotLoaded` を保持し、見出しの上に切断バナーを出します。
  - [j5ik2o/takt-dash#381](https://github.com/j5ik2o/takt-dash/pull/381)：`disconnected: true` により判断待ちの回答ボタン・確定ボタンが無効化され、フォーカス移動と再接続時の復帰がそのまま動きます。

### 4. テスト方針（既存テスト・新規単体・e2e）

- `frontend/web/test/components/state-stream.test.ts` の `open` ヘルパは、既定で偽の見張りスケジューラ（`scheduleWatchdog`）を渡し、本物の 40 秒タイマーを残さないようにします。
- 同ファイルの「同じ接続でエラーが複数回発火しても、再接続の予約は1つだけ残り、接続は増えない」では、修正前は閉じた同一接続の 2・3 回目の `onerror` でも再予約していたため `scheduler.scheduled.length === 3` を確認していました。修正後は最初の `onerror` で接続を切り離して 2・3 回目を無視するため、`scheduler.scheduled.length === 1`・有効予約 1 件・`onError` 呼び出し 1 回・接続数 2 本・待機時間 1 秒のままとなります。このテストを新しい契約に合わせて更新します。
- e2e テスト（`e2e/tests/issue-383-half-open-sse.spec.ts`）では Playwright の `page.clock` で時間を進め、40 秒の実待機なしに半開き検知 → 切断バナーとボタン無効化（PR #381/#378 の維持）→ 再接続とボタン再有効化を検証します。

### 変えないこと・範囲の外

- backend の `WATCH_RESEND_INTERVAL`（15 秒）、`/api/state` Route Handler、proto 定義は変えません。
- 見捨てられた半開き接続のサーバー側の負荷・掃除は別件 [j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385) で扱います。
- 猶予の見張り（期限超過後の追加待機）は入れません。
- 再接続のバックオフ仕様（1 秒から倍々、上限 30 秒）や、`online` イベント時の再接続待機短縮は入れません。
- 切断中の表示文面・バナー配置・判断待ちボタンの無効化ロジック（PR #381 / #378）は変えません。
- 待ち受けアドレス（`0.0.0.0:3000`）や接続元制限は一切触りません。
- SSE を使わない 1 秒ポーリングの詳細画面（`frontend/web/src/components/polling.ts`）は範囲外です。

## 守る不変条件

1. **見張り時間を過ぎたら必ず切断になる（スリープを挟んでも成り立つ）**：開いている接続で、接続開始または最後の有効な受信から壁時計基準で `staleTimeoutMs`（40 秒）以上何も届かなければ、見張りタイマーまたは復帰時の `checkStale()` により必ず `close()` して `onError()` を呼び、切断状態へ移って再接続を予約する。期限未満の部分スリープで復帰した場合も残り時間で見張りを張り直すため、壁時計が期限に達した時点で切断になる。
2. **開き直しは同時に 1 本だけ**：未クローズの `EventSource` は常に 1 本以下、有効な再接続予約は常に 1 本以下、さらに「接続が開いている間は再接続予約が 0 本」を保つ。
3. **古い接続のイベントで状態が戻らない・壊れない**：切断後・再接続後・`stream.close()` 後に古い接続から遅延して届いた `onmessage`・`onerror`・見張り発火はすべて無視し、`disconnected` の復帰・状態の上書き・連続失敗回数のリセット・現行接続の見張りリセット・二重再接続を起こさない。
4. **期限より前に見張りで切らない**：最後の有効な受信（または接続開始）からの経過時間が `staleTimeoutMs` 未満の間は、見張りタイマーの早期発火や `checkStale()` で切断しない（残り時間で張り直す）。
5. **見張りタイマーは現在の接続 1 本だけに紐づく**：有効な見張りタイマーは最大 1 本であり、`source === undefined` または `closed === true` のときは 0 本。
6. **`stream.close()` 後は接続もタイマーも残らない**：`close()` 呼び出し後は、開いている接続 0 本、有効な再接続予約 0 本、有効な見張りタイマー 0 本、かつ以後のどのイベントもコールバックを発火しない。

## PBT の性質

`frontend/web` には fast-check 等が入っていないため、決定的な PRNG（シード固定）でランダムな操作列を大量生成する状態機械 PBT を `frontend/web/test/components/state-stream.test.ts` に実装します。除外はつけません。

- **偽の時計は 2 本**：タイマー用の単調時計 `monoNow` と、`now()` 用の壁時計 `wallNow` を独立して持ちます。
- **入力空間（除外なし）**：
  - 乱数シード 200 系列 × 各 60 手（計 12,000 遷移）。
  - 各手でランダムに選ぶ操作：
    1. **両方進める**（`monoNow` と `wallNow` を同時に進め、期限に達したタイマーを時刻順に発火）
    2. **壁時計だけ進める**（OS スリープを模擬：`wallNow` のみ進め `monoNow` は止めたまま）
    3. **壁時計を戻す**（NTP 逆行を模擬：`wallNow` のみ戻す）
    4. **見張りを任意に発火**（期限前でも見張りタイマーを強制発火。発火時に `now() - lastActivityAt` を検証して残り時間で張り直すため、除外なしで入れられる）
    5. 現在接続での有効メッセージ受信・不正ペイロード受信・`onerror`
    6. 過去の任意接続 ID への遅延 `onmessage`・遅延 `onerror`
    7. 再接続タイマーの発火
    8. `checkStale()` 呼び出し
    9. `stream.close()`（閉じた後も遅延イベントやタイマー発火を継続試行）
  - 境界値（`staleTimeoutMs - 1` ms、ちょうど `staleTimeoutMs` ms、初回無音、部分スリープ、コールバック内 `close()`、同期スケジューラ、不正 `staleTimeoutMs`）は個別テストで全パターンを固定します。
- **全ステップで検証する性質**：
  1. 未クローズの `FakeEventSource` 数は常に `<= 1`。
  2. 有効（未取消・未発火）な再接続タイマー数は常に `<= 1`、かつ未クローズ接続がある間は `0`。
  3. 有効な見張りタイマー数は未クローズ接続があるとき `1`、無いとき `0`。
  4. `closed` 後、または現在接続以外の古い接続のイベントで `onState`・`onError` が一度も呼ばれない。接続中（`disconnected === false`）なら現在接続が存在し、再接続後の接続は自身で 1 回以上受信している。
  5. `0 <= wallNow - lastActivityAt < staleTimeoutMs` の間に見張りタイマー発火や `checkStale()` が起きても切断せず、残り時間 `staleTimeoutMs - (wallNow - lastActivityAt)` で見張りタイマーを張り直す。`wallNow < lastActivityAt` のときは誤切断せず `lastActivityAt` を `wallNow` に詰めて `staleTimeoutMs` で張り直す。
  6. 開いている接続が `wallNow - lastActivityAt >= staleTimeoutMs` に達した状態で、見張りタイマー発火または `checkStale()` が実行されたとき（あるいは `wallNow` の逆行なしに両方進めて期限に達したとき）は、必ず `disconnected === true` になり、その接続が閉じられ、再接続タイマーが 1 本予約される。
  7. `stream.close()` 後は未クローズ接続・有効見張りタイマー・有効再接続タイマーがすべて `0`、かつ以後の `onState`・`onError`・新規接続作成が `0`。

## 状態遷移と Quint

`specs/quint/state-stream-watchdog/` に Quint の仕様とテストを置いています。

- `specs/quint/state-stream-watchdog/stateStreamWatchdog.qnt`：単調時計（`monoNow`）と壁時計（`wallNow`）の 2 本の時計を持つ状態機械、`deliverMessage(id)`・`deliverError(id)` の一本化、実状態のみで記述した 10 の安全不変条件（`safety`。開いている現在の接続には必ず見張りが付く `activeConnHasWatchdog` を PR のレビューで足した）、8 つの到達条件（見張りタイマーで切って戻る `witnessWatchdogTimerRecover` と、復帰時確認で切って戻る `witnessWakeCheckRecover` を含む）。不変条件 4 は、切断のたびに残す原因（`lastDisconnectCause`）と切った時点の壁時計の経過（`lastDisconnectGap`）で書く
- `specs/quint/state-stream-watchdog/stateStreamWatchdog_test.qnt`：17 件の筋書きテスト
- `specs/quint/state-stream-watchdog/README.md`：モデルの範囲・前提・変異の反例・実行結果

検査結果（quint 0.32.0）：

- `quint typecheck`：`stateStreamWatchdog.qnt`・`stateStreamWatchdog_test.qnt` ともにエラーなし。
- `quint test stateStreamWatchdog_test.qnt --main=stateStreamWatchdog_test`：17 件すべて成功（992ms）。
- `quint run --main=current --invariant=silentConnectionMustDisconnect --max-steps=25 --max-samples=10000 --seed=0x383`：4 手で違反（修正前の半開き未検知を再現）。
- `quint run --main=watchdogOnly --invariant=safety --max-steps=25 --max-samples=10000 --seed=0x383`：8 手で違反（古い接続の遅延受信で `disconnected` が `false` に戻り `connectedImpliesValidActiveConn` 等を破る反例）。
- `quint run --main=noWakeRearm --invariant=silentConnectionMustDisconnect --max-steps=25 --max-samples=10000 --seed=0x383`：6 手で違反（部分スリープ後に残り時間で張り直さないと期限超過する反例）。
- `quint run --main=earlyWakeCutoff --invariant=safety --max-steps=25 --max-samples=10000 --seed=0x383`：4 手で違反（`checkStale` が 25 秒で切断し、`lastDisconnectGap = 5 < 8` で不変条件 4 を破る反例）。
- `quint run --main=fixed --invariant=safety --max-steps=40 --max-samples=20000 --seed=0x383`：20,000 サンプルすべて反例なし、8 つの witness すべてが到達（`witnessWatchdogTimerRecover` 2 件、`witnessWakeCheckRecover` 276 件）。
- `quint run --main=fixedOpenSession --invariant=safety --max-steps=60 --max-samples=20000 --seed=0x383`：20,000 サンプルすべて反例なし、`witnessWatchdogDisconnect` 662 件（3.31%）、`witnessWatchdogTimerRecover` 36 件（0.18%）、`witnessWakeCheckRecover` 2,985 件（14.92%）。見張りタイマーで切って戻る経路は薄いため、筋書きテストでも決まった手順で通す。詳細は `specs/quint/state-stream-watchdog/README.md`。

## 壊しうる入力の一覧

| 分類 | 具体的な入力・状況 | 期待する振る舞い |
| --- | --- | --- |
| 境界 | 最後の受信から `staleTimeoutMs - 1` ms（39,999ms）経過時点で受信 | 切断せず見張りを 40,000ms で張り直す |
| 境界 | 最後の受信からちょうど `staleTimeoutMs` ms（40,000ms）経過 | 切断として扱い、接続を閉じて再接続を 1 本予約 |
| 境界 | `staleTimeoutMs` に `0`・負数・`NaN`・`Infinity`・`2_147_483_648` 以上を渡す | `RangeError` を投げて即座に拒否する |
| 境界 | 接続直後に 1 件も受信しないまま 40 秒経過（初回ハング／再接続ハング） | 切断として扱い、接続を閉じて次の再接続を予約する |
| 並行 | 見張り切断の直後（再接続待機中）に古い接続の `onmessage`／`onerror` が届く | 無視する。`disconnected` を `false` に戻さず、再接続予約も増やさない |
| 並行 | 再接続で新しい接続が開いた後に古い接続の `onmessage`／`onerror` が届く | 無視する。新しい接続の見張りや状態を壊さず、二重接続を作らない |
| 並行 | 同一接続で `onerror` が複数回発火する、または `close()` 内で `onerror` が再入する | 最初の 1 回だけ処理し、2 回目以降は無視する |
| 並行 | `onState` または `onError` のコールバック内で同期的に `stream.close()` を呼ぶ | 閉じた後に見張りや再接続を予約せず、タイマー・接続の漏れを残さない |
| 並行 | テスト用の同期スケジューラが `scheduleWatchdog`／`scheduleReconnect` 内で即時実行する | 既に完了した取消関数で新しい予約を上書きしない |
| 並行 | 見張りタイマーが期限より早く発火する（タイマー揺らぎ等） | `now() - lastActivityAt < staleTimeoutMs` を確認し、切断せず残り時間で張り直す |
| 再開 | OS スリープ（60 秒以上、TCP が切れていても生きていても）から復帰し `visibilitychange`／`online`／`focus` が発火 | `checkStale()` が `now() - lastActivityAt >= 40_000` を検知し即座に切断・再接続予約。旧接続の遅延受信は捨てる |
| 再開 | 期限未満の部分スリープ（例：25 秒、単調時計停止）から復帰し `visibilitychange`／`focus` が発火 | 切断せず、残り時間（15 秒）で見張りタイマーを張り直す |
| 再開 | NTP 等で壁時計 `now()` が `lastActivityAt` より前へ逆行した状態で `checkStale()` や見張りが発火 | 誤切断せず `lastActivityAt` を現在時刻へ詰めて `staleTimeoutMs` で見張りを張り直す |
| 区切り・入れ子・長さ・Unicode | 不正 JSON、必須項目欠落、Unicode や長い文字列を含む正常な `DashboardState` JSON | 不正ペイロードは既存のとおり検証で弾き見張り・`lastActivityAt` を更新しない。有効な JSON は従来どおり復元して見張りを更新 |

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- **上流（takt、takt-mcp）**：影響なし。
- **保存済みデータ**：影響なし。
- **公開 API（proto、`/api/*`）**：影響なし。変更は画面内部の `openStateStream`（省略可能なオプションと `checkStale` メソッドの追加）と `StateStreamProvider` のみ。なお、画面側が半開き接続を閉じて開き直す際に見捨てられた旧接続がサーバー側（Next.js Route Handler / backend の `WatchState`）に残る負荷の問題は、別件 [j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385) に切り出して追跡します。
- **守りの仕組み（CI、関門、ルールセット）**：影響なし。

## 設計レビューを受けて変えた点

| # | 重要度 | 扱い | 変更内容 |
| --- | --- | --- | --- |
| 1 | 中 | 直す | Quint の `deliverMessage(id)`・`deliverError(id)` を各 1 本にまとめ、実物と同じ `closed or id != activeConn` 判定へ統一。印フラグ（`staleStateApplied`・`staleErrorApplied`）を廃止し、不変条件を実状態（`connectedImpliesValidActiveConn`・`appliedStateMatchesActiveWhenConnected` 等）で記述。`watchdogOnly` 変異で `safety` が破れることを `README.md` に記録。 |
| 2 | 中 | 直す | Quint で健全な `Open` 接続が期限を越えて黙る経路と、スリープ後も TCP が生きている経路を許可し、接続 1 本・遅延受信の破棄・次の接続での復帰を検証。猶予の見張りは入れず一瞬の切断を受け入れる方針を設計へ明記。不変条件 4 を「期限より前に見張りで切らない」に狭め、前提を `README.md` に記載。 |
| 3 | 中 | 直す | 見張り発火時と `checkStale()` の両方で `now() - lastActivityAt` を確認し、期限未満なら残り時間で見張りを張り直す設計へ変更。Quint を単調時計（`monoNow`）と壁時計（`wallNow`）の 2 本に分け、`wakeFromSleep` は壁時計だけを進める形にし、`noWakeRearm` 変異の反例と `fixed` の無違反でスリープを挟んでも不変条件 1 が成り立つことを検証。 |
| 4 | 中 | 直す | PBT の偽の時計を単調時計と壁時計の 2 本にし、「両方進める」「壁時計だけ進める」「壁時計を戻す」「見張りを任意に発火」を除外なしで導入。性質 5・6 を 2 本の時計と残り時間張り直しに合わせて書き直し。 |
| 5 | 低 | 直す | 見張り時間 40 秒の根拠を「間隔の上限は 15 秒＋組み立て時間なので、組み立て等が 25 秒遅れても誤検知しない」に書き直し、バックグラウンドタブの間引きと凍結（freeze）復帰時の挙動を追記。 |
| 6 | 低 | 別件 | 見捨てられた接続のサーバー側負荷は [j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385) に切り出し、本設計の「範囲の外」と「影響」にリンクを追記。 |
| 7 | 低 | 直す | 既存単体テストの `open` ヘルパの既定を偽の見張りスケジューラにして本物の 40 秒タイマーを残さない方針を明記。e2e は Playwright の `page.clock` で時間を進める方針を明記。 |
| 8 | 低 | 直す | Quint の witness に「見張りで切断 → 開き直し → 受信で戻る」の一続き（`witnessWatchdogRecover`）を追加し、見張り経路を厚く通る `fixedOpenSession` の実行結果を `README.md` に追加。 |

## 再レビュー（2 回目）を受けて変えた点

| # | 重要度 | 扱い | 変更内容 |
| --- | --- | --- | --- |
| 1 | 中 | 直す | 見張りタイマーでも `checkStale` でも、切るたびに原因（`lastDisconnectCause`）と切った時点の壁時計の経過（`lastDisconnectGap`）を状態に残し、不変条件 4 を「原因が見張りか復帰確認なら `lastDisconnectGap >= STALE_TIMEOUT`」と書き直した。`checkStale` が 10 秒で切る変異 `earlyWakeCutoff` で `safety` が破れることを `README.md` に記録。 |
| 2 | 低 | 直す | witness を `witnessWatchdogTimerRecover` と `witnessWakeCheckRecover` に分け、`README.md` の書き方を実行結果の数に合わせた。 |
| 3 | 低 | 直す | §1 の括弧書きを消した。`README.md` の前提に、壁時計が前へ飛ぶ場合は `wakeFromSleep` でしか表していないことを足した。 |
