# takt-dash

taktDash は、takt manager 群の状態を集約する読み取り用のダッシュボードです。
バックエンド（Rust / gRPC）とフロントエンド（Next.js）で構成します。

## 必要なもの

- Rust ツールチェーン（`rust-toolchain.toml` に従う）
- Node.js と npm
- `protoc`（proto の生成に使う）
- 初回は `npm --prefix frontend/web ci` でフロントエンドの依存を入れる

## バックエンドを起動する

設定ファイル（既定は `taktdash.config.json`）を用意し、gRPC サーバーを起動します。

```bash
cargo build -p takt-dash-backend
./target/debug/takt-dash-backend \
  --config taktdash.config.json \
  --listen 127.0.0.1:50051 \
  --sqlite data/taktdash.sqlite
```

- `--listen`: 待ち受けアドレス。省略時は設定の `listen`、無ければ既定の `127.0.0.1:50051`。
- `--sqlite`: SQLite の置き場所。省略時は設定の `sqlite.path`。
- `--config`: 設定ファイルのパス。省略時は `taktdash.config.json`。

`taktdash.config.json` には takt-mcp の起動方法（`mcpServers`）、集約するサーバーと
プロジェクト（`servers` / `projects`）、読み取り間隔、異常判定の閾値を書きます。
takt-mcp に接続できない環境でも gRPC サーバー自体は起動し、状態は空として返します。

## 読み取りを常駐 HTTP サーバーへ向ける

`mcpServers` のエントリは、子プロセスを起動する stdio 起動方法か、ホストごとに常駐する
takt-mcp の HTTP サーバーの接続先（`type: "http"`）のどちらかです。HTTP 接続先に向けた
プロジェクトの読み取り（定期読み取りと画面の詳細）は、常駐サーバーへ `tools/call` を送ります。
HTTP サーバーは `127.0.0.1` のみで待ち受け、`Authorization: Bearer <token>` の完全一致を要求します。
書き込み操作（回答・マージ・承認・却下）は stdio の子プロセスが担います。

`taktdash.config.json` の `mcpServers` に、ホストごとの HTTP 接続先を登録します。値は環境依存です。
`taktdash.config.json` は `.gitignore` の対象で、リポジトリには実トークンを入れません。

```json
{
  "mcpServers": {
    "mac-mini-read": {
      "type": "http",
      "url": "http://127.0.0.1:43117/",
      "headers": { "Authorization": "Bearer <実際のトークン>" }
    }
  }
}
```

URL のホストは `127.0.0.1` に限ります。`Authorization` は `Bearer ` で始まる必要があります。

読み取りを HTTP へ向けたプロジェクトでも、書き込みは stdio の takt-mcp へ向けます。プロジェクトの
`server` が読み取り接続先、`writeServer` が書き込み接続先です。`writeServer` は stdio（`command`）の
エントリである必要があり、省略した場合は `server` と同じ id を使います。

```json
{
  "mcpServers": {
    "mac-mini-read": {
      "type": "http",
      "url": "http://127.0.0.1:43117/",
      "headers": { "Authorization": "Bearer <実際のトークン>" }
    },
    "mac-mini-write": { "command": "node", "args": ["/Users/you/local/takt-custom/dist/app/mcp/index.js"], "env": {} }
  },
  "projects": [
    { "name": "takt-dash", "server": "mac-mini-read", "writeServer": "mac-mini-write", "cwd": "/Users/you/Sources/j5ik2o.github.com/j5ik2o/takt-dash" }
  ]
}
```

### 常駐サーバーを起動・停止・再起動する

実機の takt-custom の dist にある takt-mcp を、ポートとトークンを固定して HTTP モードで起動します。

```bash
# 起動（--http-port と --http-token で固定する。トークンは環境依存の設定から渡す）
node /Users/you/local/takt-custom/dist/app/mcp/index.js --http --http-port 43117 --http-token "$TAKTDASH_HTTP_TOKEN"
# 停止
kill "$(cat ~/Library/Application\ Support/taktdash/takt-mcp-http.pid)"
# 再起動
kill "$(cat ~/Library/Application\ Support/taktdash/takt-mcp-http.pid)"
node /Users/you/local/takt-custom/dist/app/mcp/index.js --http --http-port 43117 --http-token "$TAKTDASH_HTTP_TOKEN"
```

サーバーは起動時に `MCP HTTP listening on http://127.0.0.1:<port>/` を標準エラーへ出します。
サーバーが落ちていると、backend の読み取りは空の結果ではなく失敗として表面化し、
該当プロジェクトの接続状態が遅延中・応答なしへ進みます（`プロジェクトの読み取りに失敗しました` のログ）。

