// -*- mode: Bluespec; -*-
// PR #379（issue #279）の筋書きを、修正前・途中版・現在のコード（修正後）でたどる。
module childProcess_test {
  import childProcess(
    TAKE_DURING_REQUEST = false,
    WARMUP_RETRY = false,
    DETACH_UNKEYED_WRITE = false
  ) as before from "./childProcess"

  import childProcess(
    TAKE_DURING_REQUEST = true,
    WARMUP_RETRY = true,
    DETACH_UNKEYED_WRITE = false
  ) as noDetach from "./childProcess"

  import childProcess(
    TAKE_DURING_REQUEST = true,
    WARMUP_RETRY = true,
    DETACH_UNKEYED_WRITE = true
  ) as after from "./childProcess"

  // 1. 修正前: 往復中の読み取りを打ち切ると子プロセスが破棄されず残り、次の読み取りが読みかけの応答を持つ子プロセスを使い回す。
  run cancelledReadLeavesAndReusesDirtyChildBeforeFixTest =
    before::init
      .then(before::arrive(0, before::WarmUp))
      .then(before::acquireLock(0))
      .then(before::beginSpawn(0))
      .then(before::completeSpawn(0))
      .then(before::arrive(1, before::ReadCall))
      .then(before::acquireLock(1))
      .then(before::startAlreadyRunning(1))
      .then(before::beginRequest(1))
      .then(before::cancelCaller(1))
      .expect(
        before::children.get(0).status == before::Alive
        and before::children.get(0).dirty
        and not(before::noChildRemainsAfterCancel)
        and not(before::idleSlotIsClean)
      )
      .then(before::arrive(2, before::ReadCall))
      .then(before::acquireLock(2))
      .then(before::startAlreadyRunning(2))
      .then(before::beginRequest(2))
      .expect(
        before::children.get(0).usedWhileDirty
        and not(before::noDirtyChildReused)
      )

