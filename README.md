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

## フロントエンドを起動する

バックエンドの待ち受けアドレスを `TAKTDASH_BACKEND_ADDR` で指定して起動します。

```bash
TAKTDASH_BACKEND_ADDR=127.0.0.1:50051 npm --prefix frontend/web run dev
```

- `TAKTDASH_BACKEND_ADDR`: フロントエンドが接続するバックエンドのアドレス。未設定時は
  `127.0.0.1:50051`。
- 待ち受けポートを変えるときは `-- --port <番号>` を付けます（例: `npm --prefix frontend/web run dev -- --port 53580`）。
- 開発サーバーの既定は http://localhost:3000。

固定ポート（3000・50051）が使用中のときは、バックエンドの `--listen` とフロントエンドの
`--port`、`TAKTDASH_BACKEND_ADDR` を空きポートに揃えて起動してください。

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
