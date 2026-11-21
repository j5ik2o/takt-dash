# issue #72 計測記録

状態ストアを読むだけのページ（`/`、`/decisions`、`/projects`、`/activity`）の応答時間を、
Next.js 側の処理、gRPC の往復、backend の処理、SQLite の接続ロック待ちに分解して計測した記録です。
原因の修正はこのタスクの対象外で、ここでは計測と切り分けだけを記録します。

- 基点: `4b9325c12996ddc7437c684c66ecb023f84796a5`
- 計測日時: 2026-11-15 22:53 JST（ログのタイムスタンプは UTC 13:53）
- 対象: `/`、`/decisions`、`/projects`、`/activity`

## 計測方法

### 計測コード

Next.js 側は `frontend/web/src/telemetry/timing.ts` で、`measurePage` の呼び出し（layout と各ページの
ローダー）ごとに1行を `console.info` へ出します。`/projects` では layout とローダーが別々に計測される
ため、次の2行になります（数値は形式例）。

```
[taktDash][timing] scope=page page=layout total_ms=3.00 rpc_ms=2.50 next_ms=0.50 rpcs=GetOverview
[taktDash][timing] scope=page page=/projects total_ms=2.57 rpc_ms=2.16 next_ms=0.41 rpcs=GetProjects
```

`/activity` も同様に `page=layout ... rpcs=GetOverview` と `page=/activity ... rpcs=GetActivity` の
2行になります。

- `total_ms`: `measurePage` が囲んだローダー実行の経過時間（`performance.now()` の差分）。
- `rpc_ms`: そのローダー内で `timedUnary` が記録した gRPC 往復の合計。
- `next_ms`: `max(0, total_ms - rpc_ms)`。React のシリアライズ、HTTP 転送、layout の後に描画される
  ページ本体の時間は含みません。
- RPC ごとにも次の1行を出します。

```
[taktDash][timing] scope=rpc rpc=GetProjects duration_ms=9.42
```

- `/` と `/decisions` のサーバー側処理は layout だけです。どちらも `page=layout` の行で観測します。
- `/projects` は `page=layout`（`GetOverview`）と `page=/projects`（`GetProjects`）の2行、
  `/activity` は `page=layout`（`GetOverview`）と `page=/activity`（`GetActivity`）の2行になります。
- `GetPendingDecisions` と `GetRunDetail` は4ページのサーバー側経路では呼ばれないため、実測値はありません
  （計測の実装とテストだけがあります）。

backend 側は `backend/src/grpc/mod.rs` の `RpcTimingGuard` が、5つの unary ハンドラの終端で次の1行を
`tracing::info!` で出します。

```
INFO takt_dash_backend::grpc: unary RPC を処理しました。 rpc=GetOverview elapsed_us=192 lock_wait_us=0
```

- `elapsed_us`: ハンドラの開始から終了までの時間。tonic の応答シリアライズと転送は含みません。
- `lock_wait_us`: `ActivityStore` が接続 Mutex の取得を待った累積時間の、ハンドラ開始時と終了時の差。
  SQLite のクエリ実行時間そのものではありません。累積カウンタはプロセス全体で共有するため、
  同じ時間帯に動く別ハンドラの待ちも含み得ます。

### 起動と計測手順

1. backend を debug ビルドし、専用ポートで起動する（既に 50051 で別の backend が動いていたため 50061 を使用）。

   ```sh
   cargo build --bin takt-dash-backend
   ./target/debug/takt-dash-backend --config taktdash.config.json \
     --sqlite "$OUT/taktdash.sqlite" --listen 127.0.0.1:50061
   ```

2. Next.js を production build して起動する。

   ```sh
   npm --prefix frontend/web run build
   TAKTDASH_BACKEND_ADDR=127.0.0.1:50061 npm --prefix frontend/web run start -- -p 3111
   ```

3. 各ページを2回ずつウォームアップしたあと、`/`、`/decisions`、`/projects`、`/activity` を
   この順に10回ずつ逐次に取得し、TTFB を記録する。

   ```sh
   for p in / /decisions /projects /activity; do
     for i in 1 2; do curl -sS -o /dev/null "http://127.0.0.1:3111$p"; done
   done
   for p in / /decisions /projects /activity; do
     for i in $(seq 1 10); do
       curl -sS -o /dev/null -w '%{time_starttransfer}\n' "http://127.0.0.1:3111$p"
     done
   done
   ```