### launchd に登録する

`~/Library/LaunchAgents/com.example.taktdash-takt-mcp-http.plist` を作り、起動時に常駐サーバーを
立ち上げます。トークンは plist に直書きせず、`launchctl setenv` で渡します。
`ProgramArguments` は環境変数を展開しないため、シェルを経由して環境からトークンを読ませます。

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.example.taktdash-takt-mcp-http</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>exec node /Users/you/local/takt-custom/dist/app/mcp/index.js --http --http-port 43117 --http-token "$TAKTDASH_HTTP_TOKEN"</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/tmp/taktdash-takt-mcp-http.err.log</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.example.taktdash-takt-mcp-http.plist    # 登録して起動
launchctl unload ~/Library/LaunchAgents/com.example.taktdash-takt-mcp-http.plist  # 停止して解除
launchctl kickstart -k gui/"$(id -u)"/com.example.taktdash-takt-mcp-http           # 再起動
```

## フロントエンドを起動する

バックエンドの待ち受けアドレスを `TAKTDASH_BACKEND_ADDR` で指定して起動します。

```bash
TAKTDASH_BACKEND_ADDR=127.0.0.1:50051 npm --prefix frontend/web run dev
```

- `TAKTDASH_BACKEND_ADDR`: フロントエンドが接続するバックエンドのアドレス。未設定時は
  `127.0.0.1:50051`。
- `TAKTDASH_ALLOWED_HOSTS`: 公開の DNS で引ける名前で開くときに、書き込みを追加で許可するホストの一覧。詳しくは下の「書き込みを許可するホスト」で説明します。
- 待ち受けポートを変えるときは `-- --port <番号>` を付けます。
- 開発サーバーの既定は http://localhost:3000。

固定ポート（3000・50051）が使用中のときは、バックエンドの `--listen` とフロントエンドの
`--port`、`TAKTDASH_BACKEND_ADDR` を空きポートに揃えて起動してください。

### 書き込みを許可するホスト

判断待ちの書き込み API は、ブラウザが送る Origin ヘッダのホストと Host ヘッダを比べます。
次の3つをすべて満たすときだけ、API は書き込みを受け付けます。

1. Origin ヘッダがある。
2. Origin のホストとポートが、Host ヘッダと一致する（画面を開いた URL と同じオリジンからの要求）。
3. そのホスト名が、次のどれかに当たる。
   - IP アドレス（LAN の IP、VPN 経由で見えるアドレス、Tailscale の IP、`127.0.0.1` など）
   - ドットの無い名前（`localhost`、`my-mac` など）
   - 私設の名前（`.local`、`.lan`、`.home.arpa`、`.internal`、`.localhost` で終わる名前）
   - 環境変数 `TAKTDASH_ALLOWED_HOSTS` で許可した名前

機械の IP やポートが変わっても、3 の最初の3つに当たる URL で開いていれば、設定は要りません。

`TAKTDASH_ALLOWED_HOSTS` は、公開の DNS で引ける名前（Tailscale の `*.ts.net` の名前など）で開くときにだけ設定します。

- 形式: `host:port` をカンマで区切って並べます（例: `my-mac.example.ts.net:3000`）。
  前後の空白は取り除かれ、大文字と小文字は区別されません。`http://` やパスを付けた値は無効となり、一覧から除かれます。
- フロントエンドはこの値をリクエストのたびに読みます。

```bash
TAKTDASH_ALLOWED_HOSTS=my-mac.example.ts.net:3000 npm --prefix frontend/web run start
```

API は、次の要求を 403 で拒否します。

- Origin ヘッダの無い要求（curl など、ブラウザ以外からの直接の要求）
- Host ヘッダと Origin ヘッダのホストが一致しない要求（別のサイトのページからの要求）
- `sec-fetch-site: cross-site` が付いた要求
- 公開の DNS で引ける名前で、設定で許可していないもの。攻撃者が自分の名前の向き先をこの機械へ
  向け直す手口（DNS リバインディング）では、Host と Origin がその名前でそろうためです。

この検査は、ブラウザ経由の別サイトからの書き込みと、ヘッダを付けない直接の要求を止めます。
認証ではありません。Web に届く機械の上で Origin と Host を自分で付けられるクライアントは通ります。

