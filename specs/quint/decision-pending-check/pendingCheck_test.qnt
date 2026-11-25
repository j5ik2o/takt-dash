// -*- mode: Bluespec; -*-
// pendingCheck.qnt の筋書きテスト。issue #319 の筋書きを修正前と修正案の遷移でたどる。
module pendingCheck_test {
  import pendingCheck(CHECK = false, TAKT_GUARD = true) as before from "./pendingCheck"
  import pendingCheck(CHECK = true, TAKT_GUARD = true) as after from "./pendingCheck"
  import pendingCheck(CHECK = true, TAKT_GUARD = false) as unguarded from "./pendingCheck"

  // 修正前。画面が revision 1 を表示したまま登録案が作り直され、状態ストアが revision 2 を
  // 読んだあとの承認は、古い revision のまま takt へ送られる。
  run staleRevisionIsSentBeforeFixTest =
    before::init
      .then(before::taktRevise)
      .then(before::poll)
      .then(before::render)
      .then(before::taktRevise)
      .then(before::poll)
      .then(before::decideRegistration)
      .then(all {
        assert(before::sentRevisions == Set({ revision: 1, listed: 2 })),
        assert(not(before::sentRevisionIsListed)),
        // 実物の takt-mcp は自分の値と照合して断るため、適用はされない。
        assert(before::appliedRevisions == Set()),
        before::poll,
      })

  // 修正案。同じ筋書きで、backend は takt へ送らずに断る。
  run staleRevisionIsRejectedAfterFixTest =
    after::init
      .then(after::taktRevise)
      .then(after::poll)
      .then(after::render)
      .then(after::taktRevise)
      .then(after::poll)
      .then(after::decideRegistration)
      .then(all {
        assert(after::sentRevisions == Set()),
        assert(after::rejectedByBackend == 1),
        assert(after::writesMatchPendingList),
        after::poll,
      })

  // 修正案。画面を読み直したあとの承認は適用される。
  run freshRevisionIsAppliedAfterFixTest =
    after::init
      .then(after::taktRevise)
      .then(after::poll)
      .then(after::render)
      .then(after::decideRegistration)
      .then(all {
        assert(after::appliedRevisions == Set({ revision: 1, current: 1 })),
        assert(not(after::taktPending)),
        after::poll,
      })

  // 修正前。別のゴールを goal_id に入れた回答は、そのゴールへ送られる。
  run answerToAnotherGoalIsSentBeforeFixTest =
    before::init
      .then(before::answer("g2"))
      .then(all {
        assert(before::sentAnswers == Set({ goal: "g2", listed: "g1" })),
        assert(not(before::sentAnswerGoesToListedGoal)),
        before::poll,
      })

  // 修正案。別のゴールを goal_id に入れた回答は断り、判断待ちは残る。正しい回答は届く。
  run answerToAnotherGoalIsRejectedAfterFixTest =
    after::init
      .then(after::answer("g2"))
      .then(all {
        assert(after::sentAnswers == Set()),
        assert(after::rejectedByBackend == 1),
        assert(after::questionListed),
        after::answer("g1"),
      })
      .then(all {
        assert(after::sentAnswers == Set({ goal: "g1", listed: "g1" })),
        after::poll,
      })

  // 修正案の限界。状態ストアが takt より古いと、backend の照合は通る。takt-mcp が照合しなければ、
  // 作り直される前の revision の承認が、新しい登録案に適用される。
  run staleStorePassesBackendCheckTest =
    unguarded::init
      .then(unguarded::taktRevise)
      .then(unguarded::poll)
      .then(unguarded::render)
      .then(unguarded::taktRevise)
      .then(unguarded::decideRegistration)
      .then(all {
        assert(unguarded::writesMatchPendingList),
        assert(unguarded::appliedRevisions == Set({ revision: 1, current: 2 })),
        assert(not(unguarded::appliedRevisionIsCurrent)),
        unguarded::poll,
      })

  // 修正案。承認のあとに新しい登録案ができても、番号は使い回されない。承認済みの revision を
  // 表示したままの画面からの承認は、新しい登録案に適用されない。
  run approvedRevisionIsNotReusedTest =
    after::init
      .then(after::taktRevise)
      .then(after::poll)
      .then(after::render)
      .then(after::decideRegistration)
      .then(after::taktRevise)
      .then(after::poll)
      .then(after::decideRegistration)
      .then(all {
        assert(after::taktRevision == 2),
        assert(after::taktPending),
        assert(after::appliedRevisions == Set({ revision: 1, current: 1 })),
        assert(after::rejectedByBackend == 1),
        after::poll,
      })
}
