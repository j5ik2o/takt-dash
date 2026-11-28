// -*- mode: Bluespec; -*-
// issue #40 の筋書きを、設計の案（after）とわざと壊した版でたどる。接続の番号は開いた順に 0 から。
// 時刻は 1 目盛り = 1 秒。最初の状態は、画面 2 つが visible・レート 1 秒・下限 1 秒・接続なし。
module refreshDemand_test {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, RECOMPUTE = true, POLLER_REWAKE = true,
    SHARED_ATTEMPTS = true) as after from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = false, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, RECOMPUTE = true, POLLER_REWAKE = true,
    SHARED_ATTEMPTS = true) as acquireEarly from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = false, RECOMPUTE = true, POLLER_REWAKE = true,
    SHARED_ATTEMPTS = true) as expiryKeepsTask from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, RECOMPUTE = false, POLLER_REWAKE = true,
    SHARED_ATTEMPTS = true) as fixedAtStart from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, RECOMPUTE = true, POLLER_REWAKE = false,
    SHARED_ATTEMPTS = true) as noRewake from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, RECOMPUTE = true, POLLER_REWAKE = true,
    SHARED_ATTEMPTS = false) as resetAttempts from "./refreshDemand"
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = false,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, RECOMPUTE = true, POLLER_REWAKE = true,
    SHARED_ATTEMPTS = true) as noOwnerCheck from "./refreshDemand"

  // 前提の数値: 寿命の最大（3 × 10 秒 = 30 秒）は画面の見張り（40 秒）より短い。要求なしの
  // 15 秒の再送も、見張りの半分より短い（specs/quint/state-stream-watchdog の STALE_TIMEOUT > 2 * RESEND_INTERVAL）。
  run ttlStaysUnderWatchdogTest =
    after::init.expect(
      after::TTL_FACTOR * after::MAX_RATE < after::STALE_TIMEOUT
        and after::MIN_READS.forall(m => after::TTL_FACTOR * after::max2(m, after::MAX_RATE) < after::STALE_TIMEOUT)
        and 2 * 15 < after::STALE_TIMEOUT)

  // 初期状態を送るまでは要求を登録しない。送ったら登録し、poller は 1 秒で読む。
  run registersAfterInitialSendTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .expect(after::registry.get(0).conn == after::NONE and after::effective(after::registry, after::minRead) == 30)
      .then(after::initialSend(0))
      .expect(after::registry.get(0).conn == 0 and after::registry.get(0).deadline == 3
        and after::effective(after::registry, after::minRead) == 1 and after::safety)

  // 変異: 受け付けで登録すると、初期の組み立てが寿命より長いとき最初の送信の前に切れる。
  run acquireEarlyExpiresBeforeFirstSendTest =
    acquireEarly::init
      .then(acquireEarly::connect(0))
      .then(acquireEarly::arrive(0))
      .then(acquireEarly::tick(1))
      .then(acquireEarly::pollerRead)
      .then(acquireEarly::tick(1))
      .then(acquireEarly::pollerRead)
      .then(acquireEarly::tick(1))
      .then(acquireEarly::expire(0))
      .expect(acquireEarly::conns.get(0).reason == acquireEarly::Expired and not(acquireEarly::noExpiryBeforeFirstSend))

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
      .expect(after::effective(after::registry, after::minRead) == 1)
      .then(after::hide(0))
      .then(after::releaseArrives(0))
      .expect(after::effective(after::registry, after::minRead) == 10 and after::registry.get(0).conn == after::NONE
        and after::safety)

  // 下限 3 秒では、1 秒の要求でも適用間隔は 3 秒。poller も 3 秒。
  run floorRaisesAppliedTest =
    after::init
      .then(after::configChange(3))
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .expect(after::conns.get(0).applied == 3 and after::registry.get(0).deadline == 9
        and after::effective(after::registry, after::minRead) == 3 and after::safety)

  // 下限が開いている接続の途中で上がっても、次の送信から新しい下限で送る。
  run floorChangeReachesOpenConnTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(after::configChange(3))
      .then(after::tick(1))
      .then(after::send(0))
      .expect(after::conns.get(0).applied == 3 and after::appliedTracksFloor)

  // 変異: 接続の開始で固定すると、下限を上げても 1 秒のまま送り、applied が下限と食い違う。
  run fixedAtStartIgnoresFloorTest =
    fixedAtStart::init
      .then(fixedAtStart::connect(0))
      .then(fixedAtStart::arrive(0))
      .then(fixedAtStart::initialSend(0))
      .then(fixedAtStart::configChange(3))
      .then(fixedAtStart::tick(1))
      .then(fixedAtStart::send(0))
      .expect(fixedAtStart::conns.get(0).applied == 1 and not(fixedAtStart::appliedTracksFloor))

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
    noOwnerCheck::init
      .then(noOwnerCheck::connect(0))
      .then(noOwnerCheck::arrive(0))
      .then(noOwnerCheck::initialSend(0))
      .then(noOwnerCheck::changeRate(0, 5))
      .then(noOwnerCheck::connect(0))
      .then(noOwnerCheck::arrive(1))
      .then(noOwnerCheck::initialSend(1))
      .expect(noOwnerCheck::registry.get(0).conn == noOwnerCheck::NONE and not(noOwnerCheck::liveConnsRegistered))

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

  // 寿命: 送信が詰まると、最後の送信から 3 × 適用間隔で登録が外れ、送信タスクも閉じる。
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
      .then(after::tick(15))
      .then(after::pollerRead)
      .then(after::tick(15))
      .expect(after::now == 3 and after::noOverdueEntry)
      .then(after::expire(0))
      .expect(after::conns.get(0).reason == after::Expired and after::conns.get(0).releases == 1
        and after::effective(after::registry, after::minRead) == 30)
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::initialSend(1))
      .expect(after::registry.get(0).conn == 1 and after::registry.get(0).attempt == 2 and after::safety)

  // 変異: 寿命切れで登録だけ外すと、送信タスクは要求なしで送り続け、applied が嘘になる（読みの指摘 ③）。
  run expiryKeepsTaskLiesTest =
    expiryKeepsTask::init
      .then(expiryKeepsTask::connect(0))
      .then(expiryKeepsTask::arrive(0))
      .then(expiryKeepsTask::initialSend(0))
      .then(expiryKeepsTask::stall(0))
      .then(expiryKeepsTask::tick(1))
      .then(expiryKeepsTask::pollerRead)
      .then(expiryKeepsTask::tick(1))
      .then(expiryKeepsTask::pollerRead)
      .then(expiryKeepsTask::tick(1))
      .then(expiryKeepsTask::expire(0))
      .expect(expiryKeepsTask::conns.get(0).phase == expiryKeepsTask::Live
        and not(expiryKeepsTask::liveConnsRegistered))

  // 要求が増えたら、poller は残りの待ちを切り上げて読む。
  run pollerRewakesOnDemandTest =
    after::init
      .then(after::tick(15))
      .expect(after::now == 15)
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .expect(after::nextDue(after::registry, after::lastReadAt, after::effective(after::registry, after::minRead),
        after::capturedInterval) <= after::now)
      .then(after::pollerRead)
      .expect(after::lastReadAt == 15 and after::pollerNotLate)

  // 変異: poller が前回の 30 秒の待ちのままだと、1 秒の要求があっても読まずに時間が過ぎる。
  run noRewakeSleepsThroughDemandTest =
    noRewake::init
      .then(noRewake::tick(15))
      .then(noRewake::connect(0))
      .then(noRewake::arrive(0))
      .then(noRewake::initialSend(0))
      .then(noRewake::tick(1))
      .expect(noRewake::now == 16 and noRewake::lastReadAt == 0 and not(noRewake::pollerNotLate))

  // 試行番号を Provider が持つ: 寿命切れの再接続（2）のあとにレートを変えても次は 3 で、置き換えになる。
  run sharedAttemptsNeverRejectLatestTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::initialSend(0))
      .then(after::stall(0))
      .then(after::tick(1)).then(after::pollerRead)
      .then(after::tick(1)).then(after::pollerRead)
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
    resetAttempts::init
      .then(resetAttempts::connect(0))
      .then(resetAttempts::arrive(0))
      .then(resetAttempts::initialSend(0))
      .then(resetAttempts::stall(0))
      .then(resetAttempts::tick(1)).then(resetAttempts::pollerRead)
      .then(resetAttempts::tick(1)).then(resetAttempts::pollerRead)
      .then(resetAttempts::tick(1))
      .then(resetAttempts::expire(0))
      .then(resetAttempts::connect(0))
      .then(resetAttempts::arrive(1))
      .then(resetAttempts::initialSend(1))
      .then(resetAttempts::changeRate(0, 5))
      .then(resetAttempts::connect(0))
      .then(resetAttempts::arrive(2))
      .then(resetAttempts::initialSend(2))
      .expect(resetAttempts::conns.get(2).attempt == 1 and resetAttempts::conns.get(2).reason == resetAttempts::Rejected
        and not(resetAttempts::latestNeverRejected))

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
