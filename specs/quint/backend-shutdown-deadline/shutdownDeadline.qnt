// -*- mode: Bluespec; -*-
// backend の停止で、takt-mcp への接続を閉じる段の状態機械（issue #364）。
//
// 閉じる対象は 2 種類。
//   - HTTP セッション（レジストリの常駐サーバーごとの置き場と、定期読み取りのプロジェクトごとの
//     読み取りハンドル）。閉じる処理は DELETE を送る。確立の途中なら確立の鍵を待つ。
//     上限は確立の待ち（10 秒）と DELETE（5 秒）で、合わせて CLOSE_BOUND。
//   - stdio の子プロセス。閉じる処理はクライアントの鍵を取って kill する。鍵は、画面の読み書きの
//     要求が道具を呼んでいる間は空かない。待つ長さは道具の呼び出しの上限（OP_BOUND）まで。
//
// 時間は 1 単位を 5 秒とする。応答するサーバーの DELETE と、鍵が空いている子プロセスの kill は、
// 1 単位より短く終わる（終わるまで時間は進まない）。
//
// PARALLEL = false は修正前の遷移。1 件ずつ順に閉じる。
// PARALLEL = true は、すべてを同時に閉じ始める遷移。
// DEADLINE = false は修正前の遷移。全体の上限が無く、すべて終わるまで待つ。
// DEADLINE = true は、停止を始めてから SHUTDOWN_DEADLINE で打ち切る遷移。打ち切ったものは
// 見捨てる（stdio の子プロセスは、プロセスの終了で kill_on_drop により止まる）。
module shutdownDeadline {
  const PARALLEL: bool
  const DEADLINE: bool

  pure val CLOSE_BOUND: int = 3       // 15 秒
  pure val OP_BOUND: int = 6          // 30 秒（道具の呼び出しの上限の例）
  pure val SHUTDOWN_DEADLINE: int = 4 // 20 秒

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
  type Item = { status: Status, elapsed: int, responsive: bool, busy: bool, wasBusy: bool }

  type Phase = Running | Stopping | Exited

  var phase: Phase
  var clock: int
  var items: int -> Item

  // ---- 純粋な計算 ----

  pure def isHttp(i: int): bool = HTTP_ITEMS.contains(i)

  pure def finished(it: Item): bool = it.status == Closed or it.status == GaveUp or it.status == Abandoned

  // 1 単位より短く終わる処理が残っているか。残っている間は時間を進めません。
  pure def canCompleteNow(i: int, it: Item): bool =
    it.status == Closing and (if (isHttp(i)) it.responsive else not(it.busy))

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

  // 時間を 1 単位進める。上限に達した HTTP は打ち切り、上限に達した道具の呼び出しは鍵を放す。
  pure def tickItem(i: int, it: Item): Item =
    if (it.status != Closing) it
    else {
      val elapsed = it.elapsed + 1
      if (isHttp(i)) {
        if (elapsed >= CLOSE_BOUND) { ...it, status: GaveUp, elapsed: elapsed } else { ...it, elapsed: elapsed }
      } else {
        if (elapsed >= OP_BOUND) { ...it, busy: false, elapsed: elapsed } else { ...it, elapsed: elapsed }
      }
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
    all {
      phase' = Running,
      clock' = 0,
      items' = ITEMS.mapBy(i => {
        status: Waiting, elapsed: 0,
        responsive: responsive.contains(i), busy: busy.contains(i), wasBusy: busy.contains(i),
      }),
    }
  }

  // 停止の合図を受け、閉じる段に入る。
  action beginStop = all {
    phase == Running,
    phase' = Stopping,
    clock' = 0,
    items' = if (PARALLEL) ITEMS.mapBy(i => { ...items.get(i), status: Closing }) else startNext(items),
  }

  // 応答するサーバーの DELETE が返る、または鍵の空いた子プロセスを kill する。
  action complete(i: int): bool = all {
    phase == Stopping,
    canCompleteNow(i, items.get(i)),
    items' = startNext(items.set(i, { ...items.get(i), status: Closed })),
    phase' = phase, clock' = clock,
  }

  // 子プロセスの鍵を持つ要求が、上限より前に道具の呼び出しを終える。
  action operationEnds(i: int): bool = all {
    phase == Stopping,
    STDIO_ITEMS.contains(i),
    items.get(i).busy,
    items' = items.set(i, { ...items.get(i), busy: false }),
    phase' = phase, clock' = clock,
  }

  // 時間が 1 単位進む。全体の上限に達したら、残りを見捨てて終える。
  action tick = {
    val ticked = startNext(ITEMS.mapBy(i => tickItem(i, items.get(i))))
    all {
      phase == Stopping,
      not(ITEMS.exists(i => canCompleteNow(i, items.get(i)))),
      ITEMS.exists(i => not(finished(items.get(i)))),
      clock' = clock + 1,
      if (DEADLINE and clock + 1 >= SHUTDOWN_DEADLINE) all {
        items' = abandonRest(ticked),
        phase' = Exited,
      } else all {
        items' = ticked,
        phase' = phase,
      },
    }
  }

  // すべて閉じ終えて、停止の段を終える。
  action finishStop = all {
    phase == Stopping,
    ITEMS.forall(i => finished(items.get(i))),
    phase' = Exited,
    clock' = clock, items' = items,
  }

  action step = {
    nondet i = ITEMS.oneOf()
    any { beginStop, complete(i), operationEnds(i), tick, finishStop }
  }

  // ---- 不変条件 ----

  // 閉じる段は、停止を始めてから SHUTDOWN_DEADLINE を超えない。
  val stopWithinDeadline: bool = clock <= SHUTDOWN_DEADLINE

  // 応答するサーバーのセッションは、停止の段を終えるまでに閉じる。
  val respondingSessionsClosed: bool =
    phase == Exited implies HTTP_ITEMS.forall(i => items.get(i).responsive implies items.get(i).status == Closed)

  // 停止を始めた時点で鍵の空いていた子プロセスは、停止の段を終えるまでに kill する。
  val idleChildrenKilled: bool =
    phase == Exited implies STDIO_ITEMS.forall(i => not(items.get(i).wasBusy) implies items.get(i).status == Closed)

  // 見捨てるのは、全体の上限に達したときだけ。
  val abandonOnlyAtDeadline: bool =
    ITEMS.exists(i => items.get(i).status == Abandoned) implies (DEADLINE and clock == SHUTDOWN_DEADLINE)

  val safety: bool = stopWithinDeadline and respondingSessionsClosed and idleChildrenKilled and abandonOnlyAtDeadline

  // ---- 進行の条件（--witnesses） ----

  val stoppedWithEverythingClosed: bool = phase == Exited and ITEMS.forall(i => items.get(i).status == Closed)
  val unresponsiveServerGaveUp: bool = phase == Exited and ITEMS.exists(i => items.get(i).status == GaveUp)
  val busyChildAbandoned: bool = phase == Exited and ITEMS.exists(i => items.get(i).status == Abandoned)
  val busyChildKilledAfterOperation: bool =
    phase == Exited and STDIO_ITEMS.exists(i => items.get(i).wasBusy and items.get(i).status == Closed)
}

module current {
  import shutdownDeadline(PARALLEL = false, DEADLINE = false).*
}

// 全体の上限だけを入れた遷移。
module deadlineOnly {
  import shutdownDeadline(PARALLEL = false, DEADLINE = true).*
}

// 同時に閉じるだけの遷移。
module parallelOnly {
  import shutdownDeadline(PARALLEL = true, DEADLINE = false).*
}

module fixed {
  import shutdownDeadline(PARALLEL = true, DEADLINE = true).*
}