4. Next.js の標準出力から `scope=page` / `scope=rpc` の行、backend の標準エラー出力から
   `unary RPC を処理しました。` の行を取り出す。ウォームアップ分（最初の13行）を除いた
   60行ずつを実測値として集計した。

## 実行条件

| 項目 | 値 |
|------|-----|
| 日時 | 2026-11-15 22:53 JST |
| ホスト | macOS 27.2 / Apple M4 Pro / 14 コア |
| Node.js | v24.22.3 |
| Next.js | 16.4.6（production build、`next start -p 3111`） |
| Rust | 1.100.0（backend は debug ビルド） |
| 基点 | `4b9325c12996ddc7437c684c66ecb023f84796a5` |
| ポーリング間隔 | 30 秒（`taktdash.config.json` の `intervalSeconds`） |
| 設定上のプロジェクト数 | 4（corporate-site、event-store-adapter、takt、takt-dash） |
| 負荷 | 追加の負荷なし。計測クライアントは逐次に1要求ずつ |
| run 件数 | 計測時に確認していない（下記「未計測・制約」を参照） |

backend のログには、計測開始直後に `project=takt-dash error=takt-mcp の子プロセスが終了しました` の
警告が1件ありました。状態ストアが実データで満たされていない可能性があります。このため絶対値は
「実データを大量に載せたダッシュボード」の代表値ではありません。

## 実測値

すべて4ページ × 10回の逐次計測です。ウォームアップの2回は含みません。

### TTFB（ms、`curl` の `time_starttransfer`）

| ページ | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | min / median / max |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `/` | 5.621 | 4.980 | 5.135 | 4.974 | 4.924 | 4.877 | 5.143 | 4.465 | 3.935 | 5.562 | 3.94 / 4.98 / 5.62 |
| `/decisions` | 5.112 | 5.155 | 4.724 | 5.302 | 4.959 | 5.003 | 4.984 | 4.846 | 4.578 | 4.945 | 4.58 / 4.97 / 5.30 |
| `/projects` | 6.882 | 5.312 | 5.241 | 5.840 | 5.951 | 5.557 | 4.868 | 5.400 | 5.427 | 6.545 | 4.87 / 5.49 / 6.88 |
| `/activity` | 5.294 | 5.041 | 5.099 | 5.313 | 6.107 | 4.769 | 5.120 | 5.086 | 4.777 | 6.756 | 4.77 / 5.11 / 6.76 |

### Next.js 側のログ（ms）

