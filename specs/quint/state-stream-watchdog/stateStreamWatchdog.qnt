// -*- mode: Bluespec; -*-
// 画面の SSE 状態ストリーム（openStateStream）の半開き検知と再接続の状態機械（issue #383）。
//
// 画面は /api/state の EventSource を 1 本だけ開き、backend が 15 秒ごとに送り直す
// DashboardState を受け取る。VPN・Wi-Fi の切り替えやスリープからの復帰で接続が半開きに
// なると onerror が出ず、画面は切断に気づけない。
//
// 時間は 1 目盛り = 5 秒の離散時刻で表す。
// - RESEND_INTERVAL = 3（15 秒。backend の WATCH_RESEND_INTERVAL）
// - STALE_TIMEOUT   = 8（40 秒。2 * 15 秒 + 10 秒の余裕）
//
// 定数による遷移の切り替え:
// - WATCHDOG = false, STALE_GUARD = false（current）: 修正前。無音の見張りも古い接続の無視も無い。
// - WATCHDOG = true,  STALE_GUARD = false（watchdogOnly）: 見張りだけを入れ、古い接続からの
//   遅延 onmessage / onerror を無視しない版。
// - WATCHDOG = true,  STALE_GUARD = true（fixed）: 修正案。無音の見張りとスリープ復帰時の
//   鮮度確認を入れ、現在の接続以外からの受信・エラー・見張り発火をすべて無視する。
module stateStreamWatchdog {
  const WATCHDOG: bool
  const STALE_GUARD: bool

  type ConnStatus = Open | HalfOpen | Closed

  pure val NONE: int = -1
  // 1 目盛り = 5 秒。
  pure val RESEND_INTERVAL: int = 3
  pure val STALE_TIMEOUT: int = 8
  // 状態空間を閉じるための上限。
  pure val MAX_CONNS: int = 4
  pure val MAX_TIME: int = 32
  // スリープ時間の候補（2=10秒、8=40秒、12=60秒）。
  pure val SLEEP_JUMPS: Set[int] = Set(2, 8, 12)

  var closed: bool
  var disconnected: bool
  var activeConn: int
  var nextConn: int
  var conns: int -> ConnStatus
  var pendingReconnects: int
  var watchdogArmed: bool
  var watchdogConn: int
  var lastActivity: int
  var now: int
  var consecutiveFailures: int

  // 性質の検証と到達確認に使う記録。
  var staleStateApplied: bool
  var staleErrorApplied: bool
  var prematureWatchdogDisconnect: bool
  var watchdogDisconnections: int
  var wakeDisconnects: int
  var reconnections: int
  var messagesReceived: int
  var invalidMessages: int
  var staleEventsIgnored: int

  // 現在の接続で見張り期限に達しているか。
  // WATCHDOG = true のとき、時刻は期限を素通りできず、先に watchdogTimeout が発火する。
  val watchdogDue: bool =
    WATCHDOG and watchdogArmed and not(closed) and activeConn != NONE
      and now >= lastActivity + STALE_TIMEOUT

  action init: bool = all {
    closed' = false,
    disconnected' = false,
    activeConn' = 0,
    nextConn' = 1,
    conns' = Map(0 -> Open),
    pendingReconnects' = 0,
    watchdogArmed' = WATCHDOG,
    watchdogConn' = if (WATCHDOG) 0 else NONE,
    lastActivity' = 0,
    now' = 0,
    consecutiveFailures' = 0,
    staleStateApplied' = false,
    staleErrorApplied' = false,
    prematureWatchdogDisconnect' = false,
    watchdogDisconnections' = 0,
    wakeDisconnects' = 0,
    reconnections' = 0,
    messagesReceived' = 0,
    invalidMessages' = 0,
    staleEventsIgnored' = 0,
  }

  // 時刻を 1 目盛り（5 秒）進める。
  // 健全な Open 接続では、backend の再送間隔の余裕（STALE_TIMEOUT 未満）の範囲内までしか
  // 無音のまま時刻は進まない（それを超える無音は goHalfOpen で半開きになった状態として表す）。
  action tick: bool = all {
    not(closed),
    now < MAX_TIME,
    not(watchdogDue),
    (activeConn != NONE and conns.get(activeConn) == Open)
      implies (now + 1 < lastActivity + STALE_TIMEOUT),
    now' = now + 1,
    closed' = closed, disconnected' = disconnected, activeConn' = activeConn,
    nextConn' = nextConn, conns' = conns, pendingReconnects' = pendingReconnects,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    lastActivity' = lastActivity, consecutiveFailures' = consecutiveFailures,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
  }

