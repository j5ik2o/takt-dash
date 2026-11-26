// -*- mode: Bluespec; -*-
// run 詳細の取得（GetRunDetail）の期限と、stdio の画面読み取りレーンの起動・道具呼び出しの打ち切り
// （issue #59 と #180）。
//
// 画面は 1 つで、run 詳細の要求を順に出す（前の要求が終わってから次を出す。画面の自動更新と
// 手での再読み込みに当たる）。要求は、保存場所から読めない run の回り道（takt_get_run を
// CALLS 回呼ぶ）を通る。読み取りレーンは 1 本で、子プロセスは 1 つ。読み取り（起動と初期化、
// 道具の呼び出し）を「op」と呼び、レーンの鍵を持つ op は同時に 1 つだけ。
//
// 時間の 1 単位は 1 秒。成功する初期化は INIT_TIMEOUT 未満、成功する道具の呼び出しは
// CALL_TIMEOUT 未満で終わる（期限ちょうどで終わるものは期限切れ）。初期化と呼び出しの失敗は
// モデルに入れない（失敗の扱いは変えないため）。
//
// DETACH で、要求が終わった（期限で打ち切られた）ときに op をどうするかを切り替える。
//   "none"：修正前。op は要求の future の中にあり、打ち切りで捨てられる。初期化中なら子プロセスは
//           kill_on_drop で止まり、呼び出し中なら読みかけの子プロセスを捨てる（どちらも次の要求が
//           起動し直す）。
//   "init"：初期化だけを要求から切り離す（設計の途中で考えた案）。初期化は最後まで進むが、
//           呼び出し中の打ち切りでは子プロセスを捨てる。
//   "op"  ：修正案。op 全体（鍵の取得、起動と初期化、道具の呼び出し）を要求から切り離して最後まで
//           走らせる。要求は結果を待つのをやめるだけ。
// BUDGET は backend の総上限（0 は上限なし）。WEB_DEADLINE は Web の gRPC の deadline。
// CHECK_WAITER は、DETACH = "op" で切り離したタスクが鍵を待つ間に、要求がまだ待っているかを
// 確かめるか。確かめないと、要求が諦めた後もタスクは鍵の待ちに残り、鍵が空いてから、誰も
// 待っていない読み取りを流す（設計レビューの指摘 1）。確かめれば、要求が終わった時点で鍵の待ちを
// やめる。鍵を取って読み取りを始めた後は、要求が終わっても最後まで進める（issue #180）。
module runDetailDeadline {
  const WEB_DEADLINE: int
  const BUDGET: int
  const DETACH: str
  const CHECK_WAITER: bool

  pure val INIT_TIMEOUT: int = 60
  pure val CALL_TIMEOUT: int = 15
  pure val CALLS: int = 3
  pure val MAX_REQUESTS: int = 3

  type OpPhase = OpNone | OpInit | OpCalling

  type Outcome = { id: int, result: str, elapsed: int, foundReady: bool }

  var now: int
  // 子プロセスが初期化を終えて使える状態か。
  var procReady: bool
  // 子プロセスを起動した回数。
  var spawns: int
  // 要求の打ち切りで、子プロセス（初期化中・呼び出し中）を捨てたことがあるか。
  var killedByCancel: bool
  var opPhase: OpPhase
  // op の次の出来事（初期化の終わり、呼び出しの終わり）の時刻。
  var opEventAt: int
  var opCallsLeft: int
  // op を待つ要求がもういない（要求が先に終わった）か。
  var opOrphan: bool
  var reqCount: int
  var reqActive: bool
  var reqStartedAt: int
  // 要求が、前の要求から切り離された op の終わりを待っている（レーンの鍵を待つ）か。
  var reqQueued: bool
  // 要求の始めに、子プロセスが使える状態で、レーンも空いていたか。
  var reqFoundReady: bool
  var outcomes: List[Outcome]
  // 鍵を待っているタスク（待っている要求のタスクと、要求に諦められたタスク）の数。
  var waitingTasks: int
  // 要求に諦められた後で、鍵を取って読み取りを始めたタスクの数。
  var abandonedOps: int

