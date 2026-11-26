// -*- mode: Bluespec; -*-
// backend の停止の状態機械（issue #364）。停止の合図から、takt-mcp への接続を閉じ終えて
// プロセスが終わるまで。
//
// 本番の backend は launchd で動き、launchd は SIGTERM の EXIT_TIMEOUT 後に SIGKILL する。
// SIGKILL されると、閉じていないセッションは takt-mcp に残る。停止はその前に終える。
//
// 停止の合図が届いたとき、定期読み取りの周期の途中かもしれない。周期は道具を何回か呼び、
// 1 回ごとの上限があるだけなので、周期の全体は CYCLE_BOUND まで続きうる。
//
// 閉じる対象は 2 種類。
//   - HTTP セッション（レジストリの常駐サーバーごとの置き場と、定期読み取りのプロジェクトごとの
//     読み取りハンドル）。閉じる処理は DELETE を送る。確立の途中なら確立の鍵を待つ。
//     上限は確立の待ちと DELETE を合わせて CLOSE_BOUND。
//   - stdio の子プロセス。閉じる処理はクライアントの鍵を取って kill する。鍵は、要求が道具を
//     呼んでいる間は空かない。待つ長さは道具の呼び出しの上限（OP_BOUND）まで。
//
// 時間は 1 単位を 1 秒とする。応答するサーバーの DELETE と、鍵の空いた子プロセスの kill は、
// 1 単位より短く終わる（終わるまで時間は進まない）。
//
// PARALLEL = false は修正前の遷移。1 件ずつ順に閉じる。true は、すべてを同時に閉じ始める。
// DEADLINE = false は修正前の遷移。上限が無い。true は、閉じる段を SHUTDOWN_DEADLINE で、
// 停止の全体（合図から）を STOP_DEADLINE で打ち切る。打ち切ったものは見捨てる（stdio の子プロセスは、
// プロセスの終了で kill_on_drop により止まる）。
// CANCEL_CYCLE = false は修正前の遷移。周期の途中なら、周期が終わるのを待ってから閉じる。
// true は、周期を CYCLE_GRACE だけ待ち、それでも終わらなければ打ち切って閉じ始める。ふだんの短い
// 周期は最後まで走らせ、その周期の出来事を記録してから止まる。
// ABORT_CYCLE_TASKS = false は、周期を打ち切っても、周期がプロジェクトごとに起動した読み取りの
// タスクは走り続ける遷移（タスクは読み取りハンドルやクライアントの鍵を持ったまま）。true は、
// 周期を打ち切ったらタスクも中断し、鍵を放す遷移。
module shutdownDeadline {
  const PARALLEL: bool
  const DEADLINE: bool
  const CANCEL_CYCLE: bool
  // 合図のあと周期を待つ上限（CANCEL_CYCLE = true のときだけ使う）。修正案は 2 秒。
  const CYCLE_GRACE: int
  const ABORT_CYCLE_TASKS: bool

  pure val CLOSE_BOUND: int = 10       // 確立の待ち 5 秒 ＋ DELETE 5 秒
  pure val OP_BOUND: int = 30          // 道具の呼び出しの上限の例
  pure val CYCLE_BOUND: int = 30       // 周期の全体が続きうる長さの例
  pure val SHORT_CYCLE: int = 2        // ふだんの周期の長さの上限の見込み
  pure val SHUTDOWN_DEADLINE: int = 15 // 閉じる段の上限
  pure val STOP_DEADLINE: int = 18     // 停止の全体（合図から）の上限
  pure val EXIT_TIMEOUT: int = 20      // launchd が SIGKILL するまで（ExitTimeOut の既定）

  // 0〜2 は HTTP セッション、3〜4 は stdio の子プロセス。修正前の順は番号の順。
  pure val HTTP_ITEMS: Set[int] = 0.to(2)
  pure val STDIO_ITEMS: Set[int] = 3.to(4)
  pure val ITEMS: Set[int] = HTTP_ITEMS.union(STDIO_ITEMS)

  type Status =
    | Waiting    // まだ閉じ始めていない
    | Closing    // 閉じている途中（DELETE の応答待ち、または鍵の待ち）
    | Closed     // 閉じた（DELETE が応答した、または kill した）
    | GaveUp     // 自分の上限で打ち切った（応答しないサーバー）
    | Abandoned  // 全体の上限で見捨てた

  // responsive は HTTP のサーバーが応答するか。busy は stdio の鍵を要求が持っているか。
  // wasBusy は停止を始めた時点で鍵が埋まっていたか（不変条件のために残す）。
  // cycleHeld は、合図の時点で途中の周期の読み取りタスクが、この相手の鍵を持っているか。
  type Item = { status: Status, elapsed: int, responsive: bool, busy: bool, wasBusy: bool, cycleHeld: bool }

