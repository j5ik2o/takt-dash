// -*- mode: Bluespec; -*-
// issue #59 と #180 の筋書きを、修正前・途中の版・修正案でたどる。
module runDetailDeadline_test {
  import runDetailDeadline(WEB_DEADLINE = 30, BUDGET = 0, DETACH = "none") as before from "./runDetailDeadline"
  import runDetailDeadline(WEB_DEADLINE = 50, BUDGET = 45, DETACH = "none") as budgetOnly from "./runDetailDeadline"
  import runDetailDeadline(WEB_DEADLINE = 50, BUDGET = 45, DETACH = "init") as initOnly from "./runDetailDeadline"
  import runDetailDeadline(WEB_DEADLINE = 50, BUDGET = 45, DETACH = "op") as after from "./runDetailDeadline"

  // 期限の並び：呼び出しの期限 × 回り道の呼び出しの回数 ≦ backend の総上限 < Web の deadline。
  run deadlinesAreOrderedTest =
    assert(after::CALLS * after::CALL_TIMEOUT <= after::BUDGET and after::BUDGET < after::WEB_DEADLINE)

  // issue #59：子プロセスは使える状態。呼び出し 3 回が各 11 秒（合わせて 33 秒）で成功する。
  // 修正前は Web の 30 秒の deadline で打ち切られる。
  run slowButSuccessfulReadFailsBeforeFixTest =
    before::init
      .then(before::startRequest(1, 1)).then(before::advance(1)).then(before::initDone(1))
      .then(before::advance(1)).then(before::callDone(1)).then(before::advance(1)).then(before::callDone(1))
      .then(before::advance(1)).then(before::callDone(1))
      .then(before::startRequest(1, 11)).then(before::advance(1)).then(before::callDone(11))
      .then(before::advance(1)).then(before::callDone(11))
      .then(before::advance(1)).then(before::webDeadlineExpires)
      .expect(before::outcomes.length() == 2 and before::outcomes[1].foundReady
        and before::outcomes[1].result == "WebDeadline" and before::outcomes[1].elapsed == 30
        and not(before::readyReadsWithinCallTimeoutsSucceed) and not(before::backendAnswersFirst))

  // 修正案：同じ 33 秒の取得が成功する。
  run slowButSuccessfulReadSucceedsAfterFixTest =
    after::init
      .then(after::startRequest(1, 1)).then(after::advance(1)).then(after::initDone(1))
      .then(after::advance(1)).then(after::callDone(1)).then(after::advance(1)).then(after::callDone(1))
      .then(after::advance(1)).then(after::callDone(1))
      .then(after::startRequest(1, 11)).then(after::advance(1)).then(after::callDone(11))
      .then(after::advance(1)).then(after::callDone(11))
      .then(after::advance(1)).then(after::callDone(11))
      .expect(after::outcomes.length() == 2 and after::outcomes[1].result == "Ok"
        and after::outcomes[1].elapsed == 33 and after::safety)

  // issue #180：初期化に 40 秒かかる。修正前は Web の 30 秒で要求が打ち切られ、初期化中の子プロセスも
  // 捨てられる。次の要求も起動し直して同じく打ち切られ、どの要求も成功しない。
  run slowInitNeverCompletesBeforeFixTest =
    before::init
      .then(before::startRequest(40, 4)).then(before::advance(1)).then(before::webDeadlineExpires)
      .then(before::startRequest(40, 4)).then(before::advance(1)).then(before::webDeadlineExpires)
      .then(before::startRequest(40, 4)).then(before::advance(1)).then(before::webDeadlineExpires)
      .expect(before::outcomes.length() == 3 and before::spawns == 3 and not(before::procReady)
        and before::outcomes.foldl(true, (acc, o) => acc and o.result == "WebDeadline")
        and not(before::noKillByCancel))

