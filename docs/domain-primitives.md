# ドメインプリミティブと値オブジェクトの適用一覧

issue #3（値オブジェクトにドメインプリミティブを徹底する）の第1段として、現在のコードでドメイン上の値が生のプリミティブ型のまま受け渡されている箇所と、その対象・対象外の判定をまとめます。この段ではコードの挙動を変えません。型の置き換えは、この一覧の実施順に従って後続の段で行います。

- 基点: `af13db4`（origin/main `6cfe571` を合流した時点）
- 規則: `.takt/facets/knowledge/taktdash-rules.md` の「値オブジェクトとドメインプリミティブ」節
- 調査対象: `backend/src`、`frontend/web/src`、`frontend/cli/src`、`proto/`（読むだけ）
- 調査方法: 対象ファイルを読み、生の型のフィールド・引数・生成箇所を `grep` と目視で確認した。行番号はすべて基点 `af13db4` の内容に対応する。

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

| 値の意味 | 現在の生の型 | 代表的受け渡し箇所（file:line） | 想定する置き換え型名 | 検証する順序 | 対象・対象外 | 対象外の理由 |
|----------|--------------|--------------------------------|----------------------|--------------|--------------|--------------|
| サーバー ID | `String`（`BTreeMap` のキー） | `backend/src/config.rs:40`、`backend/src/grpc/mod.rs:312`、`backend/src/model.rs:283` | `ServerId` | size（1文字以上）→ lexical content（許可文字）→ syntax（設定のキー形式）→ semantics（設定に存在する） | 対象 | - |
| プロジェクト名 | `String` | `backend/src/config.rs:69`、`backend/src/grpc/mod.rs:318`、`backend/src/model.rs:319` | `ProjectName` | size（1文字以上）→ lexical content（許可文字）→ syntax（設定の名前形式）→ semantics（同一サーバー内で一意） | 対象 | - |
| ゴール ID | `String` | `backend/src/model.rs:98`、`backend/src/events.rs:59`、`backend/src/grpc/mod.rs:682` | `GoalId` | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（対象プロジェクトに存在する） | 対象 | - |
| ゴール ID（任意） | `Option<String>` | `backend/src/model.rs:115`、`backend/src/model.rs:128`、`backend/src/model.rs:179` | `Option<GoalId>` | 同上（`None` は「ゴールに属さない」として許容） | 対象 | - |
| タスク名 | `String` | `backend/src/model.rs:114`、`backend/src/events.rs:114` | `TaskName` | size（1文字以上）→ lexical content（許可文字）→ syntax（タスク名の書式）→ semantics（同一ゴール内で一意） | 対象 | - |
| run slug | `String` / `Option<String>` | `backend/src/model.rs:176`、`backend/src/model.rs:118`、`backend/src/mcp/tools.rs:587` | `RunSlug` / `Option<RunSlug>` | size（1文字以上）→ lexical content（許可文字）→ syntax（run ディレクトリ名の形式）→ semantics（実在する run） | 対象 | - |
| inquiry ID | `String` | `backend/src/model.rs:127`、`backend/src/model.rs:294`、`backend/src/grpc/mod.rs:621` | `InquiryId` | size（1文字以上）→ lexical content（許可文字）→ syntax（ID の書式）→ semantics（保留中の問い合わせに一致する） | 対象 | - |
| プルリクエスト番号 | `Option<i64>` | `backend/src/model.rs:103`、`backend/src/model.rs:290`、`backend/src/grpc/mod.rs:685` | `PullRequestNumber` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（対象ゴールの PR と一致する） | 対象 | - |
| プルリクエストの URL | `Option<String>` | `backend/src/model.rs:104`、`backend/src/model.rs:291`、`backend/src/grpc/mod.rs:182` | 生の `String` を維持 | なし（値の集合は takt 側が決める） | 対象外 | takt が返す URL をそのまま運ぶ値で、taktDash 側では不変条件を確定できない |
| 承認待ちの revision | `Option<i32>` / `i32` | `backend/src/model.rs:296`、`backend/src/model.rs:302`、`backend/src/grpc/mod.rs:706` | `RegistrationRevision` | size（正の整数）→ lexical content（数字）→ syntax（10進表記）→ semantics（承認待ちの提案と一致する） | 対象 | - |
| ゴール登録提案のディレクトリ | `String` | `backend/src/mcp/tools.rs:756`、`backend/src/model.rs:303`、`backend/src/grpc/mod.rs:759` | 生の `String` を維持 | なし（値は takt が返す提案値） | 対象外 | takt が返す提案値で、taktDash 側では不変条件を確定できない |
| until（マージの範囲） | `String`（`"prepare"` / `"merge"`） | `backend/src/grpc/mod.rs:688`、`backend/src/grpc/mod.rs:695`、`proto/taktdash.proto:414` | `MergeUntil`（列挙） | size（1文字以上）→ lexical content（ASCII 小文字）→ syntax（既知の2値）→ semantics（対象 PR の状態で実行できる） | 対象 | - |
| 閾値（判断待ちの滞留・ログの無音） | `u32` | `backend/src/config.rs:78`、`backend/src/config.rs:79`、`backend/src/config.rs:199` | `StaleMinutes` / `SilentMinutes` | size（0 以上、上限）→ lexical content（数字）→ syntax（10進表記）→ semantics（運用上の範囲） | 対象 | - |
| 読み取り間隔 | `u64` | `backend/src/config.rs:43`、`backend/src/config.rs:195`、`backend/src/poller.rs:97` | `IntervalSeconds` | size（1 以上）→ lexical content（数字）→ syntax（10進表記）→ semantics（ポーリング周期として有効） | 対象 | - |
| タスクの状態ごとの件数 | `i32` | `backend/src/grpc/mod.rs:191`、`backend/src/grpc/mod.rs:195`、`proto/taktdash.proto:141` | `TaskCount`（`TaskStatusCounts` の各値） | size（0 以上）→ lexical content（数字）→ syntax（10進表記）→ semantics（総数と一致する） | 対象 | - |
| 日次集計の日付 | `String`（`"YYYY-MM-DD"`） | `backend/src/activity.rs:20`、`backend/src/activity.rs:156` | `DailyDate` | size（10文字）→ lexical content（数字と `-`）→ syntax（`YYYY-MM-DD`）→ semantics（JST の実在日） | 対象 | - |
| 状態の種別（ゴール・実行・タスク・出来事） | 専用 enum（`GoalStatus` など） | `backend/src/model.rs:10`、`backend/src/model.rs:18`、`backend/src/model.rs:26`、`backend/src/model.rs:37` | 既存 enum を維持 | 変換時に既知の値だけを受ける（既に専用型） | 対象外 | 既に専用の型で、不正な種別を表現できない |
| run の状態 | 専用 enum（`RunStatus`） | `backend/src/model.rs:149`、`backend/src/mcp/tools.rs:610`、`backend/src/grpc/mapping.rs:55` | `RunStatus`（変換境界で必須化） | 既知の値だけを受ける（現在は `backend/src/mcp/tools.rs:610-615` が未知の値と欠落を `Running` に丸める） | 対象 | - |
| 時刻 | `DateTime<Utc>` | `backend/src/model.rs:106`、`backend/src/model.rs:133`、`backend/src/model.rs:201` | 既存型を維持 | 既に専用型（範囲を型が持つ） | 対象外 | 既に専用の型で、文字列ではない |
| 設定のファイルパス | `PathBuf` | `backend/src/config.rs:45`、`backend/src/config.rs:72`、`backend/src/config.rs:178` | 既存型を維持 | 既に専用型（絶対パスの検証は `backend/src/config.rs:179` にある） | 対象外 | 列挙した設定値は既に専用の型（`PathBuf`）で、文字列ではない |
| 待ち受けアドレス | `SocketAddr` | `backend/src/config.rs:47`、`backend/src/config.rs:207` | 既存型を維持 | 既に専用型（構文検証は `parse` が行う） | 対象外 | 既に専用の型で、不正なアドレスを表現できない |
| 自由文（目的・本文・理由・要約） | `String` | `backend/src/model.rs:101`、`backend/src/model.rs:108`、`backend/src/model.rs:130`、`backend/src/model.rs:208` | 生の `String` を維持 | なし | 対象外 | 真に単純な属性で、範囲・書式・他の属性との関係を持たない |
| takt が返す表示用の状態文字列 | `String` | `backend/src/model.rs:219`、`backend/src/model.rs:238` | 生の `String` を維持 | なし（値の集合は takt 側が決める） | 対象外 | takt が決める値をそのまま運ぶ項目で、taktDash 側では不変条件を確定できない |

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
| 2 | backend の値の置き換え | 12 種類・約 20 箇所 | `backend/src/model.rs`、`backend/src/config.rs`、`backend/src/grpc/mod.rs`、`backend/src/events.rs`、`backend/src/activity.rs`、`backend/src/mcp/tools.rs` |
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

- `ApproveGoalMergeRequest.until`（`proto/taktdash.proto:414`）を `string` から `MergeUntil` の enum へ変える。既存の送信側（`backend/src/grpc/mod.rs:688`、`frontend/cli/src/main.rs:472`）と生成物の両方に影響します。
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
| takt が返す表示用の状態 | `backend/src/model.rs:219` の `StepLogEntry.status` と `backend/src/model.rs:238` の `RunInstructionEntry.state` は `String` で、値の集合は takt 側が決める | takt は対象外のため「対象外」とする。taktDash 側で不変条件を決める必要が出た場合は、その時点で別の要求として扱う |
| 件数を値オブジェクトにする範囲 | `proto/taktdash.proto:141`〜`:148` の `TaskStatusCounts`、`backend/src/activity.rs:20`〜`:22` の `DailyCount`、`frontend/cli/src/main.rs:962`〜`:974` の集計が該当する | 「時刻や間隔などの量」に件数を含めて「対象」とする。総数との一致という意味上の制約を持つため、単純な属性ではないと判断した |
