// -*- mode: Bluespec; -*-
// 判断待ちの書き込み（ゴール登録の承認・却下、問い合わせへの回答）を、判断待ちの一覧と照合する
// 状態機械（issue #319）。
//
// 登場するのは、takt、backend の状態ストア（takt の定期読み取りの写し）、画面の3者。
// 写しは遅れる。takt の登録案が作り直されても、状態ストアは次の読み取りまで前の revision を持ち、
// 画面は次の描画まで、状態ストアのさらに前の値を表示している。
//
// CHECK = false は修正前の遷移（backend は revision が正の数か、goal_id の形が正しいかだけを見る）。
// CHECK = true は修正案の遷移（backend は状態ストアの判断待ちと照合し、食い違えば takt へ送らない）。
// TAKT_GUARD = true は、takt-mcp が自分の現在の値と照合して断ること（実物の takt-mcp の挙動）。
// TAKT_GUARD = false は、takt-mcp が照合しないと仮定したもの。backend の照合だけで足りるかを確かめる。
module pendingCheck {
  const CHECK: bool
  const TAKT_GUARD: bool

  type Goal = str

  // takt へ送った承認・却下。listed は、送った時点で状態ストアが持っていた revision。
  type SentRevision = { revision: int, listed: int }
  // takt が適用した承認・却下。current は、適用した時点の takt の revision。
  type AppliedRevision = { revision: int, current: int }
  // takt へ送った回答。listed は、状態ストアの判断待ちが持つゴール。
  type SentAnswer = { goal: Goal, listed: Goal }

  pure val MAX_REVISION: int = 3
  // 問い合わせが属するゴール。問い合わせの識別子は1つのゴールにだけ属し、変わらない。
  pure val OWNER: Goal = "g1"
  pure val GOALS: Set[Goal] = Set("g1", "g2")
  // 要求が goal_id を持たないこと。
  pure val NO_GOAL: Goal = ""

  // 承認待ちの revision。0 は承認待ちが無いこと。
  var taktRevision: int
  var storeRevision: int
  var screenRevision: int
  var sentRevisions: Set[SentRevision]
  var appliedRevisions: Set[AppliedRevision]
  // 問い合わせが、状態ストアの判断待ちに載っているか。
  var questionListed: bool
  var sentAnswers: Set[SentAnswer]
  var rejectedByBackend: int

  action init = all {
    taktRevision' = 0,
    storeRevision' = 0,
    screenRevision' = 0,
    sentRevisions' = Set(),
    appliedRevisions' = Set(),
    questionListed' = true,
    sentAnswers' = Set(),
    rejectedByBackend' = 0,
  }

  // takt で登録案が作られる、または作り直される。
  action taktRevise = all {
    taktRevision < MAX_REVISION,
    taktRevision' = taktRevision + 1,
    storeRevision' = storeRevision,
    screenRevision' = screenRevision,
    sentRevisions' = sentRevisions,
    appliedRevisions' = appliedRevisions,
    questionListed' = questionListed,
    sentAnswers' = sentAnswers,
    rejectedByBackend' = rejectedByBackend,
  }

  // backend の定期読み取りが、takt の値を状態ストアへ写す。
  action poll = all {
    taktRevision' = taktRevision,
    storeRevision' = taktRevision,
    screenRevision' = screenRevision,
    sentRevisions' = sentRevisions,
    appliedRevisions' = appliedRevisions,
    questionListed' = questionListed,
    sentAnswers' = sentAnswers,
    rejectedByBackend' = rejectedByBackend,
  }

  // 画面が、状態ストアの値を表示し直す。
  action render = all {
    taktRevision' = taktRevision,
    storeRevision' = storeRevision,
    screenRevision' = storeRevision,
    sentRevisions' = sentRevisions,
    appliedRevisions' = appliedRevisions,
    questionListed' = questionListed,
    sentAnswers' = sentAnswers,
    rejectedByBackend' = rejectedByBackend,
  }