  // ---- 補助 ----

  pure def min2(a: int, b: int): int = if (a < b) a else b

  val reqId: int = reqCount - 1

  // 要求の op を始める。子プロセスが使えれば最初の呼び出しを、使えなければ起動と初期化を始める。
  action beginOp(initD: int, callD: int): bool = all {
    if (procReady) all {
      opPhase' = OpCalling,
      opEventAt' = now + callD,
      opCallsLeft' = CALLS,
      spawns' = spawns,
    } else all {
      opPhase' = OpInit,
      opEventAt' = now + initD,
      opCallsLeft' = CALLS,
      spawns' = spawns + 1,
    },
    opOrphan' = false,
  }

  // 要求を終える。結果を記録し、op への打ち切りの効果を DETACH に従って与える。
  action endRequest(result: str): bool = all {
    reqActive' = false,
    reqQueued' = false,
    reqStartedAt' = reqStartedAt,
    reqFoundReady' = reqFoundReady,
    reqCount' = reqCount,
    outcomes' = outcomes.append({ id: reqId, result: result, elapsed: now - reqStartedAt, foundReady: reqFoundReady }),
    now' = now,
    spawns' = spawns,
    opEventAt' = opEventAt,
    opCallsLeft' = opCallsLeft,
    // 鍵を待っていた要求が終わると、そのタスクは鍵の待ちをやめる。ただし、切り離したタスクが
    // 要求を確かめない版では、タスクは鍵の待ちに残る。
    waitingTasks' = if (reqQueued and not(DETACH == "op" and not(CHECK_WAITER))) waitingTasks - 1 else waitingTasks,
    abandonedOps' = abandonedOps,
    // 鍵を待っていた要求の終わりは、前の要求の op に触れない。
    if (reqQueued or opPhase == OpNone or result == "Ok") all {
      opPhase' = opPhase,
      opOrphan' = opOrphan,
      procReady' = procReady,
      killedByCancel' = killedByCancel,
    } else if (DETACH == "op" or (DETACH == "init" and opPhase == OpInit)) all {
      opPhase' = opPhase,
      opOrphan' = true,
      procReady' = procReady,
      killedByCancel' = killedByCancel,
    } else all {
      // 初期化中・呼び出し中の子プロセスを捨てる。
      opPhase' = OpNone,
      opOrphan' = false,
      procReady' = false,
      killedByCancel' = true,
    },
  }

  // ---- 遷移 ----

  action startRequest(initD: int, callD: int): bool = all {
    not(reqActive),
    reqCount < MAX_REQUESTS,
    reqActive' = true,
    reqCount' = reqCount + 1,
    reqStartedAt' = now,
    reqFoundReady' = (procReady and opPhase == OpNone and waitingTasks == 0),
    outcomes' = outcomes,
    now' = now,
    killedByCancel' = killedByCancel,
    abandonedOps' = abandonedOps,
    if (opPhase == OpNone and waitingTasks == 0) all {
      reqQueued' = false,
      procReady' = procReady,
      waitingTasks' = waitingTasks,
      beginOp(initD, callD),
    } else all {
      reqQueued' = true,
      waitingTasks' = waitingTasks + 1,
      procReady' = procReady,
      opPhase' = opPhase,
      opEventAt' = opEventAt,
      opCallsLeft' = opCallsLeft,
      opOrphan' = opOrphan,
      spawns' = spawns,
    },
  }

  // 前の要求から切り離された op が終わり、鍵が空いたので、待っていた要求の op を始める。
  // 鍵の待ちは先着順なので、諦められたタスクが先に並んでいれば、そちらが先に鍵を取る。
  action dequeue(initD: int, callD: int): bool = all {
    reqActive,
    reqQueued,
    opPhase == OpNone,
    waitingTasks == 1,
    waitingTasks' = 0,
    abandonedOps' = abandonedOps,
    reqQueued' = false,
    reqActive' = reqActive,
    reqCount' = reqCount,
    reqStartedAt' = reqStartedAt,
    reqFoundReady' = reqFoundReady,
    outcomes' = outcomes,
    now' = now,
    killedByCancel' = killedByCancel,
    procReady' = procReady,
    beginOp(initD, callD),
  }

