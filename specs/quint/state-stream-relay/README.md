# 状態の配信の中継が、見捨てられた接続を溜めないこと（issue #385）

`frontend/web/app/api/state/route.ts`（backend の `WatchState` を SSE に中継する）が、画面に見捨てられた接続のサーバー側の資源（route.ts の `ReadableStream`、gRPC のストリーム、backend の `WatchState` のタスク）を長く溜めないことを Quint で確かめた記録です。設計は `docs/design/issue-385.md`。

仕様とコードは機械的にはつながっていません。コードの挙動は `frontend/web/test/api/state-route.test.ts`、`frontend/web/test/server/state-relay.test.ts`（状態機械の PBT）、`frontend/web/test/components/state-stream.test.ts`、`frontend/web/test/components/live-provider-wiring.test.ts`、`e2e/tests/issue-385-abandoned-sse.spec.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `stateStreamRelay.qnt` | 画面 2 つ・接続 5 本の状態機械、10 の安全不変条件（`safety`）、7 つの到達条件（witnesses） |
| `stateStreamRelay_test.qnt` | issue #385 の筋書きを 5 構成でたどる 15 件のテスト |

遷移は 4 つの定数で切り替えます。

| モジュール | `SUPERSEDE` | `REJECT_STALE` | `CONFLATE` | `FINISH_ONCE` | 表すもの |
| --- | --- | --- | --- | --- | --- |
| `current` | false | false | false | false | 修正前の main（`d740c80`） |
| `noRejectStale` | true | false | true | true | 付け替えはするが、遅れて届いた古い試行を開いてしまう変異 |
| `noConflate` | true | true | false | true | 付け替えだけで、詰まった接続の列へ積み続ける変異 |
| `noFinishOnce` | true | true | true | false | `cancel` が後始末の守りを通らない変異 |
| `fixed` | true | true | true | true | 修正案 |

## モデルの範囲と前提

- 画面は 2 つ（`SCREENS`）、作る接続は合わせて 5 本まで（`MAX_CONNS`）、backend が 1 本の接続へ送る状態は 3 件まで（`MAX_EMITS`）。接続を使い切ると遷移が止まる（終わりのある模型）。
- 経路（`pathUp`）は切れたら戻りません。戻った経路は、画面が開き直した新しい接続で表します（画面は半開きを見張りで検知すると古い接続を閉じて開き直す。issue #383）。
- 経路が切れている接続には、画面が閉じたこと（FIN）も状態も届きません。`abortArrives` は経路が生きているときだけ、`nextPulls`（Next が列から読んで書く）も経路が生きているときだけ起きます。
- OS が TCP をあきらめて閉じる（`tcpTimeout`）のは、経路が切れた接続でいつか起きる遷移として表しています。時間は扱いません。
- `ReadableStream` の列の上限は既定の `highWaterMark = 1`（`HIGH_WATER_MARK`）です。Next 16.4.6 は書けないと `drain` を待ち、その間は列から読みません（`node_modules/next/dist/server/pipe-readable.js` の `createWriterFromResponse`）。
- `nextCancels` は Next が応答の本体を `cancel` する遷移です。実物は、応答が閉じたとき（`res` の `close`）に `pipeTo` の打ち切りで呼ばれます。模型では、閉じたあとの接続と、画面が閉じた接続のどちらでも 1 回だけ起こせるようにして、実物より広く探します。
- 画面の識別子（`screen`）を持たない接続（古い画面の JavaScript、curl）は模型に入れていません。実装では付け替えの対象にせず、修正前と同じに扱います（単体テストで確かめる）。
- 同じ試行の番号の要求が 2 本届く場合（ブラウザの HTTP の再送）は模型に入れていません。実装は「後から届いたほうを残す」で、取り違えたら画面の見張りが開き直して戻ります（設計の「壊しうる入力」）。

## 条件

不変条件（安全性）

- `noDoubleRelease`：同じ接続の資源を二度閉じない。
- `atMostOneOpenPerScreen`：画面ごとに、サーバーで開いている接続は 1 本まで。
- `closedOnNextArrival`：同じ画面の次の接続がサーバーに届いて開いたら、それより前に届いた接続は閉じている。見捨てられた接続は、画面が開き直した接続が届いた時点で閉じる。
- `staleNeverOpensBesideNewer`：同じ画面のもっと新しい試行がサーバーで開いている間は、遅れて届いた古い試行を開かない。
- `liveNotClosedByRelay`：画面がいま使っている接続を、サーバーの都合（付け替え、断り、abort、cancel）で閉じない。閉じてよいのは backend の終了と、経路が切れて OS が閉じた場合だけ。
- `registryPointsToOpen`：画面ごとの登録は、開いている同じ画面の接続だけを指す（閉じた接続の登録が残らない）。
- `backendTaskOnlyWhileOpen`：backend の購読は、開いている接続の分だけ生きている。
- `queueBounded`：`ReadableStream` の列は `highWaterMark` を超えて積まない。
- `deliveredInOrder`：古い版を新しい版の後に送らない。
- `latestNeverDropped`：開いている接続では、backend から受けた最新の版を捨てない（送ったか、列にあるか、持っている）。
- `safety`：上の 10 をすべて満たす。

進行の条件（`--witnesses` で数える）

- `witnessSuperseded`：開き直した接続が届いて、古い接続を閉じる。
- `witnessRejected`：遅れて届いた古い試行を断る。
- `witnessSkipped`：詰まった間の途中の版を飛ばし、最新を送り終える。
- `witnessTcpTimeout`：経路が切れた接続を OS が閉じる。
- `witnessAbortThenCancel`：abort のあとに Next の cancel が来る。
- `witnessAbandonedHalfOpenSuperseded`：半開きの接続を付け替えで閉じる。
- `witnessLiveDelivers`：いま使っている接続が 2 件以上送る。

## 結果

quint 0.32.0 で実行しました。

```sh
cd specs/quint/state-stream-relay
quint typecheck stateStreamRelay.qnt
quint typecheck stateStreamRelay_test.qnt
quint test stateStreamRelay_test.qnt --main=stateStreamRelay_test
```

- `quint typecheck`：2 ファイルともエラーなし。
- `quint test`：15 件すべて成功。

修正前と変異は `safety` に違反します（`--max-steps=30 --max-samples=10000 --seed=0x385`）。不変条件を 1 つずつ渡して違反したもの:

| モジュール | 違反する不変条件 |
| --- | --- |
| `current` | `noDoubleRelease`、`atMostOneOpenPerScreen`、`closedOnNextArrival`、`staleNeverOpensBesideNewer`、`queueBounded` |
| `noRejectStale` | `liveNotClosedByRelay` |
| `noConflate` | `queueBounded` |
| `noFinishOnce` | `noDoubleRelease` |

修正案は反例なし:

```sh
quint run stateStreamRelay.qnt --main=fixed --invariant=safety --max-steps=40 --max-samples=20000 --seed=0x385 \
  --witnesses witnessSuperseded witnessRejected witnessSkipped witnessTcpTimeout witnessAbortThenCancel \
  witnessAbandonedHalfOpenSuperseded witnessLiveDelivers
```

20,000 サンプルで反例なし。到達条件はすべて到達しました（`witnessSuperseded` 1,429 件、`witnessRejected` 963 件、`witnessSkipped` 15 件、`witnessTcpTimeout` 2,085 件、`witnessAbortThenCancel` 3,129 件、`witnessAbandonedHalfOpenSuperseded` 234 件、`witnessLiveDelivers` 9 件）。`witnessSkipped` と `witnessLiveDelivers` は薄いため、`conflateKeepsLatestTest` と `flowingDeliversEveryVersionTest` で決まった手順でも通します。

## 仕様を直すとき

route.ts の後始末、画面ごとの付け替え、列の間引きのどれかを変えるときは、先にこの仕様を直して `quint test` と `quint run` を流し、そのあとでコードを変えます。
