// -*- mode: Bluespec; -*-
// issue #364 の 1 点目の筋書きを、修正前・片方だけの版・修正案でたどる。
module shutdownDeadline_test {
  import shutdownDeadline(PARALLEL = false, DEADLINE = false) as before from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = false, DEADLINE = true) as deadlineOnly from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = false) as parallelOnly from "./shutdownDeadline"
  import shutdownDeadline(PARALLEL = true, DEADLINE = true) as after from "./shutdownDeadline"

  // 閉じる 1 件の上限は、全体の上限より短い（同時に閉じれば、応答しないサーバーも上限内に打ち切る）。
  run closeBoundFitsInTheDeadlineTest =
    assert(after::CLOSE_BOUND < after::SHUTDOWN_DEADLINE)

  // すべての HTTP サーバーが応答せず、子プロセスの鍵が空いている初期状態。
  action allUnresponsiveBefore = all {
    before::phase' = before::Running, before::clock' = 0,
    before::items' = before::ITEMS.mapBy(i => { status: before::Waiting, elapsed: 0, responsive: false, busy: false, wasBusy: false }),
  }

  // 修正前：応答しないサーバー 3 台を 1 台ずつ閉じると、15 秒 × 3 で、全体の上限を超える。
  run sequentialClosesExceedTheDeadlineBeforeFixTest =
    allUnresponsiveBefore
      .then(before::beginStop)
      .then(9.reps(_ => before::tick))
      .expect(before::clock == 9 and not(before::stopWithinDeadline))

  // 番号 0 と 1 は応答せず、2 は応答する。順に閉じて上限で打ち切ると、応答する 2 を閉じる前に見捨てる。
  action lastRespondsDeadlineOnly = all {
    deadlineOnly::phase' = deadlineOnly::Running, deadlineOnly::clock' = 0,
    deadlineOnly::items' = deadlineOnly::ITEMS.mapBy(i => { status: deadlineOnly::Waiting, elapsed: 0, responsive: i == 2, busy: false, wasBusy: false }),
  }

  run deadlineAloneAbandonsARespondingServerTest =
    lastRespondsDeadlineOnly
      .then(deadlineOnly::beginStop)
      .then(4.reps(_ => deadlineOnly::tick))
      .expect(deadlineOnly::phase == deadlineOnly::Exited
        and deadlineOnly::items.get(2).status == deadlineOnly::Abandoned
        and not(deadlineOnly::respondingSessionsClosed))

  // 子プロセス 3 の鍵を、道具の呼び出しが上限（30 秒）まで持つ。
  action busyChildParallelOnly = all {
    parallelOnly::phase' = parallelOnly::Running, parallelOnly::clock' = 0,
    parallelOnly::items' = parallelOnly::ITEMS.mapBy(i => { status: parallelOnly::Waiting, elapsed: 0, responsive: true, busy: i == 3, wasBusy: i == 3 }),
  }

  // 同時に閉じるだけでは、鍵を持つ道具の呼び出しを待って、全体の上限を超える。
  run parallelAloneWaitsForABusyChildTest =
    busyChildParallelOnly
      .then(parallelOnly::beginStop)
      .then(parallelOnly::complete(0)).then(parallelOnly::complete(1)).then(parallelOnly::complete(2))
      .then(parallelOnly::complete(4))
      .then(5.reps(_ => parallelOnly::tick))
      .expect(parallelOnly::clock == 5 and not(parallelOnly::stopWithinDeadline))

  action mixedAfter = all {
    after::phase' = after::Running, after::clock' = 0,
    after::items' = after::ITEMS.mapBy(i => { status: after::Waiting, elapsed: 0, responsive: i == 2, busy: i == 3, wasBusy: i == 3 }),
  }

  // 修正案：応答するサーバーと鍵の空いた子プロセスはすぐ閉じ、応答しないサーバーは 15 秒で打ち切り、
  // 鍵を持たれた子プロセスは 20 秒で見捨てる。
  run parallelWithDeadlineClosesWhatCanBeClosedTest =
    mixedAfter
      .then(after::beginStop)
      .then(after::complete(2)).then(after::complete(4))
      .then(4.reps(_ => after::tick))
      .expect(after::phase == after::Exited
        and after::clock == after::SHUTDOWN_DEADLINE
        and after::items.get(0).status == after::GaveUp
        and after::items.get(2).status == after::Closed
        and after::items.get(3).status == after::Abandoned
        and after::items.get(4).status == after::Closed
        and after::safety)

  // 修正案：鍵を持つ道具の呼び出しが上限より前に終われば、その子プロセスも kill して、上限を待たずに終える。
  run busyChildIsKilledWhenItsOperationEndsTest =
    mixedAfter
      .then(after::beginStop)
      .then(after::complete(2)).then(after::complete(4))
      .then(after::tick)
      .then(after::operationEnds(3)).then(after::complete(3))
      .then(2.reps(_ => after::tick))
      .then(after::finishStop)
      .expect(after::phase == after::Exited and after::clock == 3
        and after::items.get(3).status == after::Closed and after::safety)
}