  // 要求に諦められたのに鍵の待ちに残ったタスクが、鍵を取って読み取りを始める（CHECK_WAITER が
  // 偽の版だけで起きる）。
  action abandonedDequeue(initD: int, callD: int): bool = all {
    opPhase == OpNone,
    waitingTasks > (if (reqActive and reqQueued) 1 else 0),
    waitingTasks' = waitingTasks - 1,
    abandonedOps' = abandonedOps + 1,
    if (procReady) all {
      opPhase' = OpCalling,
      opEventAt' = now + callD,
      spawns' = spawns,
    } else all {
      opPhase' = OpInit,
      opEventAt' = now + initD,
      spawns' = spawns + 1,
    },
    opCallsLeft' = CALLS,
    opOrphan' = true,
    procReady' = procReady,
    killedByCancel' = killedByCancel,
    now' = now,
    reqActive' = reqActive,
    reqQueued' = reqQueued,
    reqCount' = reqCount,
    reqStartedAt' = reqStartedAt,
    reqFoundReady' = reqFoundReady,
    outcomes' = outcomes,
  }

  // 初期化が終わる。待つ要求がいれば最初の呼び出しを始め、いなければ（DETACH = "init" で
  // 切り離した初期化）初期化だけで終える。DETACH = "op" の切り離した op は、呼び出しまで進む。
  action initDone(callD: int): bool = all {
    waitingTasks' = waitingTasks,
    abandonedOps' = abandonedOps,
    opPhase == OpInit,
    now == opEventAt,
    procReady' = true,
    if (opOrphan and DETACH == "init") all {
      opPhase' = OpNone,
      opEventAt' = opEventAt,
      opOrphan' = false,
    } else all {
      opPhase' = OpCalling,
      opEventAt' = now + callD,
      opOrphan' = opOrphan,
    },
    opCallsLeft' = opCallsLeft,
    spawns' = spawns,
    killedByCancel' = killedByCancel,
    now' = now,
    reqActive' = reqActive,
    reqQueued' = reqQueued,
    reqCount' = reqCount,
    reqStartedAt' = reqStartedAt,
    reqFoundReady' = reqFoundReady,
    outcomes' = outcomes,
  }

  // 道具の呼び出しが 1 回終わる。最後の呼び出しなら op を終え、待つ要求がいれば成功で返す。
  action callDone(callD: int): bool = all {
    waitingTasks' = waitingTasks,
    abandonedOps' = abandonedOps,
    opPhase == OpCalling,
    now == opEventAt,
    if (opCallsLeft > 1) all {
      opPhase' = OpCalling,
      opEventAt' = now + callD,
      opCallsLeft' = opCallsLeft - 1,
      opOrphan' = opOrphan,
      reqActive' = reqActive,
      reqQueued' = reqQueued,
      outcomes' = outcomes,
    } else if (opOrphan) all {
      opPhase' = OpNone,
      opEventAt' = opEventAt,
      opCallsLeft' = 0,
      opOrphan' = false,
      reqActive' = reqActive,
      reqQueued' = reqQueued,
      outcomes' = outcomes,
    } else all {
      opPhase' = OpNone,
      opEventAt' = opEventAt,
      opCallsLeft' = 0,
      opOrphan' = false,
      reqActive' = false,
      reqQueued' = false,
      outcomes' = outcomes.append({ id: reqId, result: "Ok", elapsed: now - reqStartedAt, foundReady: reqFoundReady }),
    },
    procReady' = procReady,
    spawns' = spawns,
    killedByCancel' = killedByCancel,
    now' = now,
    reqCount' = reqCount,
    reqStartedAt' = reqStartedAt,
    reqFoundReady' = reqFoundReady,
  }

  // backend の総上限に達した要求を DEADLINE_EXCEEDED で返す。
  action budgetExpires: bool = all {
    BUDGET > 0,
    reqActive,
    now == reqStartedAt + BUDGET,
    endRequest("BackendDeadline"),
  }