| ページ | 行 | 指標 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | min / median / max |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `/` | layout | total_ms | 2.63 | 2.47 | 2.56 | 2.60 | 2.56 | 2.45 | 2.70 | 2.37 | 2.02 | 3.09 | 2.02 / 2.56 / 3.09 |
| `/` | layout | rpc_ms | 2.13 | 1.91 | 2.04 | 2.05 | 1.97 | 1.91 | 2.12 | 1.84 | 1.56 | 2.52 | 1.56 / 2.00 / 2.52 |
| `/` | layout | next_ms | 0.50 | 0.56 | 0.52 | 0.55 | 0.58 | 0.54 | 0.58 | 0.53 | 0.46 | 0.56 | 0.46 / 0.55 / 0.58 |
| `/decisions` | layout | total_ms | 2.66 | 2.83 | 2.53 | 2.81 | 2.73 | 2.74 | 2.55 | 2.52 | 2.42 | 2.62 | 2.42 / 2.64 / 2.83 |
| `/decisions` | layout | rpc_ms | 2.11 | 2.22 | 2.04 | 2.21 | 2.25 | 2.14 | 2.03 | 2.02 | 1.92 | 2.07 | 1.92 / 2.09 / 2.25 |
| `/decisions` | layout | next_ms | 0.55 | 0.62 | 0.49 | 0.60 | 0.48 | 0.61 | 0.52 | 0.50 | 0.50 | 0.55 | 0.48 / 0.54 / 0.62 |
| `/projects` | layout | total_ms | 3.12 | 2.98 | 2.94 | 3.23 | 3.48 | 3.21 | 2.59 | 3.00 | 2.99 | 3.00 | 2.59 / 3.00 / 3.48 |
| `/projects` | layout | rpc_ms | 2.63 | 2.50 | 2.48 | 2.61 | 2.88 | 2.69 | 2.14 | 2.49 | 2.48 | 2.46 | 2.14 / 2.50 / 2.88 |
| `/projects` | layout | next_ms | 0.49 | 0.47 | 0.46 | 0.61 | 0.60 | 0.51 | 0.46 | 0.51 | 0.51 | 0.54 | 0.46 / 0.51 / 0.61 |
| `/projects` | page | total_ms | 2.64 | 2.50 | 2.51 | 2.62 | 2.94 | 2.67 | 2.19 | 2.52 | 2.52 | 2.75 | 2.19 / 2.57 / 2.94 |
| `/projects` | page | rpc_ms | 2.24 | 2.12 | 2.16 | 2.16 | 2.48 | 2.22 | 1.85 | 2.16 | 2.15 | 2.37 | 1.85 / 2.16 / 2.48 |
| `/projects` | page | next_ms | 0.39 | 0.38 | 0.35 | 0.46 | 0.47 | 0.45 | 0.34 | 0.36 | 0.37 | 0.39 | 0.34 / 0.39 / 0.47 |
| `/activity` | layout | total_ms | 2.99 | 2.81 | 2.81 | 2.99 | 3.37 | 2.58 | 2.85 | 2.91 | 2.67 | 4.52 | 2.58 / 2.88 / 4.52 |
| `/activity` | layout | rpc_ms | 2.51 | 2.32 | 2.30 | 2.48 | 2.76 | 2.11 | 2.36 | 2.39 | 2.18 | 4.06 | 2.11 / 2.38 / 4.06 |
| `/activity` | layout | next_ms | 0.48 | 0.49 | 0.52 | 0.51 | 0.61 | 0.47 | 0.49 | 0.51 | 0.50 | 0.46 | 0.46 / 0.49 / 0.61 |
| `/activity` | page | total_ms | 2.81 | 2.62 | 2.57 | 2.73 | 3.16 | 2.35 | 2.60 | 2.66 | 2.42 | 4.34 | 2.35 / 2.64 / 4.34 |
| `/activity` | page | rpc_ms | 2.42 | 2.25 | 2.21 | 2.31 | 2.76 | 1.97 | 2.26 | 2.21 | 2.06 | 4.00 | 1.97 / 2.25 / 4.00 |
| `/activity` | page | next_ms | 0.39 | 0.37 | 0.37 | 0.42 | 0.39 | 0.37 | 0.34 | 0.45 | 0.36 | 0.34 | 0.34 / 0.37 / 0.45 |

### backend のログ（µs）

| ページ | RPC | 指標 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | min / median / max |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `/` | GetOverview | elapsed_us | 157 | 181 | 184 | 201 | 182 | 164 | 196 | 187 | 156 | 181 | 156 / 181.5 / 201 |
| `/` | GetOverview | lock_wait_us | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 / 0 |
| `/decisions` | GetOverview | elapsed_us | 187 | 198 | 191 | 189 | 195 | 184 | 189 | 202 | 193 | 197 | 184 / 192 / 202 |
| `/decisions` | GetOverview | lock_wait_us | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 / 0 / 0 |
| `/projects` | GetOverview | elapsed_us | 295 | 283 | 261 | 275 | 338 | 296 | 194 | 255 | 275 | 195 | 194 / 275 / 338 |
| `/projects` | GetProjects | elapsed_us | 336 | 324 | 303 | 325 | 413 | 340 | 320 | 297 | 317 | 361 | 297 / 324.5 / 413 |
| `/projects` | （同時実行の2ハンドラ共通） | lock_wait_us | 235 | 224 | 201 | 211 | 260 | 232 | 141 | 193 | 210 | 135 | 135 / 210.5 / 260 |
| `/activity` | GetOverview | elapsed_us | 299 | 206 | 171 | 275 | 364 | 201 | 178 | 212 | 188 | 275 | 171 / 209 / 364 |
| `/activity` | GetActivity | elapsed_us | 540 | 545 | 505 | 520 | 634 | 527 | 503 | 556 | 510 | 514 | 503 / 523.5 / 634 |
| `/activity` | （同時実行の2ハンドラ共通） | lock_wait_us | 238 | 150 | 116 | 209 | 276 | 145 | 122 | 156 | 132 | 207 | 116 / 153 / 276 |