  type Phase =
    | Running    // 停止の合図の前
    | Draining   // 合図を受けたが、周期の終わりを待っている
    | Stopping   // 閉じる段
    | Exited     // プロセスが終わった

  var phase: Phase
  // 停止の合図からの時間と、閉じる段に入ってからの時間。
  var sinceSignal: int
  var clock: int
  // 合図の時点で周期の途中なら、周期が終わるまでの長さ（0 は周期の外。初期状態で選び、変えない）。
  // 合図から周期を待った時間。
  var cycleLength: int
  var cycleElapsed: int
  // 待っていた周期を、待つ上限（CYCLE_GRACE）で打ち切ったか。
  var cycleCancelled: bool
  var items: int -> Item

  // ---- 純粋な計算 ----

  pure def isHttp(i: int): bool = HTTP_ITEMS.contains(i)

  pure def finished(it: Item): bool = it.status == Closed or it.status == GaveUp or it.status == Abandoned

  // 1 単位より短く終わる処理が残っているか。残っている間は時間を進めません。
  pure def canCompleteNow(i: int, it: Item): bool =
    it.status == Closing and not(it.cycleHeld) and (if (isHttp(i)) it.responsive else not(it.busy))

  // 周期の読み取りタスクが終わる（または中断される）と、持っていた鍵を放す。
  pure def releaseCycleHeld(its: int -> Item): int -> Item = ITEMS.mapBy(i => { ...its.get(i), cycleHeld: false })

  // 順に閉じる遷移で、閉じている途中が無ければ、番号の最も小さい待ちを閉じ始める。
  pure def startNext(its: int -> Item): int -> Item = {
    val closing = ITEMS.exists(i => its.get(i).status == Closing)
    val waiting = ITEMS.filter(i => its.get(i).status == Waiting)
    if (PARALLEL or closing or waiting.size() == 0) its
    else {
      val first = waiting.fold(100, (m, i) => if (i < m) i else m)
      its.set(first, { ...its.get(first), status: Closing })
    }
  }

  pure def startClosing(its: int -> Item): int -> Item =
    if (PARALLEL) ITEMS.mapBy(i => { ...its.get(i), status: Closing }) else startNext(its)

  // 時間を 1 単位進める。上限に達した HTTP は打ち切り、上限に達した道具の呼び出しは鍵を放す。
  // 道具の呼び出しは、合図の前から続いているものとして、合図からの時間で数えます。
  pure def tickItem(i: int, it: Item): Item =
    if (isHttp(i)) {
      // 周期の読み取りタスクが鍵を持っている間は、閉じる処理は鍵を待ち、DELETE をまだ送りません。
      if (it.status != Closing or it.cycleHeld) it
      else if (it.elapsed + 1 >= CLOSE_BOUND) { ...it, status: GaveUp, elapsed: it.elapsed + 1 }
      else { ...it, elapsed: it.elapsed + 1 }
    } else {
      if (it.busy and it.elapsed + 1 >= OP_BOUND) { ...it, busy: false, elapsed: it.elapsed + 1 }
      else { ...it, elapsed: it.elapsed + 1 }
    }

  pure def abandonRest(its: int -> Item): int -> Item =
    ITEMS.mapBy(i => {
      val it = its.get(i)
      if (finished(it)) it else { ...it, status: Abandoned }
    })

  // ---- 状態機械 ----

