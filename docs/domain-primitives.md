# ドメインプリミティブと値オブジェクトの適用一覧

issue #3（値オブジェクトにドメインプリミティブを徹底する）の第1段として、現在のコードでドメイン上の値が生のプリミティブ型のまま受け渡されている箇所と、その対象・対象外の判定をまとめます。この段ではコードの挙動を変えません。型の置き換えは、この一覧の実施順に従って後続の段で行います。

第2段（backend）は実施済みです。置換した型、検証内容、後続へ送った箇所、run 状態の厳格化は「第2段（backend）の実施結果」以降に追記しました。frontend/web と frontend/cli は未実施です。

- 基点: `af13db4`（origin/main `6cfe571` を合流した時点）
- 規則: `.takt/facets/knowledge/taktdash-rules.md` の「値オブジェクトとドメインプリミティブ」節
- 調査対象: `backend/src`、`frontend/web/src`、`frontend/cli/src`、`proto/`（読むだけ）
- 調査方法: 対象ファイルを読み、生の型のフィールド・引数・生成箇所を `grep` と目視で確認した。行番号は、backend 表と第2段の記載が第2段完了時点、frontend と proto の表が基点 `af13db4` の内容に対応する。

## 判定基準

- ドメイン上の値として意味を持つものは対象です。プロジェクト名、サーバー ID、ゴール ID、状態の種別、時刻・間隔・件数などの量が該当します。
- 専用の型は生成時に size → lexical content → syntax → semantics の順で検証し、不正な値を表現できない形にします。各段の意味は次のとおりです。
  - size: 長さ・件数・範囲
  - lexical content: 使える文字の種類
  - syntax: 区切りや書式
  - semantics: 意味上の制約（正の数であること、既知の種別であること、参照先が存在することなど）
- 例外は、範囲・長さ・書式の確認も不要で、ほかの属性とも関係しない真に単純な属性だけです。通常の性能は例外の理由にしません。
- 既に専用の型（enum、`DateTime<Utc>`、`PathBuf`、`SocketAddr`、`Date`）で不変条件を持てているものは対象外です。
- takt と aislop は対象外です。takt が返す値をそのまま運ぶだけの項目も、taktDash 側で不変条件を決められないため対象外とします。
- `proto/` は読むだけで、この段では変更しません。

## backend

行番号は第2段（backend）完了時点のコードに対応します。ただし `backend/src/mcp/tools/` の下の参照だけは、`tools.rs` の分割（3e35498）の後に、#416 で分割後のファイルと行へ直しています（ほかのファイルの行番号は第2段完了時点のままです）。第2段で専用型へ置き換えた行は、「対象・対象外」列に「（第2段で置換済み）」と記します。生の型の列は第1段で調査した時点の型で、置換後の型は後述の「第2段（backend）の実施結果」にまとめます。

