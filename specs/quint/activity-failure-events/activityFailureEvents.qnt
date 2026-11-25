// -*- mode: Bluespec; -*-
// backend/src/poller.rs と backend/src/events.rs の、タスクの失敗と backend の起動を
// 出来事として記録する遷移のモデル（#151）。1プロジェクトの1タスクだけを扱う。
// タスクの状態は「失敗でない」と「失敗」の2値に畳み、run の失敗も同じ形として扱う。
// 読み取りの失敗と接続の出来事は扱わない（specs/quint/poller-event-recording が扱う）。
module activityFailureEvents {
  // true のとき、失敗への遷移と起動を出来事として記録する（修正後）。
  // false のとき、どちらも出来事にしない（修正前）。
  const RECORD_FAILURES_AND_STARTS: bool

  type Task = NotFailed | Failed
  type Base = NoBase | BaseNotFailed | BaseFailed

  // takt-mcp が返す実際のタスクの状態。
  var task: Task
  var dbUp: bool
  // 差分の基準（Poller::previous）。起動の直後は無い。
  var base: Base
  // まだ記録していない起動の出来事を持っているか（Poller::pending_start）。
  var startPending: bool
  // いまのプロセスの起動を記録したか。
  var startRecorded: bool
  // 実際に起きた失敗への遷移の回数と、記録した失敗の回数。
  var realFailures: int
  var recordedFailures: int
  // 基準が「失敗でない」から「失敗」へ進んだ回数。
  var absorbedFailures: int
  // 検出したが記録に失敗し、まだ記録していない失敗があるか（観測用）。
  var detectedUnrecorded: bool
  var starts: int
  var recordedStarts: int

  action init = all {
    task' = NotFailed,
    dbUp' = true,
    base' = NoBase,
    startPending' = RECORD_FAILURES_AND_STARTS,
    startRecorded' = false,
    realFailures' = 0,
    recordedFailures' = 0,
    absorbedFailures' = 0,
    detectedUnrecorded' = false,
    starts' = 1,
    recordedStarts' = 0,
  }

  action failTask: bool = all {
    task == NotFailed,
    task' = Failed,
    realFailures' = realFailures + 1,
    dbUp' = dbUp, base' = base, startPending' = startPending, startRecorded' = startRecorded,
    recordedFailures' = recordedFailures, absorbedFailures' = absorbedFailures,
    detectedUnrecorded' = detectedUnrecorded, starts' = starts, recordedStarts' = recordedStarts,
  }

  // 失敗したタスクの再実行（failed から pending や running へ戻る）。
  action retryTask: bool = all {
    task == Failed,
    task' = NotFailed,
    dbUp' = dbUp, base' = base, startPending' = startPending, startRecorded' = startRecorded,
    realFailures' = realFailures, recordedFailures' = recordedFailures,
    absorbedFailures' = absorbedFailures, detectedUnrecorded' = detectedUnrecorded,
    starts' = starts, recordedStarts' = recordedStarts,
  }

  action setDb(up: bool): bool = all {
    dbUp' = up,
    task' = task, base' = base, startPending' = startPending, startRecorded' = startRecorded,
    realFailures' = realFailures, recordedFailures' = recordedFailures,
    absorbedFailures' = absorbedFailures, detectedUnrecorded' = detectedUnrecorded,
    starts' = starts, recordedStarts' = recordedStarts,
  }

  // プロセスの再起動。メモリ上の基準は消え、出来事の記録先（SQLite）は残る。
  action restart: bool = all {
    base' = NoBase,
    startPending' = RECORD_FAILURES_AND_STARTS,
    startRecorded' = false,
    detectedUnrecorded' = false,
    starts' = starts + 1,
    task' = task, dbUp' = dbUp,
    realFailures' = realFailures, recordedFailures' = recordedFailures,
    absorbedFailures' = absorbedFailures, recordedStarts' = recordedStarts,
  }

  // 読み取りに成功した周期（Poller::read_cycle_once_at）。
  // 起動の出来事は周期の先頭で記録し、失敗するあいだ持ち越す。
  // 失敗の出来事は、基準がある周期の差分からだけ作り、記録に成功した周期だけ基準を進める。
  action cycle: bool = {
    val startOk = startPending and dbUp
    val detected = RECORD_FAILURES_AND_STARTS and base == BaseNotFailed and task == Failed
    val recordOk = not(detected) or dbUp
    val seen = if (task == Failed) BaseFailed else BaseNotFailed
    all {
      startPending' = (startPending and not(dbUp)),
      startRecorded' = (startRecorded or startOk),
      recordedStarts' = if (startOk) recordedStarts + 1 else recordedStarts,
      base' = if (recordOk) seen else base,
      recordedFailures' = if (detected and recordOk) recordedFailures + 1 else recordedFailures,
      absorbedFailures' =
        if (recordOk and base == BaseNotFailed and task == Failed) absorbedFailures + 1 else absorbedFailures,
      detectedUnrecorded' =
        if (detected) not(recordOk) else if (recordOk and base == BaseNotFailed) false else detectedUnrecorded,
      task' = task, dbUp' = dbUp, realFailures' = realFailures, starts' = starts,
    }
  }

  action step = any {
    failTask,
    retryTask,
    nondet up = Set(true, false).oneOf()
    setDb(up),
    restart,
    cycle,
  }

  // 実際に起きた失敗より多く記録しない（再起動のたびに過去の失敗が増えない）。
  val noOverRecord = recordedFailures <= realFailures

  // 基準を失敗へ進めるのは、その失敗を記録できた周期だけ（#26 の決まり）。
  // 起動の直後の基準化（NoBase から）は、進めた回数に数えない。
  val baseAdvancesOnlyWhenRecorded = absorbedFailures == recordedFailures

  // いまの起動は、記録済みか、記録を持ち越している。起動より多くは記録しない。
  val startAccounted = (startRecorded or startPending) and recordedStarts <= starts

  val safety = noOverRecord and baseAdvancesOnlyWhenRecorded and startAccounted

  // 進行の確認用。記録の失敗を経た失敗が、復旧後に1回だけ記録済みになる状態へ届くか。
  val recordedAfterRecovery =
    realFailures == 1 and recordedFailures == 1 and base == BaseFailed and not(detectedUnrecorded)

  // 再起動をまたいでも、失敗の記録が1回のまま、起動の記録が2回になる状態へ届くか。
  val restartedWithoutRecount =
    starts == 2 and recordedStarts == 2 and realFailures == 1 and recordedFailures == 1 and base == BaseFailed

  // このモデルの範囲外の取りこぼしへ届くか。記録に失敗しているあいだにタスクが再実行されると、
  // 差分からは失敗を再検出できない。
  val detectedThenRetried = detectedUnrecorded and task == NotFailed
}

module current {
  import activityFailureEvents(RECORD_FAILURES_AND_STARTS = false).*
}

module fixed {
  import activityFailureEvents(RECORD_FAILURES_AND_STARTS = true).*
}
