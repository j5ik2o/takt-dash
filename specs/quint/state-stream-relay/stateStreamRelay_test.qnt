// -*- mode: Bluespec; -*-
// issue #385 の筋書きを、修正前（current）・変異（noRejectStale、noConflate、noFinishOnce）・
// 修正案（fixed）でたどる。接続の番号は開いた順に 0 から振られる。
module stateStreamRelay_test {
  import stateStreamRelay(SUPERSEDE = false, REJECT_STALE = false, CONFLATE = false, FINISH_ONCE = false) as before from "./stateStreamRelay"
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = false, CONFLATE = true, FINISH_ONCE = true) as noRejectStale from "./stateStreamRelay"
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = true, CONFLATE = false, FINISH_ONCE = true) as noConflate from "./stateStreamRelay"
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = true, CONFLATE = true, FINISH_ONCE = false) as noFinishOnce from "./stateStreamRelay"
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = true, CONFLATE = true, FINISH_ONCE = true) as after from "./stateStreamRelay"

  // 修正前: 経路が切れて画面が開き直すと、古い接続がサーバーに開いたまま残り、backend の購読も 2 本になる。
  run halfOpenReconnectLeavesTwoBeforeFixTest =
    before::init
      .then(before::connect(0))
      .then(before::arrive(0))
      .then(before::breakPath(0))
      .then(before::connect(0))
      .then(before::arrive(1))
      .expect(before::conns.get(0).phase == before::Open and before::conns.get(0).backendTask
        and before::conns.get(1).phase == before::Open
        and not(before::atMostOneOpenPerScreen) and not(before::closedOnNextArrival))

  // 修正前: 経路が切れている間も、backend から受けた状態を列へ積み続ける。
  run halfOpenQueueGrowsBeforeFixTest =
    before::init
      .then(before::connect(0))
      .then(before::arrive(0))
      .then(before::breakPath(0))
      .then(3.reps(_ => before::backendEmits(0)))
      .expect(before::conns.get(0).queue == [1, 2, 3] and not(before::queueBounded))

  // 修正前: 画面が閉じて abort が届いたあと、Next の cancel がもう一度 gRPC のストリームを閉じる。
  run abortThenCancelReleasesTwiceBeforeFixTest =
    before::init
      .then(before::connect(0))
      .then(before::arrive(0))
      .then(before::closeTab(0))
      .then(before::abortArrives(0))
      .then(before::nextCancels(0))
      .expect(before::conns.get(0).releases == 2 and not(before::noDoubleRelease))

  // 修正案: 開き直した接続がサーバーに届いた時点で、同じ画面の半開きの接続を閉じ、backend の購読を外す。
  run halfOpenSupersededTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::breakPath(0))
      .then(after::connect(0))
      .then(after::arrive(1))
      .expect(after::conns.get(0).phase == after::Closed and after::conns.get(0).reason == after::Superseded
        and not(after::conns.get(0).backendTask) and after::conns.get(0).releases == 1
        and after::conns.get(1).phase == after::Open and after::registry.get(0) == 1 and after::safety)
      // 古い接続の経路は切れたまま。OS がのちに閉じても、二度は閉じない。
      .then(after::nextCancels(0))
      .expect(after::conns.get(0).releases == 1 and after::safety)

  // 修正案: 開き直しが何度続いても、サーバーで開いている接続は画面ごとに 1 本。
  run repeatedReconnectsKeepOneTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::breakPath(0))
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::breakPath(1))
      .then(after::connect(0))
      .then(after::arrive(2))
      .expect(after::openConnsOf(0) == Set(2) and after::safety)

  // 修正案: 別の画面の接続は閉じない。
  run otherScreenUntouchedTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::connect(1))
      .then(after::arrive(1))
      .expect(after::conns.get(0).phase == after::Open and after::conns.get(1).phase == after::Open and after::safety)

  // 修正案: 新しい試行が先に届いたあと、遅れて届いた古い試行は開かず、いま使っている接続を閉じない。
  run lateStaleArrivalRejectedTest =
    after::init
      .then(after::connect(0))
      .then(after::connect(0))
      .then(after::arrive(1))
      .then(after::arrive(0))
      .expect(after::conns.get(0).reason == after::Rejected and not(after::conns.get(0).backendTask)
        and after::conns.get(0).releases == 0
        and after::conns.get(1).phase == after::Open and after::clients.get(0).live == 1 and after::safety)

  // 変異: 古い試行を断らないと、遅れて届いた古い試行が、いま使っている接続を閉じてしまう。
  run lateStaleArrivalClosesLiveWithoutRejectTest =
    noRejectStale::init
      .then(noRejectStale::connect(0))
      .then(noRejectStale::connect(0))
      .then(noRejectStale::arrive(1))
      .then(noRejectStale::arrive(0))
      .expect(noRejectStale::conns.get(1).reason == noRejectStale::Superseded
        and not(noRejectStale::liveNotClosedByRelay))

  // 修正案: Next が読めない間は最新の 1 件だけを持ち、読めたら最新を送る。途中の版は送らない。
  run conflateKeepsLatestTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(3.reps(_ => after::backendEmits(0)))
      .expect(after::conns.get(0).queue == [1] and after::conns.get(0).pending == 3
        and after::conns.get(0).skipped == 1 and after::safety)
      .then(after::nextPulls(0))
      .expect(after::conns.get(0).lastDelivered == 1 and after::conns.get(0).queue == [3] and after::safety)
      .then(after::nextPulls(0))
      .expect(after::conns.get(0).lastDelivered == 3 and after::conns.get(0).queue == [] and after::safety)

  // 修正案: 読める間は 1 件ずつ、すべての版を順に送る。
  run flowingDeliversEveryVersionTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::backendEmits(0))
      .then(after::nextPulls(0))
      .then(after::backendEmits(0))
      .then(after::nextPulls(0))
      .expect(after::conns.get(0).lastDelivered == 2 and after::conns.get(0).skipped == 0 and after::safety)

  // 変異: 間引かないと、詰まった接続の列が伸び続ける。
  run queueGrowsWithoutConflateTest =
    noConflate::init
      .then(noConflate::connect(0))
      .then(noConflate::arrive(0))
      .then(noConflate::breakPath(0))
      .then(2.reps(_ => noConflate::backendEmits(0)))
      .expect(not(noConflate::queueBounded))

  // 修正案: abort のあとに Next が cancel しても、資源は 1 回だけ閉じる。
  run abortThenCancelReleasesOnceTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::closeTab(0))
      .then(after::abortArrives(0))
      .then(after::nextCancels(0))
      .expect(after::conns.get(0).releases == 1 and after::conns.get(0).reason == after::Aborted
        and after::registry.get(0) == after::NONE and after::safety)

  // 変異: cancel が後始末の守りを通らないと、付け替えで閉じた接続をもう一度閉じる。
  run supersededThenCancelWithoutFinishOnceTest =
    noFinishOnce::init
      .then(noFinishOnce::connect(0))
      .then(noFinishOnce::arrive(0))
      .then(noFinishOnce::connect(0))
      .then(noFinishOnce::arrive(1))
      .then(noFinishOnce::nextCancels(0))
      .expect(noFinishOnce::conns.get(0).releases == 2 and not(noFinishOnce::noDoubleRelease))

  // 修正案の残り: 経路が切れている間に画面（タブ）ごと閉じると、開き直しが来ないので、
  // その接続は OS が TCP をあきらめるまで残る。残るのは画面 1 つにつき 1 本で、列は伸びない。
  run closedTabDuringOutageWaitsForTcpTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::breakPath(0))
      .then(after::closeTab(0))
      .then(3.reps(_ => after::backendEmits(0)))
      .expect(after::conns.get(0).phase == after::Open and after::conns.get(0).queue.length() == 1
        and after::conns.get(0).pending == 3 and after::safety)
      .then(after::tcpTimeout(0))
      .expect(after::conns.get(0).phase == after::Closed and not(after::conns.get(0).backendTask)
        and after::registry.get(0) == after::NONE and after::safety)

  // 修正案: backend が終えた接続を閉じたあと、同じ画面の次の接続は改めて開く。
  run reopenAfterBackendEndTest =
    after::init
      .then(after::connect(0))
      .then(after::arrive(0))
      .then(after::backendEnds(0))
      .then(after::connect(0))
      .then(after::arrive(1))
      .expect(after::conns.get(0).reason == after::BackendEnded and after::conns.get(1).phase == after::Open
        and after::registry.get(0) == 1 and after::safety)
}