`/projects` と `/activity` では、layout の `GetOverview` とページの RPC が同じ時間帯に動くため、
`lock_wait_us` は両方のハンドラで同じ値になります。合計へ二重に足さないでください。

### 応答全体の内訳（min / median / max）

layout とページのローダーが重なって動くため、ローダー時間は合計せず、遅い方（wall）で示します。

| ページ | TTFB ms | ローダー wall ms | gRPC wall ms | backend wall µs | lock_wait µs | TTFB - gRPC wall ms |
|---|---|---|---|---|---|---|
| `/` | 3.94 / 4.98 / 5.62 | 2.02 / 2.56 / 3.09 | 1.56 / 2.00 / 2.52 | 156 / 181.5 / 201 | 0 / 0 / 0 | 2.38 / 3.00 / 3.49 |
| `/decisions` | 4.58 / 4.97 / 5.30 | 2.42 / 2.64 / 2.83 | 1.92 / 2.09 / 2.25 | 184 / 192 / 202 | 0 / 0 / 0 | 2.66 / 2.87 / 3.09 |
| `/projects` | 4.87 / 5.49 / 6.88 | 2.59 / 3.00 / 3.48 | 2.14 / 2.50 / 2.88 | 297 / 324.5 / 413 | 135 / 210.5 / 260 | 2.73 / 2.93 / 4.25 |
| `/activity` | 4.77 / 5.11 / 6.76 | 2.58 / 2.88 / 4.52 | 2.11 / 2.38 / 4.06 | 503 / 523.5 / 634 | 116 / 153 / 276 | 2.60 / 2.74 / 3.35 |

- Next.js 側の実コストは `TTFB - gRPC wall` の 2.38〜4.25ms で、ログの `next_ms`（0.34〜0.62ms）より
  大きい。`next_ms` はローダー内の差分だけで、React のシリアライズと HTTP 転送を含まないため。
- gRPC wall のうち backend の処理は 156〜634µs で、残り 1.404〜3.785ms（表記 1.40〜3.79ms）が gRPC
  クライアントの接続・シリアライズ・ループバック転送などの往復コストになる。

上の範囲は、この節の表の生値（ウォームアップを除く4ページ × 10 run）から次の式で求めた値です。いずれも
小数2桁へ丸めています。

- `next_ms` の範囲: Next.js 側のログ表の `next_ms` 全60値の最小 0.34 と最大 0.62。
- 往復残差の範囲: 各 run の `gRPC wall - backend elapsed`。最小は `/` の layout の 1.56ms −
  GetOverview の 156µs = 1.404ms、最大は `/activity` の layout の 4.06ms − GetOverview の 275µs =
  3.785ms。
- `TTFB - gRPC wall` の範囲: 応答全体の内訳表の当該列の最小 2.38 と最大 4.25。

### 同時実行の証拠

backend のログのタイムスタンプと `elapsed_us` から求めた、`GetOverview` とページ RPC の重なり時間（µs）です。

- `/projects`: 192 / 275 / 338（min / median / max）
- `/activity`: 171 / 209 / 364（min / median / max）

重なりは `lock_wait_us` とほぼ一致します。layout の状態取得とページの状態取得が同じ接続 Mutex を取り合い、
待ち時間として現れています。

## 原因の切り分け

### 1. CPU の取り合い

- 支持: なし。
- 反証: backend ハンドラの中央値は 0.18〜0.52ms、最大 0.63ms。TTFB の最大は 6.88ms。数百 ms 級の
  待ちや裾の長い分布は観測されなかった。
- 判定: この条件では支持されない。追加の負荷をかけた条件は未計測なので、負荷時の CPU 競合は
  引き続き未確認。CPU の取り合いを原因とする別 issue の提案はしない。

### 2. gRPC クライアントの作り直し

- 支持: layout と2つのローダーは要求ごとに `createBackendClient()` を呼び、`close()` している。
  layout の gRPC wall は中央値 2.00ms、backend の処理は 0.18ms で、差の約 1.8ms が往復の
  オーバーヘッドになる。`/` の `GetOverview` は毎回新しいクライアントで呼ばれる。
- 反証・限界: 同じ条件でクライアントを再利用した比較は未計測（再利用はこのタスクの修正対象外）。
  往復コストには TCP ループバックとシリアライズも含むため、作り直しだけの寄与は分離できていない。
- 判定: 候補として支持。確定にはクライアント再利用版との比較計測が必要。

