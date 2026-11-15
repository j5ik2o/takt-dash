# manager の会話の保存場所と取得経路の調査

この文書は issue #38 の調査段階の結果をまとめたものです。manager の発言本文がどこに保存されているかを一次情報で確認し、backend が保存場所を直接読む経路で会話の時系列を再構成できるかを判定し、takt-mcp への道具追加の要否を判定します。この文書は調査の記録であり、コードの変更を含みません。

## 調査の実施条件

- 基点のコミットは `4b9325c`（`git rev-parse --short HEAD` で確認）です。
- 作業開始時に `git fetch origin` を実行しました。この実行は失敗し、`Please make sure you have the correct access rights and the repository exists.` というメッセージが返りました。したがって、origin の最新の状態はこの環境から取得できていません。
- `gh issue view 38` は成功し、issue 本文は `order.md` の記述と一致していました。HTTP 401 は発生していません。
- 調査は読み取りのみで行いました。会話とセッションのファイルは一切書き換えていません。SQLite は `immutable=1` を指定した読み取り専用接続で開きました。
- 秘密の値（トークン、認証情報など）は読みませんでした。`~/.codex/auth.json` と、SQLite の `account`・`credential`・`control_account` テーブルは開いていません。

## 確認した保存場所

manager の発言本文は、`~/.takt/conversations/` の記録ではなく、AI プロバイダのセッション保存領域にあります。この環境で JSON として読み取れた manager の記録はすべて `provider` が `opencode` であり、本文は `~/.local/share/opencode/opencode.db` に保存されていました。

### 状態の記録（`~/.takt/conversations/`）

- 保存先は `~/.takt/conversations/manager-<id>.json` と `~/.takt/conversations/director-<id>.json` です。形式は 1 ファイル 1 エージェントの JSON です。
- トップレベルのキーは `agent`、`assignment`、`bindings`、`contextKey`、`counterpart`、`daemonState`、`id`、`lastSpeaker`、`model`、`pending`、`provider`、`revision`、`running`、`schemaVersion`、`sessionId` です（`manager-0d1eb71e...json` で確認）。
- `schemaVersion` は `1` です。
- `sessionId` はプロバイダのセッション識別子です。例は `ses_e28a6f57cffe2N2VxzJLNCevN0` です。
- `assignment` は `{kind, repositoryDirectory}` の形です。例は `{"kind":"repository","repositoryDirectory":"/Users/j5ik2o/Sources/j5ik2o.github.com/j5ik2o/takt-dash"}` です。
- `daemonState` は `receipts`、`toolHistory`、`version` を持ちます。`receipts` は要素が `{turn, change, version}` の配列です。
- この JSON には発言の本文に相当するフィールドがありません。`text`・`content`・`message`・`body`・`response`・`prompt` という名前のフィールドを再帰的に検索したところ、1 件も見つかりませんでした。したがって、`~/.takt/conversations/` は会話の状態と受領の記録を持ち、本文は持ちません。

### 本文の記録（`~/.local/share/opencode/opencode.db`）

- 保存先は `~/.local/share/opencode/opencode.db` です。形式は SQLite です。
- 本文は `session_message` テーブルにあります。列は `id`、`session_id`、`type`、`seq`、`time_created`、`time_updated`、`data` です。
- セッションのメタ情報は `session_v2` テーブルにあります。この調査で使う列は `id`、`directory`、`version`、`time_created`、`time_updated` です。
- `session_part` テーブルも存在しますが、観測した manager セッションでは行が 0 件でした。

### 突き合わせの結果

`~/.takt/conversations/` の `sessionId` と `session_v2.id` を一致させた結果は次のとおりです。

- `~/.takt/conversations/` には manager のファイルが 11 件あります。
- そのうち 10 件は JSON として読み取れ、`sessionId` を持ち、`session_v2.id` と一致しました。一致した 10 件はすべて `provider` が `opencode` でした。
- 残る 1 件（`manager-125898f1...json`）はファイルサイズが 0 バイトであり、JSON として読み取れませんでした。
- `director-e61579db...json` は `provider` が `claude-sdk`、`assignment.kind` が `daemon` であり、`session_v2.id` とは一致しませんでした。このセッションの本文は `~/.claude/projects/-Users-j5ik2o--takt/c6c0fa1b-0dbb-4df9-94b2-d0d86e894d4d.jsonl` にありました。すなわち、プロバイダが `claude-sdk` のセッションは opencode の DB ではなく Claude の JSONL に保存されます。

一致した manager セッションの `session_message` の件数は 5 件から 456 件でした。例として `manager-269e34f2...json` のセッションは 307 件です。

## レコード構造

`session_message` の 1 行が 1 つの発言または 1 つの出来事に対応します。時刻、話者、本文は次のフィールドで表されます。

