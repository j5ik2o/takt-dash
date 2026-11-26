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
- 間引き：gRPC から状態が来たら、`controller.desiredSize > 0` なら符号化して `enqueue`。そうでなければ `pending` に最新の状態だけを持つ（前の `pending` は捨てる）。`ReadableStream` の `pull` で `pending` があれば符号化して `enqueue` し、空にする。後始末で `pending` を捨てる。
- 付け替えで閉じた接続には `controller.close()` で終わりを伝える。Next がまだ `drain` を待っているあいだ、Node の応答とソケットは OS が閉じるまで残るが、それ以上は書かない。gRPC と backend のタスクはその場で閉じる。

### 変えないこと・範囲の外

- backend（`backend/src/grpc/mod.rs`）、`WATCH_RESEND_INTERVAL`、proto は変えない。backend のタスクは、route.ts が gRPC を閉じれば `sender.closed()` で止まる（既存の作り）。
- 画面の見張り時間（40 秒）、再接続のバックオフ、切断の表示、判断待ちのボタンの無効化（#378、#381、#386）は変えない。
- 経路が切れている間にタブごと閉じた接続を、OS より早く閉じることはしない（上の比較の A を参照）。
- 待ち受けアドレス（`0.0.0.0:3000`）と接続元の制限には触らない。
- 識別子は認証ではない。他人の識別子を当てて接続を閉じさせることは、16 バイトの乱数を推測できない限りできない。画面は LAN・VPN の内側で使う前提で、認証は元から無い。

## 守る不変条件

Quint の不変条件（`specs/quint/state-stream-relay/stateStreamRelay.qnt`）と同じものです。

1. **同じ接続の資源を二度閉じない**（`noDoubleRelease`）。
2. **画面ごとに、サーバーで開いている接続は 1 本まで**（`atMostOneOpenPerScreen`）。
3. **見捨てられた接続は、画面が開き直した接続がサーバーに届いた時点で閉じる**（`closedOnNextArrival`）。届かないまま（タブを閉じた）なら、OS が TCP をあきらめた時点で閉じる。
4. **新しい試行が開いている間は、遅れて届いた古い試行を開かない**（`staleNeverOpensBesideNewer`）。
5. **動いている接続（画面がいま使っている接続）を閉じない**（`liveNotClosedByRelay`）。閉じてよいのは backend の終了と、経路が切れて OS が閉じた場合だけ。
6. **閉じた接続の登録が残らない**（`registryPointsToOpen`）。
7. **backend の購読は、開いている接続の分だけ**（`backendTaskOnlyWhileOpen`）。
8. **`ReadableStream` の列は `highWaterMark`（1 件）を超えない**（`queueBounded`）。
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
  7. 閉じた接続は gRPC を閉じている（不変条件 7）。
  8. 読まない接続の内部の列は 1 件、持ち分は 1 件を超えない（不変条件 8。`desiredSize >= 0` で確かめる）。
  9. 読んだ版は増える一方（不変条件 9）。
  10. 開いている接続で読み続ければ、最後に読む版は gRPC から最後に来た版（不変条件 10）。
- 境界は個別のテストで固定する：番号が同じ、1 小さい、`Number.MAX_SAFE_INTEGER`、識別子なし、識別子の形が違う（大文字、31・33 文字、Unicode、`%00`）、番号が `0`・負・小数・`1e3`・17 桁。

### `frontend/web/test/components/state-stream.test.ts`（足す）

#383 の PBT の操作列に、開いた接続の URL を確かめる性質を足します：同じ `openStateStream` の接続はすべて同じ識別子を持ち、番号は開いた順に 1 ずつ増える。別の `openStateStream` は別の識別子を持つ。

## 状態遷移と Quint

`specs/quint/state-stream-relay/` に仕様とテストを置きました（`README.md` に範囲・前提・結果）。

- `stateStreamRelay.qnt`：画面 2 つ・接続 5 本。経路の切断、画面の開き直しとタブの閉じ、要求の到着と付け替え・断り、abort・backend の終了・OS の打ち切り・Next の cancel、backend からの状態と Next の読み、を遷移にした状態機械。不変条件 10（`safety`）、到達条件 7。
- `stateStreamRelay_test.qnt`：筋書き 15 件。修正前の 3 つの現象（2 本残る、列が伸びる、二度閉じる）、修正案の付け替え・断り・間引き・後始末 1 本、変異 3 つの反例、タブを閉じた接続が OS を待つ残り。

検査結果（quint 0.32.0）：

- `quint typecheck`：2 ファイルともエラーなし。
- `quint test stateStreamRelay_test.qnt --main=stateStreamRelay_test`：15 件すべて成功。
- 修正前（`current`）は `noDoubleRelease`・`atMostOneOpenPerScreen`・`closedOnNextArrival`・`staleNeverOpensBesideNewer`・`queueBounded` に違反。変異は、`noRejectStale` が `liveNotClosedByRelay`、`noConflate` が `queueBounded`、`noFinishOnce` が `noDoubleRelease` に違反（各 `--max-steps=30 --max-samples=10000 --seed=0x385`）。
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
| 再開 | 画面の JavaScript が古い（識別子を付けない）まま、新しいサーバーにつなぐ | 修正前と同じに動く（付け替えなし、間引きと後始末 1 本は効く） |

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- **上流（takt、takt-mcp）**：影響なし。
- **保存済みデータ**：影響なし（識別子はどこにも保存しない）。
- **公開 API（proto、`/api/*`）**：proto は変えない。`/api/state` に省略可能な問い合わせの引数 `screen`・`attempt` が増え、古い試行には `204` を返すようになる。引数が無い要求は修正前と同じに動く。`/api/state` を使うのはこの画面だけ。
- **守りの仕組み（CI、関門、ルールセット）**：影響なし。e2e の `e2e/tests/issue-307-disconnected-decisions.spec.ts` は `page.route("**/api/state")` で接続を落としている。Playwright の glob は URL 全体に当てるので、問い合わせの引数が付くと当たらなくなる。`"**/api/state?*"` に直す。

## 見込み

相対の大きさで 3（基準：[j5ik2o/takt-dash#361](https://github.com/j5ik2o/takt-dash/issues/361) = 1、[j5ik2o/takt-dash#362](https://github.com/j5ik2o/takt-dash/issues/362) = 5）。変更は画面 1・サーバー 3 ファイルと、テスト 3〜4 ファイル。不確かさは Next の `ReadableStream` の読み方（確かめ済み）と e2e の URL の当て方。
