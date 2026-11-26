# マージ待ちの起点の確定（#90）

`backend/src/mcp/tools/goal.rs` の `resolve_awaiting_merge_at` が、ゴールのマージ待ちの起点（`awaiting_merge_at`）を読み取りごとに決める遷移を Quint で写したモデルです。仕様とコードは機械的にはつながっていません。コードの回帰テストは `backend/tests/mcp_client_test.rs` と、`backend/src/mcp/tools/goal.rs` の性質のテストにあります。設計は `docs/design/issue-90.md` です。

- `mergeWaitStart.qnt`: モデル。`current` が修正前、`fixed` が修正後の規則です。違いは定数 `FIXED` だけで、不変条件は同じです。
- `mergeWaitStart_test.qnt`: 筋書きを固定したテスト。

## モデルの範囲

1ゴールと、そのゴールの仕上げタスク1件を扱います。仕上げタスクは、無い・未着手・実行中・完了（`completedAt` あり）・完了（`completedAt` なし）の5つに畳みます。完了には失敗・上限超過・PR 失敗を含めます（どれも pending/running でない終わりの状態で、`is_human_merge_wait` は人のマージ待ちに数えます）。takt はゴールが awaiting_merge に入った時点で awaiting_merge 通知を記録します（仕上げタスクの投入より前）。backend の再起動は、前回 snapshot の起点が消える遷移です。通知の読み取りの失敗（前回の起点を引き継ぐ）は、前回の起点と同じ値なのでモデルに入れていません。

## 条件

不変条件 `safety` は次の5つの論理積です。

- `notBeforeWait`: 人の待ちが実際に始まる前の時刻を起点にしない。backend が待ちの開始以後の時刻を何も持っていない読み取りは除きます（下の「残る取りこぼし」）。
- `noStartWhileFinishing`: awaiting_merge でも、仕上げタスクが pending/running のあいだは起点を持たない。
- `noReset`: 既存の滞留防止。人の待ちが続き、そのあいだに takt の側の変化も backend の再起動も無ければ、観測時刻が進んでも起点は変わらない。
- `neverMovesBack`: 同じ待ちのあいだ、再起動が無ければ起点は前へ戻らない（仕上げタスクが一覧から消えても戻らない）。
- `notInFuture`: 起点は観測時刻より後にならない。

## 結果（quint 0.32.0）

修正前は反例が出ます。

```
quint run mergeWaitStart.qnt --main current --invariant safety --max-samples 20000 --max-steps 30 --seed 90
[violation] Found an issue
```

条件を分けると、`notBeforeWait` と `noStartWhileFinishing` が反例になり、`noReset`・`neverMovesBack`・`notInFuture` は反例なしです。awaiting_merge に入った時点の通知の時刻を起点にするため、仕上げタスクの実行中も起点を持ち、仕上げタスクが終わって人が待ち始めた時点で起点がすでに古くなります。

修正後は、同じ不変条件で、50000 サンプル、max-steps 30、seed 90 で反例なしです。

```
quint run mergeWaitStart.qnt --main fixed --invariant safety --max-samples 50000 --max-steps 30 --seed 90 --witnesses waitedAfterFinishTask evidenceLost
[ok] No violation found
waitedAfterFinishTask was witnessed in 9113 trace(s) out of 50000 explored (18.23%)
evidenceLost was witnessed in 4596 trace(s) out of 50000 explored (9.19%)
```

テストは次のとおりです。

```
quint test mergeWaitStart_test.qnt --main currentTest   # 1 passing（起点が仕上げタスクより前に確定する筋書き）
quint test mergeWaitStart_test.qnt --main fixedTest     # 5 passing
```

## 残る取りこぼし

`evidenceLost` は、修正後も届く状態です。待ちの開始以後の時刻（仕上げタスクの `completedAt`、それを起点に確定した前回の起点）が backend に何も残っていない読み取りで、起点は通知の時刻（仕上げタスクより前）に戻ります。仕上げタスクが一覧から外れた（`takt_cleanup_tasks` など）あとに backend が再起動したときなどに起きます。`completedAt` の無い完了が、読み取りのあいだに前の待ちの起点を持ったまま起きたときも同じです（いまの takt は終わりの状態で `completedAt` を必ず入れます）。どちらも、stale の表示が早まる向きの誤りで、修正前と同じ値です。