- 時刻: `time_created` は UNIX エポックのミリ秒です。例は `1794613221331` で、UTC の `2026-11-13T23:40:21.331Z` に相当します。`data` の中にも `time` があり、こちらもミリ秒です。
- 話者と種類: `type` で区別します。`manager-269e34f2...json` のセッションで観測した値と件数は、`assistant` 159 件、`agent-switched` 49 件、`user` 49 件、`idle` 48 件、`model-switched` 1 件、`system` 1 件です。
- 本文（人の発言）: `type` が `user` の行では `data.text` に本文があります。`data` のキーは `text` と `time` です。`data.text` は `## 前のターンで受け取った人の発言` と `## このターンの入力` という見出しで区切られており、人の発言とイベント JSON と業務コンテキストが同じ文字列に混在します。
- 本文（AI の発言）: `type` が `assistant` の行では `data.content` に配列があります。要素の `type` は `text` と `tool` です。`text` の要素は `{type, text}` の形です。`tool` の要素は `{id, name, state, type, time, executed}` を持ち、`state` は `{input, content, metadata, status}` を持ちます。`data` にはほかに `agent`、`cost`、`finish`、`model`、`rawFinish`、`snapshot`、`tokens`、`time` があります。
- 状態の変化: `type` が `agent-switched` の行は `data` に `agent`、`previous`、`time` を持ちます。`model-switched` の行は `model`、`time` を持ちます。`idle` の行は `outcome`、`time` を持ちます。`system` の行は `description`、`metadata`、`text`、`time` を持ちます。

引用した値は、レコードの形を説明するために `type`、時刻、フィールド名、件数に限定しました。人の発言本文と業務コンテキストの内容は引用していません。

## 複数プロジェクトの区別

プロジェクトは、`~/.takt/conversations/` の `assignment.repositoryDirectory` と `session_v2.directory` の一致によって区別できます。

- 例として `manager-269e34f2...json` の `assignment.repositoryDirectory` は `/Users/j5ik2o/Sources/j5ik2o.github.com/j5ik2o/takt-dash` であり、対応する `session_v2.directory` も同じ値でした。
- `taktdash.config.json` の `projects` 配列は `cwd` を持ちます。`takt-dash` の `cwd` は `taktdash.config.json:39` にあり、値は `/Users/j5ik2o/Sources/j5ik2o.github.com/j5ik2o/takt-dash` です。この値は `assignment.repositoryDirectory` と `session_v2.directory` に一致します。
- `session_v2.directory` は worktree ごとに異なる値を持ちます。観測した値には `/Users/j5ik2o/Sources/github.com/j5ik2o/takt-worktrees/...` のように worktree のパスを持つものがありました。したがって、会話をプロジェクトの単位にまとめる場合は、`directory` の完全一致ではなく、前方一致や設定の `cwd` への正規化が必要になります。

## 追記のされ方

会話は既存の記録の上書きではなく、ターンごとの追記で記録されます。

- `session_message` の `seq` はセッション内で一意です。`session_message_session_seq_idx` という一意索引が `(session_id, seq)` に張られています。
- `manager-269e34f2...json` のセッションでは、`seq` は最小 3、最大 1421、行数 307 で、`seq` の昇順に user と assistant の行が並びます。
- `seq` は連番ではなく、間隔が空きます。このセッションでは隣り合う `seq` の差は 1、3、5、7、11 などで、307 行に対して `seq` の範囲は 1419 です。差が 3、5、7 になる間隔が大部分を占めます。欠番を作る仕組みの詳細は未確認ですが、`seq` はセッション内で一意な単調増加の採番であり、欠番そのものは行の欠落を意味しません。
- 観測の時点で `opencode.db-wal` は `opencode.db` と同じ時刻に更新されていました。DB は約 916 MB で、WAL は観測の約 20 分の間に約 1 MB から約 2.6 MB へ増えました。WAL のサイズは一定ではなく、進行中の書き込みに応じて増えます。このことから、進行中の書き込みは WAL に追記され、チェックポイントで DB へ反映されると判断しました。

## 時系列を再構成できるかの判定

backend が `~/.local/share/opencode/opencode.db` を直接読む経路で、会話の時系列を再構成できます。ただし、プロバイダが `opencode` のセッションに限り、進行中の追記を追う場合は WAL を含めて読む必要があります。判定の根拠を 5 項目に分けて示します。

### 並び順の決め方

`session_message.seq` を昇順に並べれば、セッション内の順序を再現できます。`seq` はセッション内で一意であり、`(session_id, seq)` の一意索引があります。`session_message_session_time_created_id_idx` という `(session_id, time_created, id)` の索引も存在しますが、時刻が同じ行の順序を決めるのは `seq` が適切です。

### 重複や欠落

`seq` が連番でないため、`seq` の欠番を記録の欠落と解釈してはいけません。重複は `(session_id, seq)` の一意索引で防がれています。実際のメッセージ数は行数で数える必要があり、`max(seq)` を行数として扱うと件数を過大に見積もります。`manager-269e34f2...json` では `max(seq)` が 1421 であるのに対し、実際の行数は 307 です。欠番を作る仕組みは未確認であり、欠番の数から何かを導くこともしていません。

### 進行中の追記の追跡

