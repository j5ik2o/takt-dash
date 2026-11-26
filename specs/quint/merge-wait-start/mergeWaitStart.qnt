// -*- mode: Bluespec; -*-
// backend/src/mcp/tools/goal.rs の resolve_awaiting_merge_at が、ゴールのマージ待ちの起点を
// 読み取りごとに確定する遷移のモデル（#90）。1ゴールと、そのゴールの仕上げタスク1件を扱う。
// 仕上げタスクは、無い・未着手・実行中・完了（completedAt あり／なし）の5つに畳む。
// 完了には失敗・上限超過・PR 失敗を含める（どれも pending/running でない終わりの状態）。
module mergeWaitStart {
  // true のとき修正後の規則、false のとき修正前の規則で起点を決める。
  const FIXED: bool
  // 時刻の上限。状態を有限に保つため。
  const MAX_TIME: int

  type Start = NoStart | At(int)
  type Phase = Created | AwaitingMerge
  // takt_list_tasks に載っている、このゴールの仕上げタスク。
  type MergeTask = NoTask | Pending | Running | Done(int) | DoneNoTime

  type Read = {
    // その読み取りで確定した起点。
    start: Start,
    // その読み取りの時点で、ゴールが awaiting_merge だったか、実際に人のマージ待ちだったか。
    awaiting: bool,
    humanWait: bool,
    // その時点の、人の待ちが実際に始まった時刻。
    began: Start,
    // その待ちが、仕上げタスクが終わったことで始まったか。
    afterFinish: bool,
    // backend が、待ちの開始以後の時刻を1つでも持っていたか（下の evidenceAvailable）。
    evidence: bool,
    // その読み取りの観測時刻。
    time: int,
    // 人の待ちの区切りの番号と、backend の再起動の回数。
    period: int,
    restarts: int,
  }

  // 実際の状態（takt の側）。
  var clock: int
  var phase: Phase
  var task: MergeTask
  // このゴールの awaiting_merge 通知のうち、最も新しい recordedAt。
  var notif: Start
  // backend の前回 snapshot の起点（再起動で消える）。
  var prevStart: Start
  // 観測用：人の待ちが実際に始まった時刻。人の待ちでないあいだは NoStart。
  var waitBegan: Start
  // 観測用：いまの人の待ちが、仕上げタスクが終わったことで始まったか。
  var waitAfterFinish: bool
  // 観測用：人の待ちの区切りの番号。待ちが始まる・終わるたびに増える。
  var period: int
  var restarts: int
  // 観測用：直前の読み取りと、その1つ前の読み取り。
  var lastRead: Read
  var beforeLast: Read
  // 観測用：直前の2回の読み取りのあいだに、takt の側の状態か backend の再起動があったか。
  var changedBetween: bool
  var changedSinceRead: bool

  val noRead: Read =
    { start: NoStart, awaiting: false, humanWait: false, began: NoStart, afterFinish: false, evidence: false, time: -1, period: -1, restarts: -1 }

  pure def isActive(t: MergeTask): bool = match t {
    | Pending => true
    | Running => true
    | _ => false
  }

  pure def atLeast(s: Start, b: int): bool = match s {
    | At(t) => t >= b
    | NoStart => false
  }

  pure def later(a: Start, b: Start): Start = match a {
    | NoStart => b
    | At(x) => match b {
      | NoStart => a
      | At(y) => if (x >= y) a else b
    }
  }

  pure def completedAt(t: MergeTask): Start = match t {
    | Done(c) => At(c)
    | _ => NoStart
  }

  val humanWait = phase == AwaitingMerge and not(isActive(task))

  // backend が、いまの人の待ちの開始以後の時刻を1つでも持っているか。前回の起点、仕上げ
  // タスクの completedAt、通知のどれかが開始以後なら持っている。completedAt の無い完了で、
  // 前回の起点も無いときは、規則が観測時刻を使うので持っているとみなす。仕上げタスクの記録が
  // 一覧から消えた（cleanup）うえで再起動したときなどは持っていない。
  val evidenceAvailable = match waitBegan {
    | NoStart => true
    | At(b) =>
      atLeast(prevStart, b) or atLeast(completedAt(task), b) or atLeast(notif, b)
        or (task == DoneNoTime and prevStart == NoStart)
  }