  // VPN・Wi-Fi の切り替えなどで、現在の接続が onerror を出さずに半開きになる。
  action goHalfOpen: bool = all {
    not(closed),
    activeConn != NONE,
    conns.get(activeConn) == Open,
    conns' = conns.set(activeConn, HalfOpen),
    closed' = closed, disconnected' = disconnected, activeConn' = activeConn,
    nextConn' = nextConn, pendingReconnects' = pendingReconnects,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    lastActivity' = lastActivity, now' = now, consecutiveFailures' = consecutiveFailures,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
  }

  // 現在の健全な接続で有効な DashboardState を受信する。
  // 見張りを張り直し、切断フラグと連続失敗回数を戻す。
  action receiveActiveMessage: bool = all {
    not(closed),
    activeConn != NONE,
    conns.get(activeConn) == Open,
    disconnected' = false,
    consecutiveFailures' = 0,
    lastActivity' = now,
    watchdogArmed' = WATCHDOG,
    watchdogConn' = if (WATCHDOG) activeConn else NONE,
    messagesReceived' = messagesReceived + 1,
    closed' = closed, activeConn' = activeConn, nextConn' = nextConn, conns' = conns,
    pendingReconnects' = pendingReconnects, now' = now,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, invalidMessages' = invalidMessages,
    staleEventsIgnored' = staleEventsIgnored,
  }

  // 現在の接続で検証に失敗する不正なペイロードを受信する。
  // 状態・切断フラグ・連続失敗回数・見張り期限のいずれも更新しない。
  action receiveInvalidMessage: bool = all {
    not(closed),
    activeConn != NONE,
    conns.get(activeConn) == Open,
    invalidMessages' = invalidMessages + 1,
    closed' = closed, disconnected' = disconnected, activeConn' = activeConn,
    nextConn' = nextConn, conns' = conns, pendingReconnects' = pendingReconnects,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    lastActivity' = lastActivity, now' = now, consecutiveFailures' = consecutiveFailures,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    staleEventsIgnored' = staleEventsIgnored,
  }

  // 古い接続または close 後の接続から遅延して onmessage が届く。
  // STALE_GUARD = true なら無視する。false なら切断フラグを誤って false に戻してしまう。
  action receiveStaleMessage(id: int): bool = all {
    conns.keys().contains(id),
    id != activeConn or closed or conns.get(id) == Closed,
    if (STALE_GUARD) all {
      staleEventsIgnored' = staleEventsIgnored + 1,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      lastActivity' = lastActivity,
      watchdogArmed' = watchdogArmed,
      watchdogConn' = watchdogConn,
      staleStateApplied' = staleStateApplied,
    } else all {
      staleEventsIgnored' = staleEventsIgnored,
      disconnected' = false,
      consecutiveFailures' = 0,
      lastActivity' = if (WATCHDOG) now else lastActivity,
      watchdogArmed' = if (WATCHDOG) true else watchdogArmed,
      watchdogConn' = if (WATCHDOG) id else watchdogConn,
      staleStateApplied' = true,
    },
    closed' = closed, activeConn' = activeConn, nextConn' = nextConn, conns' = conns,
    pendingReconnects' = pendingReconnects, now' = now,
    staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages,
  }

  // 現在の接続で onerror が発火する。接続を閉じ、切断として扱い、再接続を 1 本だけ予約する。
  action receiveActiveError: bool = all {
    not(closed),
    activeConn != NONE,
    conns' = conns.set(activeConn, Closed),
    activeConn' = NONE,
    watchdogArmed' = false,
    watchdogConn' = NONE,
    disconnected' = true,
    consecutiveFailures' = consecutiveFailures + 1,
    pendingReconnects' = 1,
    closed' = closed, nextConn' = nextConn, lastActivity' = lastActivity, now' = now,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
  }

