// -*- mode: Bluespec; -*-
// issue #364 の 1 点目の筋書きを、修正前・片方だけの版・修正案でたどる。
module shutdownDeadline_test {
  import shutdownDeadline(PARALLEL = false, DEADLINE = false, CANCEL_CYCLE = false) as before from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = false, DEADLINE = true, CANCEL_CYCLE = false) as deadlineOnly from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = false, CANCEL_CYCLE = false) as parallelOnly from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = false) as noCancel from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = true) as after from "./shutdownDeadline"

  // 上限の並び：1 件を閉じる上限 < 閉じる段の上限 < 停止の全体の上限 < launchd が SIGKILL するまで。
  run boundsAreOrderedTest =
    assert(after::CLOSE_BOUND < after::SHUTDOWN_DEADLINE
      and after::SHUTDOWN_DEADLINE < after::STOP_DEADLINE
      and after::STOP_DEADLINE < after::EXIT_TIMEOUT)

  // 修正前：周期の外で、応答しないサーバー 3 台を 1 台ずつ閉じると、10 秒 × 3 で launchd の SIGKILL を超える。
  run sequentialClosesExceedTheExitTimeoutBeforeFixTest =
    all {
      before::phase' = before::Running, before::sinceSignal' = 0, before::clock' = 0,
      before::cycleLength' = 0, before::cycleElapsed' = 0,
      before::items' = before::ITEMS.mapBy(i => { status: before::Waiting, elapsed: 0, responsive: false, busy: false, wasBusy: false }),
    }
      .then(before::signal)
      .then(20.reps(_ => before::tick))
      .expect(before::sinceSignal == 20 and before::phase == before::Stopping and not(before::stopBeforeExitTimeout))

  // 上限だけ：番号 0 と 1 は応答せず、2 は応答する。順に閉じて上限で打ち切ると、2 を閉じる前に見捨てる。
  run deadlineAloneAbandonsARespondingServerTest =
    all {
      deadlineOnly::phase' = deadlineOnly::Running, deadlineOnly::sinceSignal' = 0, deadlineOnly::clock' = 0,
      deadlineOnly::cycleLength' = 0, deadlineOnly::cycleElapsed' = 0,
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
      parallelOnly::cycleLength' = 0, parallelOnly::cycleElapsed' = 0,
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
      noCancel::cycleLength' = 25, noCancel::cycleElapsed' = 0,
      noCancel::items' = noCancel::ITEMS.mapBy(i => { status: noCancel::Waiting, elapsed: 0, responsive: true, busy: false, wasBusy: false }),
    }
      .then(noCancel::signal)
      .expect(noCancel::phase == noCancel::Draining)
      .then(18.reps(_ => noCancel::tick))
      .expect(noCancel::phase == noCancel::Exited
        and noCancel::stopBeforeExitTimeout
        and not(noCancel::respondingSessionsClosed))

  // 修正案：周期の途中でも、合図で周期を打ち切ってすぐ閉じ始める。応答するサーバーと鍵の空いた
  // 子プロセスはすぐ閉じ、応答しないサーバーは 10 秒で打ち切り、鍵を持たれた子プロセスは 15 秒で見捨てる。
  run parallelWithDeadlineClosesWhatCanBeClosedTest =
    all {
      after::phase' = after::Running, after::sinceSignal' = 0, after::clock' = 0,
      after::cycleLength' = 25, after::cycleElapsed' = 0,
      after::items' = after::ITEMS.mapBy(i => { status: after::Waiting, elapsed: 0, responsive: i == 2, busy: i == 3, wasBusy: i == 3 }),
    }
      .then(after::signal)
      .expect(after::phase == after::Stopping)
      .then(after::complete(2)).then(after::complete(4))
      .then(15.reps(_ => after::tick))
      .expect(after::phase == after::Exited
        and after::sinceSignal == after::SHUTDOWN_DEADLINE
        and after::items.get(0).status == after::GaveUp
        and after::items.get(2).status == after::Closed
        and after::items.get(3).status == after::Abandoned
        and after::items.get(4).status == after::Closed
        and after::safety)

  // 修正案：鍵を持つ道具の呼び出しが上限より前に終われば、その子プロセスも kill し、上限を待たずに終える。
  run busyChildIsKilledWhenItsOperationEndsTest =
    all {
      after::phase' = after::Running, after::sinceSignal' = 0, after::clock' = 0,
      after::cycleLength' = 0, after::cycleElapsed' = 0,
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
