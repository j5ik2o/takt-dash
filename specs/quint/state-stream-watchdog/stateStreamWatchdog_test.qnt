// -*- mode: Bluespec; -*-
// issue #383 の筋書きを、修正前（current）・見張りのみ（watchdogOnly）・
// スリープ張り直し無し（noWakeRearm）・修正案（fixed）でたどる。
module stateStreamWatchdog_test {
  import stateStreamWatchdog(WATCHDOG = false, STALE_GUARD = false, REARM_ON_WAKE = false, ALLOW_CLOSE = true) as before from "./stateStreamWatchdog"
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = false, REARM_ON_WAKE = true, ALLOW_CLOSE = true) as watchdogOnly from "./stateStreamWatchdog"
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true, REARM_ON_WAKE = false, ALLOW_CLOSE = true) as noWakeRearm from "./stateStreamWatchdog"
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true, REARM_ON_WAKE = true, ALLOW_CLOSE = true) as after from "./stateStreamWatchdog"

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
      .then(after::watchdogFire)
      .expect(after::disconnected and after::activeConn == after::NONE and after::pendingReconnects == 1
        and after::conns.get(0) == after::Closed and after::safety)
      .then(after::reconnectFire)
      .expect(after::disconnected and after::activeConn == 1 and after::pendingReconnects == 0 and after::safety)
      .then(after::deliverMessage(1))
      .expect(not(after::disconnected) and after::activeConn == 1 and after::recoveredAfterWatchdog and after::safety)

  // 修正案: 健全な（Open の）接続でも 40 秒黙り続ければ見張りで切断し、
  // 遅れて届いた旧接続の受信は捨て、次の接続の受信で復帰する。
  run silentHealthyConnDisconnectsAndRecoversTest =
    after::init
      .then(8.reps(_ => after::tick))
      .then(after::watchdogFire)
      .expect(after::disconnected and after::activeConn == after::NONE and after::conns.get(0) == after::Closed and after::safety)
      .then(after::deliverMessage(0))
      .expect(after::disconnected and after::activeConn == after::NONE and after::safety)
      .then(after::reconnectFire)
      .then(after::deliverMessage(1))
      .expect(not(after::disconnected) and after::activeConn == 1 and after::recoveredAfterWatchdog and after::safety)

  // 修正案: backend の再送間隔（15 秒 = 3 目盛り）ごとに受信が続いていれば、
  // 合計時間が見張り時間（40 秒 = 8 目盛り）を大幅に超えても切断にならない。
  run continuousReceivingNeverDisconnectsTest =
    after::init
      .then(5.reps(_ =>
        after::tick.then(after::tick).then(after::tick).then(after::deliverMessage(0))
      ))
      .expect(after::wallNow == 15 and not(after::disconnected) and after::activeConn == 0
        and after::nextConn == 1 and after::watchdogDisconnections == 0 and after::safety)

  // 修正案: 期限より前に見張りが早期発火しても切断せず、残り時間で張り直す。
  run earlyWatchdogFireRearmsWithoutDisconnectTest =
    after::init
      .then(3.reps(_ => after::tick))
      .then(after::watchdogFire)
      .expect(not(after::disconnected) and after::activeConn == 0
        and after::watchdogDeadlineMono == 8 and after::watchdogDisconnections == 0 and after::safety)

  // 修正案: 見張り時間未満（35 秒 = 7 目盛り）の遅延なら切断にならず、受信で見張りが張り直される。
  run delayedReceiveBeforeTimeoutKeepsConnectionTest =
    after::init
      .then(7.reps(_ => after::tick))
      .then(after::deliverMessage(0))
      .then(7.reps(_ => after::tick))
      .then(after::deliverMessage(0))
      .expect(after::wallNow == 14 and not(after::disconnected) and after::activeConn == 0 and after::safety)

  // 見張りのみの版: 切断後、古い接続から遅延 onmessage が届くと disconnected が false に戻って
  // 状態不変条件 connectedImpliesValidActiveConn に違反する。
  run staleMessageRevertsDisconnectWithoutGuardTest =
    watchdogOnly::init
      .then(watchdogOnly::goHalfOpen)
      .then(8.reps(_ => watchdogOnly::tick))
      .then(watchdogOnly::watchdogFire)
      .expect(watchdogOnly::disconnected and watchdogOnly::activeConn == watchdogOnly::NONE)
      .then(watchdogOnly::deliverMessage(0))
      .expect(not(watchdogOnly::disconnected) and not(watchdogOnly::connectedImpliesValidActiveConn))

  // 見張りのみの版: 開き直した後に古い接続の onerror が発火すると、新しい接続が開いたまま
  // もう 1 本の再接続が走って接続が 2 本になる。
  run staleErrorOpensDuplicateConnectionWithoutGuardTest =
    watchdogOnly::init
      .then(watchdogOnly::goHalfOpen)
      .then(8.reps(_ => watchdogOnly::tick))
      .then(watchdogOnly::watchdogFire)
      .then(watchdogOnly::reconnectFire)
      .then(watchdogOnly::deliverError(0))
      .expect(not(watchdogOnly::noReconnectWhileConnected))
      .then(watchdogOnly::reconnectFire)
      .expect(not(watchdogOnly::atMostOneUnclosedConn))

  // 修正案: 切断中・開き直し後に古い接続から onmessage / onerror が届いても無視され、
  // 切断状態と新しい接続は壊れない。
  run staleEventsIgnoredAfterReconnectTest =
    after::init
      .then(after::goHalfOpen)
      .then(8.reps(_ => after::tick))
      .then(after::watchdogFire)
      .then(after::deliverMessage(0))
      .expect(after::disconnected and after::activeConn == after::NONE and after::safety)
      .then(after::reconnectFire)
      .then(after::deliverMessage(0))
      .expect(after::disconnected and after::activeConn == 1 and after::safety)
      .then(after::deliverError(0))
      .expect(after::activeConn == 1 and after::pendingReconnects == 0 and after::safety)
      .then(after::deliverMessage(1))
      .expect(not(after::disconnected) and after::activeConn == 1 and after::safety)

  // 修正案: スリープ後も TCP が生きている（Open のままの）接続で見張り時間以上経っていれば
  // 復帰時に即座に切断し、遅れて届いた旧接続の受信は捨て、次の接続で復帰する。
  run sleepWakeAliveTcpDisconnectsAndRecoversTest =
    after::init
      .then(after::tick)
      .then(after::wakeFromSleep(12))
      .expect(after::disconnected and after::activeConn == after::NONE and after::pendingReconnects == 1
        and after::wakeDisconnects == 1 and after::safety)
      .then(after::deliverMessage(0))
      .expect(after::disconnected and after::activeConn == after::NONE and after::safety)
      .then(after::reconnectFire)
      .then(after::deliverMessage(1))
      .expect(not(after::disconnected) and after::activeConn == 1 and after::recoveredAfterWatchdog and after::safety)

  // スリープ復帰の張り直し無し（noWakeRearm）: 期限未満の部分スリープ（25 秒）で単調時計が止まったあと、
  // 残り時間で張り直さないと壁時計が 40 秒を超えても切断にならず不変条件 1 に違反する。
  run partialSleepMissesDeadlineWithoutRearmTest =
    noWakeRearm::init
      .then(noWakeRearm::tick)
      .then(noWakeRearm::wakeFromSleep(5))
      .then(3.reps(_ => noWakeRearm::tick))
      .expect(noWakeRearm::wallNow == 9 and noWakeRearm::monoNow == 4
        and not(noWakeRearm::disconnected) and not(noWakeRearm::silentConnectionMustDisconnect))

  // 修正案: 期限未満の部分スリープ（25 秒）では接続を切らず、残り時間（10 秒 = 2 目盛り）で
  // 見張りを張り直すため、壁時計がちょうど 40 秒（8 目盛り）に達した時点で切断になる。
  run partialSleepRearmsAndDisconnectsOnTimeTest =
    after::init
      .then(after::tick)
      .then(after::wakeFromSleep(5))
      .expect(not(after::disconnected) and after::wallNow == 6 and after::monoNow == 1
        and after::watchdogDeadlineMono == 3 and after::safety)
      .then(2.reps(_ => after::tick))
      .then(after::watchdogFire)
      .expect(after::disconnected and after::wallNow == 8 and after::monoNow == 3 and after::safety)

  // 修正案: 壁時計が逆行した状態でチェックが起きても誤切断せず、見張りを張り直す。
  run backwardWallClockDoesNotPrematurelyDisconnectTest =
    after::init
      .then(after::tick)
      .then(after::deliverMessage(0))
      .then(after::rewindWallClock(1))
      .then(after::watchdogFire)
      .expect(not(after::disconnected) and after::activeConn == 0
        and after::lastActivityWall == 0 and after::watchdogDeadlineMono == 9 and after::safety)

  // 修正案: 不正なペイロードを受けても見張り期限は伸びず、無音が続けば切断になる。
  run invalidMessageDoesNotResetWatchdogTest =
    after::init
      .then(4.reps(_ => after::tick))
      .then(after::receiveInvalidMessage)
      .expect(after::lastActivityWall == 0)
      .then(after::goHalfOpen)
      .then(4.reps(_ => after::tick))
      .then(after::watchdogFire)
      .expect(after::disconnected and after::wallNow == 8 and after::safety)

  // 修正案: close 後は接続もタイマーも残らず、遅延メッセージも無視される。
  run closeStopsEverythingAndIgnoresLateMessageTest =
    after::init
      .then(after::closeStream)
      .expect(after::nothingActiveWhenClosed and after::safety)
      .then(after::deliverMessage(0))
      .expect(after::closed and after::safety)
}