  // 修正前：通知があれば通知、無ければ前回の起点、それも無ければ観測時刻。
  val currentRule: Start =
    if (notif != NoStart) notif
    else if (phase != AwaitingMerge) NoStart
    else if (prevStart != NoStart) prevStart
    else At(clock)

  // 修正後：仕上げタスクが pending/running のあいだは起点を持たない。終わったら、通知・
  // completedAt・前回の起点のうち最も遅いもの。どれも無ければ観測時刻。completedAt の無い
  // 完了で前回の起点も無いときは、観測時刻も候補に入れる。
  val fixedRule: Start =
    if (phase != AwaitingMerge) notif
    else if (isActive(task)) NoStart
    else {
      val upstream = later(later(notif, completedAt(task)), prevStart)
      val withObserved = if (task == DoneNoTime and prevStart == NoStart) later(upstream, At(clock)) else upstream
      if (withObserved == NoStart) At(clock) else withObserved
    }

  val resolved: Start = if (FIXED) fixedRule else currentRule

  action init = all {
    clock' = 0,
    phase' = Created,
    task' = NoTask,
    notif' = NoStart,
    prevStart' = NoStart,
    waitBegan' = NoStart,
    waitAfterFinish' = false,
    period' = 0,
    restarts' = 0,
    lastRead' = noRead,
    beforeLast' = noRead,
    changedBetween' = false,
    changedSinceRead' = false,
  }

  action tick = all {
    clock < MAX_TIME,
    clock' = clock + 1,
    phase' = phase, task' = task, notif' = notif, prevStart' = prevStart, waitBegan' = waitBegan,
    waitAfterFinish' = waitAfterFinish,
    period' = period, restarts' = restarts, lastRead' = lastRead, beforeLast' = beforeLast,
    changedBetween' = changedBetween, changedSinceRead' = changedSinceRead,
  }

  // takt の側の状態の変化。読み取りの基準（prevStart）と観測用の読み取りは変えない。
  action upstream(nextPhase: Phase, nextTask: MergeTask, nextNotif: Start): bool = {
    val nowWaiting = nextPhase == AwaitingMerge and not(isActive(nextTask))
    val began =
      if (not(nowWaiting)) NoStart
      else if (not(humanWait)) At(clock)
      // 待ちのまま仕上げタスクが（読み取りのあいだに）走って終わったら、そこから待ち直す。
      else if (completedAt(nextTask) != completedAt(task) and completedAt(nextTask) != NoStart) completedAt(nextTask)
      else if (nextTask == DoneNoTime and task != DoneNoTime) At(clock)
      else waitBegan
    val afterFinish = if (not(nowWaiting)) false else if (humanWait) waitAfterFinish else isActive(task)
    all {
      phase' = nextPhase,
      task' = nextTask,
      notif' = nextNotif,
      waitBegan' = began,
      waitAfterFinish' = afterFinish,
      period' = if (began != waitBegan) period + 1 else period,
      changedSinceRead' = true,
      clock' = clock, prevStart' = prevStart, restarts' = restarts,
      lastRead' = lastRead, beforeLast' = beforeLast, changedBetween' = changedBetween,
    }
  }

  // ゴールが awaiting_merge に入る。takt は入った時点で awaiting_merge 通知を記録する
  // （仕上げタスクの投入より前。src/infra/goals/integration.ts の finishGoal）。
  action enterAwaitingMerge = all {
    phase == Created,
    upstream(AwaitingMerge, task, At(clock)),
  }

  action enqueueMergeTask = all {
    phase == AwaitingMerge,
    not(isActive(task)),
    upstream(phase, Pending, notif),
  }

  action startMergeTask = all {
    task == Pending,
    upstream(phase, Running, notif),
  }

  action finishMergeTask = all {
    task == Running,
    nondet withTime = Set(true, false).oneOf()
    upstream(phase, if (withTime) Done(clock) else DoneNoTime, notif),
  }

  // 終わったタスクを一覧から外す（takt_cleanup_tasks など）。
  action cleanupTasks = all {
    task == DoneNoTime or completedAt(task) != NoStart,
    upstream(phase, NoTask, notif),
  }

