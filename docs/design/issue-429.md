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

一覧側の置き場所も確かめた。ルートの `loading.tsx` を外した状態で `frontend/web/app/activity/loading.tsx` だけを置き、モックの応答を 3 秒遅らせて `/activity` を `waitUntil: "commit"` で開くと、**200 のまま** `role="status"` の「読み込んでいます…」が先に見え、そのあと見出し「アクティビティ」に置き換わった。セグメントに置いた `loading.tsx` は期待どおり動く。

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
6. 詳細ページ、3 つの loader、`frontend/web/app/not-found.tsx`、`frontend/web/app/error.tsx`、proto、backend は変えない。

`/goals` に `loading.tsx` を置かない理由：`app/goals/page.tsx`（一覧）と `app/goals/[goalId]/page.tsx`（詳細）が同じセグメントの下にある。`app/goals/loading.tsx` は `[goalId]` も包む（上の公式の「any children below」）ので、ゴール詳細の 404 が 200 に戻る。一覧に読み込み中の画面が要るなら `app/goals/(list)/` のような route group に分ける必要があるが、一覧は layout が読んだ状態から即座に描くので要らない。

`/`、`/projects`、`/decisions` に置かない理由：page 自身が RPC を await しない（`LiveOverview`、`ProjectsLoader` は要求内で共有した `loadServerState()`、`LiveDecisions` は client component）。layout の GetOverview は `loading.js` では包めないので、置いても効かない。`/runs` と `/tasks` は詳細だけ。

### 詳細の読み込み中の画面をどうするか

詳細では読み込み中の画面を出さない。これは意図した代償で、次の 2 つは両立しない。

- 存在チェック（`getRunDetail`・`getGoalDetail`・`getTaskDetail`）より前に fallback を流す → 200 になる（いまの不具合）。
- 存在チェックのあとに fallback を流す → その時点で詳細のデータは取得済みで、待つものが無い。

存在チェックそのものが詳細のデータ取得なので、詳細では fallback を出す余地が無い。HTTP の状態を正しくすることを優先する（issue の「存在しないものを 404 で返すは変えない」）。詳細の TTFB は gRPC の完了まで遅れるが、内容が届く時刻はほぼ変わらない。クライアント側の遷移では、Next が前の画面を残したまま新しい RSC を待つ。

## 採らなかった案と理由

