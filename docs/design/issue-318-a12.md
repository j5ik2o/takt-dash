# issue #318 A-12: アクティビティの日付の条件を時刻の範囲にして索引を効かせる

対象: [j5ik2o/takt-dash#318](https://github.com/j5ik2o/takt-dash/issues/318) の A-12。この文書が扱うのは A-12 の「日付の条件で索引が効かない」だけ。

## 現状（main 54b10a6 で確かめた）

`backend/src/activity.rs` で、JST の日を数える 2 つの読み取りが、日付の条件を `date(occurred_at, '+9 hours') = ?1` と書いている。

- `count_on`（377-398 行）: `daily_summaries` から 7 日分 × 2 回（総数と判断待ち）、1 回の呼び出しで 14 本流れる。
  - `SELECT COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1`
  - `SELECT COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1 AND kind IN (?, ?)`
- `today_counts_by_server`（319-339 行）:
  - `SELECT server_id, COUNT(*) FROM events WHERE date(occurred_at, '+9 hours') = ?1 AND server_id != '' GROUP BY server_id ORDER BY COUNT(*) DESC, server_id`

索引 `events_occurred_at ON events (occurred_at DESC)` は `migrate`（372 行）ですでに作っている。条件が列そのものではなく `date(...)` の結果を比べるので、SQLite はこの索引で範囲を絞れない。main の実行計画は次のとおり（下の索引のテストの失敗の出力から写した）。

| 読み取り | 実行計画 |
| --- | --- |
| `count_on` | `SCAN events USING COVERING INDEX events_occurred_at`（索引を頭から全部読む） |
| `today_counts_by_server` | `SCAN events`、`USE TEMP B-TREE FOR GROUP BY`、`USE TEMP B-TREE FOR ORDER BY` |

どちらも、表の行の数に比例して読む。events には保持期限が無いので、行は増え続ける。

## 直した後の条件

JST の日 `d` を、UTC の時刻の半開区間 `[start, end)` に置き換える。

- `start` = `d` の JST 00:00:00.000 を UTC にした時刻（`d` の前日の 15:00:00.000Z）。
- `end` = `start` + 1 日（`d` の 15:00:00.000Z。翌日の JST 00:00:00.000）。
- どちらも `format_timestamp`（`%Y-%m-%dT%H:%M:%S%.3fZ`）で文字列にして束縛する。保存している値と同じ書式にするため。

条件は `occurred_at >= ?start AND occurred_at < ?end` にする。ほかの条件（`kind IN (...)`、`server_id != ''`）、`GROUP BY`、`ORDER BY COUNT(*) DESC, server_id` は変えない。`completed_tasks_since`、`recent_events`、`last_connection_event` はもとから列そのものを比べているので変えない。

実装の形（範囲を作る関数を 1 つ置くか、`count_on` の引数を日付の文字列から `NaiveDate` か範囲に変えるか）は実装の担当に任せる。テストは公開の関数の結果と、実際に流れた SQL の実行計画だけを見るので、形には依らない。下書きで、上の条件に書き換えるとテストがすべて通ることを確かめた（下書きはコミットしていない）。

### 同じ結果になる理由

- events に書くのは `record` と `record_all` だけで、どちらも `occurred_at` を `format_timestamp` で書く。最初のコミット（4f94740）から書式は同じ。
- この書式は 0000〜9999 年で幅が固定なので、文字列の大小と時刻の大小が一致する（`format_timestamp` の doc にも書いてある）。
- JST には夏時間が無く、差は常に +9 時間。よって `date(t, '+9 hours') = d` と `start <= t < end` は、上の書式の `t` で同じになる。
- ミリ秒の桁まで比べるので、JST の 23:59:59.999 は `d` に入り、次の 00:00:00.000 は入らない。

## 守る不変条件

- I1: 任意の出来事の集まりと任意の `now` で、`daily_summaries(now)` が返す 7 日分の（日付、総数、判断待ち）が、main の SQL（`date(occurred_at, '+9 hours')`）で数えたものと同じで、並び（古い日から）も同じ。
- I2: 同じ入力で、`today_counts_by_server(now)` が返す（サーバー、件数）の列が、main の SQL で数えたものと同じで、並び（件数の多い順、同数はサーバー名の順）も同じ。
- I3: 2 つの読み取りで流れる events への SQL は、すべて `events_occurred_at` で範囲を絞って読む（`SEARCH`）。表も索引も頭から読まない。

## 壊しうる入力

| 入力 | 直した後の扱い | テスト |
| --- | --- | --- |
| JST の日の境目の 00:00:00.000（前日の 15:00:00.000Z） | `>=` なので、その日に入る | 例のテスト、性質のテスト（境目に寄せて生成） |
| JST の 23:59:59.999（14:59:59.999Z）と、その 1 ms 後 | 23:59:59.999 は入り、1 ms 後は `<` で次の日になる。`<=` にすると、例と性質の両方のテストが落ちることを確かめた | 同上 |
| UTC の日の境目（JST の 09:00、00:00:00.000Z の前後） | JST の日を分けない | 例のテスト、性質のテスト |
| うるう秒 | 扱わない。`occurred_at` は `Utc::now()` から作り、`Utc::now()` はうるう秒の値（`:60`）を返さない。仮に `…T14:59:60.500Z` があると、main は `date()` が NULL で数えず、直した後は前日に数える | 生成しない（`from_timestamp_millis` はうるう秒を作らない） |
| `occurred_at` の書式の揺れ（`+00:00`、ミリ秒なし、空白区切り、別の時差） | 書くのは `format_timestamp` だけなので、保存済みのデータに揺れは無い。揺れがあると結果が変わりうる（例: `…T15:00:00+00:00` は `…T15:00:00.000Z` より小さく並ぶ）。だから、events に書く経路を増やすときは `format_timestamp` を通すこと | 書く経路が `record`・`record_all` だけであることを確かめた |
| 0000〜9999 年の外の時刻 | `format_timestamp` は `+10000-…`・`-0001-…` を作り、`date()` は NULL を返す。`Utc::now()` からは作られない | 生成の範囲を 0001-01-10〜9999-12-30 にした（SQLite の日付関数が扱う範囲の内側。7 日前と翌日まで収まる） |
| 空のテーブル | 7 日とも 0 件、サーバー別は空 | 例のテスト、性質のテスト（0 件の集まりも生成する） |

## 保存済みのデータ（SQLite）の形

変えない。表の列、`occurred_at` の書式、索引はそのまま使う。索引 `events_occurred_at` はすでにあるので、作る文（マイグレーション）は要らない。索引は `DESC` だが、SQLite は降順の索引でも範囲を絞って読める（下書きの実行計画で `SEARCH` になった）。

## 索引が効くことの確かめ方

`backend/src/activity_index_tests.rs`（`activity.rs` の `#[cfg(test)]` の子のモジュール）で確かめる。

1. メモリ上の store に、対象の日と対象の外の日の出来事を記録する。
2. writer の接続（メモリ上の store は読み取りも同じ接続を使う）に `trace_v2`（`SQLITE_TRACE_PROFILE`）を付け、`daily_summaries` と `today_counts_by_server` を呼ぶ。流れた SQL を、束縛した値を埋めた形（`expanded_sql`）と `FULLSCAN_STEP` の値と一緒に集める。
3. events を読む SQL のそれぞれについて、`EXPLAIN QUERY PLAN` の detail に `SEARCH events USING ... INDEX events_occurred_at (occurred_at...` があり、`SCAN events` で始まる行が無く、`FULLSCAN_STEP` が 0 であることを確かめる。

実際に流れた SQL を見るので、実装が SQL の文面や関数の分け方を変えても、テストは書き換えなくてよい。`trace_v2` を使うため、`backend/Cargo.toml` の dev-dependencies で rusqlite に `trace` の feature を足した（テストのビルドだけに入る）。

## テスト

- `backend/tests/activity_jst_day_test.rs`
  - 性質（proptest、256 件、除外なし）`jst_day_reads_match_the_date_function_oracle`: `now` と出来事の集まり（0〜39 件）を生成し、ファイルの SQLite に `record_all` で書く。同じファイルを別の接続で開き、main の SQL をこのファイルに写した原典（`date(occurred_at, '+9 hours')`）で数え、`daily_summaries` と `today_counts_by_server` の結果と並びまで比べる（I1、I2）。出来事の時刻の半分ほどは、`now` の JST の日の 8 日前〜翌日の、JST の日の境目（00:00:00.000、00:00:00.001、23:59:59.998、23:59:59.999）と UTC の日の境目（JST の 08:59:59.999、09:00:00.000、09:00:00.001）に寄せる。種類は 15 種類のすべて、サーバーは空を含む 4 つから選ぶ。
  - 例のテスト: `jst_day_boundary_splits_at_jst_midnight`（JST の日の境目の前後）、`utc_day_boundary_does_not_split_the_jst_day`（UTC の日の境目の前後）、`empty_table_matches_the_oracle`（空のテーブル）。境目のテストは、期待値を直接書いたうえで、原典とも比べる。
- `backend/src/activity_index_tests.rs`
  - `daily_summaries_search_the_occurred_at_index`、`today_counts_by_server_search_the_occurred_at_index`（I3）。

### main での結果

- 索引のテストは 2 件とも落ちる（red）。`count_on` は `SCAN events USING COVERING INDEX events_occurred_at`、`today_counts_by_server` は `SCAN events`。
- 性質のテストと例のテストは通る（原典と同じ SQL なので当然通る。実装の後も通ることで、結果が変わらないことを守る）。

## 範囲の外

- 保持期限（古い行を消す処理）。A-12 の直し方の案に入っているが、何日残すかを決める判断が要るので、この件では扱わない。範囲で絞れば、行が増えても読み取りは重くならない。ただし、ファイルは大きくなり続ける。
- A-13、R13、P1 など、#318 のほかの指摘。
- `occurred_at` の書式を正規化する移行（揺れは無いので要らない）。

## Quint

要らない。読み取りの条件を、同じ入力で同じ結果を返す別の条件に書き換えるだけで、状態遷移（反復、再試行、再開、ロック、並行）に関わらないため。