  // 期限だけを直した版：初期化 40 秒 ＋ 呼び出し 3 回 × 4 秒は 52 秒で、総上限 45 秒に収まらない。
  // 呼び出し中に打ち切られて子プロセスを捨てるので、次の要求も起動からやり直して打ち切られる。
  run budgetAloneRestartsTheChildAfterEveryTimeoutTest =
    budgetOnly::init
      .then(budgetOnly::startRequest(40, 4)).then(budgetOnly::advance(1)).then(budgetOnly::initDone(4))
      .then(budgetOnly::advance(1)).then(budgetOnly::callDone(4))
      .then(budgetOnly::advance(1)).then(budgetOnly::budgetExpires)
      .then(budgetOnly::startRequest(40, 4)).then(budgetOnly::advance(1)).then(budgetOnly::initDone(4))
      .then(budgetOnly::advance(1)).then(budgetOnly::callDone(4))
      .then(budgetOnly::advance(1)).then(budgetOnly::budgetExpires)
      .expect(budgetOnly::outcomes.length() == 2 and budgetOnly::spawns == 2 and not(budgetOnly::procReady)
        and budgetOnly::outcomes.foldl(true, (acc, o) => acc and o.result == "BackendDeadline")
        and budgetOnly::backendAnswersFirst and not(budgetOnly::noKillByCancel))

  // 初期化だけを切り離した版：同じ筋書きで、初期化は終わるが、呼び出し中の打ち切りで子プロセスを捨てる。
  run initDetachAloneStillKillsTheChildMidCallTest =
    initOnly::init
      .then(initOnly::startRequest(40, 4)).then(initOnly::advance(1)).then(initOnly::initDone(4))
      .then(initOnly::advance(1)).then(initOnly::callDone(4))
      .then(initOnly::advance(1)).then(initOnly::budgetExpires)
      .expect(not(initOnly::procReady) and not(initOnly::noKillByCancel))

  // 修正案：最初の要求は総上限 45 秒で DEADLINE_EXCEEDED を返すが、切り離した op は初期化と呼び出しを
  // 最後まで進め、子プロセスは使える状態で残る。次の要求は起動し直さずに成功する。
  run slowInitRecoversOnTheNextRequestAfterFixTest =
    after::init
      .then(after::startRequest(40, 4)).then(after::advance(1)).then(after::initDone(4))
      .then(after::advance(1)).then(after::callDone(4))
      .then(after::advance(1)).then(after::budgetExpires)
      .then(after::advance(10)).then(after::callDone(4))
      .then(after::advance(10)).then(after::callDone(4))
      .then(after::startRequest(1, 4)).then(after::advance(1)).then(after::callDone(4))
      .then(after::advance(1)).then(after::callDone(4))
      .then(after::advance(1)).then(after::callDone(4))
      .expect(after::outcomes.length() == 2 and after::outcomes[0].result == "BackendDeadline"
        and after::outcomes[0].elapsed == 45 and after::outcomes[1].result == "Ok"
        and after::spawns == 1 and after::procReady and after::safety
        and not(after::recoveryAfterInitTimeoutUnreachable))

  // 修正案：切り離した op がまだ走っている間に次の要求が来たら、鍵が空くのを待ってから読む。
  // 待つ間も総上限で返り、Web の deadline より先に答える。
  run requestQueuedBehindADetachedOpIsBoundedTest =
    after::init
      .then(after::startRequest(59, 14)).then(after::advance(1)).then(after::budgetExpires)
      .then(after::startRequest(1, 14)).then(after::advance(1)).then(after::initDone(14))
      .then(after::advance(1)).then(after::callDone(14))
      .then(after::advance(1)).then(after::callDone(14))
      .then(after::advance(1)).then(after::budgetExpires)
      .expect(after::outcomes.length() == 2 and after::outcomes[1].result == "BackendDeadline"
        and not(after::outcomes[1].foundReady) and after::outcomes[1].elapsed == 45
        and after::opOrphan and after::safety)
}