- **(b) 詳細のセグメントから loading を外し、その下だけ自前の `<Suspense>` で読み込み表示を出す**：採らない。`<Suspense>` の fallback も `loading.js` と同じく応答の stream を開始するので、存在チェックより前に置けば 200 になる（公式の「rendering a fallback from `loading.js` or `<Suspense>` starts streaming」）。存在チェックのあとに置いても、待つのは取得済みの詳細データではなく、読み込み表示を出す理由が無い。ルートの `loading.tsx` で実測した 200 と同じ仕組み。
- **(b') 存在チェックを上に置き、その下に `<Suspense>` を置く**：404 と読み込み表示を両立できる形ではあるが、応答を始める前に存在を安く確かめる手段が要る。いまの `DashboardState` では足りない。`runs` は概要に出す run だけで、終わった run を一覧から確かめられるとは限らない（`proto/taktdash.proto:226-246` の `Run` の説明は「走っている run」）。`goals` は読めた範囲だけのことがあり（`readCoverage`）、タスクの一覧は状態に無い。一覧から存在を判断すると、存在する run・ゴール・タスクを 404 にしてしまう。専用の軽い existence RPC を proto と backend に足せば可能だが、この不具合の修正の範囲を超える。足しても、その確認自体は待ち時間になり、その間の読み込み表示は出せない。
- **(c) 200 のままにする**：採らない。クローラには soft 404 になる（`notFound()` の `noindex` は付くが、HTTP は 200 のまま）。監視・リンクチェッカ・`curl`・外部の API 利用者は、存在しない詳細と存在する詳細を状態で区別できない。e2e も状態を固定できない。issue の「存在しないものを 404 で返すは変えない」に反する。既存の撮影 spec（`e2e/screenshot/capture.spec.ts:43`）も、0 件の状態の詳細に 404 を期待している。
- **`proxy.ts` で存在を確かめて 404 にする**：応答の前に 404 を決められるので、詳細に `loading.tsx` を残したまま両立できる。ただし詳細の RPC を要求ごとに 2 回呼ぶことになり、proxy にも gRPC の経路・タイムアウト・許可ホストの扱いが要る。公式も proxy には「Keep Proxy checks fast」とだけ書いており、backend への照会を前提にしていない。いまは採らない。将来、軽い existence RPC を backend に足すなら候補になる。
- **存在チェックを `app/runs/[slug]/layout.tsx` に移す**：同じセグメントの `loading.tsx` は layout を包まないので 404 は保てるが、layout の await が応答を止めるので読み込み表示は出ず、詳細を page へ渡す手段（context か二重取得）も要る。採らない。
- **ルートの `loading.tsx` を残し、route group で詳細だけ外す**：ルート直下の `loading.tsx` は route group の下も含め、`app/` のすべての children を包む。詳細を外すにはルートの `loading.tsx` を消すしかない。

## 確かめ方（e2e）

新規 `e2e/tests/issue-429-status.spec.ts`。`../support/test` の `test`・`expect` と、`../support/fixture` の `PROJECT_NAME` を使う。

### 1. 存在しない詳細は 404（共有の `stack`）

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
  await expect(page.getByRole("status")).toHaveCount(0);
}
```

- 確かめること：存在しない run・ゴール・タスクの詳細 URL が **404** を返す。画面はアプリの日本語の not-found（見出しと「概要に戻る」）で、読み込み中の表示は出ていない。状態の検査が本体で、見出しとリンクは、Next の既定の英語の 404 ではなくアプリの画面であることを固定する。
- 識別子：run とタスクは fixture に無い名前、ゴールは UUID の形で存在しない ID。ルートの `loading.tsx` を外した状態で 3 つとも 404 になることを実測済み。
- `page.goto` は読み込みの完了まで待つ。Suspense 境界が無いので、返った時点で本文は描き終わっている。

### 2. 読み込み中の画面は残る（`isolatedStack`）

```ts
await stack.setMockDelay(3000);
const response = await page.goto(`${stack.webUrl}/activity`, { waitUntil: "commit" });
expect(response?.status()).toBe(200);
await expect(page.getByRole("status")).toHaveText("読み込んでいます…");
await expect(page.getByRole("heading", { name: "アクティビティ", level: 1 })).toBeVisible({ timeout: 15_000 });
await expect(page.getByRole("status")).toHaveCount(0);
```

- 確かめること：読み込み中の画面（日本語）が `/activity` で先に出て、内容に置き換わる。ページは存在するので 200 が正しい。
- `waitUntil: "commit"` が要る。既定の `load` は stream の終わりまで待つので、fallback はもう消えている。
- `setMockDelay` は共有のモックの遅延を変えるため、`isolatedStack` を使う。3 秒は実測で使った値で、layout の GetOverview と `getActivity` がそれぞれ遅れる。内容の到着に備えて待ち時間は 15 秒にする。

実行は `make e2e-build` のあと `npm --prefix e2e test -- tests/issue-429-status.spec.ts`（`cd e2e && npx playwright test tests/issue-429-status.spec.ts` でも同じ）。

既存のテストで直すのは D17 の読み込み中の 1 件（読み込み先のパスだけ）。撮影 spec の 404 の期待は、変更後はそのまま合う。

## 影響

- **直リンク・クローラ・監視**：存在しない詳細は 404 に戻る。200 の soft 404 が消え、`noindex` に頼らず状態で区別できる。`notFound()` の `noindex` は残る。
- **存在する詳細**：200 のまま。fallback を先に送らないので TTFB は詳細 RPC の完了まで遅れるが、内容の到着時刻はほぼ変わらない。クライアント遷移では前の画面が残り、「読み込んでいます…」は出ない。リンク内の pending 表示が必要なら、`useLinkStatus`（`loading.js` の無い動的な行き先に公式が勧めている）を別 issue で足せる。今回は扱わない。
- **一覧**：`/activity` と `/conversations` は今までどおり「読み込んでいます…」を出す。`/`、`/goals`、`/projects`、`/decisions` は layout の状態から即座に描くので `loading.tsx` を置かない。layout の GetOverview が遅いときはどのページも読み込み中の画面が出ないが、これは今も同じ（`loading.js` は layout を包めない）。
- **404・エラーの画面**：`not-found.tsx` と `error.tsx` はルートのまま変えない。詳細で想定外の例外が出たとき、いまは fallback が先に流れて 200 になるが、変更後は応答の開始前に落ちれば 500 になり得る（未確認）。
- **既存の撮影 spec**：`e2e/screenshot/capture.spec.ts:43` は 0 件の状態の詳細に 404 を期待している。いまの 200 とは食い違う（実行はしていない）。変更後は期待どおりになる。
- **範囲の外**：proto、backend、API、保存データ、監視の設定は変えない。`measurePage` の計測点も変えない。

## Quint の要否

書かない。状態遷移は無く、1 回の HTTP 応答の状態という性質を e2e で固定すれば足りるため。

## 未確認

- `next dev` での同じ挙動（実測は `next build` + `next start`）。
- 詳細で想定外の例外が 500 になるか。
- どのルートにも一致しない URL の状態が、`loading.tsx` の有無で変わるか。
- 撮影 spec の現状の成否（実行していない）。
- TTFB の実際の差（状態だけを実測し、時間は測っていない）。
