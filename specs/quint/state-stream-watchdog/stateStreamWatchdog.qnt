// -*- mode: Bluespec; -*-
// 画面の SSE 状態ストリーム（openStateStream）の半開き検知と再接続の状態機械（issue #383）。
//
// 画面は /api/state の EventSource を 1 本だけ開き、backend が 15 秒ごとに送り直す
// DashboardState を受け取る。VPN・Wi-Fi の切り替えやスリープからの復帰で接続が半開きに
// なると onerror が出ず、画面は切断に気づけない。
//
// 時計は実装と同じ 2 本に分ける（1 目盛り = 5 秒）:
// - monoNow: タイマー（setTimeout）が使う単調時計。OS スリープ中は進まないことがある。
// - wallNow: Date.now() の壁時計。OS スリープ中は進み、NTP 等で逆行することもある。
// - RESEND_INTERVAL = 3（15 秒。backend の WATCH_RESEND_INTERVAL）
// - STALE_TIMEOUT   = 8（40 秒。15 秒再送に対し組み立て・遅延を最大 25 秒許容）
//
// 定数による遷移の切り替え:
// - WATCHDOG: 無音見張りタイマーとスリープ復帰時の鮮度確認（checkStale）の有無。
// - STALE_GUARD: closed または id != activeConn の受信・エラー・見張り発火を無視するか。
// - REARM_ON_WAKE: checkStale で期限未満のときに残り時間で見張りを張り直すか。
// - WAKE_CUTOFF: wakeFromSleep（checkStale）が切断と判定する壁時計経過のしきい値（通常 STALE_TIMEOUT = 8）。
// - ALLOW_CLOSE: stream.close() を遷移に含めるか（false は長時間開いたタブの探索用）。
module stateStreamWatchdog {
  const WATCHDOG: bool
  const STALE_GUARD: bool
  const REARM_ON_WAKE: bool
  const WAKE_CUTOFF: int
  const ALLOW_CLOSE: bool

  type ConnStatus = Open | HalfOpen | Closed
  type DisconnectCause = NoDisconnect | ActiveError | WatchdogTimer | WakeCheck

  pure val NONE: int = -1
  // 1 目盛り = 5 秒。
  pure val RESEND_INTERVAL: int = 3
  pure val STALE_TIMEOUT: int = 8
  // 状態空間を閉じるための上限。
  pure val MAX_CONNS: int = 4
  pure val MAX_TIME: int = 40
  // スリープ時間の候補（2=10秒、5=25秒は期限未満の部分スリープ、8=40秒、12=60秒は期限以上）。
  pure val SLEEP_JUMPS: Set[int] = Set(2, 5, 8, 12)
  // NTP 等による壁時計の逆行幅（1=5秒、2=10秒）。
  pure val REWIND_STEPS: Set[int] = Set(1, 2)

  var closed: bool
  var disconnected: bool
  var activeConn: int
  var nextConn: int
  var conns: int -> ConnStatus
  // 有効な DashboardState を一度でも適用した接続 ID の集合（状態不変条件に使う）。
  var receivedConns: Set[int]
  // 最後に状態を適用した接続 ID（初回 SSR 状態は NONE）。
  var lastAppliedConn: int
  var pendingReconnects: int
  var watchdogArmed: bool
  var watchdogConn: int
  // 単調時計（タイマー用）と壁時計（now() 用）。
  var monoNow: int
  var wallNow: int
  var lastActivityWall: int
  var watchdogDeadlineMono: int
  var consecutiveFailures: int
  // 直近の切断の原因と、その切断時点の壁時計経過（wallNow - lastActivityWall）。
  // 不変条件 4（期限より前に見張り・復帰確認で切らない）の判定に使う実状態。
  var lastDisconnectCause: DisconnectCause
  var lastDisconnectGap: int

