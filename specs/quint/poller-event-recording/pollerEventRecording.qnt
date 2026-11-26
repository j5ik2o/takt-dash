// -*- mode: Bluespec; -*-
// backend/src/poller.rs の、接続断の出来事の記録と比較基準の進め方のモデル。
// 1プロジェクトだけを扱う。遅延（Delayed）としきい値は省き、読み取りの失敗は
// すぐ応答なしへ跨ぐものとする。記録の照会の保留（seed_pending）と再起動は扱わない。
module pollerEventRecording {
  // true のとき、記録に失敗した接続断を保持して再試行する（修正後）。
  // false のとき、復旧後の差分検出だけに頼る（修正前）。
  const HOLD_UNRECORDED_LOST: bool

  type Conn = Connected | Unresponsive

  var mcpUp: bool
  var dbUp: bool
  // 状態ストアの接続状態（表示用。失敗のたびに進む）。
  var store: Conn
  // 最後に記録へ成功した接続状態（Poller::notified）。
  var notified: Conn
  // 記録に失敗した接続断を保持しているか（Poller::unrecorded_lost）。
  var held: bool
  // いまの読取先の世代で、接続断を検出した回数・記録した回数・復帰を記録した回数。
  var detected: int
  var recordedLost: int
  var recordedRestored: int

  action init = all {
    mcpUp' = true,
    dbUp' = true,
    store' = Connected,
    notified' = Connected,
    held' = false,
    detected' = "type_error",
    recordedLost' = 0,
    recordedRestored' = 0,
  }

  action setMcp(up: bool): bool = all {
    mcpUp' = up,
    dbUp' = dbUp, store' = store, notified' = notified, held' = held,
    detected' = detected, recordedLost' = recordedLost, recordedRestored' = recordedRestored,
  }

  action setDb(up: bool): bool = all {
    dbUp' = up,
    mcpUp' = mcpUp, store' = store, notified' = notified, held' = held,
    detected' = detected, recordedLost' = recordedLost, recordedRestored' = recordedRestored,
  }

  // 読み取りに失敗した周期（Poller::apply_failure）。
  action failedCycle: bool = {
    val redetected = notified != Unresponsive
    val hasLost = redetected or (HOLD_UNRECORDED_LOST and held)
    val recordOk = not(hasLost) or dbUp
    all {
      not(mcpUp),
      store' = Unresponsive,
      detected' = if (store == Connected and redetected) detected + 1 else detected,
      notified' = if (recordOk) Unresponsive else notified,
      recordedLost' = if (hasLost and recordOk) recordedLost + 1 else recordedLost,
      held' = if (recordOk) false else (HOLD_UNRECORDED_LOST and hasLost),
      recordedRestored' = recordedRestored,
      mcpUp' = mcpUp, dbUp' = dbUp,
    }
  }

  // 読み取りに成功した周期（Poller::read_cycle_once_at の成功側）。
  action succeededCycle: bool = {
    val retried = HOLD_UNRECORDED_LOST and held
    val restored = retried or notified == Unresponsive
    val recordOk = not(retried or restored) or dbUp
    all {
      mcpUp,
      // 記録に失敗した周期は、状態ストアも比較基準も進めない。
      store' = if (recordOk) Connected else store,
      notified' = if (recordOk) Connected else notified,
      held' = if (recordOk) false else held,
      recordedLost' = if (retried and recordOk) recordedLost + 1 else recordedLost,
      recordedRestored' = if (restored and recordOk) recordedRestored + 1 else recordedRestored,
      detected' = detected,
      mcpUp' = mcpUp, dbUp' = dbUp,
    }
  }

  // 読取先の作り直し（Poller::reset_project_baseline）。前の世代の勘定を捨てる。
  // 修正前は保持そのものが無いので、held は常に false のまま。
  action rebuild: bool = all {
    store' = Connected,
    notified' = Connected,
    held' = false,
    detected' = "type_error",
    recordedLost' = 0,
    recordedRestored' = 0,
    mcpUp' = mcpUp, dbUp' = dbUp,
  }

  action step = any {
    nondet up = Set(true, false).oneOf()
    setMcp(up),
    nondet up = Set(true, false).oneOf()
    setDb(up),
    failedCycle,
    succeededCycle,
    rebuild,
  }

  // 検出した接続断を、検出した回数より多く記録しない（重複と、前の世代の混入が無い）。
  val atMostOnce = recordedLost <= detected

  // 復帰は、記録済みの接続断に対応する分だけ記録する。
  val restoredIsPaired = recordedRestored <= recordedLost

  // 未記録の接続断があるなら、それを後で記録する手がかりが残っている。
  // 手がかりは、保持している出来事か、再検出できる差分（基準が接続中で状態が応答なし）。
  val noLoss = detected > recordedLost implies
    (held or (store == Unresponsive and notified == Connected))

  val safety = atMostOnce and restoredIsPaired and noLoss

  // 進行の確認用。記録の失敗を経た接続断が、復旧後に記録済みになる状態へ届くか。
  val recoveredAndRecorded =
    detected == 1 and recordedLost == 1 and recordedRestored == 1 and store == Connected
}

module current {
  import pollerEventRecording(HOLD_UNRECORDED_LOST = false).*
}

module fixed {
  import pollerEventRecording(HOLD_UNRECORDED_LOST = true).*
}