| 値の意味 | 第1段時点の生の型 | 代表的な現在の箇所（file:line） | 想定する置き換え型名 | 検証する順序 | 対象・対象外 | 対象外の理由 |
|----------|--------------------|----------------------------------|----------------------|--------------|--------------|--------------|
| サーバー ID | `String`（`BTreeMap` のキー） | `backend/src/config.rs:42`、`backend/src/grpc/mod.rs:320`、`backend/src/model.rs:337` | `ServerId` | size（1文字以上）→ lexical content（空白のみ・ASCII 制御文字を拒否）→ syntax（設定のキー形式）→ semantics（設定に存在する） | 対象（第2段で置換済み） | - |
| プロジェクト名 | `String` | `backend/src/config.rs:71`、`backend/src/grpc/mod.rs:320`、`backend/src/model.rs:335` | `ProjectName` | size（1文字以上）→ lexical content（空白のみ・ASCII 制御文字を拒否）→ syntax（設定の名前形式）→ semantics（同一サーバー内で一意） | 対象（第2段で置換済み） | - |
| ゴール ID | `String` | `backend/src/model.rs:100`、`backend/src/events.rs:59`、`backend/src/grpc/mod.rs:171` | `GoalId` | size（1文字以上）→ lexical content（空白のみ・ASCII 制御文字を拒否）→ syntax（ID の書式）→ semantics（対象プロジェクトに存在する） | 対象（第2段で置換済み） | - |
| ゴール ID（任意） | `Option<String>` | `backend/src/model.rs:117`、`backend/src/model.rs:130`、`backend/src/model.rs:195` | `Option<GoalId>` | 同上（`None` は「ゴールに属さない」として許容） | 対象（第2段で置換済み） | - |
| タスク名 | `String` | `backend/src/model.rs:116`、`backend/src/events.rs:115`、`backend/src/mcp/tools/goal.rs:164` | `TaskName` | size（1文字以上）→ lexical content（空白のみ・ASCII 制御文字を拒否）→ syntax（タスク名の書式）→ semantics（同一ゴール内で一意） | 対象（第2段で置換済み） | - |
| run slug | `String` / `Option<String>` | `backend/src/model.rs:192`（`RunEntry` は生のまま）、`backend/src/model.rs:120`、`backend/src/mcp/tools/run.rs:127` | `RunSlug` / `Option<RunSlug>` | size（1文字以上）→ lexical content（空白のみ・ASCII 制御文字を拒否）→ syntax（`/`・`\`・`.`・`..` を拒否）→ semantics（実在する run） | 対象（第2段で置換済み。`RunEntry.run_slug` だけ後続） | - |
| inquiry ID | `String` | `backend/src/model.rs:129`、`backend/src/model.rs:310`、`backend/src/grpc/mod.rs:639` | `InquiryId` | size（1文字以上）→ lexical content（空白のみ・ASCII 制御文字を拒否）→ syntax（ID の書式）→ semantics（保留中の問い合わせに一致する） | 対象（第2段で置換済み） | - |
| プルリクエスト番号 | `Option<i64>` | `backend/src/model.rs:105`、`backend/src/model.rs:306`、`backend/src/mcp/tools/goal.rs:141` | `PullRequestNumber` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（対象ゴールの PR と一致する） | 対象（第2段で置換済み） | - |
| プルリクエストの URL | `Option<String>` | `backend/src/model.rs:106`、`backend/src/model.rs:307`、`backend/src/grpc/mod.rs:216` | 生の `String` を維持 | なし（値の集合は takt 側が決める） | 対象外 | takt が返す URL をそのまま運ぶ値で、taktDash 側では不変条件を確定できない |
| 承認待ちの revision | `Option<i32>` / `i32` | `backend/src/model.rs:312`、`backend/src/model.rs:318`、`backend/src/grpc/mod.rs:800` | `RegistrationRevision` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（承認待ちの提案と一致する） | 対象（第2段で置換済み） | - |
| ゴール登録提案のディレクトリ | `String` | `backend/src/mcp/tools/goal_registration.rs:95`、`backend/src/model.rs:319`、`backend/src/grpc/mod.rs:826` | 生の `String` を維持 | なし（値は takt が返す提案値） | 対象外 | takt が返す提案値で、taktDash 側では不変条件を確定できない |
| until（マージの範囲） | `String`（`"prepare"` / `"merge"`） | `backend/src/grpc/mod.rs:705`、`backend/src/grpc/mod.rs:718`、`proto/taktdash.proto:414` | `MergeUntil`（列挙） | size（1文字以上）→ lexical content（ASCII 小文字）→ syntax（既知の2値）→ semantics（対象 PR の状態で実行できる） | 対象（第2段で置換済み） | - |
| 閾値（判断待ちの滞留・ログの無音） | `u32` | `backend/src/config.rs:80`、`backend/src/config.rs:81`、`backend/src/config.rs:206` | `StaleMinutes` / `SilentMinutes` | size（0 以上、上限 `i32::MAX`）→ lexical content（数字）→ syntax（10進表記）→ semantics（運用上の範囲） | 対象（第2段で置換済み） | - |
| 読み取り間隔 | `u64` | `backend/src/config.rs:45`、`backend/src/config.rs:202`、`backend/src/poller.rs:97` | `IntervalSeconds` | size（1 以上）→ lexical content（数字）→ syntax（10進表記）→ semantics（ポーリング周期として有効） | 対象（後続。`poller.rs` が生の `u64` を読む） | - |
| タスクの状態ごとの件数 | `i32` | `backend/src/grpc/mod.rs:198`、`backend/src/grpc/mod.rs:184`、`proto/taktdash.proto:142` | `TaskCount`（`TaskStatusCounts` の各値） | size（0 以上）→ lexical content（数字）→ syntax（10進表記）→ semantics（総数と一致する） | 対象（第2段で置換済み） | - |
| 日次集計の日付 | `String`（`"YYYY-MM-DD"`） | `backend/src/activity.rs:21`、`backend/src/activity.rs:152` | `DailyDate` | size（10文字）→ lexical content（数字と `-`）→ syntax（`YYYY-MM-DD`）→ semantics（JST の実在日） | 対象（第2段で置換済み） | - |
| 状態の種別（ゴール・実行・タスク・出来事） | 専用 enum（`GoalStatus` など） | `backend/src/model.rs:12`、`backend/src/model.rs:20`、`backend/src/model.rs:28`、`backend/src/model.rs:39` | 既存 enum を維持 | 変換時に既知の値だけを受ける（既に専用型） | 対象外 | 既に専用の型で、不正な種別を表現できない |
| run の状態 | 専用 enum（`RunStatus`） | `backend/src/model.rs:151`、`backend/src/mcp/tools/run.rs:166`、`backend/src/grpc/mapping.rs:55` | `RunStatus`（変換境界で必須化） | 既知の値だけを受ける（第2段で欠落・未知を `ToolError::Parse` に変更） | 対象（第2段で置換済み） | - |
| 時刻 | `DateTime<Utc>` | `backend/src/model.rs:108`、`backend/src/model.rs:135`、`backend/src/model.rs:201` | 既存型を維持 | 既に専用型（範囲を型が持つ） | 対象外 | 既に専用の型で、文字列ではない |
| 設定のファイルパス | `PathBuf` | `backend/src/config.rs:47`、`backend/src/config.rs:74`、`backend/src/config.rs:186` | 既存型を維持 | 既に専用型（絶対パスの検証は `backend/src/config.rs:186` にある） | 対象外 | 列挙した設定値は既に専用の型（`PathBuf`）で、文字列ではない |
| 待ち受けアドレス | `SocketAddr` | `backend/src/config.rs:49`、`backend/src/config.rs:216` | 既存型を維持 | 既に専用型（構文検証は `parse` が行う） | 対象外 | 既に専用の型で、不正なアドレスを表現できない |
| 自由文（目的・本文・理由・要約） | `String` | `backend/src/model.rs:103`、`backend/src/model.rs:131`、`backend/src/model.rs:196`、`backend/src/model.rs:210` | 生の `String` を維持 | なし | 対象外 | 真に単純な属性で、範囲・書式・他の属性との関係を持たない |
| takt が返す表示用の状態文字列 | `String` | `backend/src/model.rs:235`、`backend/src/model.rs:254` | 生の `String` を維持 | なし（値の集合は takt 側が決める） | 対象外 | takt が決める値をそのまま運ぶ項目で、taktDash 側では不変条件を確定できない |

## 第2段（backend）の実施結果

backend の値オブジェクトは `backend/src/domain.rs` にまとめ、生成方法を `TryFrom` に統一しました。無検査の公開コンストラクタは置かず、保持値の参照は `as_str()` / `get()` だけにしています。テキスト型は共通の `validate_text`（`backend/src/domain.rs:38-46`）で、空を size、空白のみと ASCII 制御文字を lexical content として拒否します。

| 値 | 型名 | 定義 | 検証内容 | 主な変更ファイル |
|----|------|------|----------|------------------|
| サーバー ID | `ServerId` | `backend/src/domain.rs:78`（`text_value!` は `:48-76`） | 空 → `DomainError::Size`。空白のみ・ASCII 制御文字 → `DomainError::Lexical` | `config.rs`、`grpc/mod.rs`、`model.rs`、`store.rs`、`activity.rs` |
| プロジェクト名 | `ProjectName` | `backend/src/domain.rs:79` | 同上 | `config.rs`、`grpc/mod.rs`、`model.rs`、`store.rs`、`conversation.rs` |
| ゴール ID | `GoalId` / `Option<GoalId>` | `backend/src/domain.rs:80` | 同上。`None` はゴールに属さない値を表す | `model.rs`、`events.rs`、`store.rs`、`mcp/tools/goal.rs`、`mcp/tools/goal_registration.rs`、`mcp/tools/mod.rs`、`mcp/tools/sets.rs`、`mcp/tools/summary_cache.rs`、`grpc/mod.rs` |
| タスク名 | `TaskName` | `backend/src/domain.rs:81` | 同上 | `model.rs`、`events.rs`、`mcp/tools/goal.rs`、`mcp/tools/mod.rs`、`mcp/tools/sets.rs`、`grpc/mod.rs` |
| inquiry ID | `InquiryId` / `Option<InquiryId>` | `backend/src/domain.rs:82` | 同上。`PendingDecision` では問い合わせ以外を `None` にする | `model.rs`、`store.rs`、`mcp/tools/goal.rs`、`mcp/tools/mod.rs`、`mcp/tools/sets.rs`、`grpc/mod.rs` |
| run slug | `RunSlug` / `Option<RunSlug>` | `backend/src/domain.rs:86-105` | 上記に加え、`/`・`\`・`.`・`..` を `DomainError::Syntax` で拒否 | `model.rs`、`store.rs`、`events.rs`、`activity.rs`、`mcp/tools/goal.rs`、`mcp/tools/mod.rs`、`mcp/tools/run.rs`、`mcp/tools/sets.rs`、`grpc/mod.rs` |
| プルリクエスト番号 | `PullRequestNumber` | `backend/src/domain.rs:115-133` | `TryFrom<i64>`。0 以下を `DomainError::Size` で拒否 | `model.rs`、`mcp/tools/goal.rs`、`mcp/tools/mod.rs`、`mcp/tools/sets.rs`、`grpc/mod.rs` |
| 承認待ちの revision | `RegistrationRevision` | `backend/src/domain.rs:143-161` | `TryFrom<i32>`。0 以下を `DomainError::Size` で拒否 | `model.rs`、`store.rs`、`mcp/tools/goal_registration.rs`、`mcp/tools/sets.rs`、`grpc/mod.rs` |
| 閾値（滞留・無音） | `StaleMinutes` / `SilentMinutes` | `backend/src/domain.rs:169-202` | `TryFrom<u32>`。`i32::MAX` 超を `DomainError::Size` で拒否。`get()` は範囲保証済みの `i32` 射影で、切り詰めの `as i32` を持たない | `config.rs`、`store.rs`、`grpc/mod.rs`、テスト fixture |
| タスクの件数 | `TaskCount` | `backend/src/domain.rs:206-224` | `TryFrom<i64>`。0 未満と `i32::MAX` 超を `DomainError::Size` で拒否 | `activity.rs`、`grpc/mod.rs` |
| 日次集計の日付 | `DailyDate` | `backend/src/domain.rs:234-281` | size（10文字）→ lexical content（数字と `-`）→ syntax（`YYYY-MM-DD`）→ semantics（`NaiveDate` で実在日）の順。`From<NaiveDate>` は失敗しない | `activity.rs`、`grpc/mod.rs` |
| until | `MergeUntil` | `backend/src/domain.rs:285-314` | 空 → size、ASCII 小文字以外 → lexical content、`prepare` / `merge` 以外 → syntax | `mcp/tools/mod.rs`、`mcp/tools/sets.rs`、`grpc/mod.rs` |
| run の状態 | `RunStatus` | `backend/src/model.rs:151-170` | `TryFrom<&str>` は `running` / `completed` / `aborted` / `failed` だけを受け、未知は `DomainError::Semantics` | `model.rs`、`mcp/tools/run.rs`、テスト fixture |
| run のレポートの名前（issue #70 で追加） | `ReportFilename` | `backend/src/domain.rs:113-157` | size（1024 バイト以下）→ lexical content（`validate_text`）→ syntax（絶対パス・`\`・空の区切り・`.`・`..` を拒否）→ semantics（`.md` で終わる）。`reports/` の下に収まることは読み取り時に実パスで確かめる（`backend/src/run_log.rs` の `read_report`）。画面側は `toReportFilename`（`frontend/web/src/state/domain-primitives.ts:113`）が同じ順で検証する | `grpc/mod.rs`、`run_log.rs`、`run_progress.rs`、`app/api/runs/[slug]/content/route.ts` |
| run への指示書の ID（issue #70 で追加） | `InstructionId` | `backend/src/domain.rs:159-194` | `TryFrom<i32>` / `TryFrom<i64>`。0 以下と `i32` の範囲外を `DomainError::Size` で拒否。画面側は `toInstructionId`（`frontend/web/src/state/domain-primitives.ts:132`） | `grpc/mod.rs`、`run_log.rs`、`app/api/runs/[slug]/content/route.ts` |

件数の飽和について: `task_status_counts`（`backend/src/grpc/mod.rs:198`）は状態ごとの件数を `TaskCount` に通し、proto の `int32` に収まらない件数だけ `i32::MAX` へ飽和させます。`completed_tasks_since`（`backend/src/activity.rs:211`）は SQL の `COUNT(*)` を `TaskCount::try_from` で復元し、失敗を `rusqlite::Error::FromSqlConversionFailure` に写します。

SQL からの復元では、`ServerCount.server_id`（`backend/src/activity.rs:31`）と `ActivityEvent.run_slug`（`backend/src/model.rs:292`）も `TryFrom` を通し、失敗を同じ変換エラーにしています。`DailyCount.total` / `.attention` と `ServerCount.count` は出来事の件数で、backend 表の対象（タスクの状態ごとの件数）ではないため生の `i32` を維持しています。

### run 状態の厳格化（第2段の明示的な挙動変更）

第1段の調査時点では、takt 応答の `status` が欠落または未知のとき `RunStatus::Running` に丸めていました。第2段では `parse_run_status`（`backend/src/mcp/tools/run.rs:166`）が欠落と未知を `ToolError::Parse` にし、既知の4値だけを受けます。そのため、`status` を持たない run 応答は読み取りエラーになります。これは要求 `R17` で明示された唯一の外部挙動の変更です。既存の run fixture には有効な `"status"` を追加し、欠落・未知の期待を `ToolError::Parse` に更新しました。既知4値の応答と gRPC の射影は従来と同じです。

### 詳細モデルの追随（issue #157）

第2段の置換から漏れていた詳細モデル（`TaskDetail`、`GoalDetail`、`GoalTaskEntry`、`backend/src/model.rs`）の識別子も、既存の値オブジェクトへ置き換えました。`TaskDetail` はプロジェクト名（`ProjectName`）、タスク名（`TaskName`）、ゴール ID（`Option<GoalId>`）、run slug（`Option<RunSlug>` と `Vec<RunSlug>`）、プルリクエスト番号（`Option<PullRequestNumber>`）を持ち、`GoalDetail` はゴール ID（`GoalId`）、`GoalTaskEntry` はタスク名（`TaskName`）と run slug（`Option<RunSlug>`）を持ちます。

解析（`parse_task_detail`、`backend/src/mcp/tools/goal.rs`）は、一覧の解析（`parse_task`、`parse_goal`）と同じ流儀で `domain_value` を通して検証し、不正な値は `ToolError::Parse` にします。たとえば `prNumber` が 0 の応答は、詳細の読み取り全体が解析エラーになります。空文字の `goalId`・`runSlug`・`sourceRunSlug` は、一覧と同じく未所属（`None`）として扱います。`GoalDetail` と `GoalTaskEntry` の識別子は、検証済みの要求値（`GoalId`）と一覧の `TaskEntry` から組み立てるため、新しい検証は増えていません。proto の定義は変えず、gRPC の組み立て（`backend/src/grpc/mod.rs`、`backend/src/grpc/mapping.rs`）で値オブジェクトと proto の生の型を相互変換します。

### 値オブジェクトで検証しない意味制約

参照先の存在・一意性・一致は外部状態を要するため、型では検証せず、既存の境界に残しています。

- 存在: 設定にサーバーとプロジェクトがあること（`backend/src/config.rs:206` 以降と `backend/src/grpc/mod.rs:320`）、ゴールが snapshot にあること、問い合わせが保留中であること（`backend/src/grpc/mod.rs:616`）、run が状態ストアにあること（`backend/src/grpc/mod.rs:436`）。
- 一意性: サーバー内のプロジェクト名、ゴール内のタスク名、`(server_id, project, inquiry_id)` で一意な保留項目（`backend/src/grpc/mod.rs:632`）。
- 一致: PR 番号とゴールの対応、revision と承認待ち提案の対応（`backend/src/store.rs:203`）、JST の実在日（`DailyDate` が型で検証）。

### 後続へ送った箇所

`backend/src/poller.rs` はこのゴールで編集しないため、次の値は生の型のまま残しました。poller が設定値の生 `String` や `u64` をキー・比較に使うことが理由です。

| 箇所 | 現在の型 | 後続に送る理由 |
|------|----------|----------------|
| `config::Project.name` / `.server`（`backend/src/config.rs:71-72`） | `String` | `backend/src/poller.rs:105-118,181-187` が `&project.server` / `&project.name` と `project.name.clone()` を BTreeMap のキーに使う |
| `config::Config.servers` / `server_meta` のキー（`backend/src/config.rs:42-43`） | `String` | `backend/src/poller.rs:184` が `config.servers.get(&project.server)` を呼ぶ |
| `model::ProjectView.name` / `.server_id`（`backend/src/model.rs:335,337`） | `String` | `backend/src/poller.rs:255` の `ProjectSignature` と `backend/src/store.rs:136-139` が生のまま比較・保持する |
| `model::RunEntry.run_slug`（`backend/src/model.rs:192`） | `String` | `backend/src/poller.rs:254` が `is_log_silent` のシグネチャ（`RunEntry`）と `silent_runs` のキーに使う |
| `model::PendingDecision.server_id` / `.project` / `.goal_id`（`backend/src/model.rs:299-301`） | `String` / `Option<String>` | `backend/src/poller.rs:263-264` の `PendingDecisionSignature` が生のまま比較・保持する |
| `model::ActivityEvent.server_id` / `.project`（`backend/src/model.rs:288-289`） | `String` / `Option<String>` | `backend/src/events.rs:18` の `detect` が `backend/src/poller.rs:113,132` から生の `&str` を受ける |
| 読み取り間隔 | `u64` | `backend/src/config.rs:45` を `backend/src/poller.rs:97` が生の `u64` として読む。別ゴール 819c7e2a が担当 |
| proto の各フィールド | `string` / `int32` / `int64` | `proto/` は API の正で、frontend の生成物に影響するため後続段。破壊的変更の候補は後述 |

## frontend/web

| 値の意味 | 現在の生の型 | 代表的受け渡し箇所（file:line） | 想定する置き換え型名 | 検証する順序 | 対象・対象外 | 対象外の理由 |
|----------|--------------|--------------------------------|----------------------|--------------|--------------|--------------|
| サーバー ID | `string` | `frontend/web/src/state/restore-state.ts:35`、`frontend/web/src/state/restore-state.ts:121`、`frontend/web/src/state/restore-state.ts:69` | `ServerId` | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（一覧に存在する） | 対象 | - |
| プロジェクト名 | `string` | `frontend/web/src/state/restore-state.ts:44`、`frontend/web/src/state/restore-state.ts:70`、`frontend/web/src/state/restore-state.ts:122` | `ProjectName` | size（1文字以上）→ lexical content（許可文字）→ syntax（名前の書式）→ semantics（サーバーとの対応が設定にある） | 対象 | - |
| ゴール ID | `string` / `string` または `undefined` | `frontend/web/src/state/restore-state.ts:68`、`frontend/web/src/state/restore-state.ts:107`、`frontend/web/src/state/restore-state.ts:123` | `GoalId` / `Option<GoalId>` に相当する型 | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（対象プロジェクトに存在する） | 対象 | - |
| タスク名 | `string` | `frontend/web/src/state/restore-state.ts:108` | `TaskName` | size（1文字以上）→ lexical content（許可文字）→ syntax（タスク名の書式）→ semantics（同一ゴール内で一意） | 対象 | - |
| run slug | `string` | `frontend/web/src/state/restore-state.ts:103` | `RunSlug` | size（1文字以上）→ lexical content（許可文字）→ syntax（run ディレクトリ名の形式）→ semantics（実在する run） | 対象 | - |
| inquiry ID | `string` | `frontend/web/src/state/restore-state.ts:136` | `InquiryId` | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（保留中の問い合わせに一致する） | 対象 | - |
| プルリクエスト番号 | `number` | `frontend/web/src/state/restore-state.ts:79`、`frontend/web/src/state/restore-state.ts:131` | `PullRequestNumber` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（対象ゴールの PR と一致する） | 対象 | - |
| プルリクエストの URL | `string` | `frontend/web/src/state/restore-state.ts:80`、`frontend/web/src/state/restore-state.ts:132`、`frontend/web/src/state/decision-actions.ts:299` | 生の `string` を維持 | なし（値の集合は takt 側が決める） | 対象外 | takt が返す URL をそのまま運ぶ値で、taktDash 側では不変条件を確定できない |
| 承認待ちの revision | `number` または `undefined` | `frontend/web/src/state/restore-state.ts:137` | `RegistrationRevision` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（承認待ちの提案と一致する） | 対象 | - |
| タスクの状態ごとの件数 | `number` | `frontend/web/src/state/restore-state.ts:58`、`frontend/web/src/state/restore-state.ts:63` | `TaskCount` | size（0 以上）→ lexical content（数字）→ syntax（10進表記）→ semantics（総数と一致する） | 対象 | - |
| 閾値（判断待ちの滞留・ログの無音） | `number` | `frontend/web/src/state/restore-state.ts:146`、`frontend/web/src/state/restore-state.ts:147`、`frontend/web/src/state/dashboard.ts:16` | `StaleMinutes` / `SilentMinutes` | size（0 以上、上限）→ lexical content（数字）→ syntax（10進表記）→ semantics（運用上の範囲） | 対象 | - |
| 状態の種別（ゴール・実行・タスク・run・判断待ち） | `number` | `frontend/web/src/state/restore-state.ts:72`、`frontend/web/src/state/restore-state.ts:115`、`frontend/web/src/state/restore-state.ts:120` | `GoalStatus` などの画面側の専用型 | 既知の値だけを受ける（現在の `enumFromWire` は任意の数値を通す） | 対象 | - |
| 時刻 | `Date` / `Date` または `undefined` | `frontend/web/src/state/restore-state.ts:125`、`frontend/web/src/state/restore-state.ts:83` | 既存型を維持 | 既に専用型（`toDate` が復元する） | 対象外 | `Date` が範囲を表現でき、文字列ではない |
| 自由文（本文・選択肢・要約） | `string` / `string[]` | `frontend/web/src/state/restore-state.ts:126`、`frontend/web/src/state/restore-state.ts:127`、`frontend/web/src/state/restore-state.ts:109` | 生の `string` を維持 | なし | 対象外 | 真に単純な属性で、範囲・書式・他の属性との関係を持たない |

## frontend/cli

| 値の意味 | 現在の生の型 | 代表的受け渡し箇所（file:line） | 想定する置き換え型名 | 検証する順序 | 対象・対象外 | 対象外の理由 |
|----------|--------------|--------------------------------|----------------------|--------------|--------------|--------------|
| サーバー ID | `String` | `frontend/cli/src/main.rs:207`、`frontend/cli/src/main.rs:209`、`frontend/cli/src/main.rs:731` | `ServerId` | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（設定に存在する） | 対象 | - |
| プロジェクト名 | `String` | `frontend/cli/src/main.rs:207`、`frontend/cli/src/main.rs:174`、`frontend/cli/src/main.rs:732` | `ProjectName` | size（1文字以上）→ lexical content（許可文字）→ syntax（名前の書式）→ semantics（サーバーとの対応が設定にある） | 対象 | - |
| ゴール ID | `Option<String>` / `String` | `frontend/cli/src/main.rs:207`、`frontend/cli/src/main.rs:209`、`frontend/cli/src/main.rs:733` | `Option<GoalId>` / `GoalId` | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（対象プロジェクトに存在する） | 対象 | - |
| inquiry ID | `String` | `frontend/cli/src/main.rs:207`、`frontend/cli/src/main.rs:720` | `InquiryId` | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（保留中の問い合わせに一致する） | 対象 | - |
| プルリクエスト番号 | `i64` | `frontend/cli/src/main.rs:209`、`frontend/cli/src/main.rs:461`、`frontend/cli/src/main.rs:734` | `PullRequestNumber` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（対象ゴールの PR と一致する） | 対象 | - |
| タスクの状態ごとの件数 | `i32`（proto `TaskStatusCounts` 由来） | `frontend/cli/src/main.rs:989`、`frontend/cli/src/main.rs:990`、`frontend/cli/src/main.rs:992` | `TaskCount` | size（0 以上）→ lexical content（数字）→ syntax（10進表記）→ semantics（総数と一致する） | 対象 | - |
| 承認待ちの revision | `i32` | `frontend/cli/src/main.rs:213`、`frontend/cli/src/main.rs:482`、`frontend/cli/src/main.rs:762` | `RegistrationRevision` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（承認待ちの提案と一致する） | 対象 | - |
| until（マージの範囲） | `String` | `frontend/cli/src/main.rs:209`、`frontend/cli/src/main.rs:472`、`frontend/cli/src/main.rs:735` | `MergeUntil`（列挙） | size（1文字以上）→ lexical content（ASCII 小文字）→ syntax（既知の2値）→ semantics（対象 PR の状態で実行できる） | 対象 | - |
| 閾値（判断待ちの滞留） | `i32` | `frontend/cli/src/main.rs:1102`、`frontend/cli/src/main.rs:1113` | `StaleMinutes` | size（0 以上、上限）→ lexical content（数字）→ syntax（10進表記）→ semantics（運用上の範囲） | 対象 | - |
| 状態の種別（run・判断待ち・問い合わせ） | `i32`（enum の数値比較） | `frontend/cli/src/main.rs:962`、`frontend/cli/src/main.rs:1114`、`frontend/cli/src/main.rs:1118` | 画面側の専用型（`RunStatus` など） | 既知の値だけを受ける | 対象 | - |
| 接続先アドレス | `String` | `frontend/cli/src/main.rs:944`、`frontend/cli/src/main.rs:945` | 既存の `String` を維持し、解決時に `Endpoint` などへ変換する | 解決時に構文と到達性を確認する | 対象外 | 接続設定の値で、taktDash のドメインの値を表さない |
| 時刻 | `prost_types::Timestamp` から変換した `chrono::DateTime` | `frontend/cli/src/main.rs:1151`、`frontend/cli/src/main.rs:1173`、`proto/taktdash.proto:210` | 既存型を維持 | 既に専用型 | 対象外 | 既に専用の型で、文字列ではない |
| 自由文（回答本文・要約・表示文） | `String` | `frontend/cli/src/main.rs:207`、`frontend/cli/src/main.rs:221` | 生の `String` を維持 | なし | 対象外 | 真に単純な属性で、範囲・書式・他の属性との関係を持たない |

## proto（読むだけ。この段では変更しない）

`proto/` は API の正であり、この段では変更しません。各行の「対象」は、proto の公開契約を変える必要がある候補という意味です。

| 値の意味 | 現在の生の型 | 代表的受け渡し箇所（file:line） | 想定する置き換え型名 | 検証する順序 | 対象・対象外 | 対象外の理由 |
|----------|--------------|--------------------------------|----------------------|--------------|--------------|--------------|
| サーバー ID | `string` | `proto/taktdash.proto:122`、`proto/taktdash.proto:206`、`proto/taktdash.proto:410` | `ServerId` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| プロジェクト名 | `string` | `proto/taktdash.proto:132`、`proto/taktdash.proto:207`、`proto/taktdash.proto:411` | `ProjectName` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| ゴール ID | `string` | `proto/taktdash.proto:158`、`proto/taktdash.proto:191`、`proto/taktdash.proto:412` | `GoalId` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| タスク名 | `string` | `proto/taktdash.proto:192`、`proto/taktdash.proto:419` | `TaskName` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| run slug | `string` | `proto/taktdash.proto:187` | `RunSlug` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| inquiry ID | `string` | `proto/taktdash.proto:215`、`proto/taktdash.proto:398`、`proto/taktdash.proto:405` | `InquiryId` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| プルリクエスト番号 | `int64` | `proto/taktdash.proto:152`、`proto/taktdash.proto:413` | `PullRequestNumber` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| プルリクエストの URL | `string` | `proto/taktdash.proto:153` | 生の `string` を維持 | なし（値の集合は takt 側が決める） | 対象外 | takt が返す URL をそのまま運ぶ値で、taktDash 側では不変条件を確定できない |
| 承認待ちの revision | `int32` | `proto/taktdash.proto:216`、`proto/taktdash.proto:427`、`proto/taktdash.proto:432` | `RegistrationRevision` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| ゴール登録提案のディレクトリ | `string` | `proto/taktdash.proto:454` | 生の `string` を維持 | なし（値は takt が返す提案値） | 対象外 | takt が返す提案値で、taktDash 側では不変条件を確定できない |
| until（マージの範囲） | `string` | `proto/taktdash.proto:414` | `MergeUntil`（列挙） | size → lexical content → syntax → semantics | 対象 | - |
| 閾値（判断待ちの滞留・ログの無音） | `int32` | `proto/taktdash.proto:221`、`proto/taktdash.proto:222` | `StaleMinutes` / `SilentMinutes` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| タスクの状態ごとの件数 | `int32` | `proto/taktdash.proto:142`、`proto/taktdash.proto:147` | `TaskCount` メッセージ | size → lexical content → syntax → semantics | 対象 | - |
| 状態の種別 | enum | `proto/taktdash.proto:162`、`proto/taktdash.proto:199`、`proto/taktdash.proto:205` | 既存 enum を維持 | 既知の値だけを受ける | 対象外 | proto の enum が既に専用の型になっている |
| 時刻 | `google.protobuf.Timestamp` | `proto/taktdash.proto:210`、`proto/taktdash.proto:227` | 既存型を維持 | 既に専用型 | 対象外 | 既に専用の型で、文字列ではない |
| 自由文（本文・選択肢・要約） | `string` / `repeated string` | `proto/taktdash.proto:211`、`proto/taktdash.proto:212`、`proto/taktdash.proto:193` | 生の `string` を維持 | なし | 対象外 | 真に単純な属性で、範囲・書式・他の属性との関係を持たない |

## 置き換えの実施順

段は backend → frontend/web → frontend/cli の順に進めます。前の段で確定した型名と検証規則を、後の段が proto 越しに受け取る形にします。件数は今回の一覧に挙げた代表箇所の概算です。

| 段 | 対象 | 概算 | 主に触るファイル |
|----|------|------|------------------|
| 1（今回） | 規則の記録と一覧の作成 | 対象2ファイル | `.takt/facets/knowledge/taktdash-rules.md`、`docs/domain-primitives.md` |
| 2（実施済み） | backend の値の置き換え | 13 種類・約 30 箇所＋`RunStatus` の厳格化 | `backend/src/domain.rs`（新規）、`backend/src/lib.rs`、`backend/src/model.rs`、`backend/src/config.rs`、`backend/src/grpc/mod.rs`、`backend/src/events.rs`、`backend/src/store.rs`、`backend/src/activity.rs`、`backend/src/conversation.rs`、`backend/src/mcp/tools/` |
| 3 | frontend/web の値の置き換え | 11 種類・約 15 箇所 | `frontend/web/src/state/restore-state.ts`、`frontend/web/src/state/dashboard.ts`、`frontend/web/src/state/proto-values.ts` |
| 4 | frontend/cli の値の置き換え | 10 種類・約 12 箇所 | `frontend/cli/src/main.rs` |

### 編集禁止ファイル（このゴールでは編集しない）

次のファイルは別のゴールの担当が編集しているため、このゴールのどの段でも編集しません。一覧に載せるだけにとどめます。段3・段4の対象箇所のうちこれらのファイルに含まれるものは、担当が切り替わるまで着手しません。

- `backend/src/poller.rs`
- `frontend/cli/src/main.rs`
- `frontend/cli/src/tests.rs`
- `frontend/web/src/state/decision-actions.ts`
- `frontend/web/src/grpc/client.ts`
- `tools/repo-gate/repo_gate.py`

段3の `frontend/web/src/state/*` のうち `decision-actions.ts` は上記のとおり編集禁止です。段4が触る `frontend/cli/src/main.rs` と `frontend/cli/src/tests.rs` も同じです。

## proto の破壊的変更の候補

この段では `proto/` を変更しません（`git diff -- proto/` が空であることを確認します）。後続の段で proto の公開契約を変える場合は、次の変更が破壊的変更になります。

- `ApproveGoalMergeRequest.until`（`proto/taktdash.proto:414`）を `string` から `MergeUntil` の enum へ変える。既存の送信側（`backend/src/grpc/mod.rs:722`、`frontend/cli/src/main.rs:472`）と生成物の両方に影響します。
- `TaskStatusCounts` の各 `int32`（`proto/taktdash.proto:142`〜`proto/taktdash.proto:147`）を `TaskCount` メッセージへ包む。JSON 表現と生成コードが変わります。
- `PullRequest.number`（`proto/taktdash.proto:152`）を `PullRequestNumber` メッセージへ包む。`PullRequest` を参照する全メッセージに影響します。
- `server_id`・`project`・`goal_id`（`proto/taktdash.proto:122`、`:132`、`:158` ほか）を各メッセージへ包む。フィールド型が変わるため、既存の利用側すべての移行が必要です。
- `AnomalyThresholds` の各 `int32`（`proto/taktdash.proto:221`、`:222`）を `StaleMinutes` / `SilentMinutes` へ包む。既定値の解決箇所（`frontend/web/src/state/dashboard.ts:16`）にも影響します。

いずれも `npm run proto:generate` による生成物の再生成が必要です。生成物は手で編集しません。

## 既存の成果

- 先行する一覧はありません。`docs/` に `domain-primitives.md` は無く、リポジトリ全体で `domain-primitives` を参照する記述は、この段で追記した `taktdash-rules.md` の1行だけです。
- issue #3 のコメントなど、取り込むべき既存の一覧はこの作業ディレクトリにありません。

### 一覧に載せないと決めたもの

- 生成コード `frontend/web/src/proto/taktdash.ts` と backend の tonic 生成物は、`.proto` から生成する派生物です。手で編集せず、`proto/` の判断だけを一覧に載せます。
- `frontend/web/app`（Route Handlers）は、`order.md` の調査対象が `frontend/web/src` のため一覧の対象にしません。確認した生の検証は「判断に迷った箇所」に記録します。
- ログの行や JSONL の生テキストは、外部が書くファイルの内容で、taktDash の型では不変条件を決められません。

## 判断に迷った箇所

| 箇所 | 確認した事実 | 選んだ扱い |
|------|--------------|------------|
| `frontend/web/app` の入力検証 | `frontend/web/app/api/decisions/_shared.ts:75`〜`:79` に `isNonEmptyString` と `isPositiveInteger` があり、`frontend/web/app/api/decisions/merge/route.ts:12`〜`:24` が `serverId`・`project`・`goalId`・`prNumber` を生の値で検証している。同じファイルの `:27` は `until: "merge"` を直書きしている | `order.md` の調査対象が `frontend/web/src` のため一覧には載せない。ただし同じ値の重複検証であり、後続の段で `frontend/web/src` の型を決めたあとに、この境界も同じ型へ寄せるかを再検討する |
| web 側の状態の種別 | `frontend/web/src/state/proto-values.ts:120`〜`:129` の `enumFromWire` は、数値ならそのまま返し、未知の文字列は `-1` にする。`GoalStatus` などの専用型ではなく `number` を返す | 「対象」として一覧に載せる。既知の値だけを受ける型へ変えるのは後続の段とする |
| CLI の接続先アドレス | `frontend/cli/src/main.rs:944`〜`:946` の `env_backend_addr` は環境変数を `String` のまま返し、`:88` と `:119` で空文字だけを弾く | taktDash のドメインの値ではなく接続設定のため「対象外」とする。解決時の変換は既存の接続処理に任せる |
| takt が返す表示用の状態 | `backend/src/model.rs:310` の `StepLogEntry.status` は `String` で、値の集合は takt 側が決める。指示書の状態は issue #70 で `backend/src/run_log.rs:458` の `InstructionState`（pending / deliveredSameSession / deliveredNextStep / unconsumedWarned の4種類）にし、出力するときに takt の状態名へ変換する | `StepLogEntry.status` は takt が決める値のため「対象外」とする。taktDash 側で不変条件を決める必要が出た場合は、その時点で別の要求として扱う |
| 件数を値オブジェクトにする範囲 | `proto/taktdash.proto:141`〜`:148` の `TaskStatusCounts`、`backend/src/activity.rs:21`〜`:25` の `DailyCount`、`frontend/cli/src/main.rs:962`〜`:974` の集計が該当する | 「時刻や間隔などの量」に件数を含めて「対象」とする。総数との一致という意味上の制約を持つため、単純な属性ではないと判断した |