  action init = {
    nondet responsive = HTTP_ITEMS.powerset().oneOf()
    nondet busy = STDIO_ITEMS.powerset().oneOf()
    nondet cycle = 0.to(CYCLE_BOUND).oneOf()
    nondet held = ITEMS.powerset().oneOf()
    all {
      phase' = Running,
      sinceSignal' = 0,
      clock' = 0,
      cycleLength' = cycle,
      cycleCancelled' = false,
      cycleElapsed' = 0,
      items' = ITEMS.mapBy(i => {
        status: Waiting, elapsed: 0,
        responsive: responsive.contains(i), busy: busy.contains(i), wasBusy: busy.contains(i),
        cycleHeld: cycle > 0 and held.contains(i),
      }),
    }
  }

  // 停止の合図を受ける。周期の途中で、周期を打ち切らない遷移なら、周期の終わりを待つ。
  action signal = all {
    phase == Running,
    if (cycleLength > 0) all {
      phase' = Draining, items' = items,
    } else all {
      phase' = Stopping, items' = startClosing(items),
    },
    sinceSignal' = 0, clock' = 0, cycleElapsed' = cycleElapsed, cycleLength' = cycleLength, cycleCancelled' = cycleCancelled,
  }

  // 待っていた周期が終わり、閉じる段に入る。
  action cycleEnds = all {
    phase == Draining,
    cycleElapsed == cycleLength,
    phase' = Stopping,
    cycleLength' = cycleLength, cycleCancelled' = cycleCancelled,
    items' = startClosing(items),
    sinceSignal' = sinceSignal, clock' = 0, cycleElapsed' = cycleElapsed,
  }

  // 周期を待つ上限に達し、周期を打ち切って閉じる段に入る。
  action cancelCycle = all {
    CANCEL_CYCLE,
    phase == Draining,
    cycleElapsed == CYCLE_GRACE,
    cycleElapsed < cycleLength,
    phase' = Stopping,
    cycleCancelled' = true,
    items' = startClosing(if (ABORT_CYCLE_TASKS) releaseCycleHeld(items) else items),
    sinceSignal' = sinceSignal, clock' = 0, cycleElapsed' = cycleElapsed, cycleLength' = cycleLength,
  }

  // 応答するサーバーの DELETE が返る、または鍵の空いた子プロセスを kill する。
  action complete(i: int): bool = all {
    phase == Stopping,
    canCompleteNow(i, items.get(i)),
    items' = startNext(items.set(i, { ...items.get(i), status: Closed })),
    phase' = phase, sinceSignal' = sinceSignal, clock' = clock,
    cycleLength' = cycleLength, cycleCancelled' = cycleCancelled, cycleElapsed' = cycleElapsed,
  }

  // 子プロセスの鍵を持つ要求が、上限より前に道具の呼び出しを終える。
  action operationEnds(i: int): bool = all {
    phase == Draining or phase == Stopping,
    STDIO_ITEMS.contains(i),
    items.get(i).busy,
    items' = items.set(i, { ...items.get(i), busy: false }),
    phase' = phase, sinceSignal' = sinceSignal, clock' = clock,
    cycleLength' = cycleLength, cycleCancelled' = cycleCancelled, cycleElapsed' = cycleElapsed,
  }

  // 時間が 1 単位進む。待っている周期は cycleLength で終わる。上限に達したら、残りを見捨てて終える。
  action tick = {
    // 周期の読み取りタスクは、中断されない限り cycleLength まで走り、終われば鍵を放す。
    val tasksRunning = cycleElapsed < cycleLength and not(cycleCancelled and ABORT_CYCLE_TASKS)
    val nextCycleElapsed = if (tasksRunning) cycleElapsed + 1 else cycleElapsed
    val stepped = ITEMS.mapBy(i => tickItem(i, items.get(i)))
    val tickedItems = if (tasksRunning and nextCycleElapsed >= cycleLength) releaseCycleHeld(stepped) else stepped
    // 閉じる段の前（周期を待っている間）は、閉じ始めません。
    val ticked = if (phase == Stopping) startNext(tickedItems) else tickedItems
    val nextClock = if (phase == Stopping) clock + 1 else clock
    val overDeadline =
      DEADLINE and (sinceSignal + 1 >= STOP_DEADLINE or (phase == Stopping and nextClock >= SHUTDOWN_DEADLINE))
    all {
      phase == Draining or phase == Stopping,
      phase == Draining implies (cycleElapsed < cycleLength and (not(CANCEL_CYCLE) or cycleElapsed < CYCLE_GRACE)),
      phase == Stopping implies not(ITEMS.exists(i => canCompleteNow(i, items.get(i)))),
      phase == Stopping implies ITEMS.exists(i => not(finished(items.get(i)))),
      sinceSignal' = sinceSignal + 1,
      clock' = nextClock,
      cycleElapsed' = nextCycleElapsed,
      cycleLength' = cycleLength, cycleCancelled' = cycleCancelled,
      if (overDeadline) all {
        items' = abandonRest(ticked),
        phase' = Exited,
      } else all {
        items' = ticked,
        phase' = phase,
      },
    }
  }

  // すべて閉じ終えて、プロセスが終わる。
  action finishStop = all {
    phase == Stopping,
    ITEMS.forall(i => finished(items.get(i))),
    phase' = Exited,
    sinceSignal' = sinceSignal, clock' = clock, items' = items,
    cycleLength' = cycleLength, cycleCancelled' = cycleCancelled, cycleElapsed' = cycleElapsed,
  }

  action step = {
    nondet i = ITEMS.oneOf()
    any { signal, cycleEnds, cancelCycle, complete(i), operationEnds(i), tick, finishStop }
  }

  // ---- 不変条件 ----

  // launchd が SIGKILL する前に、プロセスが終わる。
  val stopBeforeExitTimeout: bool = sinceSignal < EXIT_TIMEOUT

  // 閉じる段は SHUTDOWN_DEADLINE を超えない。
  val closesWithinDeadline: bool = clock <= SHUTDOWN_DEADLINE

  // 応答するサーバーのセッションは、プロセスが終わるまでに閉じる。
  val respondingSessionsClosed: bool =
    phase == Exited implies HTTP_ITEMS.forall(i => items.get(i).responsive implies items.get(i).status == Closed)

  // 停止の合図の時点で鍵の空いていた子プロセスは、プロセスが終わるまでに kill する。
  val idleChildrenKilled: bool =
    phase == Exited implies STDIO_ITEMS.forall(i => not(items.get(i).wasBusy) implies items.get(i).status == Closed)

  // 見捨てるのは、上限に達したときだけ。
  val abandonOnlyAtDeadline: bool =
    ITEMS.exists(i => items.get(i).status == Abandoned)
      implies (DEADLINE and (clock == SHUTDOWN_DEADLINE or sinceSignal == STOP_DEADLINE))

  // ふだんの短い周期（SHORT_CYCLE 以下）は打ち切らない（その周期の出来事は記録してから止まる）。
  val shortCyclesComplete: bool = cycleCancelled implies cycleLength > SHORT_CYCLE

  val safety: bool =
    stopBeforeExitTimeout and closesWithinDeadline and respondingSessionsClosed and idleChildrenKilled
      and abandonOnlyAtDeadline and shortCyclesComplete

  // ---- 進行の条件（--witnesses） ----

  val stoppedWithEverythingClosed: bool = phase == Exited and ITEMS.forall(i => items.get(i).status == Closed)
  val unresponsiveServerGaveUp: bool = phase == Exited and ITEMS.exists(i => items.get(i).status == GaveUp)
  val busyChildAbandoned: bool = phase == Exited and ITEMS.exists(i => items.get(i).status == Abandoned)
  val busyChildKilledAfterOperation: bool =
    phase == Exited and STDIO_ITEMS.exists(i => items.get(i).wasBusy and items.get(i).status == Closed)
  // 周期の途中で合図を受け、応答する相手をすべて閉じて終える。
  val stoppedDuringACycle: bool = phase == Exited and cycleLength > 0 and respondingSessionsClosed
  // 短い周期を最後まで走らせてから閉じて終える。
  val shortCycleCompletedBeforeStop: bool =
    phase == Exited and cycleLength > 0 and cycleLength <= SHORT_CYCLE and not(cycleCancelled)
  // 長い周期を打ち切って閉じて終える。
  val longCycleCancelled: bool = phase == Exited and cycleCancelled and respondingSessionsClosed
  // 周期の読み取りタスクが鍵を持っていた応答する相手を、周期を打ち切ったあとで閉じる。
  val cycleHeldSessionClosedAfterCancel: bool =
    phase == Exited and cycleCancelled and cycleLength > SHUTDOWN_DEADLINE + CYCLE_GRACE
      and HTTP_ITEMS.exists(i => items.get(i).responsive and items.get(i).status == Closed)
}