フロントエンドはビルド時に git から短縮コミット SHA とブランチ名を取得し、
`TAKTDASH_FRONTEND_COMMIT` / `TAKTDASH_FRONTEND_BRANCH` として埋め込みます（手動設定は不要）。
git を取得できない環境では `unknown` になります。サイドバー下端には frontend と backend の
短縮 SHA とブランチ名を表示し、両方のコミットが取得できていて異なるときだけ「不一致」を示します。

## TAKT でこのリポジトリを扱うときの運用

TAKT は、同じ PR やブランチを対象にした run どうしを、実行の間は排他しません（issue #336）。
同じブランチに2つの run が重なると、互いの push や Caccia の対応がぶつかります。重ならないよう、次の決まりを守ります。

- PR ブランチの作業は、キュー経由（`takt run` か `takt watch`）に限ります。
  `takt --pipeline` を直接打たないでください。`--pipeline` は実行のロックもタスクストアも通りません。
- キューを処理するチェックアウトは、このリポジトリにつき1つにします。実行のロック（`.takt/execution.lock`）はチェックアウトごとに別です。
- merge のタスクは `takt_merge_goal_pr` で登録します。この経路は、同じ PR やブランチの merge タスクがすでにあるかを照合します。
  `takt add --pr` と MCP の enqueue は、同じ PR のタスクを重ねて登録できてしまいます。
- `.takt/config.yaml` の `concurrency` は 1 にしています。TAKT はキューから取り出すとき、実行中のタスクと同じブランチかを照合しないためです。
  そのぶん、別のブランチのタスクも1件ずつ順に動き、並列で進める利点はなくなります。

## 検証する

```bash
make done
```

`make done` は次を順に実行します。

- `make proto-check`: proto から生成物を再生成し、差分が無いことを確認
- `make fmt`: `cargo +nightly fmt --all -- --check`
- `make clippy`: `cargo clippy --workspace --all-targets -- -D warnings`
- `make test`: `cargo test --workspace` と `npm --prefix frontend/web run test`
- `make typecheck`: `npm --prefix frontend/web run typecheck`
- `make lint`: `npm --prefix frontend/web run lint`

### aislop の受け入れ一覧

`scripts/aislop-accepted.json` の `accepted` は、aislop の指摘を一時的に受け入れる一覧です。
指摘はコードの修正で解消するのが原則で、受け入れは次の条件を満たす例外です。

- エントリの追加には窓口の承認が必要です。TAKT の run（fix_aislop_gate など）は自分で追加・変更しません。
- `reason` には、解消を追跡する issue の URL（`https://github.com/j5ik2o/takt-dash/issues/<番号>`）を書きます。
- 各 finding の `maxLines` に、受け入れ時点の対象ファイルの実測行数を記録します。
- 解消 issue が閉じたら、そのエントリを削除します。

`tools/repo-gate` の workflow-machine-gate 検査（`make test` の `repo_gate_test.py` からも実行）は、
次の場合に失敗します。

- `reason` に issue の URL が無い。
- `maxLines` が無い（findings が空の場合を含む）。
- 対象ファイルが `maxLines` を超える行数になった。

`missingToolsAllowed` はこの検査の対象外です。

## proto を変更したとき

API の正は `proto/taktdash.proto` です。変更したら Rust（tonic）と TypeScript の生成物を
`make proto-gen` で再生成し、生成物は手で編集しません。

```bash
make proto-gen   # frontend/web/src/proto を再生成
make proto-check # 再生成後の差分が 0 であることを確認
```

Rust 側の生成物は `backend/build.rs` がビルド時に `proto/taktdash.proto` から生成します。

## 起動の実測結果（issue #73 / AC5）

空きポートで実バックエンドと実フロントエンドを起動し、ヘッドレス Chrome の `--dump-dom` で
サイドバー下端を確認しました。

```bash
# backend（空きポート 53579）
./target/debug/takt-dash-backend --config taktdash.config.json \
  --listen 127.0.0.1:53579 --sqlite <一時ディレクトリ>/taktdash.sqlite

# frontend（空きポート 53580）
TAKTDASH_BACKEND_ADDR=127.0.0.1:53579 npm --prefix frontend/web run dev -- --port 53580

# 確認（ヘッドレス Chrome）
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --disable-gpu --no-sandbox --virtual-time-budget=8000 \
  --dump-dom http://127.0.0.1:53580/
```

実際に得られたサイドバー下端の表示:

```text
frontend 1c6e486 (takt/20261116T0138-issue-73-https-github-com-j5ik)
backend  1c6e486 (takt/20261116T0138-issue-73-https-github-com-j5ik)
```

確認に使ったスクリーンショット・DOM ダンプ・`next dev` の生成物はコミットしません。