  // 到達確認（witness）用の記録。
  var watchdogDisconnections: int
  var wakeDisconnects: int
  var reconnections: int
  var messagesReceived: int
  var invalidMessages: int
  var staleEventsIgnored: int
  var recoveredAfterWatchdogTimer: bool
  var recoveredAfterWakeCheck: bool

  // 単調時計が見張り期限に達しているか。
  // WATCHDOG = true のとき、通常の時刻進行（tick）は期限を素通りできない。
  val watchdogDue: bool =
    WATCHDOG and watchdogArmed and not(closed) and activeConn != NONE
      and monoNow >= watchdogDeadlineMono

  action init: bool = all {
    closed' = false,
    disconnected' = false,
    activeConn' = 0,
    nextConn' = 1,
    conns' = Map(0 -> Open),
    receivedConns' = Set(),
    lastAppliedConn' = NONE,
    pendingReconnects' = 0,
    watchdogArmed' = WATCHDOG,
    watchdogConn' = if (WATCHDOG) 0 else NONE,
    monoNow' = 0,
    wallNow' = 0,
    lastActivityWall' = 0,
    watchdogDeadlineMono' = if (WATCHDOG) STALE_TIMEOUT else 0,
    consecutiveFailures' = 0,
    lastDisconnectCause' = NoDisconnect,
    lastDisconnectGap' = 0,
    watchdogDisconnections' = 0,
    wakeDisconnects' = 0,
    reconnections' = 0,
    messagesReceived' = 0,
    invalidMessages' = 0,
    staleEventsIgnored' = 0,
    recoveredAfterWatchdogTimer' = false,
    recoveredAfterWakeCheck' = false,
  }

