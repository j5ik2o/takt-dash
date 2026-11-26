# issue #90 の設計：マージ待ちの起点を、仕上げタスクが終わって人が待ち始めた後に確定する

対象の issue：[j5ik2o/takt-dash#90](https://github.com/j5ik2o/takt-dash/issues/90)

## 現在の main での確認（`ca9f308`）

コードを読んで、issue の現象が起きる作りであることを確かめました。

1. **起点はタスクを見ずに決まる。** `backend/src/mcp/tools/goal.rs:51-68` の `resolve_awaiting_merge_at` は、引数にタスクを持たない。awaiting_merge 通知があればその `recordedAt`、無ければ前回の起点、それも無ければ観測時刻を返す。仕上げタスク（`mergeTask: true`）が pending/running でも起点を持つ。
2. **通知も仕上げタスクより前に記録される。** takt（`origin/integration/takt-mcp` `94e7318`）の `src/infra/goals/integration.ts` は、ゴールを awaiting_merge にするのと同時に awaiting_merge 通知を記録する（理由は `Goal PR requires preparation through a PR merge task`）。仕上げタスクはその後に投入される。issue は「通知が無いまま」の経路を挙げているが、通知がある普通の経路でも、起点は仕上げタスクの前になる。
3. **判断待ちの一覧は、仕上げタスクが終わってから出す。** `backend/src/store.rs:538-545` の `is_human_merge_wait` は、仕上げタスクに pending/running があれば人のマージ待ちにしない。`merge_decisions`（`store.rs:465-505`）は、出すときに `awaiting_merge_at` からの経過で stale を決める。仕上げタスクが長く走ると、一覧に出た時点ですでに stale になりうる。

### takt-mcp の応答にある情報

`takt_list_tasks` の各タスクは、`goalId`・`status`・`mergeTask`・`completedAt` を返す（takt の `src/features/mcp/operations.ts` の `taskSummary`）。`completedAt` は、終わりの状態（completed・failed・pr_failed など）へ移すときに takt が入れる（`src/infra/task/taskLifecycleService.ts`）。backend はすでに `goalId`・`status`・`mergeTask` を読んでいる（`goal.rs` の `parse_task`）。足りないのは `completedAt` だけで、上流に追加を頼む情報は無い。別の issue に切り出すものは無い。

## 何をどう変えるか

1. **`TaskEntry` に `completed_at: Option<DateTime<Utc>>` を足す**（`backend/src/model.rs`）。`parse_task` は `completedAt` を `startedAt` と同じ `parse_time` で読む（無ければ `None`、形が不正なら解析エラー。`startedAt` と同じ扱い）。
2. **「ゴールの仕上げタスクが pending/running か」の判定を1か所にする。** `model.rs` に `TaskEntry::is_active_merge_task_of(&self, goal_id: &GoalId) -> bool`（`merge_task` が真、`goal_id` が一致、状態が Pending/Running）を足す。`is_human_merge_wait` もこれを使う（判定は変えない）。
3. **`resolve_awaiting_merge_at` にゴールのタスクを渡す。** 引数を `(goal, notification_start, tasks: &[TaskEntry], prev, observed_at)` にする。規則：
   - awaiting_merge でないゴール：いまと同じ（通知の起点、無ければ `None`）。
   - awaiting_merge で、仕上げタスクが pending/running：`None`（まだ人が待っていない）。
   - awaiting_merge で、仕上げタスクが pending/running でない：次の候補のうち最も遅いもの。
     - awaiting_merge 通知の起点（いまと同じ。読み取りに失敗したときは前回の起点を引き継いだ値）
     - そのゴールの仕上げタスクの `completed_at` のうち最も遅いもの
     - 前回 snapshot の同じゴールの起点
     - そのゴールに `completed_at` の無い終わった仕上げタスクがあり、前回の起点も無いときは、観測時刻
   - 候補が1つも無ければ観測時刻（いまと同じ）。
4. **`read_snapshot`**（`backend/src/mcp/tools/mod.rs`）は `&tasks` を渡す。タスクの読み取りはすでに通知より前に済んでいる。
5. **文の更新**：`model.rs` の `awaiting_merge_at` の説明、`read_snapshot` と `resolve_awaiting_merge_at` の説明、`proto/taktdash.proto` の `Goal.awaiting_merge_at` の行末の説明（「人のマージ待ちになった時刻。仕上げタスクが未着手・実行中のあいだは無い」。番号と型は変えない）。

前回の起点を候補に入れて「最も遅いもの」を取るのは、既存の滞留防止（読み取りのたびに観測時刻へ戻らない）を保ったまま、仕上げタスクが一覧から外れたとき（`takt_cleanup_tasks` など）に起点が通知の時刻へ戻らないようにするためです。前回の起点は、仕上げタスクの実行中は `None` なので、前の待ちの古い起点が新しい待ちへ持ち越されることは、読み取りのあいだに仕上げタスクの実行がまるごと収まったときしか起きず、そのときも新しい `completed_at` の方が遅いので選ばれません。

### 変えないこと・範囲の外

- 通知の読み取り（`list_notifications`・`list_goal_notifications`・`inherit_previous_notifications`）、`is_human_merge_wait` の判定、stale の閾値、判断待ちの一覧の出し方、出来事（`AwaitingMerge`）の記録。
- awaiting_merge でないゴールが通知の起点を持ち続けること（いまの振る舞い。直すなら別件）。
- web の `isHumanMergeWaiting` が「最後の」仕上げタスクだけを見て、backend が「どれか」を見る違い（別件）。
- web の画面（`awaitingMergeAt` は保存状態の復元でしか読まない）。

### 画面への影響（相談）

`Goal.awaiting_merge_at` は、仕上げタスクが pending/running のあいだ無くなります。CLI の `goals`（`frontend/cli/src/render.rs:278-281`）は、マージ待ちの「あり／なし」をこの値の有無で出すため、仕上げタスクの実行中は「マージ待ち: なし」に変わります（いまは「あり」）。web の `isHumanMergeWaiting` は実行中を人のマージ待ちとしないので、CLI の表示が web とそろう向きです。CLI のコードは変えず、`frontend/cli/README.md` の説明に「仕上げタスクが未着手・実行中のあいだは『なし』」を足す案にしています。これでよいかを、起動した担当に確かめます。

## 守る不変条件

- **待ちの前にしない**：awaiting_merge で仕上げタスクが pending/running でないとき、起点はそのゴールの仕上げタスクのどの `completed_at` よりも前にならない。
- **実行中は持たない**：awaiting_merge で仕上げタスクが pending/running のとき、起点は `None`。
- **滞留防止（既存）**：上流の値が同じなら、観測時刻が進んでも起点は変わらない。前回の起点がある読み取りで、起点は観測時刻に依らない。
- **戻らない**：awaiting_merge で仕上げタスクが pending/running でないとき、起点は前回の起点より前にならない。
- **他のゴール・通常タスクに依らない**：ほかのゴールのタスク、`goalId` の無いタスク、仕上げでないタスクは、起点に影響しない。
- **未来にしない**：候補（通知・`completed_at`・前回）と観測時刻のどれかと一致する値しか返さない。

## PBT の性質（`backend/src/mcp/tools/goal.rs` の `#[cfg(test)]`、proptest）

生成：ゴールの状態（3種）、通知の起点（`Option<時刻>`）、タスク 0〜5 件（`goal_id` は対象ゴール・別ゴール・無し、`merge_task` は真偽、状態は6種、`completed_at` は `Option<時刻>`）、前回 snapshot（無し、または対象ゴールの起点 `Option<時刻>`）、観測時刻。除外はつけません。

1. **実行中は持たない**：awaiting_merge で、対象ゴールの仕上げタスクに pending/running があれば `None`。
2. **待ちの前にしない・戻らない**：awaiting_merge で、対象ゴールの仕上げタスクに pending/running が無ければ `Some(s)` で、`s` は通知の起点・対象ゴールの仕上げタスクのどの `completed_at`・前回の起点のどれ以上。
3. **滞留防止**：1回目の結果を前回の起点にして、観測時刻だけを後へずらして2回目を解くと、同じ値になる（すべての入力で）。
4. **他に依らない**：対象ゴール以外のタスクと仕上げでないタスクを足しても、結果は変わらない。
5. **値の出どころ**：結果は、通知の起点・`completed_at`・前回の起点・観測時刻のどれかと一致する。候補が1つも無ければ観測時刻。
6. **awaiting_merge でない**：結果は通知の起点と一致する（いまと同じ）。

ほかに、`read_snapshot` を通した回帰テストを `backend/tests/mcp_client_test.rs` に足します（先に書いて、修正前に落ちることを確かめる）。

- 仕上げタスクが running のあいだは `awaiting_merge_at` が `None`。
- 通知（仕上げタスクより前）があり、仕上げタスクが `completedAt` 付きで終わったら、起点は `completedAt`。
- 起点を確定したあと、観測時刻の違う読み取りを重ねても起点が変わらない（既存の滞留防止。仕上げタスクを経た待ちでも）。
- 起点を確定したあと、仕上げタスクが一覧から消えても、起点は通知の時刻へ戻らない。
- `completedAt` の無い終わった仕上げタスクは、最初の観測時刻を起点にし、次の読み取りで保つ。

既存のテスト（`awaiting_merge_at_comes_from_the_awaiting_merge_notification_recorded_at` など、仕上げタスクの無いゴール）は、そのまま通るはずです。

## Quint

`specs/quint/merge-wait-start/`（`mergeWaitStart.qnt`、`mergeWaitStart_test.qnt`、`README.md`）。`specs/quint/ci-specs.json` に加え、CI の `quint` ジョブで検査します。モデルの範囲・条件・結果は README に書きました。要点：

- 1ゴールと仕上げタスク1件。動作：時刻を進める、awaiting_merge に入る（通知を記録）、仕上げタスクの投入・開始・終了（`completedAt` あり／なし）、一覧から外す、reopen、backend の再起動、読み取り。
- 不変条件 `safety` = `notBeforeWait`・`noStartWhileFinishing`・`noReset`・`neverMovesBack`・`notInFuture`。
- 結果（quint 0.32.0）：`current`（修正前）は `notBeforeWait` と `noStartWhileFinishing` で反例。`noReset` などは反例なし。`fixed` は 50000 サンプル、max-steps 30、seed 90 で反例なし。`quint test` は currentTest 1、fixedTest 5 が通る。
- **残る取りこぼし**（witness `evidenceLost`）：待ちの開始以後の時刻（`completedAt`、それで確定した前回の起点）が backend に何も残っていない読み取りでは、起点は通知の時刻に戻る。仕上げタスクが一覧から外れたあとに backend が再起動したときなど。stale が早まる向きで、修正前と同じ値です。`notBeforeWait` はこの読み取りを除いて検査します。

## 壊しうる入力

- `completedAt` が無い（古い takt）：上の規則の観測時刻の候補で受ける。形が不正：解析エラーで周期を失敗にする（`startedAt` と同じ）。オフセット付きの時刻：`parse_time` が UTC にそろえる。
- `completedAt` が観測時刻より後（時計のずれ）：その値を起点にする。経過が負になり stale にならない向き（早まらない）。
- 1ゴールに仕上げタスクが複数（再試行・再投入）：1件でも pending/running なら `None`、終わったものの `completed_at` の最も遅いもの。
- `goalId` が無い・別ゴールの仕上げタスク、仕上げでない pending/running のタスク：影響しない。
- 前回 snapshot が無い（起動の直後、接続先の切り替え）：候補（通知・`completed_at`）から決まる。どれも無ければ観測時刻（いまと同じ）。
- 通知の読み取りの失敗：いまと同じく前回の起点を通知の起点として引き継ぐ。
- reopen して読み取りのあいだにもう一度 awaiting_merge に入った：新しい通知か `completed_at` が前回の起点より遅いので、そちらを取る。
- 仕上げタスクを止めた（stop）・外した（remove）：pending/running が無くなった時点から人の待ちになる（`is_human_merge_wait` と同じ）。

## 上流・保存済みデータ・公開 API・守りの仕組みへの影響

- **上流（takt-mcp）**：既にある `completedAt` を読むだけ。変更や依頼は無い。
- **保存済みデータ**：snapshot はメモリ上だけ。web の保存状態は `awaitingMergeAt` を任意の値として復元するので、無い値でも読める。
- **公開 API（proto）**：番号・型・名前は変えない（行末の説明だけ）。`Goal.awaiting_merge_at` の意味が「人のマージ待ちの起点」に絞られ、仕上げタスクの実行中は無くなる。`PendingDecision.created_at` は、判断待ちに出るゴールでは仕上げタスクの終了時刻以後になる。
- **守りの仕組み**：CI の `quint` ジョブに仕様が1つ増える（`ci-specs.json` への追記だけ。ワークフローとスクリプトは変えない）。ルールセットは変えない。