`seq` が単調増加であるため、最後に読んだ `seq` より大きい行を読む差分取得で追記を追えます。ただし、書き込みは WAL に追記されるため、読み取り側が別プロセスで接続する場合は WAL の内容も見える必要があります。DB ファイルだけをコピーして読むと、チェックポイント前の最新の行を取りこぼします。

### プロジェクトの識別

`session_v2.directory` と `assignment.repositoryDirectory` の一致でプロジェクトを識別できます。worktree では `directory` が worktree のパスになるため、プロジェクトへの正規化が必要です。

### フォーマットの安定性

`~/.takt/conversations/` の `schemaVersion` は `1` です。`session_v2.version` は観測したセッションで `2.2.5`、Claude のセッションの `version` は `2.1.300` でした。`session_message.data` はプロバイダの内部形式であり、`manager-269e34f2...json` のように `type` ごとに `data` の形が異なります。したがって、`data` の構造はバージョン間で変わる可能性があり、backend が `data` を解析する場合はバージョンの変化に備える必要があります。`session_message` の列（`id`、`session_id`、`type`、`seq`、`time_created`、`time_updated`、`data`）という表の構造は、`data` の内部形式より安定していると判断しました。

## takt-mcp への道具追加の要否

takt-mcp への道具追加は不要と判定します。この判定の根拠は、確認できる次の3点に限ります。

- 経路 (b) の backend が保存場所を直接読む方式で、時系列を再構成できます。この判定の詳細は、この文書の「時系列を再構成できるかの判定」の節に示しました。なお、run 詳細のログと usage-events を backend がファイル直接読む方式は `docs/requirements.md:45` の 2.4 節に実在します。ただしこの先行例が定める範囲は run ログと usage-events に限られ、会話の取得の可否を定めるものではありません。この文書が会話取得の可否の根拠にするのは、あくまで「時系列を再構成できるかの判定」の節の観測結果です。
- 経路 (a) の takt-mcp に道具を追加する方式は、takt フォークと takt-mcp の変更を必要とします。`order.md` の制約は takt フォークと takt-mcp の変更を禁じており、この調査の範囲では経路 (a) を実装できません。
- `docs/requirements.md:70` は、manager と director の内部 HTTP 接続を「外部のダッシュボード向けの安定した接続点ではない」と定めています。このため、taktDash が会話を取る接続点として、その内部接続を前提にすることはできません。

この判定は本ワークスペースで確認できた事実に基づきます。takt-mcp の道具定義と manager の会話実行経路の実装は本ワークスペースの外にあり、この調査では読んでいません。そのため、経路 (a) を選んだ場合に会話の取得が可能かどうかは未確認であり、「不要」の根拠に含めていません。この未確認の事項は「未確認」の節に記載しました。

また、この判定はプロバイダが `opencode` のセッションに限ります。Claude などの他のプロバイダのセッションは別の保存形式を持つため、その取得経路は別に判定する必要があります。

## 未確認

次の項目は、この環境で実ファイルを読んで確認できませんでした。推測で確定していません。

- Mac Studio 上の保存先。この作業環境は Mac mini 相当であり、Mac Studio の保存領域は参照できません。
- takt manager と takt director を起動するデーモンの側が会話をどこへ複製するか。`daemonState` に本文が無いことは確認しましたが、デーモンが別の場所に本文を複製しているかは確認していません。
- `provider` が `claude-sdk` 以外のセッションの保存形式。観測した `claude-sdk` のセッションは `~/.claude/projects/` の JSONL にあり、`timestamp`、`sessionId`、`cwd`、`uuid`、`parentUuid`、`message` を持ちますが、他のプロバイダの形式は確認していません。
- `~/.codex/` 配下のセッション形式。`~/.codex/sessions/` に JSONL のファイルがあることは確認しましたが、内容の構造と `sessionId` の対応は確認していません。
- takt-mcp の道具定義と manager の会話実行経路の実装。takt フォークと takt-mcp は本ワークスペースの外にあり、この調査では読んでいません。経路 (a) で会話の本文を取得できるかどうかは未確認です。
- `session_part` の用途。観測したセッションでは行が 0 件でしたが、他のセッションで使われるかは確認していません。
- `opencode.db` の `session_message` の `data` が将来のバージョンで変わるかどうか。

## 読んだファイルの一覧

- `~/.takt/conversations/manager-*.json`（11 件。1 件は 0 バイト）
- `~/.takt/conversations/director-e61579db1f9e18cbbe88de98df4bf0747e786e1d8065e15fc567d6a4bcab8910.json`
- `~/.local/share/opencode/opencode.db`（`session_v2`、`session_message`、`session_part`。`immutable=1` の読み取り専用接続）
- `~/.claude/projects/-Users-j5ik2o--takt/c6c0fa1b-0dbb-4df9-94b2-d0d86e894d4d.jsonl`
- `taktdash.config.json`
- `docs/requirements.md`
- `proto/taktdash.proto`
- `backend/src/mcp/tools.rs`
