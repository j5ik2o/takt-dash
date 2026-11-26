# 設計：CI で Quint 仕様を検査する

`specs/quint/` 配下の Quint 仕様（`.qnt`）を GitHub Actions の CI で自動検査するジョブを追加するための設計書である。
手本：[j5ik2o/fraktor-ng-rs#216](https://github.com/j5ik2o/fraktor-ng-rs/pull/216)（`quint` ジョブと `scripts/quint-check-specs.sh`）。

## 背景と目的

`specs/quint/` 配下には現在 12 件の仕様（`.qnt` 24 ファイル、筋書きテスト 126 件）があるが、CI では実行していない。このため、仕様の型誤り・テスト失敗・不変条件の破れ・README の実行条件との食い違いが混入しても、PR 時点で機械的に気づけない。

本変更では、`specs/quint/` 配下のすべての仕様に対して `quint typecheck`・`quint test`・`quint run` を実行する独立ジョブ `quint` を `.github/workflows/ci.yml` に追加する。

なお、このジョブが直接確かめるのは仕様ファイル（`.qnt`）自身の整合性とマニフェスト／README との一致であり、Rust / TypeScript 実装との機械的な照合は行わない。実装が仕様の性質を守るかは、各仕様の README が対応づけている Rust・Web・E2E のテスト群（`done`・`e2e` ジョブ）が引き続き受け持つ。

## 何をどう変えるか

| ファイル | 変更内容 |
| --- | --- |
| `package.json` / `package-lock.json` | `devDependencies` に `"@informalsystems/quint": "0.32.0"` を範囲指定なし（厳密固定）で追加し、`npm ci` で再現可能にする。 |
| `specs/quint/ci-specs.json` | 新規。`specs/quint/` 配下の各仕様について、仕様ファイル・テストファイル・`quint test` の `--main` 一覧・`quint run` の `--main` / `--invariant` / `--max-steps` / `--max-samples` を宣言するマニフェスト。各仕様の `README.md` の値に合わせる。 |
| `scripts/quint-check-manifest.py` | 新規。`specs/quint/` 配下の全 `.qnt` ファイルと `specs/quint/ci-specs.json` の 1 対 1 一致（未登録仕様・欠損ファイル・重複・不正値）および各仕様の `README.md` に書かれた `quint test` / `quint run` コマンドとの一致を検証する補助スクリプト。 |
| `scripts/quint-check-specs.sh` | 新規。検査本体。`./node_modules/.bin/quint` だけを使い、(1) マニフェストと README の整合検証、(2) 全 `.qnt` の `quint typecheck`、(3) 全 `_test.qnt` の IR 検査と `quint test`、(4) 各仕様の不変条件の `quint run` を順に実行する。手元でもリポジトリ直下から `bash scripts/quint-check-specs.sh` で同じ検査を流せる。 |
| `.github/workflows/ci.yml` | `quint` ジョブを追加する（`needs: ci-revision`、`timeout-minutes: 10`）。Rust 評価器をキャッシュ＋認証付き `gh release download` で取得し、tar.gz と実行ファイルの両方を固定 sha256 で照合した上で `bash scripts/quint-check-specs.sh` を実行する。必須チェック（`required-ci`）への組み込み案は後述。 |
| `docs/design/quint-ci.md` | 本設計書。 |

### `scripts/quint-check-specs.sh` の検査手順

1. **前段検証（マニフェスト・網羅性・README 一致）**
   - `python3 scripts/quint-check-manifest.py` を実行する。
   - `specs/quint/` 配下を `find` 相当で再帰走査し、見つかった全 `.qnt` ファイルの集合が `specs/quint/ci-specs.json` に登録された `spec`・`test` の集合と完全一致することを検証する（0 件・未登録ファイル・欠損ファイルはいずれも失敗）。
   - 各仕様ディレクトリに `README.md` が存在し、`ci-specs.json` に書いた全 `test_modules`（`quint test ... --main ...`）および全 `runs`（`quint run ... --main ... --invariant ... --max-steps ... --max-samples ...`）が、その `README.md` 内のコードブロックのコマンドと一致することを検証する（`\` の行継続と `--flag val` / `--flag=val` の両書式を正規化して照合）。
2. **型検査（`quint typecheck`）**
   - 各仕様のモデル本体 `.qnt` とテストファイル `*_test.qnt` のそれぞれに対して `./node_modules/.bin/quint typecheck <file>` を 1 ファイルずつ実行する。
   - モデル本体 `.qnt` を `quint parse` した IR に `qualifier == "run"` が紛れ込んでいないことも確かめる（モデル側に書かれた `run` がテストされずに放置されるのを防ぐ）。
3. **筋書きテスト（`quint test`）**
   - 各 `*_test.qnt` を `quint parse --out=...` で IR に落とし、`jq` で次を検証する（[j5ik2o/fraktor-ng-rs#216](https://github.com/j5ik2o/fraktor-ng-rs/pull/216) の F1・F2 対策と同様）：
     - `qualifier == "run"` を 1 本以上持つモジュール集合が、`ci-specs.json` の `test_modules` と完全一致すること（新しいテストモジュールを追加してマニフェストへ書き忘れた場合に落とす）。
     - 各 `test_modules` に `run` が 1 本以上あること（0 本で `quint test` が exit 0 になるのを防ぐ）。
     - 各 `run` の名前がすべて `Test` で終わること（`quint test` は `--match` なしでは `Test` 語尾しか流さないため、名前違いによる黙ったスキップを防ぐ）。
   - 各 `test_modules` ごとに `./node_modules/.bin/quint test --main="$mod" "$test_file"` を実行し、出力の `N passing` が IR から数えた `run` 本数と一致することを検証する。
4. **不変条件のシミュレーション（`quint run`）**
   - 各仕様の `runs` ごとに次を実行する：
     `./node_modules/.bin/quint run --main="$main" --invariant="$invariant" --max-steps="$max_steps" --max-samples="$max_samples" --verbosity=2 "$spec_file"`
   - 出力から `Trace length statistics: max=N` を読み、行が存在しない場合（出力形式の変化）または `N <= 1` の場合（`step` が一度も発火せず初期状態しか検査していない場合）は落とす。
   - **seed の方針**：手本（[j5ik2o/fraktor-ng-rs#216](https://github.com/j5ik2o/fraktor-ng-rs/pull/216)）にならい、`--seed` は固定せず毎回乱数で実行する。回を重ねるほど探索が広がり、違反が出た場合は必ず本物の反例であってログの `Use --seed=0x... --backend=rust to reproduce.` で再現できるため。
   - **無関係な PR が本物の反例で止まったときの手順**：`quint` は必須チェックに入るため、仕様を変えていない PR でも乱数探索でまれに本物の反例が見つかって止まることがある。その場合は (1) ログの `Use --seed=0x... --backend=rust to reproduce.` で手元再現し、(2) 反例を添えて `種別:バグ` の issue を立てて仕様か実装を直し、(3) 止まった PR はその直しが `main` に入ってから追随する（`scripts/quint-check-specs.sh` の失敗時メッセージにも同じ手順を出力する）。
5. **全件実行と失敗報告**
   - 1 仕様で失敗しても残りの仕様をすべて実行し、最後に失敗した仕様を `::error file=...::` で列挙して非ゼロ終了する。

### 範囲外（本 PR で行わないこと）

- 既存の `.qnt` 仕様ファイルおよび各仕様の `README.md` の中身は変更しない。
- `quint verify`（Apalache による有界モデル検査）は入れない（JVM / Apalache 導入と実行時間の重さが別物であるため）。
- `specs/quint/mcp-child-process/check_mutations.py`（19 変異の個別確認スクリプト）や、各仕様 README に載っている修正前・変異モジュールの「反例が出ること」の確認は、本件の受け入れ条件（typecheck / `_test.qnt` の test / 各仕様の不変条件の run）の範囲外とする。
- `Makefile` の `make done` には組み込まない（ローカルに Linux 向け評価器 sha256 や `quint` 導入を強制せず、CI では `quint` ジョブを並列実行し、手元では `bash scripts/quint-check-specs.sh` で再現できるようにする）。
- GitHub のリポジトリルールセットは変更しない。
- 画面の待ち受け（`0.0.0.0:3000`、LAN・SoftEther VPN・tailscale からの利用）に関わるコード・設定は一切変更しない。

## どの仕様を何で検査するかの表

現在 `specs/quint/` 配下にある 12 仕様（24 `.qnt` ファイル、筋書きテスト 126 件、`quint run` 13 構成）をすべて対象とする。パラメータはすべて各ディレクトリの `README.md` の記載値に合わせている。

| ディレクトリ（`specs/quint/` 配下） | `typecheck` 対象 | `quint test`（ファイル / `--main` / テスト数） | `quint run`（`--main`） | `--invariant` | `--max-steps` | `--max-samples` |
| --- | --- | --- | --- | --- | ---: | ---: |
| `activity-failure-events` | `activityFailureEvents.qnt`<br>`activityFailureEvents_test.qnt` | `activityFailureEvents_test.qnt`<br>`currentTest` (1), `fixedTest` (5) | `fixed` | `safety` | 30 | 10000 |
| `backend-shutdown-deadline` | `shutdownDeadline.qnt`<br>`shutdownDeadline_test.qnt` | `shutdownDeadline_test.qnt`<br>`shutdownDeadline_test` (11) | `fixed` | `safety` | 60 | 50000 |
| `decision-offline-actions` | `offlineActions.qnt`<br>`offlineActions_test.qnt` | `offlineActions_test.qnt`<br>`offlineActions_test` (10) | `fixed` | `safety` | 30 | 20000 |
| `decision-pending-check` | `pendingCheck.qnt`<br>`pendingCheck_test.qnt` | `pendingCheck_test.qnt`<br>`pendingCheck_test` (7) | `fixed` | `writesMatchPendingList and appliedRevisionIsCurrent` | 20 | 50000 |
| `decision-write-retry` | `writeRetry.qnt`<br>`writeRetry_test.qnt` | `writeRetry_test.qnt`<br>`writeRetry_test` (5) | `fixed` | `safety` | 20 | 50000 |
| `mcp-child-process` | `childProcess.qnt`<br>`childProcess_test.qnt` | `childProcess_test.qnt`<br>`childProcess_test` (15) | `current` | `safety` | 35 | 50000 |
| `mcp-http-session` | `httpSession.qnt`<br>`httpSession_test.qnt` | `httpSession_test.qnt`<br>`httpSession_test` (10) | `fixed` | `safety` | 30 | 50000 |
| `mcp-http-session-endpoint` | `sessionEndpoint.qnt`<br>`sessionEndpoint_test.qnt` | `sessionEndpoint_test.qnt`<br>`sessionEndpoint_test` (10) | `fixed` | `safety` | 25 | 50000 |
| `poller-event-recording` | `pollerEventRecording.qnt`<br>`pollerEventRecording_test.qnt` | `pollerEventRecording_test.qnt`<br>`currentTest` (1), `fixedTest` (3) | `fixed` | `safety` | 30 | 10000 |
| `run-detail-deadline` | `runDetailDeadline.qnt`<br>`runDetailDeadline_test.qnt` | `runDetailDeadline_test.qnt`<br>`runDetailDeadline_test` (12) | `fixed` | `safety` | 60 | 100000 |
| `state-stream-relay` | `stateStreamRelay.qnt`<br>`stateStreamRelay_test.qnt` | `stateStreamRelay_test.qnt`<br>`stateStreamRelay_test` (19) | `fixed` | `safety` | 40 | 20000 |
| `state-stream-watchdog` | `stateStreamWatchdog.qnt`<br>`stateStreamWatchdog_test.qnt` | `stateStreamWatchdog_test.qnt`<br>`stateStreamWatchdog_test` (17) | `fixed`<br>`fixedOpenSession` | `safety`<br>`safety` | 40<br>60 | 20000<br>20000 |

補足：
- `mcp-child-process` は後追い仕様のため、現在の実装（PR #379 適用後）を表すモジュール名が `current` になっている（他の 11 仕様は `fixed`）。
- `decision-pending-check` は `safety` を定義しておらず、README に合わせて式 `writesMatchPendingList and appliedRevisionIsCurrent` をそのまま `--invariant` に渡す。
- `activity-failure-events` と `poller-event-recording` は 1 つの `_test.qnt` に `currentTest` と `fixedTest` の 2 モジュールを持つため、両方を `--main` 指定で実行する。
- `state-stream-watchdog` は README で `fixed`（40 手・20,000 サンプル）と `fixedOpenSession`（60 手・20,000 サンプル）の 2 構成を検査しているため、両方を流す。
- ローカル（macOS / quint 0.32.0 / Rust 評価器 v0.6.0）での全 12 仕様・13 構成（`typecheck`・`test`・`run` 全体）の実測所要時間は合計約 70 秒。

## 固定する版と sha256

| 対象 | 固定する版 | 取得元・検証方法 | ハッシュ値 |
| --- | --- | --- | --- |
| `@informalsystems/quint` | `0.32.0` | `package.json` に厳密版（`"0.32.0"`）を書き、`package-lock.json` の `integrity` を `npm ci` で検証する。実行は `./node_modules/.bin/quint` を直接呼び、`npx` や PATH 上の別版を使わない。 | npm tarball `integrity`（`package-lock.json` で固定）：<br>`sha512-9Yoji7DU6moRm1ofDaN+ftdxNcEGciiGbm7eN4MjlcCbfv7/Y/zchadcXXRmMZmN9oTPXUk9GmhjQOhADJsiSQ==` |
| Rust 評価器アーカイブ<br>（`quint_evaluator-x86_64-unknown-linux-gnu.tar.gz`） | `v0.6.0`<br>（`informalsystems/quint` のタグ `evaluator/v0.6.0`） | 導入済み `quint` の `QUINT_EVALUATOR_VERSION` から版を読み、キャッシュミス時に `GH_TOKEN: ${{ github.token }}` で `gh release download` し、**展開前に** `sha256sum -c` で照合する。 | `sha256`:<br>`61755a09d5052d93a4e75e840059edfd0d3674aeda164b9d2464be3d6e21b1c2` |
| Rust 評価器実行ファイル<br>（`~/.quint/rust-evaluator-v0.6.0/quint_evaluator`） | `v0.6.0` | 展開後および `actions/cache` ヒット時の**毎回**、`test -x` と `sha256sum -c` で照合する。キャッシュキーにも `executable_sha256` を含める。 | `sha256`:<br>`b2efdeac5713d153e41bf2143b94ed75d888fdd5637f4a5d61a04c695313510a` |

Rust 評価器をジョブ側で事前取得・検証する理由（[j5ik2o/fraktor-ng-rs#216](https://github.com/j5ik2o/fraktor-ng-rs/pull/216) と同じ）：
1. `quint 0.32.0` の既定バックエンド（Rust 評価器）は npm パッケージに含まれず、初回実行時に認証なしの GitHub Releases API（1 ページ目 30 件のみ）からダウンロードを試みるため、共有ランナーの IP レート制限（60 回/時）や今後のリリース増加による 1 ページ目落ちで不安定になる。
2. `evaluator/v0.6.0` リリースは `isImmutable: false` であり資産が差し替えられうるため、アーカイブと実行ファイルの両方を固定 sha256 で検証する。
3. 実行ファイルを `~/.quint/rust-evaluator-<版>/quint_evaluator` に配置しておけば、`quint` は外部 API を一切呼ばずにローカルの実行ファイルを使う。

## 壊しうる入力

| 分類 | 具体的な状況・入力 | 期待する振る舞い（どう防ぐか） |
| --- | --- | --- |
| 仕様が増えたとき | `specs/quint/<新ディレクトリ>/foo.qnt` を追加したが `ci-specs.json` に登録し忘れた（深い階層を含む） | `quint-check-manifest.py` が `specs/quint/` 配下の全 `.qnt` と `ci-specs.json` を集合比較し、未登録ファイルを `::error file=...::` で名指しして落とす。黙って未検査にならない。 |
| 仕様が増えたとき | 既存仕様の `*_test.qnt` に新しいテストモジュールを足したが `ci-specs.json` の `test_modules` に書き忘れた | `quint parse` の IR から `qualifier == "run"` を持つ全モジュールを抽出し、`test_modules` と完全一致しないため落とす。 |
| 仕様が増えたとき | `*_test.qnt` に `Test` で終わらない名前の `run` を書いた、または `run` が 0 本になった | IR 検査で `Test` 語尾でない `run` 名または 0 本を検出して落とす。さらに `quint test` の `N passing` と IR の `run` 数を突き合わせる。 |
| 仕様が減った／空のとき | `specs/quint/` に `.qnt` が 1 本も見つからない、または `ci-specs.json` に存在しないファイルが残っている | 0 本検出・欠損ファイル検出で明示的に `::error::` を出して落とす。 |
| README と合わないとき | 誰かが `specs/quint/<dir>/README.md` の `quint run` の `--max-steps`・`--max-samples`・`--invariant`・`--main` や `quint test` の `--main` を更新したが、`ci-specs.json` を更新しなかった（または逆） | `quint-check-manifest.py` が各仕様の `README.md` のコードブロックから `quint test` / `quint run` コマンドを解析し、`ci-specs.json` の設定と完全一致するコマンドが無い場合に `::error file=specs/quint/<dir>/README.md::` で落とす。 |
| README と合わないとき | 仕様ディレクトリに `README.md` が無い | `quint-check-manifest.py` が欠損として落とす。 |
| 評価器を取れないとき | キャッシュミスかつ GitHub Releases 障害・トークン不備・資産名変更で `gh release download` が失敗する | `set -euo pipefail` により即座にジョブを失敗させる。無認証 API へのフォールバックや検査スキップはしない。 |
| 評価器が改ざん・版ズレしたとき | リリース資産が差し替えられた、キャッシュが壊れた、または `quint` の版上げで `QUINT_EVALUATOR_VERSION` が `v0.6.0` 以外になった | 未登録の版は `case $version` で即座に `::error::` 終了。アーカイブと実行ファイルの不一致は `sha256sum -c` が `FAILED` で落とす。 |
| ランナー環境が想定外のとき | `ubuntu-latest` が Linux X64 以外になった | 評価器ステップの先頭で `RUNNER_OS == Linux`・`RUNNER_ARCH == X64` を検証し、違えば `::error::` で落とす。 |
| 仕様の中身が壊れたとき | 型誤り／`quint test` の失敗／不変条件違反／`step` が一度も発火しない（`max <= 1`） | それぞれ `quint typecheck`・`quint test`・`quint run`・`Trace length statistics` 検査で非ゼロ終了する。 |

## 必須のチェック（`takt-dash required CI`）に入れるかの案

**採用：`required-ci`（`takt-dash required CI`）に入れる。GitHub のルールセットは変更しない。**

具体的な変更（`.github/workflows/ci.yml`）：
- `required-ci` の `needs` を `[ci-revision, done, e2e]` から `[ci-revision, done, e2e, quint]` に広げ、`QUINT_RESULT: ${{ needs.quint.result }}` が `success` でなければ `exit 1` にする。
- ルールセット（`Require review thread resolution`、必須コンテキスト `takt-dash required CI`・`Check unresolved comments`）は一切変更しない。既存の集約ジョブ `takt-dash required CI` の内側に収めるため、ルールセット側の追従は不要。

推奨する理由：
1. **壊れた仕様のマージを止められる**：非必須にすると、`quint` が落ちていても `takt-dash required CI` が緑になりマージできてしまう。
2. **PR の待ち時間を増やさない**：全 12 仕様の実行はローカル実測で約 70 秒、CI でもキャッシュヒット時は 2 分未満の見込み。`ci-revision` の後に `done`・`e2e` と並列で走るため、クリティカルパスを伸ばさない。
3. **外部依存のリスクが既存と同等**：npm レジストリ依存は既存の `done` ジョブの `npm ci` と同じ。Rust 評価器は `actions/cache`＋認証付き `gh release download`＋固定 sha256 で保護している。
4. **独立ジョブなので切り分けやすい**：`done` に混ぜず独立ジョブ `quint` にすることで、Rust ビルドを待たずに終わり、落ちた原因もジョブ名から即座に判別できる。

代替案（入れない案）：
- `quint` ジョブを `required-ci` の `needs` に入れず参考チェックとして運用する案もある。ただしその間は仕様破壊のままマージ可能になるため、本設計では **最初から `required-ci` に入れる案** を採る（設計レビューで異論があれば非必須へ切り替えられる）。

## CI と守りの仕組みへの影響

- **`.github/workflows/ci.yml`**：
  - `quint` ジョブを 1 つ追加する（`needs: ci-revision`、`if: needs.ci-revision.outputs.sha != ''`、`runs-on: ubuntu-latest`、`timeout-minutes: 10`、`permissions: { contents: read }`）。
  - `required-ci` の `needs` と結果判定に `quint` を加える（ルールセットは不変更）。
  - `done` ジョブはすでにルートで `npm ci` を実行しているため、ルート `package.json` に `@informalsystems/quint` を足すことで `done` ジョブの `npm ci` が数秒程度伸びる。`done` ジョブのキャッシュや他のステップの動作には影響しない。
- **`tools/repo-gate`（リポジトリ関門）**：
  - `tools/repo-gate/gate_ci.py` は `make done` を呼ぶジョブの Rust/Node セットアップと `aislop` 実行順を検査する。`quint` ジョブの追加で既存条件は壊れない。
  - `tools/repo-gate/gate_makefile.py` は `Makefile` の `done` ターゲットを検査する。`Makefile` は変えないため影響なし。
  - `scripts/aislop-check-bootstrap.mjs`（`aislop`）はリポジトリ全体を検査するため、新規追加する `scripts/quint-check-specs.sh`・`scripts/quint-check-manifest.py`・`specs/quint/ci-specs.json` が `npm run check:aislop` を通ることを実装後に確認する。
- **上流・保存済みデータ・公開 API・待ち受け**：
  - 上流（`takt` / `takt-mcp`）、保存済みデータ（SQLite）、公開 API（proto・`/api/*`）、画面の待ち受け（`0.0.0.0:3000`、LAN・SoftEther VPN・tailscale）への影響は一切なし。

## 版の上げ方

`@informalsystems/quint` の版を上げるときは、次を同じ PR で同時に替える。

1. `package.json` の `@informalsystems/quint` を新しい厳密版へ変え、`npm install --package-lock-only`（または `npm install`）で `package-lock.json` を更新する。
2. 新しい版が参照する Rust 評価器の版を `node -p "require('@informalsystems/quint/dist/src/rust/binaryManager.js').QUINT_EVALUATOR_VERSION"` で確かめる。
3. 評価器の版が変わっていれば、`informalsystems/quint` の該当タグ（`evaluator/<版>`）から `quint_evaluator-x86_64-unknown-linux-gnu.tar.gz` を取得して、アーカイブと展開後 `quint_evaluator` 実行ファイルの 2 つの sha256 を計算し、`.github/workflows/ci.yml` の `case $version in` と本設計書の表を更新する。

## 戻し方

1. `.github/workflows/ci.yml` から `quint` ジョブと `required-ci` への `quint` 参照を削除する。
2. `package.json` から `@informalsystems/quint` を削除し、`npm install` で `package-lock.json` を戻す。
3. `specs/quint/ci-specs.json`、`scripts/quint-check-manifest.py`、`scripts/quint-check-specs.sh`、`docs/design/quint-ci.md` を削除する。

## 検証計画

1. **ローカル検証**
   - `rm -rf node_modules && npm ci` で `./node_modules/.bin/quint --version` が `0.32.0` であることを確認。
   - `bash scripts/quint-check-specs.sh` が全 12 仕様で exit 0 になることを確認。
   - マニフェスト検証の単体テスト（未登録 `.qnt`・欠損ファイル・README 不一致・`Test` 語尾でない `run`・`max=1`）を確認。
   - `make done`・`npm run check:aislop`・`python3 tools/repo-gate/repo_gate_test.py` が通ることを確認（`tools/repo-gate` が `required-ci` の `needs` や `ci.yml` のジョブ集合を固定していないかも確認する）。
2. **CI でのわざと壊した仕様の検証（受け入れ条件）**
   - まず正常系を push して PR を開き、全 CI が緑になることを確認する。
   - 次に作業ブランチで以下の 3 パターンを **1 コミットずつ** 入れ、毎回 push して `gh pr checks <番号> -R j5ik2o/takt-dash --watch --fail-fast` で `quint` ジョブが落ちることと失敗内容を確認・記録する：
     1. **型の誤り**：`pollerEventRecording.qnt` に型の誤りを入れ、`quint typecheck` で落ちることを確認。
     2. **`test` の失敗**：`pollerEventRecording_test.qnt` の `expect` を壊し、`typecheck` を通った上で `quint test` で落ちることを確認。
     3. **不変条件が破れる変更**：`pollerEventRecording.qnt` の `fixed` で `typecheck`・`quint test` は通るが不変条件が破れる変更を入れ、`quint run`（`Invariant violated`）で落ちることを確認。
   - 壊したコミットは `git reset` / `git push --force-with-lease` を使わず、**それぞれ `git revert` で戻す**（squash マージのため `main` の履歴は汚れない）。最後に全 CI 緑とレビューボット対応を完了する。

## 設計レビューを受けて変えた点

| # | 重要度 | 扱い | 変更内容 |
| --- | --- | --- | --- |
| 1 | 中 | 直す | seed 非固定により無関係な PR が本物の反例で止まったときの手順（`--seed` で再現 → 反例を添えて `種別:バグ` の issue を立てて仕様か実装を直す → `main` 反映後に追随）を設計書と `scripts/quint-check-specs.sh` の失敗文面へ追記。 |
| 2 | 中 | 直す | 壊した仕様の確かめで `git reset` / `git push --force-with-lease` を使わず、各コミットを `git revert` で戻す手順へ変更。 |
| 3 | 低 | 直す | `tools/repo-gate`（`repo_gate_test.py`・`gate_ci.py`）が `required-ci` の `needs` や `ci.yml` のジョブ集合を固定していないか確認し、固定していれば同じ PR で直すことを明記。 |
| 4 | 低 | 直す | `quint` の版上げ手順（`package.json`、評価器の版、2 つの sha256 の同時更新）を「戻し方」の直前に追記。 |