  // 古い接続（現在の接続ではない）から遅延または重複して onerror が発火する。
  // STALE_GUARD = true なら無視する。false なら現在の接続を閉じずに再接続を予約してしまう。
  action receiveStaleError(id: int): bool = all {
    not(closed),
    conns.keys().contains(id),
    id != activeConn,
    if (STALE_GUARD) all {
      staleEventsIgnored' = staleEventsIgnored + 1,
      conns' = conns,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      pendingReconnects' = pendingReconnects,
      staleErrorApplied' = staleErrorApplied,
    } else all {
      staleEventsIgnored' = staleEventsIgnored,
      conns' = conns.set(id, Closed),
      disconnected' = true,
      consecutiveFailures' = consecutiveFailures + 1,
      pendingReconnects' = 1,
      staleErrorApplied' = true,
    },
    closed' = closed, activeConn' = activeConn, nextConn' = nextConn,
    watchdogArmed' = watchdogArmed, watchdogConn' = watchdogConn,
    lastActivity' = lastActivity, now' = now,
    staleStateApplied' = staleStateApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages,
  }

  // 無音の見張りタイマーが期限に達して発火する。
  // 現在の接続を閉じ、切断として扱い、再接続を 1 本だけ予約する。
  action watchdogTimeout: bool = all {
    WATCHDOG,
    watchdogArmed,
    not(closed),
    activeConn != NONE,
    now >= lastActivity + STALE_TIMEOUT,
    if (STALE_GUARD and watchdogConn != activeConn) all {
      watchdogArmed' = false,
      watchdogConn' = NONE,
      staleEventsIgnored' = staleEventsIgnored + 1,
      conns' = conns,
      activeConn' = activeConn,
      disconnected' = disconnected,
      consecutiveFailures' = consecutiveFailures,
      pendingReconnects' = pendingReconnects,
      watchdogDisconnections' = watchdogDisconnections,
      prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    } else all {
      conns' = conns.set(activeConn, Closed),
      activeConn' = NONE,
      watchdogArmed' = false,
      watchdogConn' = NONE,
      disconnected' = true,
      consecutiveFailures' = consecutiveFailures + 1,
      pendingReconnects' = 1,
      watchdogDisconnections' = watchdogDisconnections + 1,
      prematureWatchdogDisconnect' = prematureWatchdogDisconnect or (now - lastActivity < STALE_TIMEOUT),
      staleEventsIgnored' = staleEventsIgnored,
    },
    closed' = closed, nextConn' = nextConn, lastActivity' = lastActivity, now' = now,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    wakeDisconnects' = wakeDisconnects, reconnections' = reconnections,
    messagesReceived' = messagesReceived, invalidMessages' = invalidMessages,
  }

  // OS のスリープからの復帰（壁時計が jump だけ進み、visibilitychange / online / focus で
  // checkStale が呼ばれる）。経過が見張り時間以上なら即座に切断して再接続を予約する。
  // 経過が見張り時間未満なら接続を切らない。
  action wakeFromSleep(jump: int): bool = {
    val newNow = now + jump
    val expired = newNow - lastActivity >= STALE_TIMEOUT
    all {
      not(closed),
      newNow <= MAX_TIME,
      activeConn != NONE,
      // 健全な接続のまま見張り時間を超えて眠ることはなく、長いスリープでは半開きになる。
      (conns.get(activeConn) == Open) implies not(expired),
      now' = newNow,
      if (WATCHDOG and expired) all {
        conns' = conns.set(activeConn, Closed),
        activeConn' = NONE,
        watchdogArmed' = false,
        watchdogConn' = NONE,
        disconnected' = true,
        consecutiveFailures' = consecutiveFailures + 1,
        pendingReconnects' = 1,
        wakeDisconnects' = wakeDisconnects + 1,
      } else all {
        conns' = conns,
        activeConn' = activeConn,
        watchdogArmed' = watchdogArmed,
        watchdogConn' = watchdogConn,
        disconnected' = disconnected,
        consecutiveFailures' = consecutiveFailures,
        pendingReconnects' = pendingReconnects,
        wakeDisconnects' = wakeDisconnects,
      },
      closed' = closed, nextConn' = nextConn, lastActivity' = lastActivity,
      staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
      prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
      watchdogDisconnections' = watchdogDisconnections, reconnections' = reconnections,
      messagesReceived' = messagesReceived, invalidMessages' = invalidMessages,
      staleEventsIgnored' = staleEventsIgnored,
    }
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
    lastActivity' = now,
    watchdogArmed' = WATCHDOG,
    watchdogConn' = if (WATCHDOG) nextConn else NONE,
    reconnections' = reconnections + 1,
    closed' = closed, disconnected' = disconnected, now' = now,
    consecutiveFailures' = consecutiveFailures,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    messagesReceived' = messagesReceived, invalidMessages' = invalidMessages,
    staleEventsIgnored' = staleEventsIgnored,
  }