### 3. layout とページの二重取得

- 支持: `/projects` と `/activity` では、layout の `GetOverview` とページの RPC が 171〜364µs 重なって
  実行されている。両方とも `assemble_state_for` を通って状態を組み立てる。`GetProjects` と
  `GetActivity` の応答も `state` を含むため、1つのページ要求で状態を2回組み立てている。
- 反証: `/` と `/decisions` は layout の `GetOverview` だけなので、ロック待ちは 0。
- 判定: 支持。

### 4. SQLite の同期呼び出し

- 支持: `ActivityStore` は単一の `Mutex<Connection>` の下で同期クエリを実行する。layout とページの
  ハンドラが重なると、接続ロックの待ちが 116〜276µs 発生する。`GetActivity` の処理時間の中央値
  523.5µs のうち、ロック待ちは 153µs（約29%）。
- 反証・限界: `GetOverview` 単独ではロック待ちは 0。ロック待ちは応答全体（約5ms）の数%で、
  SQLite のクエリ実行時間そのものは分離していない。
- 判定: 同時実行時の待ちとして支持。ただし応答全体への寄与は小さい。

### 5. 状態ストアの複製

- 支持: layout の `GetOverview` と、`GetProjects` / `GetActivity` のハンドラが、同じ
  `assemble_state_for` を通って状態を2回組み立てる。状態の投影と SQLite のクエリも2回走る。
- 判定: 支持。3 と同じ根本原因で、要求ごとの状態組み立ての重複として整理できる。

## 未計測・制約

- **応答の遅さそのものは再現していない。** この条件の TTFB は 4〜7ms で、issue が扱う「遅い」状態を
  再現していない。原因の切り分けは、この負荷での内訳の相対関係に基づく。
- **状態ストアの実データ量を確認していない。** backend のログに `takt-dash` の MCP 読み取り失敗が
  あった。プロジェクト数は設定上4件だが、run・イベントの件数は計測時に確認していない。
- **`GetPendingDecisions` と `GetRunDetail` の実測値はない。** 4ページのサーバー側経路では呼ばれない。
  計測の実装は5つの unary すべてにあり、`backend/tests/rpc_timing_test.rs` で確認している。
- **クライアント再利用の比較は未計測。** 修正にあたるため、このタスクでは行わない。
- **`lock_wait_us` はプロセス全体の累積カウンタの差**で、同じ時間帯の別ハンドラの待ちも含む。
  逐次1要求の `/` と `/decisions` では 0 だが、並行する `/projects` と `/activity` では
  2つのハンドラが同じ値を報告する。
- **`Server-Timing` は採用していない。** Next.js の RSC 応答に任意のヘッダーを設定する公開 API を
  確認できず、AC72A-1 がログまたは Server-Timing を許容しているため、ログで満たした。
- **`next_ms` は Next.js 側の実コストを過小評価する。** React のシリアライズと HTTP 転送を
  含まないため、応答全体は TTFB と併記して読む必要がある。

## 再現手順

```sh
# 1. backend（専用ポートと SQLite）
cargo build --bin takt-dash-backend
OUT=$(mktemp -d)
./target/debug/takt-dash-backend --config taktdash.config.json \
  --sqlite "$OUT/taktdash.sqlite" --listen 127.0.0.1:50061 > "$OUT/backend.log" 2>&1 &
BACK_PID=$!

# 2. Next.js（production build）
npm --prefix frontend/web run build
TAKTDASH_BACKEND_ADDR=127.0.0.1:50061 npm --prefix frontend/web run start -- -p 3111 > "$OUT/next.log" 2>&1 &
NEXT_PID=$!

# 3. ウォームアップと計測
for p in / /decisions /projects /activity; do
  for i in 1 2; do curl -sS -o /dev/null "http://127.0.0.1:3111$p"; done
done
for p in / /decisions /projects /activity; do
  for i in $(seq 1 10); do
    curl -sS -o /dev/null -w '%{time_starttransfer}\n' "http://127.0.0.1:3111$p"
  done
done

# 4. 後片付け
kill "$NEXT_PID" "$BACK_PID"
```

ログの確認:

```sh
grep 'scope=page' "$OUT/next.log"
grep 'scope=rpc' "$OUT/next.log"
grep 'unary RPC を処理しました。' "$OUT/backend.log"
```
