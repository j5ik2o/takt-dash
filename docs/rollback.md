# backend を前の版へ戻す手順（出来事の種類が増えた変更のあと）

アクティビティに記録する出来事の種類が増える変更（#151 の `task_failed`、`task_exceeded`、`task_pr_failed`、`run_failed`、`run_aborted`、`backend_started` など）を本番へ入れたあとに、backend を前の版へ戻すときの手順です。web だけを戻すときは、この手順は要りません。

## 何が起きるか

前の版の backend は、出来事の一覧を読むときに保存済みの種類を既知の値へ変換します（`backend/src/activity.rs` の `recent_events`、`backend/src/model.rs` の `EventKind::from_stored`）。未知の種類は変換に失敗するため、取得範囲に新しい種類の行が1件でもあると、一覧全体が読めません。gRPC では `GetActivity` と `GetProjects` が `Status::internal` を返します。

`events` テーブルは削除も期限も持ちません。新しい種類の行を消さずに戻すと、前の版ではアクティビティと最近の出来事を読み続けられません。

## 手順

1. backend と web を止める。
2. 記録を残す場合は、SQLite を退避する。退避はこの時点で行う。
   ```sh
   sqlite3 data/taktdash.sqlite ".backup events-before-rollback.sqlite"
   ```
   SQLite の置き場所は設定の `sqlite.path` です（既定は設定ファイルと同じディレクトリの `data/taktdash.sqlite`）。
3. 新しい種類の行を `events` から消す。残すのは、戻す版が知っている種類だけにする。
   ```sql
   DELETE FROM events
    WHERE kind IN ('task_failed', 'task_exceeded', 'task_pr_failed', 'run_failed', 'run_aborted', 'backend_started');
   ```
4. 前の版の backend と web を起動する。

## web だけ前の版へ戻す場合

出来事の記録はそのまま続けます。データは消しません。前の版の web は、知らない種類の出来事を「その他」として表示します。そのあと backend も戻すときは、上の手順 2 と 3 が必要です。

## 関連

- 前の版へ戻さずに新しい版を入れ直すときは、proto の列挙値と出来事の種類が増えているため、backend と web の両方を入れ替えます。データの移行は要りません。