  // 画面のアンマウントなどで stream.close() を呼ぶ。接続と全タイマーを止める。
  action closeStream: bool = all {
    not(closed),
    closed' = true,
    conns' = if (activeConn != NONE) conns.set(activeConn, Closed) else conns,
    activeConn' = NONE,
    pendingReconnects' = 0,
    watchdogArmed' = false,
    watchdogConn' = NONE,
    disconnected' = disconnected, nextConn' = nextConn, lastActivity' = lastActivity,
    now' = now, consecutiveFailures' = consecutiveFailures,
    staleStateApplied' = staleStateApplied, staleErrorApplied' = staleErrorApplied,
    prematureWatchdogDisconnect' = prematureWatchdogDisconnect,
    watchdogDisconnections' = watchdogDisconnections, wakeDisconnects' = wakeDisconnects,
    reconnections' = reconnections, messagesReceived' = messagesReceived,
    invalidMessages' = invalidMessages, staleEventsIgnored' = staleEventsIgnored,
  }

  action step: bool = any {
    tick,
    goHalfOpen,
    receiveActiveMessage,
    receiveInvalidMessage,
    {
      nondet id = conns.keys().oneOf()
      any {
        receiveStaleMessage(id),
        receiveStaleError(id),
      }
    },
    receiveActiveError,
    watchdogTimeout,
    {
      nondet jump = SLEEP_JUMPS.oneOf()
      wakeFromSleep(jump)
    },
    reconnectFire,
    closeStream,
  }

  // ===== 不変条件（安全性） =====

  // 1. 見張りの時間を過ぎたら必ず切断になる。
  val silentConnectionMustDisconnect: bool =
    (not(closed) and activeConn != NONE and now > lastActivity + STALE_TIMEOUT)
      implies disconnected

  // 2a. 開いたままの接続は同時に 1 本まで。
  val atMostOneUnclosedConn: bool =
    conns.keys().filter(id => conns.get(id) != Closed).size() <= 1

  // 2b. 予約中の再接続タイマーは同時に 1 本まで。
  val atMostOnePendingReconnect: bool = pendingReconnects <= 1

  // 2c. 接続が開いている間に再接続タイマーが同時に予約されない。
  val noReconnectWhileConnected: bool =
    activeConn != NONE implies pendingReconnects == 0

  // 3a. 切断後・開き直し後に、古い接続の受信で状態が戻らない。
  val noStaleStateRestore: bool = not(staleStateApplied)

  // 3b. 古い接続のエラーで現在の接続や再接続予約を壊さない。
  val noStaleErrorInterference: bool = not(staleErrorApplied)

  // 4. 見張り時間未満で受信が続いている間は切断にしない。
  val noPrematureWatchdogDisconnect: bool = not(prematureWatchdogDisconnect)

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
    noStaleStateRestore,
    noStaleErrorInterference,
    noPrematureWatchdogDisconnect,
    watchdogMatchesActiveConn,
    nothingActiveWhenClosed,
  }

  // ===== 進行の条件（到達できること。--witnesses で数える） =====

  val witnessMessageReceived: bool = messagesReceived > 0
  val witnessWatchdogDisconnect: bool = watchdogDisconnections > 0
  val witnessWakeDisconnect: bool = wakeDisconnects > 0
  val witnessReconnectAndRecover: bool = reconnections > 0 and not(disconnected) and activeConn > 0
  val witnessStaleEventIgnored: bool = staleEventsIgnored > 0
  val witnessStreamClosed: bool = closed
}

module current {
  import stateStreamWatchdog(WATCHDOG = false, STALE_GUARD = false).*
}

module watchdogOnly {
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = false).*
}

module fixed {
  import stateStreamWatchdog(WATCHDOG = true, STALE_GUARD = true).*
}
