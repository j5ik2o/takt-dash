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

### backend を反映する

launchd のジョブで動かしている backend は、反映するチェックアウトで次のスクリプトを流して作り直します。
plist の `ProgramArguments` の先頭が、そのチェックアウトの `target/debug/takt-dash-backend` で、`RunAtLoad` が true であることが前提です。

```sh
scripts/deploy-backend.sh --label <backend のジョブのラベル>
scripts/deploy-backend.sh --label <backend のジョブのラベル> --skip-build   # 前の版のバイナリへ戻したあと
```

- スクリプトは `cargo build --locked -p takt-dash-backend` でビルドし、`launchctl bootout` と `launchctl bootstrap` でジョブを起こし直します。
  起こし直したあとの3秒のあいだ、最初の pid のまま動き続けていなければ、`NG:` を出して 0 以外で終わります。
- `--plist` を省くと `~/Library/LaunchAgents/<ラベル>.plist` を読みます。
- ビルドに失敗したときは、ジョブに触らずに終わります。動いている版はそのままです。
- `launchctl kickstart -k` で起こし直してはいけません（issue #402）。ログインのときに読まれたジョブには、
  macOS の Background Task Management が、plist の実行ファイルの署名から作った起動の制約を付けます。
  ad-hoc の署名の cdhash はビルドのたびに変わるため、作り直したあとの最初の起動がこの制約に合わず、
  カーネルが `OS_REASON_CODESIGNING`（Launch Constraint Violation）で止めます。backend は、launchd が起こし直すまでの約8秒止まります。
  `launchctl bootstrap` で読み直したジョブには、この制約が付きません。
- 前の版のバイナリを置き直して戻すときも、同じ理由で `--skip-build` を付けて流します。
- `bootstrap` は plist を読み直します。plist を書き換えたまま反映すると、その書き換えも効きます。

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
    "mac-mini-write": {
      "command": "node",
      "args": ["/Users/you/local/takt-custom/dist/app/mcp/index.js"],
      "env": { "PATH": "/Users/you/local/takt-custom/bin:/Users/you/.local/share/mise/installs/node/24/bin:/Users/you/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin" }
    }
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
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>/Users/you/local/takt-custom/bin:/Users/you/.local/share/mise/installs/node/24/bin:/Users/you/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/tmp/taktdash-takt-mcp-http.err.log</string>
</dict>
</plist>
```

`PATH` の先頭は共有 TAKT の `bin` にし、`node` の置き場も入れます（`ProgramArguments` の `node` はこの `PATH` で探します）。
タスクのエージェントの CLI や `gh` もこの `PATH` で探すので、その置き場（例の `~/.local/bin`・`/opt/homebrew/bin`）も入れます。
takt-mcp が起こす daemon とその子（タスクのエージェント・終了フック）はこの `PATH` を継ぐので、
先頭が mise の shims だと、タスクの中の `takt` が npm の takt になります（「run の前に TAKT の版を確かめる」）。

`PATH` を変えたら、`kickstart` ではなく `bootout` と `bootstrap` で plist を読み直させます。
`kickstart` は、launchd が覚えている前の設定のまま起こし直します。

```bash
launchctl bootstrap gui/"$(id -u)" ~/Library/LaunchAgents/com.example.taktdash-takt-mcp-http.plist  # 登録して起動
launchctl bootout gui/"$(id -u)"/com.example.taktdash-takt-mcp-http                               # 停止して解除
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

### run の前に TAKT の版を確かめる

`.takt/config.yaml` には、フォークにしかないキー（`task_finish_hook`）を書きます（いまは外してあります。戻す条件は `.takt/README.md`）。
mise で入れた npm の takt（0.77.0）はこのキーを知らず、キーがあると `takt list` でも設定エラーで止まります。
そこで、根の `mise.toml` で `PATH` の先頭を共有 TAKT（`~/local/takt-custom/bin`）にしています。
共有 TAKT の切替の手順は、フォーク（`j5ik2o/takt` の `integration/takt-mcp`）の `docs/shared-takt-switch.md` にあります。

#### mise.toml が効くところ

`mise.toml` が効くのは、mise がこのチェックアウトで `PATH` を組み直すときだけです。

- 効く: 対話シェルでこのディレクトリへ `cd` したとき、Orca の対話端末をこのチェックアウトで開いたとき、
  `mise exec -C <チェックアウト> -- takt ...`。
- 効かない:
  - Orca のボットと構造化ワーカー（`orca-provider-exec` が起こすエージェント）。起動したときの固定の `PATH` で動き、
    `cd` しても `mise.toml` を読みません。
  - 非対話の `zsh -c`。dotfiles の `.zshenv` が mise の shims を `PATH` の先頭に積み直すため、`takt` は
    `shims/takt`（npm の takt）になります。dotfiles の直しが反映されるまで、この形のままです。
  - `PATH` が mise の shims だけのとき（`shims/takt` は `_.path` があっても npm の takt を起こします）。
  - launchd。plist の `PATH` で先頭を共有 TAKT にします（「launchd に登録する」）。