  // 2. 修正後: 往復中の読み取りを打ち切ると子プロセスが破棄され、次の読み取りは新しい子プロセスを起動して成功する。
  run cancelledReadDisposesChildAndRebuildsAfterFixTest =
    after::init
      .then(after::arrive(0, after::WarmUp))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::completeSpawn(0))
      .then(after::arrive(1, after::ReadCall))
      .then(after::acquireLock(1))
      .then(after::startAlreadyRunning(1))
      .then(after::beginRequest(1))
      .then(after::cancelCaller(1))
      .expect(
        after::children.get(0).status == after::Closed
        and after::children.get(0).closeCount == 1
        and after::client.slot == after::NONE
        and after::safety
      )
      .then(after::arrive(2, after::ReadCall))
      .then(after::acquireLock(2))
      .then(after::startAlreadyRunning(2).fail())
      .expect(true)

  // 2b. 修正後: 打ち切り後の次の読み取りは新しい子プロセス（id = 1）を起動して応答を読む。
  run cancelledReadNextCallSpawnsFreshChildTest =
    after::init
      .then(after::arrive(0, after::WarmUp))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::completeSpawn(0))
      .then(after::arrive(1, after::ReadCall))
      .then(after::acquireLock(1))
      .then(after::startAlreadyRunning(1))
      .then(after::beginRequest(1))
      .then(after::cancelCaller(1))
      .then(after::arrive(2, after::ReadCall))
      .then(after::acquireLock(2))
      .then(after::beginSpawn(2))
      .then(after::completeSpawn(2))
      .then(after::beginRequest(2))
      .then(after::completeExchange(2))
      .expect(
        after::ops.get(2).phase == after::Done
        and after::client.slot == 1
        and after::children.get(0).status == after::Closed
        and after::children.get(1).status == after::Alive
        and not(after::children.get(1).dirty)
        and after::safety
        and after::cancelledReadDisposedAndRebuilt
      )

  // 3. 修正前: 温めの起動失敗が next_spawn_at を設定し、直後の呼び出しを Backoff で拒否する。
  run warmUpFailureBlocksNextCallBeforeFixTest =
    before::init
      .then(before::arrive(0, before::WarmUp))
      .then(before::acquireLock(0))
      .then(before::failSpawnImmediate(0))
      .expect(
        before::client.nextSpawnAt != before::NONE
        and before::client.consecutiveFailures == 1
        and not(before::warmUpFailureDoesNotStartBackoff)
      )
      .then(before::arrive(1, before::UnkeyedWrite))
      .then(before::acquireLock(1))
      .then(before::startBlockedByBackoff(1))
      .expect(before::ops.get(1).phase == before::Failed and before::audit.sawBackoffReject)

  // 4. 修正後: 温めの起動失敗は next_spawn_at を始めず、直後の呼び出しは起動を試みる。
  run warmUpFailureDoesNotBlockNextCallAfterFixTest =
    after::init
      .then(after::arrive(0, after::WarmUp))
      .then(after::acquireLock(0))
      .then(after::failSpawnImmediate(0))
      .expect(
        after::client.nextSpawnAt == after::NONE
        and after::client.consecutiveFailures == 0
        and after::safety
        and after::warmUpFailedWithoutBackoff
      )
      .then(after::arrive(1, after::UnkeyedWrite))
      .then(after::acquireLock(1))
      .then(after::beginSpawn(1))
      .then(after::completeSpawn(1))
      .then(after::beginRequest(1))
      .then(after::completeExchange(1))
      .expect(
        after::ops.get(1).phase == after::Done
        and after::ops.get(1).writeRecorded
        and after::safety
      )

  // 5. 修正後: 呼び出し経路の起動失敗は next_spawn_at を始め、抑制中は Backoff で断り、満了後に回復する。
  run callSpawnFailureStartsBackoffAndRecoversTest =
    after::init
      .then(after::arrive(0, after::ReadCall))
      .then(after::acquireLock(0))
      .then(after::failSpawnImmediate(0))
      .expect(
        after::client.nextSpawnAt == 1
        and after::client.consecutiveFailures == 1
        and after::safety
      )
      .then(after::arrive(1, after::ReadCall))
      .then(after::acquireLock(1))
      .then(after::startBlockedByBackoff(1))
      .expect(after::ops.get(1).phase == after::Failed and after::callBackedOffAfterFailure and after::safety)
      .then(after::advanceTime)
      .then(after::arrive(2, after::ReadCall))
      .then(after::acquireLock(2))
      .then(after::beginSpawn(2))
      .then(after::completeSpawn(2))
      .then(after::beginRequest(2))
      .then(after::completeExchange(2))
      .expect(
        after::ops.get(2).phase == after::Done
        and after::client.nextSpawnAt == after::NONE
        and after::client.consecutiveFailures == 0
        and after::recoveredAfterBackoff
        and after::safety
      )

  // 6. PR #379 初版（鍵なし書き込み未切り離し）: 呼び出し側が打ち切ると、takt へ送った書き込みが中断され子プロセスも止まる。
  run unkeyedWriteAbortedWithoutDetachTest =
    noDetach::init
      .then(noDetach::arrive(0, noDetach::UnkeyedWrite))
      .then(noDetach::acquireLock(0))
      .then(noDetach::beginSpawn(0))
      .then(noDetach::completeSpawn(0))
      .then(noDetach::beginRequest(0))
      .then(noDetach::cancelCaller(0))
      .expect(
        noDetach::ops.get(0).phase == noDetach::Aborted
        and noDetach::ops.get(0).sentToTakt
        and not(noDetach::ops.get(0).writeRecorded)
        and noDetach::children.get(0).status == noDetach::Closed
        and not(noDetach::unkeyedWriteSurvivesCallerCancel)
      )

  // 7. 修正後: 鍵なし書き込みは呼び出し側が打ち切っても裏のタスクが最後まで進み、書き込みを記録して子プロセスを戻す。
  run unkeyedWriteCompletesAfterCallerCancelTest =
    after::init
      .then(after::arrive(0, after::UnkeyedWrite))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::completeSpawn(0))
      .then(after::beginRequest(0))
      .then(after::cancelCaller(0))
      .expect(
        after::ops.get(0).callerCancelled
        and after::ops.get(0).phase == after::Exchanging
        and after::children.get(0).status == after::Alive
        and after::safety
      )
      .then(after::completeExchange(0))
      .expect(
        after::ops.get(0).phase == after::Done
        and after::ops.get(0).writeRecorded
        and after::client.slot == 0
        and not(after::children.get(0).dirty)
        and after::unkeyedWriteCompletesAfterCallerCancel
        and after::safety
      )

  // 8. 修正後: shutdown を 2 回呼んでも、また打ち切り破棄後に shutdown を呼んでも、閉じた子プロセスを二度閉じない。
  run closedChildIsNeverClosedTwiceTest =
    after::init
      .then(after::arrive(0, after::WarmUp))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::completeSpawn(0))
      .then(after::shutdownClient)
      .expect(
        after::children.get(0).status == after::Closed
        and after::children.get(0).closeCount == 1
        and after::client.slot == after::NONE
        and after::shutdownClosedLiveChild
        and after::safety
      )
      .then(after::shutdownClient)
      .expect(
        after::children.get(0).closeCount == 1
        and after::shutdownSkippedWhenAlreadyClosed
        and after::safety
      )

  // 9. 修正後: 起動（initialize）の途中で打ち切られても子プロセスは破棄され、残らない。
  run cancelDuringSpawnDisposesInitializingChildTest =
    after::init
      .then(after::arrive(0, after::ReadCall))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::cancelCaller(0))
      .expect(
        after::children.get(0).status == after::Closed
        and after::children.get(0).closeCount == 1
        and after::client.slot == after::NONE
        and after::cancelledDuringSpawnDisposed
        and after::safety
      )

  // 10. 修正後: 打ち切り破棄や通信失敗破棄のあとに shutdown を呼んでも、閉じた子プロセスを二度閉じない。
  run shutdownAfterCancelOrFailureDoesNotDoubleCloseTest =
    after::init
      .then(after::arrive(0, after::ReadCall))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::completeSpawn(0))
      .then(after::beginRequest(0))
      .then(after::cancelCaller(0))
      .then(after::shutdownClient)
      .expect(
        after::children.get(0).status == after::Closed
        and after::children.get(0).closeCount == 1
        and after::safety
      )
      .then(after::arrive(1, after::ReadCall))
      .then(after::acquireLock(1))
      .then(after::beginSpawn(1))
      .then(after::completeSpawn(1))
      .then(after::beginRequest(1))
      .then(after::failExchange(1))
      .then(after::shutdownClient)
      .expect(
        after::children.get(1).status == after::Closed
        and after::children.get(1).closeCount == 1
        and after::client.nextSpawnAt == after::NONE
        and after::safety
      )

  // 11. 修正後: 鍵あり書き込みも呼び出し側の打ち切りに巻き込まれず最後まで進む。
  run keyedWriteCompletesAfterCallerCancelTest =
    after::init
      .then(after::arrive(0, after::KeyedWrite))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::completeSpawn(0))
      .then(after::beginRequest(0))
      .then(after::cancelCaller(0))
      .expect(
        after::ops.get(0).callerCancelled
        and after::ops.get(0).phase == after::Exchanging
        and after::safety
      )
      .then(after::completeExchange(0))
      .expect(
        after::ops.get(0).phase == after::Done
        and after::ops.get(0).writeRecorded
        and after::keyedWriteCompletesAfterCallerCancel
        and after::safety
      )

  // 12. 修正後: 抑制の満了後に温めが spawn 失敗（initialize 失敗）しても、抑制を再開せず直後の呼び出しは起動を試みる。
  run warmUpInitFailureAfterBackoffExpiryAllowsNextCallTest =
    after::init
      .then(after::arrive(0, after::ReadCall))
      .then(after::acquireLock(0))
      .then(after::failSpawnImmediate(0))
      .then(after::advanceTime)
      .then(after::arrive(1, after::WarmUp))
      .then(after::acquireLock(1))
      .then(after::beginSpawn(1))
      .then(after::failSpawnInit(1))
      .expect(
        after::children.get(0).status == after::Closed
        and after::children.get(0).closeCount == 1
        and after::client.consecutiveFailures == 1
        and after::client.nextSpawnAt == 1
        and after::safety
      )
      .then(after::arrive(2, after::ReadCall))
      .then(after::acquireLock(2))
      .then(after::beginSpawn(2))
      .then(after::completeSpawn(2))
      .expect(
        after::client.slot == 1
        and after::client.consecutiveFailures == 0
        and after::client.nextSpawnAt == after::NONE
        and after::safety
      )

  // 13. 修正後: 道具エラー（isError）は子プロセスを破棄せず再利用可能に戻すが、書き込みの記録（record_write）は行わない。
  run toolErrorReusesChildWithoutRecordingWriteTest =
    after::init
      .then(after::arrive(0, after::UnkeyedWrite))
      .then(after::acquireLock(0))
      .then(after::beginSpawn(0))
      .then(after::completeSpawn(0))
      .then(after::beginRequest(0))
      .then(after::completeExchangeToolError(0))
      .expect(
        after::ops.get(0).phase == after::Failed
        and not(after::ops.get(0).writeRecorded)
        and after::client.slot == 0
        and after::children.get(0).status == after::Alive
        and not(after::children.get(0).dirty)
        and after::toolErrorReusesCleanChild
        and after::safety
      )
      .then(after::arrive(1, after::UnkeyedWrite))
      .then(after::acquireLock(1))
      .then(after::startAlreadyRunning(1))
      .then(after::beginRequest(1))
      .then(after::completeExchange(1))
      .expect(
        after::ops.get(1).phase == after::Done
        and after::ops.get(1).writeRecorded
        and after::nextChildId == 1
        and after::safety
      )

  // 14. 修正後: 時刻 0・1・3 で 3 回連続して起動失敗すると 4 単位のバックオフ（nextSpawnAt = 7）に入り、
  //     時刻 7 で満了したあとに起動して回復する。
  run threeConsecutiveCallFailuresBackOffAndRecoverAtCapTest =
    after::init
      .then(after::arrive(0, after::ReadCall))
      .then(after::acquireLock(0))
      .then(after::failSpawnImmediate(0))
      .expect(after::client.consecutiveFailures == 1 and after::client.nextSpawnAt == 1)
      .then(after::advanceTime)
      .then(after::arrive(1, after::ReadCall))
      .then(after::acquireLock(1))
      .then(after::failSpawnImmediate(1))
      .expect(after::client.consecutiveFailures == 2 and after::client.nextSpawnAt == 3)
      .then(after::advanceTime)
      .then(after::advanceTime)
      .then(after::arrive(2, after::ReadCall))
      .then(after::acquireLock(2))
      .then(after::failSpawnImmediate(2))
      .expect(after::client.consecutiveFailures == 3 and after::client.nextSpawnAt == 7)
      .then(after::advanceTime)
      .then(after::advanceTime)
      .then(after::advanceTime)
      .then(after::arrive(3, after::ReadCall))
      .then(after::acquireLock(3))
      .then(after::startBlockedByBackoff(3))
      .expect(after::now == 6 and after::ops.get(3).phase == after::Failed and after::safety)
      .then(after::advanceTime)
      .then(after::arrive(4, after::ReadCall))
      .then(after::acquireLock(4))
      .then(after::beginSpawn(4))
      .then(after::completeSpawn(4))
      .then(after::beginRequest(4))
      .then(after::completeExchange(4))
      .expect(
        after::now == 7
        and after::ops.get(4).phase == after::Done
        and after::client.consecutiveFailures == 0
        and after::client.nextSpawnAt == after::NONE
        and after::recoveredAfterBackoff
        and after::safety
      )
}