  // ゴールが created へ戻る（reopen）。
  action reopen = all {
    phase == AwaitingMerge,
    not(isActive(task)),
    upstream(Created, task, notif),
  }

  // backend の再起動。前回 snapshot が消える。
  action restart = all {
    prevStart' = NoStart,
    restarts' = restarts + 1,
    changedSinceRead' = true,
    clock' = clock, phase' = phase, task' = task, notif' = notif, waitBegan' = waitBegan,
    waitAfterFinish' = waitAfterFinish,
    period' = period, lastRead' = lastRead, beforeLast' = beforeLast, changedBetween' = changedBetween,
  }

  // backend の読み取り（read_snapshot）。観測時刻は clock。
  action read = all {
    prevStart' = resolved,
    beforeLast' = lastRead,
    lastRead' = {
      start: resolved,
      awaiting: phase == AwaitingMerge,
      humanWait: humanWait,
      began: waitBegan,
      afterFinish: waitAfterFinish,
      evidence: evidenceAvailable,
      time: clock,
      period: period,
      restarts: restarts,
    },
    changedBetween' = changedSinceRead,
    changedSinceRead' = false,
    clock' = clock, phase' = phase, task' = task, notif' = notif, waitBegan' = waitBegan,
    waitAfterFinish' = waitAfterFinish, period' = period, restarts' = restarts,
  }

  action step = any {
    tick,
    enterAwaitingMerge,
    enqueueMergeTask,
    startMergeTask,
    finishMergeTask,
    cleanupTasks,
    reopen,
    restart,
    read,
  }

  // 人の待ちが始まる前の時刻を起点にしない（#90）。backend が待ちの開始以後の時刻を
  // 何も持っていない読み取りは除く（README の「残る取りこぼし」）。
  val notBeforeWait =
    (lastRead.humanWait and lastRead.evidence) implies match lastRead.began {
      | At(b) => atLeast(lastRead.start, b)
      | NoStart => false
    }

  // awaiting_merge でも、仕上げタスクが pending/running のあいだは起点を持たない。
  val noStartWhileFinishing = (lastRead.awaiting and not(lastRead.humanWait)) implies lastRead.start == NoStart

  // 既存の滞留防止：人の待ちが続き、そのあいだに takt の側の変化も再起動も無ければ、
  // 観測時刻が進んでも起点は変わらない。
  val noReset =
    (lastRead.humanWait and beforeLast.humanWait and not(changedBetween)) implies lastRead.start == beforeLast.start

  // 同じ待ちのあいだ、再起動が無ければ起点は前へ戻らない。
  val neverMovesBack =
    (lastRead.humanWait and beforeLast.humanWait and lastRead.period == beforeLast.period
      and lastRead.restarts == beforeLast.restarts) implies match beforeLast.start {
      | At(s) => atLeast(lastRead.start, s)
      | NoStart => true
    }

  // 起点は観測時刻より後にならない。
  val notInFuture = match lastRead.start {
    | At(s) => s <= lastRead.time
    | NoStart => true
  }

  val safety = notBeforeWait and noStartWhileFinishing and noReset and neverMovesBack and notInFuture

  // 進行の確認用：仕上げタスクが終わって始まった待ちで、起点が待ちの開始（completedAt か、
  // completedAt の無い完了では最初の観測時刻以前の時刻）に一致し、そのあと時刻が進んだ読み取りへ届くか。
  // `afterFinish` が、仕上げタスクを経ない待ち（awaiting_merge に入っただけ）と区別する目印です。
  val waitedAfterFinishTask = lastRead.humanWait and lastRead.afterFinish and match lastRead.began {
    | At(b) => lastRead.start == At(b) and b > 0 and lastRead.time > b
    | NoStart => false
  }

  // 残る取りこぼしへ届くか：backend が待ちの開始以後の時刻を何も持っていない読み取り。
  val evidenceLost = lastRead.humanWait and not(lastRead.evidence)
}

module current {
  import mergeWaitStart(FIXED = false, MAX_TIME = 6).*
}

module fixed {
  import mergeWaitStart(FIXED = true, MAX_TIME = 6).*
}
