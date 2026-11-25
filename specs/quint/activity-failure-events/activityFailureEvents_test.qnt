// -*- mode: Bluespec; -*-
module currentTest {
  import activityFailureEvents(RECORD_FAILURES_AND_STARTS = false).* from "./activityFailureEvents"

  // 修正前は、失敗を読んだ周期で基準だけが進み、失敗も起動も0件のまま残る。
  run failureAndStartAreNotRecordedTest =
    init
      .then(cycle).then(failTask).then(cycle)
      .then(all {
        assert(realFailures == 1 and recordedFailures == 0 and base == BaseFailed),
        assert(recordedStarts == 0 and not(baseAdvancesOnlyWhenRecorded) and not(startAccounted)),
        cycle,
      })
}

module fixedTest {
  import activityFailureEvents(RECORD_FAILURES_AND_STARTS = true).* from "./activityFailureEvents"

  run failureIsRecordedOnceTest =
    init
      .then(cycle).then(failTask).then(cycle).then(cycle)
      .then(all { assert(recordedFailures == 1 and recordedStarts == 1 and safety), cycle })

  // 起動の直後の最初の周期は基準化だけで、すでに失敗していたタスクを記録しない。
  run restartDoesNotRecountTest =
    init
      .then(cycle).then(failTask).then(cycle)
      .then(restart).then(cycle).then(cycle)
      .then(all { assert(restartedWithoutRecount and safety), cycle })

  // 記録に失敗した周期は基準を進めず、記録先が復旧した周期で1回だけ記録する。
  run heldBackUntilTheStoreRecoversTest =
    init
      .then(cycle).then(failTask).then(setDb(false)).then(cycle).then(cycle)
      .then(all { assert(recordedFailures == 0 and base == BaseNotFailed and safety), setDb(true) })
      .then(cycle).then(cycle)
      .then(all { assert(recordedAfterRecovery and safety), cycle })

  // 起動の記録も、記録先が復旧するまで持ち越して1回だけ記録する。
  run startIsRetriedUntilTheStoreRecoversTest =
    init
      .then(setDb(false)).then(cycle).then(cycle)
      .then(all { assert(recordedStarts == 0 and startPending and safety), setDb(true) })
      .then(cycle).then(cycle)
      .then(all { assert(recordedStarts == 1 and not(startPending) and safety), cycle })
}