  // Web の deadline に達した要求を打ち切る（tonic は grpc-timeout で要求の future を捨てる）。
  action webDeadlineExpires: bool = all {
    reqActive,
    now == reqStartedAt + WEB_DEADLINE,
    endRequest("WebDeadline"),
  }

  // 今の時刻に起きる出来事。時間はこれが無いときだけ進める。
  val eventDue: bool =
    (opPhase != OpNone and now == opEventAt)
      or (reqActive and BUDGET > 0 and now == reqStartedAt + BUDGET)
      or (reqActive and now == reqStartedAt + WEB_DEADLINE)
      or (opPhase == OpNone and waitingTasks > 0)

  // 次の出来事の時刻まで時間を進める。要求が無いときは、次の要求までの間として gap 秒まで進める。
  action advance(gap: int): bool = {
    val opNext = if (opPhase != OpNone) opEventAt else now + 1000
    val reqNext =
      if (reqActive) min2(reqStartedAt + WEB_DEADLINE, if (BUDGET > 0) reqStartedAt + BUDGET else now + 1000)
      else now + gap
    val nextAt = min2(opNext, reqNext)
    all {
      not(eventDue),
      reqActive or opPhase != OpNone,
      waitingTasks' = waitingTasks,
      abandonedOps' = abandonedOps,
      nextAt > now,
      now' = nextAt,
      procReady' = procReady,
      spawns' = spawns,
      killedByCancel' = killedByCancel,
      opPhase' = opPhase,
      opEventAt' = opEventAt,
      opCallsLeft' = opCallsLeft,
      opOrphan' = opOrphan,
      reqCount' = reqCount,
      reqActive' = reqActive,
      reqStartedAt' = reqStartedAt,
      reqQueued' = reqQueued,
      reqFoundReady' = reqFoundReady,
      outcomes' = outcomes,
    }
  }

  action stutter: bool = all {
    waitingTasks' = waitingTasks,
    abandonedOps' = abandonedOps,
    not(reqActive),
    opPhase == OpNone,
    reqCount == MAX_REQUESTS,
    waitingTasks == 0,
    now' = now,
    procReady' = procReady,
    spawns' = spawns,
    killedByCancel' = killedByCancel,
    opPhase' = opPhase,
    opEventAt' = opEventAt,
    opCallsLeft' = opCallsLeft,
    opOrphan' = opOrphan,
    reqCount' = reqCount,
    reqActive' = reqActive,
    reqStartedAt' = reqStartedAt,
    reqQueued' = reqQueued,
    reqFoundReady' = reqFoundReady,
    outcomes' = outcomes,
  }

  action init: bool = all {
    now' = 0,
    procReady' = false,
    spawns' = 0,
    killedByCancel' = false,
    opPhase' = OpNone,
    opEventAt' = 0,
    opCallsLeft' = 0,
    opOrphan' = false,
    reqCount' = 0,
    reqActive' = false,
    reqStartedAt' = 0,
    reqQueued' = false,
    reqFoundReady' = false,
    outcomes' = [],
    waitingTasks' = 0,
    abandonedOps' = 0,
  }

  action step: bool = {
    nondet initD = 1.to(INIT_TIMEOUT - 1).oneOf()
    nondet callD = 1.to(CALL_TIMEOUT - 1).oneOf()
    nondet gap = 1.to(INIT_TIMEOUT).oneOf()
    any {
      startRequest(initD, callD),
      dequeue(initD, callD),
      abandonedDequeue(initD, callD),
      initDone(callD),
      callDone(callD),
      budgetExpires,
      webDeadlineExpires,
      advance(gap),
      stutter,
    }
  }

  // ---- 不変条件 ----

  // 1. backend が Web の deadline より先に答える（成功か、backend の総上限の DEADLINE_EXCEEDED）。
  //    Web の deadline で打ち切られる要求は無い。
  val backendAnswersFirst: bool = outcomes.foldl(true, (acc, o) => acc and o.result != "WebDeadline")

