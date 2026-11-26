// -*- mode: Bluespec; -*-
module currentTest {
  import mergeWaitStart(FIXED = false, MAX_TIME = 6).* from "./mergeWaitStart"

  // 修正前は、awaiting_merge に入った時点の通知を起点にし、仕上げタスクが長く走ると、
  // 人が待ち始めた時点で起点がすでに古い（#90）。仕上げタスクの実行中も起点を持つ。
  run startPrecedesFinishTaskTest =
    init
      .then(enterAwaitingMerge).then(enqueueMergeTask).then(startMergeTask)
      .then(tick).then(tick).then(read)
      .then(all { assert(lastRead.start == At(0) and not(noStartWhileFinishing)), all { task == Running, upstream(phase, Done(clock), notif) } })
      .then(tick).then(read)
      .then(all { assert(lastRead.began == At(2) and lastRead.start == At(0) and not(notBeforeWait)), tick })
}

module fixedTest {
  import mergeWaitStart(FIXED = true, MAX_TIME = 6).* from "./mergeWaitStart"

  // 仕上げタスクの実行中は起点を持たず、終わったら completedAt を起点にする。
  run startIsCompletedAtTest =
    init
      .then(enterAwaitingMerge).then(enqueueMergeTask).then(startMergeTask)
      .then(tick).then(tick).then(read)
      .then(all { assert(lastRead.start == NoStart and safety), all { task == Running, upstream(phase, Done(clock), notif) } })
      .then(tick).then(read)
      .then(all { assert(lastRead.start == At(2) and waitedAfterFinishTask and safety), tick })

  // 仕上げタスクを経ずに awaiting_merge に入っただけの待ちは、waitedAfterFinishTask の witness にならない。
  run enteringAwaitingMergeIsNotAFinishWitnessTest =
    init
      .then(tick).then(enterAwaitingMerge).then(tick).then(read)
      .then(all { assert(lastRead.start == At(1) and not(waitedAfterFinishTask) and safety), tick })

  // 既存の滞留防止：観測時刻が進んでも、読み取りのたびに起点は現在へ戻らない。
  run startDoesNotResetTest =
    init
      .then(enterAwaitingMerge).then(tick).then(read)
      .then(tick).then(read).then(tick).then(read)
      .then(all { assert(lastRead.start == At(0) and beforeLast.start == At(0) and safety), tick })

  // completedAt の無い完了で前回の起点も無いときは、最初の観測時刻を起点にして保つ。
  run observedStartIsKeptTest =
    init
      .then(enterAwaitingMerge).then(enqueueMergeTask).then(startMergeTask).then(read)
      .then(tick).then(tick).then(all { task == Running, upstream(phase, DoneNoTime, notif) })
      .then(read).then(tick).then(read)
      .then(all { assert(lastRead.start == At(2) and beforeLast.start == At(2) and safety), tick })

  // 起点を観測したあとに仕上げタスクが一覧から消えても、起点は通知の時刻へ戻らない。
  run cleanupKeepsStartTest =
    init
      .then(enterAwaitingMerge).then(enqueueMergeTask).then(startMergeTask)
      .then(tick).then(all { task == Running, upstream(phase, Done(1), notif) })
      .then(tick).then(read).then(cleanupTasks).then(tick).then(read)
      .then(all { assert(lastRead.start == At(1) and beforeLast.start == At(1) and safety), tick })

  // 残る取りこぼし：一覧から消えたあとに再起動すると、待ちの開始以後の時刻が何も残らず、
  // 起点は通知の時刻になる。
  run cleanupThenRestartFallsBackToNotificationTest =
    init
      .then(enterAwaitingMerge).then(enqueueMergeTask).then(startMergeTask)
      .then(tick).then(all { task == Running, upstream(phase, Done(1), notif) })
      .then(tick).then(read).then(cleanupTasks).then(restart).then(tick).then(read)
      .then(all { assert(lastRead.start == At(0) and evidenceLost and safety), tick })
}
