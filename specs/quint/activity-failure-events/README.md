# タスクの失敗と backend の起動の記録（#151）

`backend/src/poller.rs` と `backend/src/events.rs` の、タスクの失敗と backend の起動を出来事として記録する遷移を Quint で写したモデルです。仕様とコードは機械的にはつながっていません。コードの回帰テストは `backend/tests/events_test.rs`、`backend/tests/poller_test.rs`、`backend/tests/backend_startup_test.rs` にあります。

- `activityFailureEvents.qnt`: モデル。`current` が修正前、`fixed` が修正後の遷移です。違いは定数 `RECORD_FAILURES_AND_STARTS` だけで、不変条件は同じです。
- `activityFailureEvents_test.qnt`: 筋書きを固定したテスト。

## モデルの範囲

1プロジェクトの1タスクだけを扱います。タスクの状態は「失敗でない」と「失敗」の2値に畳みます。失敗の種類（失敗、上限超過、PR 失敗）と run の失敗・中断は、同じ形の遷移として1つにまとめます。読み取りの失敗と接続の出来事は扱いません（`specs/quint/poller-event-recording` が扱います）。プロセスの再起動は、差分の基準（`Poller::previous`）が消え、記録先（SQLite）が残る遷移として扱います。

## 条件

不変条件 `safety` は次の3つの論理積です。

- `noOverRecord`: 実際に起きた失敗より多く記録しない。再起動のたびに過去の失敗を数え直すと、この条件に反します。
- `baseAdvancesOnlyWhenRecorded`: 差分の基準を失敗へ進めるのは、その失敗を記録できた周期だけ（#26 の決まり）。起動の直後の基準化は、進めた回数に数えません。
- `startAccounted`: いまの起動は、記録済みか、記録を持ち越している。起動より多くは記録しない。

進行は `--witnesses recordedAfterRecovery restartedWithoutRecount` と、筋書きを固定したテストで確かめます。`quint run` は時相の性質を検査しないため、「記録先が復旧すれば必ず1回記録される」ことの網羅的な確認ではありません。

## 結果（quint 0.32.0）

修正前は反例が出ます。

```
quint run activityFailureEvents.qnt --main current --invariant safety --max-samples 10000 --max-steps 30 --seed 151
[violation] Found an issue
```

`safety` の反例は初期状態です。起動を記録しておらず、持ち越してもいません（`startAccounted` に反する）。失敗の側は、条件を分けると出ます。

```
quint run activityFailureEvents.qnt --main current --invariant baseAdvancesOnlyWhenRecorded --max-samples 10000 --max-steps 30 --seed 151
[violation] Found an issue
```

反例の末尾は、本番の記録（11/16 に失敗が21件、失敗の出来事が0件）と同じ形です。基準が失敗でない状態でタスクが失敗し、その周期で基準だけが失敗へ進み、記録は0件のままです。`noOverRecord` は修正前でも反例なしです（何も記録しないため）。

修正後は、同じ不変条件と同じ引数で、10000 サンプル、max-steps 30、seed 151 で反例なしです。

```
quint run activityFailureEvents.qnt --main fixed --invariant safety --max-samples 10000 --max-steps 30 --seed 151 --witnesses recordedAfterRecovery restartedWithoutRecount detectedThenRetried
[ok] No violation found
recordedAfterRecovery was witnessed in 680 trace(s) out of 10000 explored (6.80%)
restartedWithoutRecount was witnessed in 98 trace(s) out of 10000 explored (0.98%)
detectedThenRetried was witnessed in 863 trace(s) out of 10000 explored (8.63%)
```

テストは次のとおりです。

```
quint test activityFailureEvents_test.qnt --main currentTest   # 1 passing（修正前に失敗も起動も0件のまま残る筋書き）
quint test activityFailureEvents_test.qnt --main fixedTest     # 4 passing
```

## 残る取りこぼし

`detectedThenRetried` は、修正後も届く状態です。失敗を検出した周期で記録に失敗し、記録先が復旧する前にそのタスクが再実行されると、基準も今回の状態も失敗でなくなり、差分からは失敗を再検出できません。この失敗は記録されません。記録先の失敗と再実行が重なったときだけ起きます。接続断のように検出した出来事を保持する仕組み（`Poller::unrecorded_lost`）は、失敗には入れていません。

backend が止まっているあいだに起きた失敗も記録しません。起動の直後は基準が無く、止まる前の状態と比べられないためです（#181 が扱います）。