  // 2. 子プロセスが使え、レーンも空いていた要求は、どの呼び出しも呼び出しの期限内に終わる限り成功する
  //    （issue #59：遅いが成功する取得を失敗にしない）。
  val readyReadsWithinCallTimeoutsSucceed: bool =
    outcomes.foldl(true, (acc, o) => acc and (not(o.foundReady) or o.result == "Ok"))

  // 3. 要求の打ち切りで子プロセスを捨てない（issue #180：初期化中に打ち切られても、初期化を最後まで
  //    進める。呼び出し中に打ち切られても、読みかけの子プロセスを残さないために捨てる必要が起きない）。
  val noKillByCancel: bool = not(killedByCancel)

  // 4. 初期化は失敗しないモデルなので、子プロセスの起動は 1 回だけ。
  val spawnsOnce: bool = spawns <= 1

  // 5. 要求は、backend の総上限（上限が無ければ Web の deadline）を超えて待たない。
  val requestsAreBounded: bool = {
    val bound = if (BUDGET > 0) min2(BUDGET, WEB_DEADLINE) else WEB_DEADLINE
    outcomes.foldl(true, (acc, o) => acc and o.elapsed <= bound)
      and (not(reqActive) or now - reqStartedAt <= bound)
  }

  // 6. 鍵を待つタスクの数は、待っている要求の数を超えない（設計レビューの指摘 1）。
  val waitingTasksBoundedByWaiters: bool = waitingTasks <= (if (reqActive and reqQueued) 1 else 0)

  // 7. 要求に諦められた後で、鍵の待ちから読み取りを始めるタスクは無い。
  val noReadsForAbandonedWaits: bool = abandonedOps == 0

  val safety: bool =
    backendAnswersFirst and readyReadsWithinCallTimeoutsSucceed and noKillByCancel and spawnsOnce
      and requestsAreBounded and waitingTasksBoundedByWaiters and noReadsForAbandonedWaits

  // ---- 到達の確認（quint run で違反として見つかれば、その筋書きに到達できる） ----

  // 初期化を待つ間に総上限で返った要求のあとで、別の要求が成功する。
  val recoveryAfterInitTimeoutUnreachable: bool = not(
    outcomes.indices().exists(i => outcomes.indices().exists(j =>
      i < j and outcomes[i].result == "BackendDeadline" and not(outcomes[i].foundReady)
        and outcomes[j].result == "Ok"
    ))
  )

  // 呼び出しが合わせて 30 秒を超えて成功する要求（issue #59 の「遅いが成功する」取得）。
  val slowSuccessUnreachable: bool = outcomes.foldl(true, (acc, o) => acc and not(o.result == "Ok" and o.elapsed > 30))
}

// 修正前：Web の deadline 30 秒、backend の総上限なし、op は要求の future の中。
module current {
  import runDetailDeadline(WEB_DEADLINE = 30, BUDGET = 0, DETACH = "none", CHECK_WAITER = false).*
}

// 期限だけを直した版：Web 50 秒・backend 45 秒だが、op は要求の future の中。
module budgetOnly {
  import runDetailDeadline(WEB_DEADLINE = 50, BUDGET = 45, DETACH = "none", CHECK_WAITER = false).*
}

// 初期化だけを切り離した版。
module detachInitOnly {
  import runDetailDeadline(WEB_DEADLINE = 50, BUDGET = 45, DETACH = "init", CHECK_WAITER = false).*
}

// 切り離しだけで、期限を直さない版。
module detachOnly {
  import runDetailDeadline(WEB_DEADLINE = 30, BUDGET = 0, DETACH = "op", CHECK_WAITER = true).*
}

// 期限を直し、読み取り全体を切り離したが、鍵の待ちで要求を確かめない版（設計の最初の版）。
module withoutWaiterCheck {
  import runDetailDeadline(WEB_DEADLINE = 50, BUDGET = 45, DETACH = "op", CHECK_WAITER = false).*
}

// 修正案：Web 50 秒、backend の総上限 45 秒、op 全体を要求から切り離す。
module fixed {
  import runDetailDeadline(WEB_DEADLINE = 50, BUDGET = 45, DETACH = "op", CHECK_WAITER = true).*
}