module current {
  import shutdownDeadline(PARALLEL = false, DEADLINE = false, CANCEL_CYCLE = false, CYCLE_GRACE = 2, ABORT_CYCLE_TASKS = false).*
}

// 上限だけを入れた遷移。
module deadlineOnly {
  import shutdownDeadline(PARALLEL = false, DEADLINE = true, CANCEL_CYCLE = false, CYCLE_GRACE = 2, ABORT_CYCLE_TASKS = false).*
}

// 同時に閉じるだけの遷移。
module parallelOnly {
  import shutdownDeadline(PARALLEL = true, DEADLINE = false, CANCEL_CYCLE = false, CYCLE_GRACE = 2, ABORT_CYCLE_TASKS = false).*
}

// 同時に閉じて上限で打ち切るが、周期の途中なら周期を待つ遷移（設計の最初の版）。
module withoutCycleCancel {
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = false, CYCLE_GRACE = 2, ABORT_CYCLE_TASKS = false).*
}

// 合図で周期をすぐ打ち切る遷移（設計の 2 番目の版）。
module immediateCancel {
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = true, CYCLE_GRACE = 0, ABORT_CYCLE_TASKS = true).*
}

// 2 秒待って周期を打ち切るが、周期の読み取りタスクを中断しない遷移（PR の最初の版）。
module withoutTaskAbort {
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = true, CYCLE_GRACE = 2, ABORT_CYCLE_TASKS = false).*
}

module fixed {
  import shutdownDeadline(PARALLEL = true, DEADLINE = true, CANCEL_CYCLE = true, CYCLE_GRACE = 2, ABORT_CYCLE_TASKS = true).*
}
