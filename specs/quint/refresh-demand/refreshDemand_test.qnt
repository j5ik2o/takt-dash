// -*- mode: Bluespec; -*-
// issue #40 の筋書きを、設計の案（after）とわざと壊した版でたどる。接続の番号は開いた順に 0 から。
// 時刻は 1 目盛り = 1 秒。最初の状態は、画面 2 つが visible・レート 1 秒・下限 1 秒・intervalSeconds 30 秒・
// 接続なし。1 秒の要求がある間は tick が poller の期限で止まるので、時計は「1 秒進めて poller が読む」を
// n.reps で繰り返して進める。変異の別名は m で始める（refreshDemand.qnt のモジュール名と重ねない）。
module refreshDemand_test {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true) as after from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = false, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true) as mAcquireEarly from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = false, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true) as mExpiryKeepsTask from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = false, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true) as mNoPostAssemblyCheck from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = false,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true) as mFixedAtStart from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = false, SHARED_ATTEMPTS = true) as mNoRewake from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = false) as mResetAttempts from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = false,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true) as mNoOwnerCheck from "./refreshDemand"

  // 前提の数値: 寿命の最大（max(3 × 10 秒, 15 秒) = 30 秒）は画面の見張り（40 秒）より短い。寿命の最小は
  // 要求なしの再送の間隔（15 秒）。要求なしの 15 秒の再送も、見張りの半分より短い
  // （specs/quint/state-stream-watchdog の STALE_TIMEOUT > 2 * RESEND_INTERVAL）。
  run ttlStaysUnderWatchdogTest =
    after::init.expect(
      after::RATES.forall(r => after::MIN_READS.forall(m =>
        after::ttlOf(after::appliedFor(r, m)) < after::STALE_TIMEOUT
          and after::ttlOf(after::appliedFor(r, m)) >= after::TTL_MIN))
        and after::ttlOf(10) == 30 and after::ttlOf(1) == 15 and after::ttlOf(5) == 15
        and 2 * 15 < after::STALE_TIMEOUT)

  // 初期状態を送るまでは要求を登録しない。送ったら登録し（寿命は 15 秒）、poller は 1 秒で読む。
  run registersAfterInitialSendTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .expect(after::registry.get(0).conn == after::NONE
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 30)
      .then(after::initialSend(0))
      .expect(after::registry.get(0).conn == 0 and after::registry.get(0).deadline == 15
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 1 and after::safety)

  // 変異: 受け付けで登録すると、初期の組み立てが寿命より長いとき最初の送信の前に切れる。
  run acquireEarlyExpiresBeforeFirstSendTest =
    mAcquireEarly::init
      .then(mAcquireEarly::connect(0))
      .then(mAcquireEarly::arrive(0))
      .then(14.reps(_ => mAcquireEarly::tick(1).then(mAcquireEarly::pollerRead)))
      .then(mAcquireEarly::tick(1))
      .expect(mAcquireEarly::now == 15)
      .then(mAcquireEarly::expire(0))
      .expect(mAcquireEarly::conns.get(0).reason == mAcquireEarly::Expired and not(mAcquireEarly::noExpiryBeforeFirstSend))

  // 二つの画面で、いちばん速い要求に合わせる。速い画面が消えたら遅い画面の間隔へ戻る。
  run twoScreensFollowTheFastestTest =
    after::init
      .then(after::changeRate(1, 10))
      .then(after::connect(0))
      .then(after::connect(1))
      .then(after::arrive(0))
      .then(after::arrive(1))
      .then(after::initialSend(0))
      .then(after::initialSend(1))
      .expect(after::effective(after::registry, after::minRead, after::configInterval, after::now) == 1)
      .then(after::hide(0))
      .then(after::releaseArrives(0))
      .expect(after::effective(after::registry, after::minRead, after::configInterval, after::now) == 10
        and after::registry.get(0).conn == after::NONE and after::safety)

  // 下限 3 秒では、1 秒の要求でも適用間隔は 3 秒。寿命は max(9, 15) = 15 秒。poller も 3 秒。
  run floorRaisesAppliedTest =
    after::init
      .then(after::configChange(3, 30))
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .expect(after::conns.get(0).applied == 3 and after::registry.get(0).deadline == 15
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 3 and after::safety)

  // intervalSeconds が下限より短い設定（2 秒、下限 3 秒）では、poller は設定の 2 秒で読み、接続は下限の
  // 3 秒で送る（D16。下限は要求にだけ掛かる）。
  run configFasterThanFloorTest =
    after::init
      .then(after::configChange(3, 2))
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .expect(after::conns.get(0).applied == 3
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 2
        and after::witnessConfigFasterThanFloor and after::safety)

  // 下限が開いている接続の途中で上がっても、次の送信から新しい下限で送る。
  run floorChangeReachesOpenConnTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(after::configChange(3, 30))
      .then(after::tick(1))
      .then(after::beginSend(0))
      .then(after::deliver(0))
      .expect(after::conns.get(0).applied == 3 and after::appliedTracksFloor)

  // 変異: 接続の開始で固定すると、下限を上げても 1 秒のまま送り、applied が下限と食い違う。
  run fixedAtStartIgnoresFloorTest =
    mFixedAtStart::init
      .then(mFixedAtStart::connect(0))
      .then(mFixedAtStart::arrive(0))
      .then(mFixedAtStart::initialSend(0))
      .then(mFixedAtStart::configChange(3, 30))
      .then(mFixedAtStart::tick(1))
      .then(mFixedAtStart::beginSend(0))
      .then(mFixedAtStart::deliver(0))
      .expect(mFixedAtStart::conns.get(0).applied == 1 and not(mFixedAtStart::appliedTracksFloor))

  // レート変更: 同じ画面の次の試行が登録を置き換え、古い送信タスクを閉じる。
  run rateChangeSupersedesTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(after::changeRate(0, 5))
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::initialSend(1))
      .expect(after::conns.get(0).reason == after::Superseded and after::registry.get(0).conn == 1
        and after::registry.get(0).rate == 5 and after::safety)
      // 古い接続の guard は置き換えの時点で 1 回落ち、新しい要求は消さない。あとで画面の切断が届いても
      // 閉じた接続には何も起きない（releaseArrives は閉じた接続では起きない）。
      .expect(after::conns.get(0).releases == 1 and after::registry.get(0).conn == 1)

  // 変異: 解放が持ち主を確かめないと、置き換えた古い接続の解放で新しい要求が消える。
  run noOwnerCheckDropsNewDemandTest =
    mNoOwnerCheck::init
      .then(mNoOwnerCheck::connect(0))
      .then(mNoOwnerCheck::arrive(0))
      .then(mNoOwnerCheck::initialSend(0))
      .then(mNoOwnerCheck::changeRate(0, 5))
      .then(mNoOwnerCheck::connect(0))
      .then(mNoOwnerCheck::arrive(1))
      .then(mNoOwnerCheck::initialSend(1))
      .expect(mNoOwnerCheck::registry.get(0).conn == mNoOwnerCheck::NONE and not(mNoOwnerCheck::liveConnsRegistered))

  // 遅れて届いた古い試行は断る。画面の最新の試行ではないので、画面は固まらない。
  run lateStaleAttemptRejectedTest =
    after::init
      .then(after::connect(0))
      .then(after::changeRate(0, 2))
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::initialSend(1))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .expect(after::conns.get(0).reason == after::Rejected and after::registry.get(0).conn == 1
        and after::clients.get(0).attempt == 2 and after::safety)

  // 寿命: 送信が詰まると、最後の送信から max(3 × 適用間隔, 15 秒) で登録が外れ、送信タスクも閉じる。
  // 画面は閉じたストリームを見て再接続し、次の試行で要求が戻る。
  run stalledConnExpiresAndReconnectsTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(after::stall(0))
      .then(after::tick(15))
      .expect(after::now == 1)
      .then(after::pollerRead)
      .then(13.reps(_ => after::tick(1).then(after::pollerRead)))
      .then(after::tick(15))
      .expect(after::now == 15 and after::noOverdueEntry
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 30)
      .then(after::expire(0))
      .expect(after::conns.get(0).reason == after::Expired and after::conns.get(0).releases == 1
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 30)
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::initialSend(1))
      .expect(after::registry.get(0).conn == 1 and after::registry.get(0).attempt == 2 and after::safety)

  // 遅れ: 画面と Next の読みが遅れて、1 秒の要求の送信が 4 秒ごとにしか終わらなくても、寿命（15 秒）は
  // 切れず要求は残る（3 × 1 秒 = 3 秒の寿命なら、最初の遅れで切れていた）。
  run delayedSendsKeepDemandTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(4.reps(_ => 4.reps(_ => after::tick(1).then(after::pollerRead)).then(after::beginSend(0)).then(after::deliver(0))))
      .expect(after::now == 16 and after::conns.get(0).sends == 5 and after::registry.get(0).conn == 0
        and after::registry.get(0).deadline == 31 and after::TTL_FACTOR * 1 < 4
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 1 and after::safety)

  // 組み立てが寿命より長い: 組み立て中は期限を時計が越えられる（送信タスクは select! の外）。期限を
  // 過ぎた要求は poller に数えず、組み立ての後の確かめで送らずに閉じる（2 回目の読みの指摘 5）。
  run slowAssemblyClosesAfterExpiryTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(1.reps(_ => after::tick(1).then(after::pollerRead)))
      .then(after::beginSend(0))
      .then(13.reps(_ => after::tick(1).then(after::pollerRead)))
      .then(after::tick(1))
      .expect(after::now == 15 and after::conns.get(0).assembling and after::registry.get(0).conn == 0
        and after::effective(after::registry, after::minRead, after::configInterval, after::now) == 30
        and after::noOverdueEntry)
      .then(after::tick(15))
      .expect(after::now == 30)
      .then(after::deliver(0))
      .expect(after::conns.get(0).reason == after::Expired and after::conns.get(0).sends == 1
        and after::registry.get(0).conn == after::NONE and after::safety)

  // 変異: 組み立ての後に期限を確かめないと、poller がもう数えていない要求で applied を付けて送り、
  // touch が寿命を延ばし直す（poller は 30 秒に戻ったのに、画面には 1 秒と届く）。
  run noPostAssemblyCheckLiesTest =
    mNoPostAssemblyCheck::init
      .then(mNoPostAssemblyCheck::connect(0))
      .then(mNoPostAssemblyCheck::arrive(0))
      .then(mNoPostAssemblyCheck::initialSend(0))
      .then(mNoPostAssemblyCheck::tick(1))
      .then(mNoPostAssemblyCheck::pollerRead)
      .then(mNoPostAssemblyCheck::beginSend(0))
      .then(13.reps(_ => mNoPostAssemblyCheck::tick(1).then(mNoPostAssemblyCheck::pollerRead)))
      .then(mNoPostAssemblyCheck::tick(1))
      .then(mNoPostAssemblyCheck::tick(15))
      .then(mNoPostAssemblyCheck::deliver(0))
      .expect(mNoPostAssemblyCheck::conns.get(0).phase == mNoPostAssemblyCheck::Live
        and mNoPostAssemblyCheck::conns.get(0).applied == 1
        and mNoPostAssemblyCheck::registry.get(0).deadline == 45
        and not(mNoPostAssemblyCheck::noSendAfterExpiry))

  // 変異: 寿命切れで登録だけ外すと、送信タスクは要求なしで送り続け、applied が嘘になる（読みの指摘 ③）。
  run expiryKeepsTaskLiesTest =
    mExpiryKeepsTask::init
      .then(mExpiryKeepsTask::connect(0))
      .then(mExpiryKeepsTask::arrive(0))
      .then(mExpiryKeepsTask::initialSend(0))
      .then(mExpiryKeepsTask::stall(0))
      .then(14.reps(_ => mExpiryKeepsTask::tick(1).then(mExpiryKeepsTask::pollerRead)))
      .then(mExpiryKeepsTask::tick(1))
      .then(mExpiryKeepsTask::expire(0))
      .expect(mExpiryKeepsTask::conns.get(0).phase == mExpiryKeepsTask::Live
        and not(mExpiryKeepsTask::liveConnsRegistered))

  // 要求が増えたら、poller は残りの待ちを切り上げて読む。
  run pollerRewakesOnDemandTest =
    after::init
      .then(after::tick(15))
      .expect(after::now == 15)
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .expect(after::nextDue(after::registry, after::conns, after::lastReadAt,
        after::effective(after::registry, after::minRead, after::configInterval, after::now),
        after::capturedInterval) <= after::now)
      .then(after::pollerRead)
      .expect(after::lastReadAt == 15 and after::pollerNotLate)

  // 変異: poller が前回の 30 秒の待ちのままだと、1 秒の要求があっても読まずに時間が過ぎる。
  run noRewakeSleepsThroughDemandTest =
    mNoRewake::init
      .then(mNoRewake::tick(15))
      .then(mNoRewake::connect(0))
      .then(mNoRewake::arrive(0))
      .then(mNoRewake::initialSend(0))
      .then(mNoRewake::tick(1))
      .expect(mNoRewake::now == 16 and mNoRewake::lastReadAt == 0 and not(mNoRewake::pollerNotLate))

  // 試行番号を Provider が持つ: 寿命切れの再接続（2）のあとにレートを変えても次は 3 で、置き換えになる。
  run sharedAttemptsNeverRejectLatestTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(after::stall(0))
      .then(14.reps(_ => after::tick(1).then(after::pollerRead)))
      .then(after::tick(1))
      .then(after::expire(0))
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::initialSend(1))
      .then(after::changeRate(0, 5))
      .then(after::connect(0))
      .then(after::arrive(2))
      .then(after::initialSend(2))
      .expect(after::conns.get(2).attempt == 3 and after::conns.get(1).reason == after::Superseded
        and after::registry.get(0).conn == 2 and after::safety)

  // 変異: openStateStream を呼ぶたびに 1 から数えると、レート変更の新しい接続（1）が、まだ登録の
  // 残る古い接続（2）より小さく、断られる。画面の最新の試行なので EventSource は 204 で固まる（読みの指摘 ⑲）。
  run resetAttemptsRejectsLatestTest =
    mResetAttempts::init
      .then(mResetAttempts::connect(0))
      .then(mResetAttempts::arrive(0))
      .then(mResetAttempts::initialSend(0))
      .then(mResetAttempts::stall(0))
      .then(14.reps(_ => mResetAttempts::tick(1).then(mResetAttempts::pollerRead)))
      .then(mResetAttempts::tick(1))
      .then(mResetAttempts::expire(0))
      .then(mResetAttempts::connect(0))
      .then(mResetAttempts::arrive(1))
      .then(mResetAttempts::initialSend(1))
      .then(mResetAttempts::changeRate(0, 5))
      .then(mResetAttempts::connect(0))
      .then(mResetAttempts::arrive(2))
      .then(mResetAttempts::initialSend(2))
      .expect(mResetAttempts::conns.get(2).attempt == 1 and mResetAttempts::conns.get(2).reason == mResetAttempts::Rejected
        and not(mResetAttempts::latestNeverRejected))

  // hidden → visible: 閉じてから開き直しても試行番号は進み、断られない。
  run hideShowKeepsAttemptsTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(after::hide(0))
      .then(after::show(0))
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::initialSend(1))
      .expect(after::conns.get(1).attempt == 2 and after::conns.get(0).reason == after::Superseded and after::safety)
}
