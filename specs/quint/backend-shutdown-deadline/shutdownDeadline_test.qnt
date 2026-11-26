// -*- mode: Bluespec; -*-
// issue #364 の 1 点目の筋書きを、修正前・片方だけの版・修正案でたどる。
module shutdownDeadline_test {
  import shutdownDeadline(PARALLEL = false, DEADLINE = false, CANCEL_CYCLE = false, CYCLE_GRACE = 2) as before from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = false, DEADLINE = true, CANCEL_CYCLE = false, CYCLE_GRACE = 2) as deadlineOnly from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = false, CANCEL_CYCLE = false, CYCLE_GRACE = 2) as parallelOnly from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = false, CYCLE_GRACE = 2) as noCancel from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = true, CYCLE_GRACE = 2) as after from "./shutdownDeadline"

  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = true, CYCLE_GRACE = 0) as immediate from "./shutdownDeadline"

  // 上限の並び：1 件を閉じる上限 < 閉じる段の上限、周期を待つ上限 ＋ 閉じる段の上限 < 停止の全体の上限
  // < launchd が SIGKILL するまで。ふだんの周期は、周期を待つ上限に収まる。
  run boundsAreOrderedTest =
    assert(after::CLOSE_BOUND < after::SHUTDOWN_DEADLINE
      and after::CYCLE_GRACE + after::SHUTDOWN_DEADLINE < after::STOP_DEADLINE
      and after::STOP_DEADLINE < after::EXIT_TIMEOUT
      and after::SHORT_CYCLE <= after::CYCLE_GRACE)

  // 修正案：ふだんの短い周期の途中なら、周期を最後まで走らせ（出来事を記録して）から閉じる。
  run shortCycleCompletesBeforeClosingTest =
    all {
      after::phase' = after::Running, after::sinceSignal' = 0, after::clock' = 0,
      after::cycleLength' = 1, after::cycleElapsed' = 0, after::cycleCancelled' = false,
      after::items' = after::ITEMS.mapBy(i => { status: after::Waiting, elapsed: 0, responsive: true, busy: false, wasBusy: false }),
    }
      .then(after::signal)
      .then(after::tick)
      .then(after::cycleEnds)
      .then(after::complete(0)).then(after::complete(1)).then(after::complete(2))
      .then(after::complete(3)).then(after::complete(4))
      .then(after::finishStop)
      .expect(after::phase == after::Exited and not(after::cycleCancelled) and after::sinceSignal == 1
        and after::stoppedWithEverythingClosed and after::safety)

  // 合図ですぐ周期を打ち切ると、ふだんの短い周期も打ち切り、その周期の出来事を記録せずに止まる。
  run immediateCancelDropsAShortCycleTest =
    all {
      immediate::phase' = immediate::Running, immediate::sinceSignal' = 0, immediate::clock' = 0,
      immediate::cycleLength' = 1, immediate::cycleElapsed' = 0, immediate::cycleCancelled' = false,
      immediate::items' = immediate::ITEMS.mapBy(i => { status: immediate::Waiting, elapsed: 0, responsive: true, busy: false, wasBusy: false }),
    }
      .then(immediate::signal)
      .then(immediate::cancelCycle)
      .expect(immediate::cycleCancelled and not(immediate::shortCyclesComplete))

  // 修正前：周期の外で、応答しないサーバー 3 台を 1 台ずつ閉じると、10 秒 × 3 で launchd の SIGKILL を超える。
  run sequentialClosesExceedTheExitTimeoutBeforeFixTest =
    all {
      before::phase' = before::Running, before::sinceSignal' = 0, before::clock' = 0,
      before::cycleLength' = 0, before::cycleElapsed' = 0, before::cycleCancelled' = false,
      before::items' = before::ITEMS.mapBy(i => { status: before::Waiting, elapsed: 0, responsive: false, busy: false, wasBusy: false }),
    }
      .then(before::signal)
      .then(20.reps(_ => before::tick))
      .expect(before::sinceSignal == 20 and before::phase == before::Stopping and not(before::stopBeforeExitTimeout))

  // 上限だけ：番号 0 と 1 は応答せず、2 は応答する。順に閉じて上限で打ち切ると、2 を閉じる前に見捨てる。
  run deadlineAloneAbandonsARespondingServerTest =
    all {
      deadlineOnly::phase' = deadlineOnly::Running, deadlineOnly::sinceSignal' = 0, deadlineOnly::clock' = 0,
      deadlineOnly::cycleLength' = 0, deadlineOnly::cycleElapsed' = 0, deadlineOnly::cycleCancelled' = false,
      deadlineOnly::items' = deadlineOnly::ITEMS.mapBy(i => { status: deadlineOnly::Waiting, elapsed: 0, responsive: i == 2, busy: false, wasBusy: false }),
    }
      .then(deadlineOnly::signal)
      .then(15.reps(_ => deadlineOnly::tick))
      .expect(deadlineOnly::phase == deadlineOnly::Exited
        and deadlineOnly::items.get(2).status == deadlineOnly::Abandoned
        and not(deadlineOnly::respondingSessionsClosed))

  // 同時に閉じるだけ：子プロセス 3 の鍵を道具の呼び出しが上限（30 秒）まで持つと、それを待って SIGKILL を超える。
  run parallelAloneWaitsForABusyChildTest =
    all {
      parallelOnly::phase' = parallelOnly::Running, parallelOnly::sinceSignal' = 0, parallelOnly::clock' = 0,
      parallelOnly::cycleLength' = 0, parallelOnly::cycleElapsed' = 0, parallelOnly::cycleCancelled' = false,
      parallelOnly::items' = parallelOnly::ITEMS.mapBy(i => { status: parallelOnly::Waiting, elapsed: 0, responsive: true, busy: i == 3, wasBusy: i == 3 }),
    }
      .then(parallelOnly::signal)
      .then(parallelOnly::complete(0)).then(parallelOnly::complete(1)).then(parallelOnly::complete(2))
      .then(parallelOnly::complete(4))
      .then(20.reps(_ => parallelOnly::tick))
      .expect(parallelOnly::phase == parallelOnly::Stopping and not(parallelOnly::stopBeforeExitTimeout))

  // 周期を打ち切らない：周期が 18 秒続くと、閉じる前に停止の全体の上限に達し、応答するサーバーも見捨てる。
  run waitingForACycleAbandonsEverythingTest =
    all {
      noCancel::phase' = noCancel::Running, noCancel::sinceSignal' = 0, noCancel::clock' = 0,
      noCancel::cycleLength' = 25, noCancel::cycleElapsed' = 0, noCancel::cycleCancelled' = false,
      noCancel::items' = noCancel::ITEMS.mapBy(i => { status: noCancel::Waiting, elapsed: 0, responsive: true, busy: false, wasBusy: false }),
    }
      .then(noCancel::signal)
      .expect(noCancel::phase == noCancel::Draining)
      .then(18.reps(_ => noCancel::tick))
      .expect(noCancel::phase == noCancel::Exited
        and noCancel::stopBeforeExitTimeout
        and not(noCancel::respondingSessionsClosed))

  // 修正案：長い周期の途中なら、2 秒待って周期を打ち切り、閉じ始める。応答するサーバーと鍵の空いた
  // 子プロセスはすぐ閉じ、応答しないサーバーは 10 秒で打ち切り、鍵を持たれた子プロセスは 15 秒で見捨てる。
  run parallelWithDeadlineClosesWhatCanBeClosedTest =
    all {
      after::phase' = after::Running, after::sinceSignal' = 0, after::clock' = 0,
      after::cycleLength' = 25, after::cycleElapsed' = 0, after::cycleCancelled' = false,
      after::items' = after::ITEMS.mapBy(i => { status: after::Waiting, elapsed: 0, responsive: i == 2, busy: i == 3, wasBusy: i == 3 }),
    }
      .then(after::signal)
      .then(2.reps(_ => after::tick))
      .then(after::cancelCycle)
      .expect(after::phase == after::Stopping and after::cycleCancelled)
      .then(after::complete(2)).then(after::complete(4))
      .then(15.reps(_ => after::tick))
      .expect(after::phase == after::Exited
        and after::sinceSignal == after::CYCLE_GRACE + after::SHUTDOWN_DEADLINE
        and after::items.get(0).status == after::GaveUp
        and after::items.get(2).status == after::Closed
        and after::items.get(3).status == after::Abandoned
        and after::items.get(4).status == after::Closed
        and after::safety)

  // 修正案：鍵を持つ道具の呼び出しが上限より前に終われば、その子プロセスも kill し、上限を待たずに終える。
  run busyChildIsKilledWhenItsOperationEndsTest =
    all {
      after::phase' = after::Running, after::sinceSignal' = 0, after::clock' = 0,
      after::cycleLength' = 0, after::cycleElapsed' = 0, after::cycleCancelled' = false,
      after::items' = after::ITEMS.mapBy(i => { status: after::Waiting, elapsed: 0, responsive: i == 2, busy: i == 3, wasBusy: i == 3 }),
    }
      .then(after::signal)
      .then(after::complete(2)).then(after::complete(4))
      .then(after::tick)
      .then(after::operationEnds(3)).then(after::complete(3))
      .then(9.reps(_ => after::tick))
      .then(after::finishStop)
      .expect(after::phase == after::Exited and after::sinceSignal == after::CLOSE_BOUND
        and after::items.get(3).status == after::Closed and after::safety)
}