  // 利用者が、画面に出ている登録案を承認（または却下）する。
  action decideRegistration = {
    val accepted = if (CHECK) screenRevision == storeRevision else true
    val applies = accepted and (if (TAKT_GUARD) screenRevision == taktRevision else true)
    all {
      screenRevision > 0,
      sentRevisions' = if (accepted) sentRevisions.union(Set({ revision: screenRevision, listed: storeRevision }))
        else sentRevisions,
      appliedRevisions' = if (applies)
        appliedRevisions.union(Set({ revision: screenRevision, current: taktRevision }))
        else appliedRevisions,
      rejectedByBackend' = if (accepted) rejectedByBackend else rejectedByBackend + 1,
      // 適用された登録案は、承認待ちでなくなる。状態ストアと画面は、次の読み取りまで知らない。
      taktRevision' = if (applies) 0 else taktRevision,
      storeRevision' = storeRevision,
      screenRevision' = screenRevision,
      questionListed' = questionListed,
      sentAnswers' = sentAnswers,
    }
  }

  // 利用者（画面、CLI、古い画面）が問い合わせに回答する。requested は要求の goal_id。
  action answer(requested: Goal): bool = {
    val accepted = questionListed and (if (CHECK) requested == NO_GOAL or requested == OWNER else true)
    // 修正前は要求のゴールを優先する。修正案は判断待ちのゴールだけを使う。
    val target = if (CHECK or requested == NO_GOAL) OWNER else requested
    all {
      sentAnswers' = if (accepted) sentAnswers.union(Set({ goal: target, listed: OWNER })) else sentAnswers,
      rejectedByBackend' = if (accepted) rejectedByBackend else rejectedByBackend + 1,
      // 判断待ちのゴールへ届いた回答で、問い合わせは回答済みになる。
      questionListed' = if (accepted and target == OWNER) false else questionListed,
      taktRevision' = taktRevision,
      storeRevision' = storeRevision,
      screenRevision' = screenRevision,
      sentRevisions' = sentRevisions,
      appliedRevisions' = appliedRevisions,
    }
  }

  action step = any {
    taktRevise,
    poll,
    render,
    decideRegistration,
    nondet requested = GOALS.union(Set(NO_GOAL)).oneOf()
    answer(requested),
  }

  // ---- 不変条件（安全性）----

  /// backend が takt へ送る承認・却下は、その時点の判断待ちの一覧にある revision だけ。
  val sentRevisionIsListed = sentRevisions.forall(s => s.revision == s.listed)

  /// backend が takt へ送る回答は、判断待ちの一覧が示すゴールへのものだけ。
  val sentAnswerGoesToListedGoal = sentAnswers.forall(s => s.goal == s.listed)

  /// issue #319 の受け入れ条件。backend は、一覧と食い違う書き込みを takt へ送らない。
  val writesMatchPendingList = sentRevisionIsListed and sentAnswerGoesToListedGoal

  /// 適用された承認・却下は、適用した時点の takt の登録案へのもの。
  /// 状態ストアは takt より遅れるため、これは backend の照合だけでは守れない。
  val appliedRevisionIsCurrent = appliedRevisions.forall(a => a.revision == a.current)

  // ---- 進行の条件（到達できること。--witnesses で数える）----

  /// 画面が最新なら、承認・却下は適用される。
  val freshRegistrationApplied = appliedRevisions.exists(a => a.revision == a.current)

  /// 正しい回答（goal_id なし、または判断待ちのゴール）は takt へ届く。
  val correctAnswerSent = sentAnswers.exists(s => s.goal == OWNER)

  /// backend が食い違いで断ることがある。
  val staleWriteRejected = rejectedByBackend > 0
}

// 修正前の遷移。takt-mcp は自分の値と照合する（実物）。
module current {
  import pendingCheck(CHECK = false, TAKT_GUARD = true).*
}

// 修正案の遷移。takt-mcp は自分の値と照合する（実物）。
module fixed {
  import pendingCheck(CHECK = true, TAKT_GUARD = true).*
}

// 修正案の遷移で、takt-mcp が照合しないと仮定したもの。backend の照合の限界を確かめるために置く。
module fixedWithoutTaktGuard {
  import pendingCheck(CHECK = true, TAKT_GUARD = false).*
}
