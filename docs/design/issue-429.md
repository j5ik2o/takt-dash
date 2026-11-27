# issue #429 の設計：存在しない詳細ページが HTTP 200 で返る（ルートの loading.tsx を外す）

対象の issue：[j5ik2o/takt-dash#429](https://github.com/j5ik2o/takt-dash/issues/429)
関連：[#315](https://github.com/j5ik2o/takt-dash/issues/315) の D17（404・エラー・読み込み中の画面）、[PR #406](https://github.com/j5ik2o/takt-dash/pull/406)（マージ `2f4683c`）

## いまの状態（実測）

### 仕組み（リポジトリのコードと公式の仕様）

- `frontend/web/app/loading.tsx` はルート直下にあり、`<p role="status">読み込んでいます…</p>` を出す。
- Next.js では、同じフォルダの `loading.js` は `page.js` とその配下のすべてを `<Suspense>` 境界で包む。公式の [loading.js](https://nextjs.org/docs/app/api-reference/file-conventions/loading) は「It will automatically wrap the `page.js` file and any children below in a `<Suspense>` boundary」「`loading.js` wraps `not-found.js`, `page.js`, and nested `layout.js` files in a `<Suspense>` boundary. It does **not** wrap the `layout.js`, `template.js`, or `error.js` in the same segment.」と書いている。
- ルートの layout は、`children` を返す前に `await loadServerState()`（GetOverview）を待つ（`frontend/web/app/layout.tsx:22-30`）。`loading.js` は layout 自身を包まないので、この待ちの間は読み込み中の画面は出ない（公式：「If the layout accesses uncached or runtime data … `loading.js` will not show a fallback for it.」）。
- 詳細ページは薄い wrapper で、async の loader が gRPC を await し、NOT_FOUND のときに `notFound()` を呼ぶ（`run-detail-loader.tsx:35-36`、`goal-detail-loader.tsx:35-36`、`task-detail-loader.tsx:38-39` と `:46-47`）。
- 公式の [loading.js の Status codes](https://nextjs.org/docs/app/api-reference/file-conventions/loading#status-codes) は「Streaming page responses begin with a `200` status code.」「Without Cache Components, rendering a fallback from `loading.js` or `<Suspense>` starts streaming. If neither has started the response, Next.js can still return a `404`, `403`, or `401` when the corresponding interrupt runs.」と書いている。[notFound](https://nextjs.org/docs/app/api-reference/functions/not-found) も「The response starts streaming from the `<Suspense>` boundary before `notFound()` runs, so it keeps its `200` status.」「`notFound()` returns a `404` when it runs before the response starts」と書いている。

つまり、**存在チェックより先に Suspense の fallback が流れると 200 になり、先に `notFound()` が走れば 404 になる**。ルートの `loading.tsx` は、すべての詳細ページの存在チェックより先に fallback を流す位置にある。

### 実測したこと

`b0a0edc` の作業ツリー、Next.js 16.4.6、`next build` + `next start`（e2e の `startStack`）、Chrome headless で、応答の状態と本文を読んだ。本文は `fetch` で最後まで読み、次の 2 つの文字列が入っているかを見た。

- 「ページが見つかりません」（`frontend/web/app/not-found.tsx` の見出し）
- 「読み込んでいます…」（`frontend/web/app/loading.tsx` の `role="status"`）

| ルートの `loading.tsx` | URL | HTTP | not-found の画面 | 読み込み中の画面 |
| --- | --- | --- | --- | --- |
| あり | `/runs/no-such-run-429` | **200** | ある | ある |
| あり | `/goals/00000000-0000-4000-8000-000000004290` | **200** | ある | ある |
| あり | `/tasks/e2e-corporate-site?task=no-such-task-429` | **200** | ある | ある |
| なし（一時的に外して再ビルド） | 上の 3 つ | **404** | ある | ない |

200 のときは、fallback（読み込み中）が先に同じ応答へ流れ、そのあとに not-found の画面が stream で入る。利用者には「読み込んでいます…」のあと 404 の画面が見えるが、HTTP は 200 のままなので、ブラウザ・監視・クローラからは「存在する URL」に見える。404 のときは、`notFound()` が応答の開始前に走るので、本文に読み込み中の画面は入らない。

一覧側の置き場所も確かめた。ルートの `loading.tsx` を外した状態で `frontend/web/app/activity/loading.tsx` だけを置き、モックの応答を 3 秒遅らせて `/activity` を `waitUntil: "commit"` で開くと、**200 のまま** `role="status"` の「読み込んでいます…」が先に見え、そのあと見出し「アクティビティ」に置き換わった。セグメントに置いた `loading.tsx` は期待どおり動く。この測定は使い捨てで、e2e には置かない（下記）。

一時的に外した `loading.tsx` と、実測用の spec はコミットしていない。

## 何をどう変えるか

**案 (a) を採る。** ルートの `loading.tsx` を消し、`notFound()` を出さないセグメントにだけ `loading.tsx` を置く。詳細（run・goal・task）には置かない。

1. `frontend/web/app/loading.tsx` を削除する。
2. `frontend/web/src/components/page-loading.tsx` を新設し、いまの markup をそのまま移す（`<main className="page"><p role="status">読み込んでいます…</p></main>`）。
3. `frontend/web/app/activity/loading.tsx` と `frontend/web/app/conversations/loading.tsx` を新設し、`export { PageLoading as default } from "@/src/components/page-loading";` とする。
   - この 2 つは page 自身がサーバーで RPC を await する（`activity-loader.tsx:18-33`、`conversations-loader.tsx:22-32`）ので、読み込み中の画面が実際に効く。
   - どちらも子のセグメントを持たず、`notFound()` を呼ばない（`notFound()` を呼ぶのは詳細の 3 つの loader だけ）。
4. `frontend/web/test/pages/issue-315-wording.test.ts:705-710`（D17 の読み込み中のテスト）の読み込み先を `loading.tsx` から `activity/loading.tsx` に変える。文言の検査は変えない。
5. `e2e/tests/issue-429-status.spec.ts` を新設する（下記）。
6. 詳細ページ、3 つの loader、`frontend/web/app/not-found.tsx`、`frontend/web/app/error.tsx`、proto、backend、gRPC の deadline（`frontend/web/src/grpc/client.ts` の定数）は変えない。

`/goals` に `loading.tsx` を置かない理由：`app/goals/page.tsx`（一覧）と `app/goals/[goalId]/page.tsx`（詳細）が同じセグメントの下にある。`app/goals/loading.tsx` は `[goalId]` も包む（上の公式の「any children below」）ので、ゴール詳細の 404 が 200 に戻る。一覧に読み込み中の画面が要るなら `app/goals/(list)/` のような route group に分ける必要があるが、一覧は layout が読んだ状態から即座に描くので要らない。

`/`、`/projects`、`/decisions` に置かない理由：page 自身が RPC を await しない（`LiveOverview`、`ProjectsLoader` は要求内で共有した `loadServerState()`、`LiveDecisions` は client component）。layout の GetOverview は `loading.js` では包めないので、置いても効かない。`/runs` と `/tasks` は詳細だけ。

### 詳細を開いた直後に何が出るか、待ち時間の上限

詳細には `loading.tsx` も `<Suspense>` も置かない。したがって、応答の最初のバイトは、存在チェック（`getRunDetail`・`getGoalDetail`・`getTaskDetail`）が終わるまで来ない。待っている間に出るものは、開き方で変わる。

| 開き方 | 待っている間に出るもの |
| --- | --- |
| 一覧・概要のリンクを踏む（ふだんの操作） | 詳細への入り口のリンクは Next の `Link` ではなく `<a>` である（概要の run 詳細は `frontend/web/src/components/overview-content.tsx:277`。ゴール一覧・プロジェクト・ゴール詳細・タスク詳細の run へのリンクも `<a>`）。クライアント遷移にならないので前の画面は残らず、ブラウザ自身の読み込み表示（タブのスピナー）だけが出て、本文は空白。 |
| クライアント遷移（Next の `Link` で詳細を開く） | 前の画面がそのまま残る（Next は新しい RSC が届くまで前の画面を保つ）。リンク内の pending 表示は出さない。タスク詳細の「ゴール詳細に戻る」は `Link`（`frontend/web/src/components/task-detail-content.tsx:27`）なので、ゴール詳細へ戻るときはこちらになる。 |
| 直リンク・再読み込み | ブラウザ自身の読み込み表示（タブのスピナー）だけ。本文は空白。 |
| 取得が終わった後 | 存在すれば内容、無ければ日本語の not-found（404）、想定外の例外なら error.tsx（500）、期限超過なら「backend が応答しません」の 200（前に読めたゴール・タスクでは、前回の内容に切断の通知。下の「不正な ID・例外・期限超過の状態」）。 |

**待ち時間の上限**は deadline で決まる。詳細の取得だけで、run は最大 50 秒、goal/task は最大 30 秒（`frontend/web/src/grpc/client.ts` の `RUN_DETAIL_DEADLINE_MS = 50_000` と `DEFAULT_UNARY_DEADLINE_MS = 30_000`）。期限を過ぎると `BackendUnavailableError`（`failure: "no-response"`）になり、ローダーが「backend が応答しません。読み取りが遅れている可能性があります。」を返すので、取得の待ちはそこで終わる（500 にはしない。下の「不正な ID・例外・期限超過の状態」）。これに、ルート layout の `loadServerState()`（GetOverview、同じ 30 秒）の待ちが直列に重なる。backend が接続を受け付けたまま応答しない病的な場合は、run 80 秒・goal/task 60 秒。ここで言う 80 秒・60 秒は gRPC の期限の合計で、描画の時間と、ブラウザから Web サーバーまでの通信は含まない。利用者が見る空白時間の上限は、この合計に描画と通信の分だけ加わる（実時間は測っていない。下の「未確認」）。layout の待ちは変更前から同じで、`loading.js` では包めない（この issue では変えない）。

deadline は変えない。根拠は次の 3 つ。

- 50 秒は backend の総上限 45 秒（`backend/src/grpc/mod.rs` の `RUN_DETAIL_BUDGET`、回り道 3 回 × 15 秒）より後に切れるように決めた値。短くすると、#59 で直した「遅いが成功する取得」を Web が先に切り、成功するはずの詳細が失敗になる。
- 30 秒も、backend の 1 回 15 秒の読み取りに余裕を持たせた値。goal/task の詳細だけ別の値にする根拠が無い。
- ふだんの保存場所から読める run の待ちは小さい（backend の計測では、大きい本体ログの初回でも 10 回の最大が 2 秒未満。`backend/tests/run_detail_test.rs:250-273`）。50 秒は、backend が応答を返さない病的な場合の上限で、正常時は数秒以内に終わる。

したがって「利用者の体感」を優先して 10 秒に縮める案は採らない。縮めると #59・#180 の期限の設計をやり直すことになり、この issue の範囲を超える。期限超過の再試行は、ブラウザの再読み込みか、一覧から開き直す。error.tsx の「もう一度読み込む」は想定外の例外のときだけで、期限超過は #152 の「応答しません」に出す（この表示に再試行ボタンは無い。足すなら別 issue）。

この上限を短くするには、応答を始める前に存在を安く確かめる手段（軽い existence RPC か proxy）が要る。採らなかった案 (b') と同じで、この issue の範囲を超える。上限が問題になるなら、別 issue で existence RPC を足し、存在する詳細にだけ読み込み表示を戻す。

**日本語の待ち表示は、`notFound()` を使わないページにだけ置く。** 具体的には `/activity` と `/conversations` のセグメントにだけ `loading.tsx` を置き、詳細には置かない。詳細に局所の `<Suspense>` を置くと、fallback が存在チェックより先に同じ応答へ流れ、HTTP が 200 に戻る（ルートの `loading.tsx` で実測したのと同じ仕組み）。存在チェックの後に置いても、その時点で詳細のデータは取得済みで、待つものが無い。存在チェックそのものが詳細のデータ取得なので、詳細では fallback を出す余地が無い。HTTP の状態を正しくすることを優先する（issue の「存在しないものを 404 で返すは変えない」）。

**測り方**：上限は上の定数から決まる。実装後、backend の応答を遅らせた状態（モックの takt-mcp を遅らせる、または応答しない backend）で、まだ取得していない詳細の URL を直リンクし、ブラウザに最初のバイトが届くまでの時間と、期限後に「backend が応答しません」へ切り替わることを手で 1 回見る（一度開いた詳細では、前回の内容に切断の通知が出る側になる）。時間の e2e はこの issue では置かない（下記）。

## 採らなかった案と理由

- **(b) 詳細のセグメントから loading を外し、その下だけ自前の `<Suspense>` で読み込み表示を出す**：採らない。`<Suspense>` の fallback も `loading.js` と同じく応答の stream を開始するので、存在チェックより前に置けば 200 になる（公式の「rendering a fallback from `loading.js` or `<Suspense>` starts streaming」）。存在チェックのあとに置いても、待つのは取得済みの詳細データではなく、読み込み表示を出す理由が無い。ルートの `loading.tsx` で実測した 200 と同じ仕組み。
- **(b') 存在チェックを上に置き、その下に `<Suspense>` を置く**：404 と読み込み表示を両立できる形ではあるが、応答を始める前に存在を安く確かめる手段が要る。いまの `DashboardState` では足りない。`runs` は概要に出す run だけで、終わった run を一覧から確かめられるとは限らない（`proto/taktdash.proto:226-246` の `Run` の説明は「走っている run」）。`goals` は読めた範囲だけのことがあり（`readCoverage`）、タスクの一覧は状態に無い。一覧から存在を判断すると、存在する run・ゴール・タスクを 404 にしてしまう。専用の軽い existence RPC を proto と backend に足せば可能だが、この不具合の修正の範囲を超える。足しても、その確認自体は待ち時間になり、その間の読み込み表示は出せない。
- **(c) 200 のままにする**：採らない。クローラには soft 404 になる（`notFound()` の `noindex` は付くが、HTTP は 200 のまま）。監視・リンクチェッカ・`curl`・外部の API 利用者は、存在しない詳細と存在する詳細を状態で区別できない。e2e も状態を固定できない。issue の「存在しないものを 404 で返すは変えない」に反する。既存の撮影 spec（`e2e/screenshot/capture.spec.ts:43`）も、0 件の状態の詳細に 404 を期待している。
- **`proxy.ts` で存在を確かめて 404 にする**：応答の前に 404 を決められるので、詳細に `loading.tsx` を残したまま両立できる。ただし詳細の RPC を要求ごとに 2 回呼ぶことになり、proxy にも gRPC の経路・タイムアウト・許可ホストの扱いが要る。公式も proxy には「Keep Proxy checks fast」とだけ書いており、backend への照会を前提にしていない。いまは採らない。将来、軽い existence RPC を backend に足すなら候補になる。
- **存在チェックを `app/runs/[slug]/layout.tsx` に移す**：同じセグメントの `loading.tsx` は layout を包まないので 404 は保てるが、layout の await が応答を止めるので読み込み表示は出ず、詳細を page へ渡す手段（context か二重取得）も要る。採らない。
- **ルートの `loading.tsx` を残し、route group で詳細だけ外す**：ルート直下の `loading.tsx` は route group の下も含め、`app/` のすべての children を包む。詳細を外すにはルートの `loading.tsx` を消すしかない。

## 不正な ID・例外・期限超過の状態（main との比較）

ここでの「main」は、この issue が戻そうとしている #315 より前の main（ルートの `loading.tsx` が無い状態）を指す。いまの `origin/main` は #315 の後で `loading.tsx` があり、不正 ID と例外は fallback が先に流れて 200、期限超過は fallback の有無にかかわらず 200 になる。前者の 200 はこの issue が直す回帰で、後者の 200 は正しい。

- **不正なゴール ID**（`/goals/not-a-uuid`）：backend が INVALID_ARGUMENT を返し、`unary.ts` はそれを写さずに投げ、ローダーも捕まえないので error.tsx（500）に落ちる。#315 より前の main と同じで、この issue では変えない。ID を 400 や 404 にする検証は範囲の外。
- **想定外の例外**：同じく error.tsx（500）。#315 より前の main は Next の既定のエラー画面で 500 だった。状態は同じで、画面だけ日本語の error.tsx になる（#315 で入った）。この issue では変えない。最小の Next 16.4.6 アプリで、ルートの `loading.tsx` があると 200、無いと 500 になることを実測した（リポジトリの実装そのものでは未実測）。
- **DEADLINE_EXCEEDED**：500 にはならない（main と同じ）。`unary.ts` が `BackendUnavailableError`（`failure: "no-response"`）に写し、3 つの詳細ローダーが捕まえて 200 で出す。run は、前回の取得結果があればその内容の上に「backend が応答しません。読み取りが遅れている可能性があります。」を出す（`run-detail-loader.tsx:44-50`）。ゴールとタスクは通知が異なり、前回の取得結果があれば「最新の状態を取得できません。前回取得した内容を表示しています。」を付けてその内容を表示する（`goal-detail-loader.tsx:44-49`、`task-detail-loader.tsx:55-63`、`live-goal-detail.tsx:56-60`）。前回の結果が無いときだけ「backend が応答しません。読み取りが遅れている可能性があります。」を出す。この文面を手で確かめるときは、まだ取得していない詳細を開く（前回の結果は Web サーバーのプロセスが覚えており、一度開いた詳細では切断の通知の側になる）。deadline 超過は取得の待ちの終わりで、error.tsx の「もう一度読み込む」には落ちない。

## 確かめ方（e2e）

新規 `e2e/tests/issue-429-status.spec.ts` に、**1 本だけ**置く。`../support/test` の `test`・`expect` と、`../support/fixture` の `PROJECT_NAME` を使う。

### 存在しない詳細は 404（共有の `stack`）

```ts
const targets = [
  "/runs/no-such-run-429",
  "/goals/00000000-0000-4000-8000-000000004290",
  `/tasks/${encodeURIComponent(PROJECT_NAME)}?task=no-such-task-429`,
];
for (const path of targets) {
  const response = await page.goto(`${stack.webUrl}${path}`);
  expect(response?.status(), path).toBe(404);
  await expect(page.getByRole("heading", { name: "ページが見つかりません" })).toBeVisible();
  await expect(page.getByRole("link", { name: "概要に戻る" })).toBeVisible();
  await expect(page.getByText("読み込んでいます…", { exact: true })).toHaveCount(0);
}
```

- 確かめること：存在しない run・ゴール・タスクの詳細 URL が **404** を返す。画面はアプリの日本語の not-found（見出しと「概要に戻る」）で、読み込み中の文言は出ていない。状態の検査が本体で、見出しとリンクは、Next の既定の英語の 404 ではなくアプリの画面であることを固定する。
- 識別子：run とタスクは fixture に無い名前、ゴールは UUID の形で存在しない ID。ルートの `loading.tsx` を外した状態で 3 つとも 404 になることを実測済み。
- `page.goto` は読み込みの完了まで待つ。Suspense 境界が無いので、返った時点で本文は描き終わっている。
- 読み込み表示は `getByRole("status")` ではなく「読み込んでいます…」の文言で見る。文言の方が、何を検査しているかが残る。

**読み込み表示の e2e は、この issue では置かない。** `/activity` の `loading.tsx` が先に出ることは実測済み（上の「実測したこと」）で、この issue の変更対象は詳細の状態だから。時間を含む読み込み表示の e2e は別件にする。

実行は `make e2e-build` のあと `npm --prefix e2e test -- tests/issue-429-status.spec.ts`（`cd e2e && npx playwright test tests/issue-429-status.spec.ts` でも同じ）。

既存のテストで直すのは D17 の読み込み中の 1 件（読み込み先のパスだけ）。撮影 spec の 404 の期待は、変更後はそのまま合う。

## 影響

- **直リンク・クローラ・監視**：存在しない詳細は 404 に戻る。200 の soft 404 が消え、`noindex` に頼らず状態で区別できる。`notFound()` の `noindex` は残る。
- **存在する詳細**：200 のまま。fallback を先に送らないので TTFB は詳細 RPC の完了まで遅れるが、内容の到着時刻はほぼ変わらない。Next の `Link` で開くクライアント遷移では前の画面が残り、「読み込んでいます…」は出ないが、一覧・概要から詳細を開くリンクは `<a>` なので、ふだんの操作はこれではない（タスク詳細からゴール詳細へ戻るときは `Link`）。リンク内の pending 表示が必要なら、`useLinkStatus`（`loading.js` の無い動的な行き先に公式が勧めている）を別 issue で足せる。今回は扱わない。
- **詳細の待ち**：直リンク・再読み込みと、一覧・概要の `<a>` のリンクでは、取得が終わるまで本文が無い（ブラウザ自身の読み込み表示だけ）。gRPC の期限は、詳細の取得だけで run 50 秒・goal/task 30 秒。layout の GetOverview が応答しない病的な場合は、それぞれ 80 秒・60 秒（gRPC の期限の合計で、描画とブラウザから Web サーバーまでの通信は含まない）。
- **一覧**：`/activity` と `/conversations` は今までどおり「読み込んでいます…」を出す。`/`、`/goals`、`/projects`、`/decisions` は layout の状態から即座に描くので `loading.tsx` を置かない。layout の GetOverview が遅いときはどのページも読み込み中の画面が出ないが、これは今も同じ（`loading.js` は layout を包めない）。
- **404・エラーの画面**：`not-found.tsx` と `error.tsx` はルートのまま変えない。詳細で想定外の例外が出たときは、ルートの fallback が先に流れないので 500 になり、error.tsx の日本語の画面になる（コードで確認。最小の Next 16.4.6 アプリでも 500 を実測）。期限超過（DEADLINE_EXCEEDED）は 500 にならず、200 の「backend が応答しません」になる（前に読めたゴール・タスクでは前回の内容に切断の通知。main と同じ）。
- **既存の撮影 spec**：`e2e/screenshot/capture.spec.ts:43` は 0 件の状態の詳細に 404 を期待している。いまの 200 とは食い違う（実行はしていない）。変更後は期待どおりになる。
- **範囲の外**：proto、backend、API、保存データ、監視の設定は変えない。`measurePage` の計測点も変えない。

## Quint の要否

書かない。状態遷移は無く、1 回の HTTP 応答の状態という性質を e2e で固定すれば足りるため。

## 未確認

- `next dev` での同じ挙動（実測は `next build` + `next start`）。
- リポジトリの実装そのもので、詳細の例外が 500 になることの実測（コードでは error.tsx に落ちることを確認。最小の Next 16.4.6 アプリでは 500 を実測）。
- 不正なゴール ID の 200/500 の実測（INVALID_ARGUMENT が throw と同じ経路であることから推定。最小アプリの throw では、loading あり 200・なし 500 を確認）。
- どのルートにも一致しない URL の状態が、`loading.tsx` の有無で変わるか。
- 直リンクの空白の実時間（deadline の定数から上限を出しただけで、時間は測っていない）。
- `error.tsx` の「もう一度読み込む」は `reset` で、サーバーの取得をやり直すか（Next 16 の `retry` との違い）。この issue では変えず、#430 で確かめる。
- 撮影 spec の現状の成否（実行していない）。
- TTFB の実際の差（状態だけを実測し、時間は測っていない）。
