// -*- mode: Bluespec; -*-
// issue #383 の筋書きを、修正前（current）・見張りのみ（watchdogOnly）・修正案（fixed）でたどる。
module stateStreamWatchdog_test {
  import stateStreamWatchdog(WATCHDOG = false, STALE_GUARD = false) as before from "./stateStreamWatchdog"
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = false) as watchdogOnly from "./stateStreamWatchdog"
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true) as after from "./stateStreamWatchdog"

  // 定数の前提: 見張り時間は backend 再送間隔（15 秒 = 3 目盛り）の 2 倍より長い。
  run timeoutExceedsTwiceResendIntervalTest = all {
    after::STALE_TIMEOUT > 2 * after::RESEND_INTERVAL,
    after::RESEND_INTERVAL > 0,
  }

  // 修正前: 半開きになって見張り時間（40 秒 = 8 目盛り）を過ぎても切断にならない。
  run halfOpenNeverDisconnectsBeforeFixTest =
    before::init
      .then(before::goHalfOpen)
      .then(9.reps(_ => before::tick))
      .expect(not(before::disconnected) and before::activeConn == 0 and not(before::silentConnectionMustDisconnect))

  // 修正案: 半開きになって見張り時間に達したら切断になり、再接続して新しい受信で復帰する。
  run halfOpenDisconnectsAndRecoversTest =
    after::init
      .then(after::goHalfOpen)
      .then(8.reps(_ => after::tick))
      .then(after::watchdogTimeout)
      .expect(after::disconnected and after::activeConn == after::NONE and after::pendingReconnects == 1
        and after::conns.get(0) == after::Closed and after::safety)
      .then(after::reconnectFire)
      .expect(after::disconnected and after::activeConn == 1 and after::pendingReconnects == 0 and after::safety)
      .then(after::receiveActiveMessage)
      .expect(not(after::disconnected) and after::activeConn == 1 and after::safety)

  // 修正案: backend の再送間隔（15 秒 = 3 目盛り）ごとに受信が続いていれば、
  // 合計時間が見張り時間（40 秒 = 8 目盛り）を大幅に超えても切断にならない。
  run continuousReceivingNeverDisconnectsTest =
    after::init
      .then(5.reps(_ =>
        after::tick.then(after::tick).then(after::tick).then(after::receiveActiveMessage)
      ))
      .expect(after::now == 15 and not(after::disconnected) and after::activeConn == 0
        and after::nextConn == 1 and after::watchdogDisconnections == 0 and after::safety)

  // 修正案: 見張り時間未満（35 秒 = 7 目盛り）の遅延なら切断にならず、受信で見張りが張り直される。
  run delayedReceiveBeforeTimeoutKeepsConnectionTest =
    after::init
      .then(7.reps(_ => after::tick))
      .then(after::receiveActiveMessage)
      .then(7.reps(_ => after::tick))
      .then(after::receiveActiveMessage)
      .expect(after::now == 14 and not(after::disconnected) and after::activeConn == 0 and after::safety)

  // 見張りのみの版: 切断後、古い接続から遅延 onmessage が届くと disconnected が false に戻ってしまう。
  run staleMessageRevertsDisconnectWithoutGuardTest =
    watchdogOnly::init
      .then(watchdogOnly::goHalfOpen)
      .then(8.reps(_ => watchdogOnly::tick))
      .then(watchdogOnly::watchdogTimeout)
      .expect(watchdogOnly::disconnected and watchdogOnly::activeConn == watchdogOnly::NONE)
      .then(watchdogOnly::receiveStaleMessage(0))
      .expect(not(watchdogOnly::disconnected) and not(watchdogOnly::noStaleStateRestore))

  // 見張りのみの版: 開き直した後に古い接続の onerror が発火すると、新しい接続が開いたまま
  // もう 1 本の再接続が走って接続が 2 本になる。
  run staleErrorOpensDuplicateConnectionWithoutGuardTest =
    watchdogOnly::init
      .then(watchdogOnly::goHalfOpen)
      .then(8.reps(_ => watchdogOnly::tick))
      .then(watchdogOnly::watchdogTimeout)
      .then(watchdogOnly::reconnectFire)
      .then(watchdogOnly::receiveStaleError(0))
      .expect(not(watchdogOnly::noReconnectWhileConnected))
      .then(watchdogOnly::reconnectFire)
      .expect(not(watchdogOnly::atMostOneUnclosedConn))

  // 修正案: 切断中・開き直し後に古い接続から onmessage / onerror が届いても無視され、
  // 切断状態と新しい接続は壊れない。
  run staleEventsIgnoredAfterReconnectTest =
    after::init
      .then(after::goHalfOpen)
      .then(8.reps(_ => after::tick))
      .then(after::watchdogTimeout)
      .then(after::receiveStaleMessage(0))
      .expect(after::disconnected and after::activeConn == after::NONE and after::safety)
      .then(after::reconnectFire)
      .then(after::receiveStaleMessage(0))
      .expect(after::disconnected and after::activeConn == 1 and after::safety)
      .then(after::receiveStaleError(0))
      .expect(after::activeConn == 1 and after::pendingReconnects == 0 and after::safety)
      .then(after::receiveActiveMessage)
      .expect(not(after::disconnected) and after::activeConn == 1 and after::safety)

  // 修正案: スリープからの復帰で見張り時間（40 秒）以上経っていれば即座に切断して開き直す。
  run sleepWakeDetectsStaleConnectionTest =
    after::init
      .then(after::tick)
      .then(after::goHalfOpen)
      .then(after::wakeFromSleep(12))
      .expect(after::disconnected and after::activeConn == after::NONE and after::pendingReconnects == 1
        and after::wakeDisconnects == 1 and after::safety)

  // 修正案: 短い復帰（10 秒 = 2 目盛り、見張り時間未満）では健全な接続を切らない。
  run shortWakeKeepsHealthyConnectionTest =
    after::init
      .then(after::wakeFromSleep(2))
      .expect(not(after::disconnected) and after::activeConn == 0 and after::wakeDisconnects == 0 and after::safety)

  // 修正案: 初回接続直後に 1 件も届かず半開きになっても、見張り時間で切断して開き直す。
  // さらに開き直し後も 1 件も届かずに無音なら、もう一度切断して開き直す。
  run repeatedSilentTimeoutsKeepSingleConnectionTest =
    after::init
      .then(after::goHalfOpen)
      .then(8.reps(_ => after::tick))
      .then(after::watchdogTimeout)
      .then(after::reconnectFire)
      .then(after::goHalfOpen)
      .then(8.reps(_ => after::tick))
      .then(after::watchdogTimeout)
      .expect(after::disconnected and after::consecutiveFailures == 2
        and after::pendingReconnects == 1 and after::conns.get(0) == after::Closed
        and after::conns.get(1) == after::Closed and after::safety)

  // 修正案: 不正なペイロードを受けても見張り期限は伸びず、無音が続けば切断になる。
  run invalidMessageDoesNotResetWatchdogTest =
    after::init
      .then(4.reps(_ => after::tick))
      .then(after::receiveInvalidMessage)
      .expect(after::lastActivity == 0)
      .then(after::goHalfOpen)
      .then(4.reps(_ => after::tick))
      .then(after::watchdogTimeout)
      .expect(after::disconnected and after::now == 8 and after::safety)

  // 修正案: close 後は接続もタイマーも残らず、遅延メッセージも無視される。
  run closeStopsEverythingAndIgnoresLateMessageTest =
    after::init
      .then(after::closeStream)
      .expect(after::nothingActiveWhenClosed and after::safety)
      .then(after::receiveStaleMessage(0))
      .expect(after::closed and after::safety)
}
