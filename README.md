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
立ち上げます。トークンは plist に直書きせず、環境依存の設定ファイルや `launchctl setenv` で渡します。

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.example.taktdash-takt-mcp-http</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/env</string>
    <string>node</string>
    <string>/Users/you/local/takt-custom/dist/app/mcp/index.js</string>
    <string>--http</string>
    <string>--http-port</string><string>43117</string>
    <string>--http-token</string><string>REPLACE_WITH_ENV_TOKEN</string>
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
- `TAKTDASH_ALLOWED_HOSTS`: ブラウザからの書き込みを受け付けるホストの一覧。詳しくは下の「書き込みを許可するホスト」で説明します。
- 待ち受けポートを変えるときは `-- --port <番号>` を付けます。ブラウザから書き込むときは、
  同じポートを `TAKTDASH_ALLOWED_HOSTS` にも設定します
  （例: `TAKTDASH_ALLOWED_HOSTS=localhost:53580,127.0.0.1:53580 npm --prefix frontend/web run dev -- --port 53580`）。
- 開発サーバーの既定は http://localhost:3000。

固定ポート（3000・50051）が使用中のときは、バックエンドの `--listen` とフロントエンドの
`--port`、`TAKTDASH_BACKEND_ADDR` を空きポートに揃えて起動してください。ブラウザから書き込むときは、
`TAKTDASH_ALLOWED_HOSTS` にもフロントエンドと同じポートを設定してください。

### 書き込みを許可するホスト

判断待ちの書き込み API は、ブラウザが送る Origin ヘッダのホストと Host ヘッダを比べます。
両者が一致し、そのホストが許可の一覧にあるときだけ、API は書き込みを受け付けます。
許可の一覧は環境変数 `TAKTDASH_ALLOWED_HOSTS` で指定します。

- 形式: `host:port` をカンマで区切って並べます（例: `localhost:53580,127.0.0.1:53580`）。
  前後の空白は取り除かれ、大文字と小文字は区別されません。`http://` やパスを付けた値は無効となり、一覧から除かれます。
- 未設定のとき、または空白だけのとき、API は次の4つのホストを許可します:
  `10.0.1.143:3000`、`100.106.166.1:3000`、`127.0.0.1:3000`、`localhost:3000`。
- 値を設定すると、その値が既定の一覧を置き換えます。既定の一覧に追加されるわけではないので、
  既定のホストも使うときは、それらも値に含めてください。
- API は、次の要求を 403 で拒否します。
  - 一覧に無いホストからの要求
  - Host ヘッダと Origin ヘッダのホストが一致しない要求
  - `sec-fetch-site: cross-site` が付いた要求
- Origin ヘッダの無い要求（CLI など）は照合の対象外で、API は一覧に関係なく受け付けます。
- フロントエンドはこの値をリクエストのたびに読みます。

フロントエンドはビルド時に git から短縮コミット SHA とブランチ名を取得し、
`TAKTDASH_FRONTEND_COMMIT` / `TAKTDASH_FRONTEND_BRANCH` として埋め込みます（手動設定は不要）。
git を取得できない環境では `unknown` になります。サイドバー下端には frontend と backend の
短縮 SHA とブランチ名を表示し、両方のコミットが取得できていて異なるときだけ「不一致」を示します。

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