- ボット、ワーカー、スクリプトから `takt` を呼ぶときは、`mise exec -C <チェックアウト> -- takt ...` か、
  絶対パス `~/local/takt-custom/bin/takt` を使います。
- MCP の stdio の起動（`.mcp.json`、`taktdash.config.json` の `command`）は、`env` の `PATH` の先頭に
  `~/local/takt-custom/bin` を書きます（`taktdash.config.example.json` と上の例）。`taktdash.config.json` の
  `env` の `PATH` は、backend から継いだ `PATH` を置き換えます（`${PATH}` は展開しません）。`node`、
  エージェントの CLI、`gh` の置き場も並べます。`.mcp.json` は Claude Code が `${PATH}` を展開するので、後ろに `${PATH}` を付けます。

#### mise の信頼

`mise.toml` は、信頼したディレクトリでだけ読まれます。信頼していないと `PATH` は 0.77.0 に戻り、
そのディレクトリの中の mise の shims（`python3`・`node`）はエラーで止まります。
終了フックの `python3` も shims から起きることがあるので、通知も止まります。

- 初めて置いたチェックアウトでは `mise trust` を流します。`mise trust <ディレクトリ>` は、`mise.toml` が届く前に流しても効きます。
- Orca の作業ツリー（`~/orca/workspaces/takt-dash/<名前>`）は、mise の `trusted_config_paths` に入っていません。
  入るまでは、作業ツリーを作るたびにその中で `mise trust` を流します。
- TAKT の作業用クローンの置き場（`~/Sources/j5ik2o.github.com/j5ik2o/takt-worktrees`）は `trusted_config_paths` に入っています。
- 信頼はディレクトリのパスに付きます。ディレクトリを移したり名前を変えたりしたとき、別の場所へ `git clone` したときは、
  新しいパスでもう一度 `mise trust` を流します。mise の `paranoid` を有効にしているときは、`mise.toml` を書き換えるたびに信頼し直します。

#### 確かめる

run の前に、run を流すのと同じシェルで `make takt-check` を流します。終了コードが 0 でなければ、`NG:` の行が出ていなくても run しません
（フォークを取り出せないときは `NG:` が出ません）。

```sh
make takt-check                                   # このシェルの PATH の takt を確かめる
mise exec -C <チェックアウト> -- make takt-check   # mise exec で流すときの takt を確かめる
```

- 中身は `scripts/takt-check.sh` です。フォークのチェックアウト（`TAKT_REPO`、既定は `~/Sources/github.com/j5ik2o/takt`）の
  `origin/integration/takt-mcp` から `scripts/check-takt-runtime.sh` を一時ファイルへ取り出し、
  `--key task_finish_hook --contains b1acb58ac` で流します。一時ファイルは終わるときに `trap` で消します。
- 確かめるのは、`PATH` の `takt` が共有 TAKT を指すこと、その版が `task_finish_hook` を受け付けること、
  配置した版が `b1acb58ac`（終了フックを呼ぶ処理）を含むことです。キーがあるだけでフックを呼ばない版があるため、
  `--contains` を外しません。
- `takt --version` では見分けられません。フォークは元にした上流の版番号を名乗ります。
- `make workflow-inspect` は、使う takt（`TAKT_BIN`、既定は `~/local/takt-custom/bin/takt`）で先に同じ確かめを流し、NG なら inspect しません。
- 1 番目（`PATH` の `takt`）が NG のときは、このチェックアウトの対話シェルへ `cd` し直すか、
  `mise exec -C <チェックアウト> -- takt ...` で流します。
- 2〜4 番目が NG のときは、共有 TAKT の版が古いか、配置が壊れています。`PATH` を直しても通りません。
  フォークの `docs/shared-takt-switch.md` の手順で、キーとフックを持つ版へ切り替えます。
- どちらの場合も、直したあと同じシェルで確かめを流し直し、終了コードが 0 になるまで run しません。

#### 戻し方

共有 TAKT を PATH の先頭にする形をやめるときは、次の順で戻します。キーが残ったまま npm の takt に戻ると、設定を読めなくなるためです。

1. `.takt/config.yaml` から `task_finish_hook` のブロックを消す（書いてあるとき）。
2. `mise.toml` を消す。
3. 信頼したディレクトリで `mise trust --untrust <ディレクトリ>` を流す。
4. launchd の plist を控えから戻し、`launchctl bootout` と `launchctl bootstrap` で読み直させる。

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