  // 通常の時刻進行（1 目盛り = 5 秒）。単調時計と壁時計の両方が 1 進む。
  // Open のまま黙り続ける経路（backend 遅延や一時停止）も制限せずに許す。
  action tick: bool = all {
    not(closed),
    wallNow < MAX_TIME,
    monoNow < MAX_TIME,
    not(watchdogDue),
    monoNow' = monoNow + 1,
    wallNow' = wallNow + 1,
    closed' = closed, disconnected' = disconnected, activeConn' = activeConn,
    nextConn' = nextConn, conns' = conns, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, pendingReconnects' = pendingReconnects,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    lastActivityWall' = lastActivityWall, watchdogDeadlineMono' = watchdogDeadlineMono,
    consecutiveFailures' = consecutiveFailures,
    lastDisconnectCause' = lastDisconnectCause, lastDisconnectGap' = lastDisconnectGap,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  // VPN・Wi-Fi の切り替えなどで、現在の接続が onerror を出さずに半開きになる。
  action goHalfOpen: bool = all {
    not(closed),
    activeConn != NONE,
    conns.get(activeConn) == Open,
    conns' = conns.set(activeConn, HalfOpen),
    closed' = closed, disconnected' = disconnected, activeConn' = activeConn,
    nextConn' = nextConn, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, pendingReconnects' = pendingReconnects,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    monoNow' = monoNow, wallNow' = wallNow, lastActivityWall' = lastActivityWall,
    watchdogDeadlineMono' = watchdogDeadlineMono, consecutiveFailures' = consecutiveFailures,
    lastDisconnectCause' = lastDisconnectCause, lastDisconnectGap' = lastDisconnectGap,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  // 接続 id から onmessage が届く（現在接続の通常受信と、古い・閉じた接続からの遅延受信を 1 本にまとめる）。
  // 実物と同じ判定: STALE_GUARD = true のとき closed または id != activeConn なら無視する。
  action deliverMessage(id: int): bool = all {
    conns.keys().contains(id),
    // 半開き（HalfOpen）の現在接続からは通常メッセージは届かない。
    (not(closed) and id == activeConn) implies (conns.get(id) == Open),
    if (STALE_GUARD and (closed or id != activeConn)) all {
      staleEventsIgnored' = staleEventsIgnored + 1,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      lastActivityWall' = lastActivityWall,
      watchdogArmed' = watchdogArmed,
      watchdogConn' = watchdogConn,
      watchdogDeadlineMono' = watchdogDeadlineMono,
      receivedConns' = receivedConns,
      lastAppliedConn' = lastAppliedConn,
      messagesReceived' = messagesReceived,
      recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
      recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
    } else all {
      staleEventsIgnored' = staleEventsIgnored,
      disconnected' = false,
      consecutiveFailures' = 0,
      lastActivityWall' = wallNow,
      watchdogArmed' = WATCHDOG,
      watchdogConn' = if (WATCHDOG) id else NONE,
      watchdogDeadlineMono' = if (WATCHDOG) monoNow + STALE_TIMEOUT else watchdogDeadlineMono,
      receivedConns' = receivedConns.union(Set(id)),
      lastAppliedConn' = id,
      messagesReceived' = if (not(closed) and id == activeConn) messagesReceived + 1 else messagesReceived,
      recoveredAfterWatchdogTimer' = (recoveredAfterWatchdogTimer or (not(closed) and id == activeConn and disconnected and lastDisconnectCause == WatchdogTimer)),
      recoveredAfterWakeCheck' = (recoveredAfterWakeCheck or (not(closed) and id == activeConn and disconnected and lastDisconnectCause == WakeCheck)),
    },
    closed' = closed, activeConn' = activeConn, nextConn' = nextConn, conns' = conns,
    pendingReconnects' = pendingReconnects, monoNow' = monoNow, wallNow' = wallNow,
    lastDisconnectCause' = lastDisconnectCause, lastDisconnectGap' = lastDisconnectGap,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, invalidMessages' = invalidMessages,
  }

  // 現在の接続で検証に失敗する不正なペイロードを受信する。
  // 状態・切断フラグ・連続失敗回数・見張り期限のいずれも更新しない。
  action receiveInvalidMessage: bool = all {
    not(closed),
    activeConn != NONE,
    conns.get(activeConn) == Open,
    invalidMessages' = invalidMessages + 1,
    closed' = closed, disconnected' = disconnected, activeConn' = activeConn,
    nextConn' = nextConn, conns' = conns, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, pendingReconnects' = pendingReconnects,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    monoNow' = monoNow, wallNow' = wallNow, lastActivityWall' = lastActivityWall,
    watchdogDeadlineMono' = watchdogDeadlineMono, consecutiveFailures' = consecutiveFailures,
    lastDisconnectCause' = lastDisconnectCause, lastDisconnectGap' = lastDisconnectGap,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    staleEventsIgnored' = staleEventsIgnored,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  // 接続 id で onerror が発火する（現在接続のエラーと、古い・閉じた接続からの遅延・重複エラーを 1 本にまとめる）。
  // 実物と同じ判定: closed または（STALE_GUARD = true かつ id != activeConn）なら無視する。
  action deliverError(id: int): bool = all {
    conns.keys().contains(id),
    if (closed or (STALE_GUARD and id != activeConn)) all {
      staleEventsIgnored' = staleEventsIgnored + 1,
      conns' = conns,
      activeConn' = activeConn,
      watchdogArmed' = watchdogArmed,
      watchdogConn' = watchdogConn,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      pendingReconnects' = pendingReconnects,
      lastDisconnectCause' = lastDisconnectCause,
      lastDisconnectGap' = lastDisconnectGap,
    } else all {
      staleEventsIgnored' = staleEventsIgnored,
      conns' = conns.set(id, Closed),
      activeConn' = if (id == activeConn) NONE else activeConn,
      watchdogArmed' = if (id == activeConn) false else watchdogArmed,
      watchdogConn' = if (id == activeConn) NONE else watchdogConn,
      disconnected' = true,
      consecutiveFailures' = consecutiveFailures + 1,
      pendingReconnects' = 1,
      lastDisconnectCause' = ActiveError,
      lastDisconnectGap' = wallNow - lastActivityWall,
    },
    closed' = closed, nextConn' = nextConn, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, monoNow' = monoNow, wallNow' = wallNow,
    lastActivityWall' = lastActivityWall, watchdogDeadlineMono' = watchdogDeadlineMono,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  // 見張りタイマーの発火。期限到達時だけでなく早期発火（タイマー揺らぎ等）も許す。
  // 発火時に wallNow - lastActivityWall を確かめ、期限未満なら残りの時間で張り直して切断しない。
  // 壁時計が逆行していたら lastActivityWall を wallNow に詰めて STALE_TIMEOUT で張り直す。
  action watchdogFire: bool = all {
    WATCHDOG,
    watchdogArmed,
    not(closed),
    if (STALE_GUARD and watchdogConn != activeConn) all {
      watchdogArmed' = false,
      watchdogConn' = NONE,
      staleEventsIgnored' = staleEventsIgnored + 1,
      conns' = conns,
      activeConn' = activeConn,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      pendingReconnects' = pendingReconnects,
      lastActivityWall' = lastActivityWall,
      watchdogDeadlineMono' = watchdogDeadlineMono,
      lastDisconnectCause' = lastDisconnectCause,
      lastDisconnectGap' = lastDisconnectGap,
      watchdogDisconnections' = watchdogDisconnections,
    } else if (wallNow < lastActivityWall) all {
      // 壁時計が逆行していた場合は基準を現在時刻へ詰めて張り直す。
      lastActivityWall' = wallNow,
      watchdogDeadlineMono' = monoNow + STALE_TIMEOUT,
      watchdogArmed' = true,
      watchdogConn' = activeConn,
      conns' = conns,
      activeConn' = activeConn,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      pendingReconnects' = pendingReconnects,
      lastDisconnectCause' = lastDisconnectCause,
      lastDisconnectGap' = lastDisconnectGap,
      watchdogDisconnections' = watchdogDisconnections,
      staleEventsIgnored' = staleEventsIgnored,
    } else if (wallNow - lastActivityWall < STALE_TIMEOUT) all {
      // 期限未満なら残り時間で張り直し、切断しない。
      lastActivityWall' = lastActivityWall,
      watchdogDeadlineMono' = monoNow + (STALE_TIMEOUT - (wallNow - lastActivityWall)),
      watchdogArmed' = true,
      watchdogConn' = activeConn,
      conns' = conns,
      activeConn' = activeConn,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      pendingReconnects' = pendingReconnects,
      lastDisconnectCause' = lastDisconnectCause,
      lastDisconnectGap' = lastDisconnectGap,
      watchdogDisconnections' = watchdogDisconnections,
      staleEventsIgnored' = staleEventsIgnored,
    } else all {
      // 期限以上なら切断して接続を閉じ、再接続を 1 本予約する。
      conns' = if (activeConn != NONE) conns.set(activeConn, Closed) else conns,
      activeConn' = NONE,
      watchdogArmed' = false,
      watchdogConn' = NONE,
      disconnected' = true,
      consecutiveFailures' = consecutiveFailures + 1,
      pendingReconnects' = 1,
      lastActivityWall' = lastActivityWall,
      watchdogDeadlineMono' = watchdogDeadlineMono,
      lastDisconnectCause' = WatchdogTimer,
      lastDisconnectGap' = wallNow - lastActivityWall,
      watchdogDisconnections' = watchdogDisconnections + 1,
      staleEventsIgnored' = staleEventsIgnored,
    },
    closed' = closed, nextConn' = nextConn, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, monoNow' = monoNow, wallNow' = wallNow,
    wakeDisconnects' = wakeDisconnects, reconnections' = reconnections,
    messagesReceived' = messagesReceived, invalidMessages' = invalidMessages,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  // OS スリープからの復帰。壁時計（wallNow）だけが jump だけ進み、単調時計（monoNow）は止まったまま。
  // Open のまま（スリープ後も TCP が生きている場合）でも長いスリープを許す。
  // 復帰時に checkStale() が走り、WAKE_CUTOFF（通常 STALE_TIMEOUT）以上なら即座に切断し、
  // 期限未満なら残り時間で見張りを張り直す。
  action wakeFromSleep(jump: int): bool = {
    val newWall = wallNow + jump
    val elapsed = newWall - lastActivityWall
    all {
      not(closed),
      newWall <= MAX_TIME,
      activeConn != NONE,
      wallNow' = newWall,
      monoNow' = monoNow,
      if (WATCHDOG and elapsed >= WAKE_CUTOFF) all {
        conns' = conns.set(activeConn, Closed),
        activeConn' = NONE,
        watchdogArmed' = false,
        watchdogConn' = NONE,
        watchdogDeadlineMono' = watchdogDeadlineMono,
        lastActivityWall' = lastActivityWall,
        disconnected' = true,
        consecutiveFailures' = consecutiveFailures + 1,
        pendingReconnects' = 1,
        lastDisconnectCause' = WakeCheck,
        lastDisconnectGap' = elapsed,
        wakeDisconnects' = wakeDisconnects + 1,
      } else if (WATCHDOG and REARM_ON_WAKE and elapsed >= 0 and elapsed < STALE_TIMEOUT) all {
        // 期限内なら残り時間で見張りを張り直す（単調時計が止まっていた分の遅れを解消する）。
        conns' = conns,
        activeConn' = activeConn,
        watchdogArmed' = true,
        watchdogConn' = activeConn,
        watchdogDeadlineMono' = monoNow + (STALE_TIMEOUT - elapsed),
        lastActivityWall' = lastActivityWall,
        disconnected' = disconnected,
        consecutiveFailures' = consecutiveFailures,
        pendingReconnects' = pendingReconnects,
        lastDisconnectCause' = lastDisconnectCause,
        lastDisconnectGap' = lastDisconnectGap,
        wakeDisconnects' = wakeDisconnects,
      } else if (WATCHDOG and REARM_ON_WAKE and elapsed < 0) all {
        conns' = conns,
        activeConn' = activeConn,
        watchdogArmed' = true,
        watchdogConn' = activeConn,
        watchdogDeadlineMono' = monoNow + STALE_TIMEOUT,
        lastActivityWall' = newWall,
        disconnected' = disconnected,
        consecutiveFailures' = consecutiveFailures,
        pendingReconnects' = pendingReconnects,
        lastDisconnectCause' = lastDisconnectCause,
        lastDisconnectGap' = lastDisconnectGap,
        wakeDisconnects' = wakeDisconnects,
      } else all {
        conns' = conns,
        activeConn' = activeConn,
        watchdogArmed' = watchdogArmed,
        watchdogConn' = watchdogConn,
        watchdogDeadlineMono' = watchdogDeadlineMono,
        lastActivityWall' = lastActivityWall,
        disconnected' = disconnected,
        consecutiveFailures' = consecutiveFailures,
        pendingReconnects' = pendingReconnects,
        lastDisconnectCause' = lastDisconnectCause,
        lastDisconnectGap' = lastDisconnectGap,
        wakeDisconnects' = wakeDisconnects,
      },
      closed' = closed, nextConn' = nextConn, receivedConns' = receivedConns,
      lastAppliedConn' = lastAppliedConn,
      watchdogDisconnections' = watchdogDisconnections, reconnections' = reconnections,
      messagesReceived' = messagesReceived, invalidMessages' = invalidMessages,
      staleEventsIgnored' = staleEventsIgnored,
      recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
      recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
    }
  }

  // NTP 等で壁時計（wallNow）だけが逆行する（単調時計は変わらない）。
  action rewindWallClock(delta: int): bool = all {
    not(closed),
    wallNow - delta >= 0,
    wallNow' = wallNow - delta,
    monoNow' = monoNow,
    closed' = closed, disconnected' = disconnected, activeConn' = activeConn,
    nextConn' = nextConn, conns' = conns, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, pendingReconnects' = pendingReconnects,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    lastActivityWall' = lastActivityWall, watchdogDeadlineMono' = watchdogDeadlineMono,
    consecutiveFailures' = consecutiveFailures,
    lastDisconnectCause' = lastDisconnectCause, lastDisconnectGap' = lastDisconnectGap,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  // 予約していた再接続タイマーが発火し、新しい接続を 1 本開く。
  // 接続を開いただけでは disconnected は false に戻さない（次の有効な受信で戻す）。
  action reconnectFire: bool = all {
    not(closed),
    pendingReconnects > 0,
    nextConn < MAX_CONNS,
    conns' = conns.put(nextConn, Open),
    activeConn' = nextConn,
    nextConn' = nextConn + 1,
    pendingReconnects' = pendingReconnects - 1,
    lastActivityWall' = wallNow,
    watchdogArmed' = WATCHDOG,
    watchdogConn' = if (WATCHDOG) nextConn else NONE,
    watchdogDeadlineMono' = if (WATCHDOG) monoNow + STALE_TIMEOUT else watchdogDeadlineMono,
    reconnections' = reconnections + 1,
    closed' = closed, disconnected' = disconnected, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, monoNow' = monoNow, wallNow' = wallNow,
    consecutiveFailures' = consecutiveFailures,
    lastDisconnectCause' = lastDisconnectCause, lastDisconnectGap' = lastDisconnectGap,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    messagesReceived' = messagesReceived, invalidMessages' = invalidMessages,
    staleEventsIgnored' = staleEventsIgnored,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  // 画面のアンマウントなどで stream.close() を呼ぶ。接続と全タイマーを止める。
  action closeStream: bool = all {
    ALLOW_CLOSE,
    not(closed),
    closed' = true,
    conns' = if (activeConn != NONE) conns.set(activeConn, Closed) else conns,
    activeConn' = NONE,
    pendingReconnects' = 0,
    watchdogArmed' = false,
    watchdogConn' = NONE,
    disconnected' = disconnected, nextConn' = nextConn, receivedConns' = receivedConns,
    lastAppliedConn' = lastAppliedConn, monoNow' = monoNow, wallNow' = wallNow,
    lastActivityWall' = lastActivityWall, watchdogDeadlineMono' = watchdogDeadlineMono,
    consecutiveFailures' = consecutiveFailures,
    lastDisconnectCause' = lastDisconnectCause, lastDisconnectGap' = lastDisconnectGap,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
    recoveredAfterWatchdogTimer' = recoveredAfterWatchdogTimer,
    recoveredAfterWakeCheck' = recoveredAfterWakeCheck,
  }

  action step: bool = any {
    tick,
    goHalfOpen,
    {
      nondet id = conns.keys().oneOf()
      any {
        deliverMessage(id),
        deliverError(id),
      }
    },
    receiveInvalidMessage,
    watchdogFire,
    {
      nondet jump = SLEEP_JUMPS.oneOf()
      wakeFromSleep(jump)
    },
    {
      nondet delta = REWIND_STEPS.oneOf()
      rewindWallClock(delta)
    },
    reconnectFire,
    closeStream,
  }

  // ===== 不変条件（安全性。すべて実状態で記述し、印フラグを使わない） =====

  // 1. 見張りの時間（壁時計基準）を過ぎたら必ず切断になる（スリープを挟んでも成り立つ）。
  val silentConnectionMustDisconnect: bool =
    (not(closed) and activeConn != NONE and wallNow > lastActivityWall + STALE_TIMEOUT)
      implies disconnected

  // 2a. 開いたままの接続は同時に 1 本まで。
  val atMostOneUnclosedConn: bool =
    conns.keys().filter(id => conns.get(id) != Closed).size() <= 1

  // 2b. 予約中の再接続タイマーは同時に 1 本まで。
  val atMostOnePendingReconnect: bool = pendingReconnects <= 1

  // 2c. 接続が開いている間に再接続タイマーが同時に予約されない。
  val noReconnectWhileConnected: bool =
    activeConn != NONE implies pendingReconnects == 0

  // 3a. 接続中（disconnected == false）なら、有効な現在接続が存在し、
  //     再接続後の接続ではその接続自身で少なくとも 1 回は受信している。
  val connectedImpliesValidActiveConn: bool =
    (not(closed) and not(disconnected)) implies
      (activeConn != NONE and (activeConn == 0 or receivedConns.contains(activeConn)))

  // 3b. 接続中に最後に適用した状態は、現在の接続から届いたものだけ（古い接続の受信で上書きされない）。
  val appliedStateMatchesActiveWhenConnected: bool =
    (not(closed) and not(disconnected) and lastAppliedConn != NONE) implies
      (lastAppliedConn == activeConn)

  // 4. 期限より前に見張りタイマー・復帰時確認（checkStale）のどちらでも切らない。
  val noPrematureWatchdogDisconnect: bool =
    (lastDisconnectCause == WatchdogTimer or lastDisconnectCause == WakeCheck)
      implies (lastDisconnectGap >= STALE_TIMEOUT)

  // 5. 見張りタイマーは開いている現在の接続 1 本だけに紐づく。
  val watchdogMatchesActiveConn: bool =
    watchdogArmed implies (not(closed) and activeConn != NONE and watchdogConn == activeConn)

  // 6. close の後は接続もタイマーも残らない。
  val nothingActiveWhenClosed: bool =
    closed implies (
      activeConn == NONE
      and pendingReconnects == 0
      and not(watchdogArmed)
      and conns.keys().forall(id => conns.get(id) == Closed)
    )

  val safety: bool = all {
    silentConnectionMustDisconnect,
    atMostOneUnclosedConn,
    atMostOnePendingReconnect,
    noReconnectWhileConnected,
    connectedImpliesValidActiveConn,
    appliedStateMatchesActiveWhenConnected,
    noPrematureWatchdogDisconnect,
    watchdogMatchesActiveConn,
    nothingActiveWhenClosed,
  }

  // ===== 進行の条件（到達できること。--witnesses で数える） =====

  val witnessMessageReceived: bool = messagesReceived > 0
  val witnessWatchdogDisconnect: bool = watchdogDisconnections > 0
  val witnessWakeDisconnect: bool = wakeDisconnects > 0
  val witnessReconnectAndRecover: bool = reconnections > 0 and not(disconnected) and activeConn > 0
  val witnessWatchdogTimerRecover: bool = recoveredAfterWatchdogTimer
  val witnessWakeCheckRecover: bool = recoveredAfterWakeCheck
  val witnessStaleEventIgnored: bool = staleEventsIgnored > 0
  val witnessStreamClosed: bool = closed
}

module current {
  import stateStreamWatchdog(WATCHDOG = false, STALE_GUARD = false, REARM_ON_WAKE = false, WAKE_CUTOFF = 8, ALLOW_CLOSE = true).*
}

module watchdogOnly {
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = false, REARM_ON_WAKE = true, WAKE_CUTOFF = 8, ALLOW_CLOSE = true).*
}

module noWakeRearm {
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true, REARM_ON_WAKE = false, WAKE_CUTOFF = 8, ALLOW_CLOSE = true).*
}

// checkStale が 10 秒（2 目盛り）で誤切断する変異。不変条件 4（noPrematureWatchdogDisconnect）を破る。
module earlyWakeCutoff {
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true, REARM_ON_WAKE = true, WAKE_CUTOFF = 2, ALLOW_CLOSE = true).*
}

module fixed {
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true, REARM_ON_WAKE = true, WAKE_CUTOFF = 8, ALLOW_CLOSE = true).*
}

// タブを閉じずに長く開いたままの状態を探索し、見張りタイマー切断→再接続→復帰の到達も確かめる構成。
module fixedOpenSession {
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true, REARM_ON_WAKE = true, WAKE_CUTOFF = 8, ALLOW_CLOSE = false).*
}
