// -*- mode: Bluespec; -*-
// 記録の失敗の後、記録先と takt-mcp が同じ周期までに復旧する筋書き。
module currentTest {
  import pollerEventRecording(HOLD_UNRECORDED_LOST = false).* from "./pollerEventRecording"

  // 修正前は、復旧した周期の後に接続断が0件のまま、記録する手がかりも無い。
  run lostEventIsDroppedTest =
    init
      .then(setMcp(false)).then(setDb(false)).then(failedCycle)
      .then(setMcp(true)).then(setDb(true)).then(succeededCycle)
      .then(all { assert(detected == 1 and recordedLost == 0 and not(noLoss)), rebuild })
}

module fixedTest {
  import pollerEventRecording(HOLD_UNRECORDED_LOST = true).* from "./pollerEventRecording"

  run recordedOnceAfterRecoveryTest =
    init
      .then(setMcp(false)).then(setDb(false)).then(failedCycle)
      .then(setMcp(true)).then(setDb(true)).then(succeededCycle)
      .then(all { assert(recoveredAndRecorded and safety), succeededCycle })
      .then(all { assert(recoveredAndRecorded and safety), rebuild })

  // takt-mcp だけが先に復旧し、記録先の失敗が続く間は保持したまま再試行する。
  run retriedUntilTheStoreRecoversTest =
    init
      .then(setMcp(false)).then(setDb(false)).then(failedCycle)
      .then(setMcp(true)).then(succeededCycle).then(succeededCycle)
      .then(all { assert(held and recordedLost == 0 and safety), setDb(true) })
      .then(succeededCycle)
      .then(all { assert(recoveredAndRecorded and not(held)), rebuild })

  // 読取先を作り直すと、保持していた接続断を次の世代で記録しない。
  run droppedOnRebuildTest =
    init
      .then(setMcp(false)).then(setDb(false)).then(failedCycle)
      .then(rebuild).then(setMcp(true)).then(setDb(true)).then(succeededCycle)
      .then(all { assert(recordedLost == 999 and recordedRestored == 0 and safety), rebuild })
}
