# issue #385 の設計：見捨てられた状態の配信の接続を、同じ画面の次の接続が届いた時点で閉じ、詰まった接続には最新の 1 件だけを持つ

対象の issue：[j5ik2o/takt-dash#385](https://github.com/j5ik2o/takt-dash/issues/385)
関連：[j5ik2o/takt-dash#383](https://github.com/j5ik2o/takt-dash/issues/383)、[j5ik2o/takt-dash#386](https://github.com/j5ik2o/takt-dash/pull/386)（画面側の見張りと開き直し。main に入っている）

## 現在の main での確認（`d740c80`）

コードを読んで、issue の現象が起きる作りであることを確かめました（実機の VPN 切断での再現はしていません）。

1. **画面は、閉じたことをサーバーに伝えられないまま開き直す。** `frontend/web/src/components/state-stream.ts` の `disconnect` は、見張りで半開きを検知すると `EventSource.close()` して 1 秒後に新しい接続を開く。経路が切れているので、閉じたこと（FIN）はサーバーへ届かない。
2. **route.ts は、届かない限り古い接続を閉じない。** `frontend/web/app/api/state/route.ts` が閉じるのは `request.signal` の abort（Next は `res` の `close` で立てる。`node_modules/next/dist/server/web/spec-extension/adapters/next-request.js` の `createAbortController`）、gRPC の `error`・`end`、`cancel` だけ。半開きの接続ではどれも OS が TCP をあきらめるまで（Linux の既定で十数分）起きない。
3. **その間、backend の `WatchState` のタスクは動き続ける。** `backend/src/grpc/mod.rs` の `watch_state` は 15 秒ごと（`WATCH_RESEND_INTERVAL`）と変化の通知のたびに `assemble_state_for` を呼び、送る。route.ts は gRPC を `on("data")` で流しっぱなしに受けるので、backend 側に背圧はかからない。タスクが止まるのは `sender.closed()`、つまり route.ts が gRPC を閉じたときだけ。
4. **route.ts は、Next が読めない間も列へ積み続ける。** Next 16.4.6 は書けないと `drain` を待ち、その間は `ReadableStream` から読まない（`node_modules/next/dist/server/pipe-readable.js` の `createWriterFromResponse`）。route.ts の `controller.enqueue` は `desiredSize` を見ないので、列が伸びる。
5. **同じ接続の資源を二度閉じる経路がある。** `cancel()` は `finished` を見ずに `stream.cancel()`・`stream.destroy()` を呼ぶ。列に残りがある接続で abort のあとに Next の `cancel` が来ると、二度目が走る。grpc-js は二度目を無視するので害は出ていないが、後始末が 2 本ある。

画面が開き直すたびに 1 本ずつ残るので、VPN が何度も切れると、画面 1 つにつき数本の接続と backend のタスクが溜まる（issue の現象）。

## 何をどう変えるか

### 案の比較と決定

| 案 | 見捨てられた接続を閉じるか | 動いている接続を閉じないか | 決定 |
| --- | --- | --- | --- |
| A. `desiredSize` を見て、詰まったら閉じる | 閉じるのは OS の送信バッファ（数百 KB〜数 MB）と Node のバッファが埋まってから。状態 1 件の大きさと 15 秒の間隔で決まり、こちらで上限を決められない。TCP の打ち切りより遅いこともある | 遅いが動いている接続も、詰まれば閉じてしまう | **閉じる用途には採らない。** `desiredSize` は下の C（間引き）に使う |
| B. `WatchState` の組み立てを購読者の間で共有する | 閉じない。残った接続の CPU の費用が減るだけで、接続と gRPC と route.ts の資源は残る | 影響なし | **採らない。** issue の問題（溜まること）を解かない。backend の配信の作りと、組み立ての時刻（`now`）の意味を変える。購読者が多くて組み立てが重いと分かったら別件にする |
| C. 詰まった接続には最新の 1 件だけを持つ（`desiredSize` の間引き） | 閉じない。ただし詰まった接続の列が伸びなくなる | 閉じない。状態は毎回すべてを送るので、途中の版を飛ばしても画面は最新になる | **採る** |
| D. 画面ごとの識別子と試行の番号を URL に載せ、同じ画面の新しい接続が届いたら古い接続を閉じる | 閉じる。開き直した接続がサーバーに届いた時点で、OS を待たずに閉じる | 閉じない。閉じるのは同じ画面の古い試行だけ。画面は最新の試行しか使わない | **採る** |

D で「開き直しのたびに 1 本ずつ溜まる」を解き、C で「残っている間に列が伸びる」を解きます。D で閉じられないのは、経路が切れている間に画面（タブ）ごと閉じた接続だけです。開き直しが来ないので OS が閉じるまで残ります。残るのは画面 1 つにつき 1 本で、C により列も伸びません。これは #383 の修正の前と同じ数です。

### 1. 画面：試行ごとに URL に識別子と番号を載せる（`frontend/web/src/components/state-stream.ts`）

- `openStateStream` が、開くたびに `createEventSource` へ渡す URL を `${url}?screen=<識別子>&attempt=<番号>` にする（`url` に `?` があれば `&` で足す）。
  - 識別子は `openStateStream` 1 回につき 1 つ。`crypto.getRandomValues` の 16 バイトを 16 進 32 文字にする。`crypto.randomUUID` は使わない（LAN の `http://` は安全なコンテキストではなく、使えない）。
  - 番号は 1 から始め、`connect()` のたびに 1 増やす。
  - テスト用に省略可能な `screenId?: string` を足す（既定は上の乱数）。
- 見張り、再接続の予約、`close`、`checkStale` の動き（#386）は変えない。

### 2. サーバー：画面ごとの登録（新規 `frontend/web/src/server/screen-streams.ts`）

- `parseStreamTicket(searchParams)`：`screen` が `^[0-9a-f]{32}$`、`attempt` が `^[1-9][0-9]{0,15}$` で `Number.isSafeInteger` のときだけ `{ screen, attempt }` を返す。どちらかが無い・形が違うときは `undefined`（付け替えの対象にしない。修正前と同じ扱い）。
- `createScreenStreams()` が返す登録簿（route.ts はモジュールに 1 つ持つ）：
  - `admit(ticket, close)`：同じ画面の登録が無ければ登録して `{ release }` を返す。登録があり、その番号が新しい番号**より大きい**ときは `undefined`（断る）。それ以外（新しい番号が同じか大きい）は、新しい接続を登録してから古い接続の `close` を呼び、`{ release }` を返す。
  - `release()`：登録がまだ自分を指しているときだけ外す。別の接続に付け替わったあとは何もしない。
  - 登録は開いている接続の分だけ。閉じたら外すので、画面を開くたびに増え続けない。

### 3. サーバー：中継の後始末と間引き（新規 `frontend/web/src/server/state-relay.ts`、`frontend/web/app/api/state/route.ts`）

route.ts は Next の決まりで HTTP の関数しか書き出せないので、中継の組み立てを `state-relay.ts` に移し、route.ts は呼ぶだけにする。

- `GET`：
  1. `parseStreamTicket(request.nextUrl.searchParams)`。
  2. 識別子があって `admit` に断られたら、gRPC を開かずに `204 No Content` を返す（EventSource は 204 で再接続しない。この要求の EventSource は画面がもう閉じている）。
  3. それ以外は gRPC の `watchState()` を開き、`relayStateStream` で `ReadableStream` を作って返す。`admit` の `close` には中継の後始末を渡す。
- 後始末は 1 本（`finish`）。abort、gRPC の `error`・`end`、`ReadableStream` の `cancel`、付け替え（`close`）のどれから来ても同じ関数を通り、2 回目以降は何もしない。中で `release()`、abort の聞き手を外す、`stream.cancel()`・`stream.destroy()`、`controller.close()`（`end` のときは gRPC がもう終わっているので `cancel` しない）。
- `ReadableStream` は `new CountQueuingStrategy({ highWaterMark: 1 })` を明示して作る（既定の値に頼らない）。不変条件 8 と PBT の性質 8 は、この値を前提にする。
- 間引き：gRPC から状態が来たら、`controller.desiredSize > 0` なら持ち分を捨ててから符号化して `enqueue`（持ち分はこの状態より古い。捨てないと、あとの `pull` が古い版を新しい版の後に送る）。そうでなければ `pending` に最新の状態だけを持つ（前の `pending` は捨てる）。`ReadableStream` の `pull` で `pending` があれば符号化して `enqueue` し、空にする。後始末で `pending` を捨てる。
- 付け替えで閉じた接続には `controller.close()` で終わりを伝える。Next がまだ `drain` を待っているあいだ、Node の応答とソケットは OS が閉じるまで残るが、それ以上は書かない。gRPC と backend のタスクはその場で閉じる。

### 変えないこと・範囲の外

- backend（`backend/src/grpc/mod.rs`）、`WATCH_RESEND_INTERVAL`、proto は変えない。backend のタスクは、route.ts が gRPC を閉じれば `sender.closed()` で止まる（既存の作り）。
- 画面の見張り時間（40 秒）、再接続のバックオフ、切断の表示、判断待ちのボタンの無効化（#378、#381、#386）は変えない。
- 経路が切れている間にタブごと閉じた接続を、OS より早く閉じることはしない（上の比較の A を参照）。
- 待ち受けアドレス（`0.0.0.0:3000`）と接続元の制限には触らない。
- 登録簿は Node のプロセス 1 つの中にある。付け替え（D）が効くのは、画面の配信が 1 つの Next のプロセスで動いている間だけ（いまの本番は `next start` の 1 プロセス）。複数のプロセスや複数台に分けると、同じ画面の古い接続を閉じられなくなる（修正前と同じ動きに戻る。間引きと後始末 1 本は効く）。プロセスをまたぐ登録は入れない。
- 識別子は認証ではない。他人の識別子を当てて接続を閉じさせることは、16 バイトの乱数を推測できない限りできない。画面は LAN・VPN の内側で使う前提で、認証は元から無い。

## 守る不変条件

Quint の不変条件（`specs/quint/state-stream-relay/stateStreamRelay.qnt`）と同じものです。

1. **同じ接続の資源を二度閉じない**（`noDoubleRelease`）。
2. **画面ごとに、サーバーで開いている接続は 1 本まで**（`atMostOneOpenPerScreen`）。
3. **見捨てられた接続は、画面が開き直した接続がサーバーに届いた時点で閉じる**（`closedOnNextArrival`）。届かないまま（タブを閉じた）なら、OS が TCP をあきらめた時点で閉じる。
4. **新しい試行が開いている間は、遅れて届いた古い試行を開かない**（`staleNeverOpensBesideNewer`、`staleArrivalRejected`）。
5. **動いている接続（画面がいま使っている接続）を閉じない**（`liveNotClosedByRelay`）。閉じてよいのは backend の終了と、経路が切れて OS が閉じた場合だけ。
6. **閉じた接続の登録が残らない**（`registryPointsToOpen`）。
7. **backend の購読は、開いている接続の分だけ**（`backendTaskOnlyWhileOpen`）。
8. **`ReadableStream` の列は `highWaterMark`（1 件）を超えない**（`queueBounded`）。`highWaterMark: 1` を明示して作ることが前提。
9. **古い版を新しい版の後に送らない**（`deliveredInOrder`）。
10. **開いている接続では、最新の版を捨てない**（`latestNeverDropped`）。

## PBT の性質

`frontend/web` には fast-check が無いので、#383 と同じく、シード固定の PRNG で操作の列を作る状態機械の PBT にします。除外は付けません。

### `frontend/web/test/server/state-relay.test.ts`（新規）

偽の gRPC ストリーム（`data`・`error`・`end` を起こせ、`cancel`・`destroy` の回数を数える）と、本物の `ReadableStream`、本物の `AbortController`、本物の登録簿で、`relayStateStream` を直接動かします。

- 入力：シード 200 系列 × 各 60 手。画面 2 つ、各画面で試行の番号を任意の順で開く（番号の重複、逆順、識別子なしを含む）。各手で、ある接続に `data`・`error`・`end`・abort・読む（`reader.read()`）・`reader.cancel()` のどれかを起こす。読まない接続も残す（詰まった接続）。
- 全手で確かめる性質：
  1. どの接続も、gRPC の `cancel` は 1 回まで、`destroy` は 1 回まで（不変条件 1）。
  2. 画面ごとに、閉じていない中継は 1 本まで（不変条件 2）。
  3. 同じ画面の番号の大きい接続が開いたら、それより前に開いた同じ画面の接続は閉じている（不変条件 3）。
  4. 断られた接続は gRPC を開かない（不変条件 4）。
  5. その画面で最も大きい番号の接続（同じ番号なら後に開いたもの）は、自分の `error`・`end`・abort・`cancel` 以外では閉じない（不変条件 5）。
  6. 登録簿は、閉じていない接続だけを指す（不変条件 6）。
  7. 閉じた接続は gRPC を閉じている。本文は、残りを読み切ると終わり、待っている読みも終わる（不変条件 7）。
  8. 読まない接続の内部の列は 1 件、持ち分は 1 件を超えない（不変条件 8。`highWaterMark: 1` を前提に、`desiredSize >= 0` で確かめる）。
  9. 読んだ版は増える一方（不変条件 9）。
  10. 開いている接続で読み続ければ、最後に読む版は gRPC から最後に来た版（不変条件 10）。
- 境界は個別のテストで固定する：番号が同じ、1 小さい、`Number.MAX_SAFE_INTEGER`、識別子なし、識別子の形が違う（大文字、31・33 文字、Unicode、`%00`）、番号が `0`・負・小数・`1e3`・17 桁。

### `frontend/web/test/components/state-stream.test.ts`（足す）

#383 の PBT の操作列に、開いた接続の URL を確かめる性質を足します：同じ `openStateStream` の接続はすべて同じ識別子を持ち、番号は開いた順に 1 ずつ増える。別の `openStateStream` は別の識別子を持つ。

## 状態遷移と Quint

`specs/quint/state-stream-relay/` に仕様とテストを置きました（`README.md` に範囲・前提・結果）。

- `stateStreamRelay.qnt`：画面 2 つ・接続 5 本。経路の切断、画面の開き直しとタブの閉じ、要求の到着と付け替え・断り、abort・backend の終了・OS の打ち切り・Next の cancel、backend からの状態と Next の読み、を遷移にした状態機械。不変条件 11（`safety`）、到達条件 7。
- `stateStreamRelay_test.qnt`：筋書き 19 件。修正前の 3 つの現象（2 本残る、列が伸びる、二度閉じる）、修正案の付け替え・断り・間引き・後始末 1 本、変異 3 つの反例、タブを閉じた接続が OS を待つ残り。

検査結果（quint 0.32.0）：

- `quint typecheck`：2 ファイルともエラーなし。
- `quint test stateStreamRelay_test.qnt --main=stateStreamRelay_test`：19 件すべて成功。
- 修正前（`current`）は `noDoubleRelease`・`atMostOneOpenPerScreen`・`closedOnNextArrival`・`staleNeverOpensBesideNewer`・`queueBounded` に違反。変異は、`noRejectStale` が `staleArrivalRejected`・`liveNotClosedByRelay`、`noConflate` が `queueBounded`、`noFinishOnce` が `noDoubleRelease` に違反（各 `--max-steps=30 --max-samples=10000 --seed=0x385`）。`keepStalePending` は `deliveredInOrder` に違反（`--max-samples=100000`）。
- 修正案（`fixed`）：`--invariant=safety --max-steps=40 --max-samples=20000 --seed=0x385` で反例なし。到達条件 7 つすべて到達。

## 壊しうる入力の一覧

| 分類 | 入力・状況 | 期待する振る舞い |
| --- | --- | --- |
| 境界 | 同じ画面の番号が同じ要求が 2 本届く（ブラウザの HTTP の再送） | 後から届いたほうを残し、先のほうを閉じる。取り違えても画面の見張り（40 秒）が開き直して戻る |
| 境界 | 番号の小さい要求が、大きい要求のあとに届く（経路の入れ替わり） | 大きいほうが開いていれば 204 で断り、gRPC を開かない |
| 境界 | 番号の小さい要求が、大きいほうが閉じたあとに届く | 開く（登録は開いている接続の分だけ持つので、閉じた番号は覚えていない）。画面はもう使っていないので、FIN が届いて abort で閉じるか、次の試行で付け替わる |
| 境界 | `attempt` が `0`・負・小数・`1e3`・17 桁以上・`Number.MAX_SAFE_INTEGER` を超える | 識別子なしと同じ扱い（付け替えの対象にしない） |
| 区切り | `screen` や `attempt` が 2 つある、空、`&`・`#`・`%00` を含む | `URLSearchParams.get` の最初の値を形で検査し、合わなければ識別子なしと同じ扱い |
| 長さ・Unicode | `screen` が 31・33 文字、大文字、全角、絵文字 | 形が合わないので識別子なしと同じ扱い |
| 並行 | 付け替えの `close` の中で、古い接続の abort・`error`・`cancel` が重なる | 後始末は 1 本で 2 回目以降は何もしない。`release` は自分を指すときだけ外すので、新しい接続の登録を消さない |
| 並行 | 同じ画面の別のタブ（タブの複製） | 識別子は `openStateStream` ごとの乱数で、保存しないので別の画面になる。互いに閉じない |
| 並行 | Next が読まない間に backend から状態が続けて来る | 列に 1 件、持ち分に最新の 1 件だけ。読めたら最新を送る |
| 再開 | 経路が戻り、開き直した接続が届く | 同じ画面の古い接続をその場で閉じ、backend のタスクも止まる |
| 再開 | 経路が切れている間にタブを閉じる | 開き直しが来ないので OS が閉じるまで残る。列は伸びない |
| 並行 | React の StrictMode の二重の mount で `openStateStream` が 2 回呼ばれ、識別子が 2 つできる | 先の 1 本は effect の後始末の `close` で `EventSource.close()` され、FIN が届いて abort で閉じる。別の識別子なので後の 1 本を閉じない。単体テストで、2 回目の mount のあとに開いた接続が 1 本だけ残ることを確かめる |
| 構成 | Next を複数のプロセスや複数台で動かす | 登録簿はプロセスの中だけなので、別のプロセスに届いた古い接続は閉じられない（修正前と同じ動き）。いまの本番は 1 プロセス |
| 再開 | 画面の JavaScript が古い（識別子を付けない）まま、新しいサーバーにつなぐ | 修正前と同じに動く（付け替えなし、間引きと後始末 1 本は効く） |

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- **上流（takt、takt-mcp）**：影響なし。
- **保存済みデータ**：影響なし（識別子はどこにも保存しない）。
- **公開 API（proto、`/api/*`）**：proto は変えない。`/api/state` に省略可能な問い合わせの引数 `screen`・`attempt` が増え、古い試行には `204` を返すようになる。引数が無い要求は修正前と同じに動く。`/api/state` を使うのはこの画面だけ。
- **守りの仕組み（CI、関門、ルールセット）**：影響なし。`/api/state` を当てている所を洗い出した（`grep -rn "api/state\|EventSource(" e2e frontend/web tools scripts`）：
  - `e2e/tests/issue-307-disconnected-decisions.spec.ts` の `page.route("**/api/state")`。Playwright の glob は URL 全体に当てるので、引数が付くと当たらない。引数の有無のどちらにも当たる正規表現 `/\/api\/state(\?|$)/` に直す。
  - `e2e/tests/issue-383-half-open-sse.spec.ts` は `EventSource` を包むだけで URL を見ないので、直さない。
  - 単体テストの `source.url === "/api/state"` の確かめ（`state-stream.test.ts`、`live-provider-wiring.test.ts`、`live-goals-read-scope.test.ts`）は、パスが `/api/state` で、`screen` と `attempt` が付いていることを確かめる形に直す。`state-revalidation.test.ts` は URL を確かめていないので直さない。
  - `test/api/state-route.test.ts` の `GET(new NextRequest(".../api/state"))` は引数なしの互換の確かめとして残し、引数ありのテストを足す。
  - `tools/prod-check/prod_http.py` の `fetch_state` は引数なしで `/api/state` を読む。修正前と同じに動くので直さない。
  - fetch の mock や MSW で `/api/state` を当てている所は無い。

## 見込み

相対の大きさで 3（基準：[j5ik2o/takt-dash#361](https://github.com/j5ik2o/takt-dash/issues/361) = 1、[j5ik2o/takt-dash#362](https://github.com/j5ik2o/takt-dash/issues/362) = 5）。変更は画面 1・サーバー 3 ファイルと、テスト 3〜4 ファイル。不確かさは Next の `ReadableStream` の読み方（確かめ済み）と e2e の URL の当て方。

## 設計レビューを受けて変えた点

| # | 重要度 | 扱い | 変更内容 |
| --- | --- | --- | --- |
| 1 | 中 | 直す | 登録簿がプロセス 1 つの中にあり、付け替えは 1 プロセスの間だけ効く前提を「範囲の外」と「壊しうる入力」に書いた |
| 2 | 低 | 直す | `ReadableStream` を `highWaterMark: 1` で明示して作ると書き、不変条件 8 と PBT の性質 8 がその値を前提にすると書いた |
| 3 | 低 | 直す | `/api/state` を当てている所をすべて洗い出して「影響」に書いた。e2e の glob は、引数の有無のどちらにも当たる正規表現にする |
| 4 | 低 | 直す | StrictMode の二重の mount で識別子が 2 つできる場合を「壊しうる入力」に書き、単体テストで確かめると書いた |

## PR のレビューを受けて変えた点（[j5ik2o/takt-dash#389](https://github.com/j5ik2o/takt-dash/pull/389)）

| 指摘 | 扱い | 変更内容 |
| --- | --- | --- |
| Devin：読み始めた直後に次の状態が来ると 1 → 3 → 2 の順で届く | 直す | 列へ直接入れるときに持ち分を捨てる。Quint で読み（`nextReads`）と `pull`（`relayPulls`）を別の遷移にし、変異 `keepStalePending` が `deliveredInOrder` を破ることを確かめた。PBT に、開いた直後に続けて状態が来る手と、読んだ直後に状態が来る手を足し、手の間で microtask を流し切らない並びも作った。`then` の中の確かめを手のあとに移した（`then` の中で投げても試験が落ちなかった） |
| Cursor Bugbot：`openStream` がその場で例外を投げると、登録と abort の聞き手が残る | 直す | 例外を捕まえて後始末を通してから投げ直す。単体テストを足した |
| CodeRabbit：Next は HEAD を GET で代わりに処理するので、HEAD が配信中の接続を付け替え、読まれない購読を残す | 直す | 本文を持たない `HEAD` を書き出した。単体テストを足した |
| CodeRabbit：abort 済みの要求は登録の前に除く | 採らない | 試行 2 が届くのは、画面が試行 1 を閉じたあと（`disconnect` で閉じてから開き直す）。試行 2 まで abort 済みなら試行 1 も見捨てられているので、閉じるのが正しい。Quint の `liveNotClosedByRelay` は、画面がいま使っている接続を閉じないことを確かめている |
| CodeRabbit：間引きの route テストの 300 ms 待ちは 5 件の到着を保証しない | 直す | 中継より先に data を数える聞き手を付け、5 件を受けてから読む |
| CodeRabbit：PBT が、付け替えた接続の本文が実際に終わることを見ていない | 直す | 性質 7 に「閉じた接続は残りを読み切ると本文が終わり、待っている読みも終わる」を足した。後始末で `controller.close()` を外す変異で落ちることを確かめた |
| CodeRabbit：Quint の断りの不変条件が、いま使っている接続がまだ届いていない間の取り違えを見逃す | 直す | 届いた時点の事実（`newerOpenAtArrival`）を記録し、不変条件 `staleArrivalRejected` を足した。3 回開いて試行 2・1 の順に届く筋書きを足した |
