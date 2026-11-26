// -*- mode: Bluespec; -*-
// 状態の配信の中継（frontend/web/app/api/state/route.ts）が、画面に見捨てられた接続の
// サーバー側の資源を溜めないことの状態機械（issue #385）。
//
// 画面（openStateStream）は半開きを見張りで検知すると、古い EventSource を閉じて新しい接続を
// 開く（issue #383）。経路が切れていると、閉じたこと（FIN）はサーバーへ届かない。サーバーの
// route.ts は古い接続を開いたまま、backend の WatchState を購読し続け、Next が送れない間も
// 受けた状態を ReadableStream の列へ積み続ける。OS が TCP をあきらめるまで（十数分）残る。
//
// 直し方（定数で切り替える）:
// - SUPERSEDE: 画面ごとの識別子（screen）と試行の番号（attempt）を URL に載せ、同じ画面の
//   新しい接続がサーバーへ届いたら、同じ画面の古い接続を閉じる。
// - REJECT_STALE: 同じ画面のもっと新しい接続が既に届いていたら、遅れて届いた古い試行は開かない。
// - CONFLATE: Next が読めない間（desiredSize <= 0）は列へ積まず、最新の 1 件だけを持つ。
// - FINISH_ONCE: ReadableStream の cancel も、abort・error・end と同じ 1 本の後始末を通し、
//   同じ接続の資源を二度閉じない。
//
// 時間は扱わない。経路が切れた接続を OS が閉じる（tcpTimeout）のは、いつか起きる遷移として表す。
module stateStreamRelay {
  const SUPERSEDE: bool
  const REJECT_STALE: bool
  const CONFLATE: bool
  const FINISH_ONCE: bool

  // Unused: まだ作っていない枠。InFlight: 画面が開いたがサーバーに届いていない。
  // Open: route.ts が開いて backend を購読している。Closed: route.ts が後始末を済ませた（または開かなかった）。
  // Dropped: 経路が切れていて、サーバーに届かなかった。
  type Phase = Unused | InFlight | Open | Closed | Dropped
  type CloseReason = NotClosed | Aborted | Cancelled | BackendEnded | TcpTimeout | Superseded | Rejected

  type Conn = {
    screen: int,
    attempt: int,
    phase: Phase,
    // 画面とサーバーの間の経路が生きているか。false なら FIN も状態も届かない。
    pathUp: bool,
    // 画面がこの接続を閉じて、もう使っていない。
    abandoned: bool,
    // サーバーへ届いたか（Open を経たか、届いて開かずに閉じたか）。
    arrived: bool,
    // サーバーへ届いた順番（届いていなければ NONE）。
    arrivedAt: int,
    // 資源（backend の購読と ReadableStream）を閉じた回数。
    releases: int,
    reason: CloseReason,
    // backend の WatchState のタスク（gRPC のストリーム）が生きているか。
    backendTask: bool,
    // Next が ReadableStream の cancel を呼んだか（Next は 1 回だけ呼ぶ）。
    cancelCalled: bool,
    // backend から受けた状態の版（1 から）。最後に受けた版。
    produced: int,
    // ReadableStream の列にある版。
    queue: List[int],
    // 列が詰まっている間に持っている最新の版（CONFLATE）。なければ NONE。
    pending: int,
    // Next が読んで送った最後の版。
    lastDelivered: int,
    // 古い版を新しい版の後に送ったか。
    outOfOrder: bool,
    // 送らずに捨てた途中の版の数（CONFLATE の到達確認に使う）。
    skipped: int,
  }

  type Client = {
    // 最後に開いた試行の番号。
    attempt: int,
    // 画面がいま使っている接続。なければ NONE。
    live: int,
    tabOpen: bool,
  }

  pure val NONE: int = -1
  pure val SCREENS: Set[int] = Set(0, 1)
  pure val MAX_CONNS: int = 5
  pure val CONN_IDS: Set[int] = 0.to(MAX_CONNS - 1)
  pure val MAX_EMITS: int = 3
  // ReadableStream の既定の highWaterMark（CountQueuingStrategy の 1）。
  pure val HIGH_WATER_MARK: int = 1

  pure val UNUSED_CONN: Conn = {
    screen: NONE, attempt: 0, phase: Unused, pathUp: true, abandoned: false, arrived: false, arrivedAt: NONE,
    releases: 0, reason: NotClosed, backendTask: false, cancelCalled: false,
    produced: 0, queue: [], pending: NONE, lastDelivered: 0, outOfOrder: false, skipped: 0,
  }

  var conns: int -> Conn
  var nextConn: int
  var clients: int -> Client
  // 画面ごとに、route.ts がいま開いている接続。なければ NONE（SUPERSEDE のときだけ使う）。
  var registry: int -> int
  // サーバーへ届いた要求の数（届いた順番を振る）。
  var arrivals: int

  // ===== 純粋な関数 =====

  pure def listMax(l: List[int]): int = l.foldl(0, (acc, v) => if (v > acc) v else acc)

  // 後始末（route.ts の finish）。FINISH_ONCE なら開いている接続だけを閉じる。
  pure def release(c: Conn, why: CloseReason): Conn = {
    ...c,
    phase: Closed,
    releases: c.releases + 1,
    reason: if (c.reason == NotClosed) why else c.reason,
    backendTask: false,
    pending: NONE,
  }

  pure def canFinish(c: Conn): bool = c.phase == Open

  // registry が指す接続を閉じたら、指す先を外す。別の接続を指していれば触らない。
  pure def unregister(reg: int -> int, c: Conn, id: int): int -> int =
    if (reg.get(c.screen) == id) reg.set(c.screen, NONE) else reg

  pure def opened(c: Conn, seq: int): Conn = { ...c, phase: Open, arrived: true, arrivedAt: seq, backendTask: true }

  // 同じ画面のもっと新しい試行が既にサーバーで開いているか。
  pure def isStale(cs: int -> Conn, reg: int -> int, c: Conn): bool =
    SUPERSEDE and REJECT_STALE and reg.get(c.screen) != NONE and cs.get(reg.get(c.screen)).attempt > c.attempt

  // backend から版 v を受けたときの列と持ち分。
  pure def offer(c: Conn, v: int): Conn =
    if (c.queue.length() < HIGH_WATER_MARK) { ...c, produced: v, queue: c.queue.append(v) }
    else if (CONFLATE) { ...c, produced: v, pending: v, skipped: if (c.pending != NONE) c.skipped + 1 else c.skipped }
    else { ...c, produced: v, queue: c.queue.append(v) }

  // Next が列の先頭を読んで送る。読んで空いたら pull が持ち分を列へ入れる。
  pure def pullOne(c: Conn): Conn = {
    val first = c.queue.head()
    val rest = c.queue.tail()
    val refill = c.phase == Open and c.pending != NONE
    {
      ...c,
      lastDelivered: first,
      outOfOrder: c.outOfOrder or first <= c.lastDelivered,
      queue: if (refill) rest.append(c.pending) else rest,
      pending: if (refill) NONE else c.pending,
    }
  }

  // ===== 状態機械 =====

  action init = all {
    conns' = CONN_IDS.mapBy(_ => UNUSED_CONN),
    nextConn' = 0,
    clients' = SCREENS.mapBy(_ => { attempt: 0, live: NONE, tabOpen: true }),
    registry' = SCREENS.mapBy(_ => NONE),
    arrivals' = 0,
  }

  // 画面が新しい接続を開く（初回、または見張り・onerror で切断したあとの開き直し）。
  // いま使っている接続は閉じる（abandoned）。経路が生きていれば FIN が届く（abortArrives）。
  action connect(s: int): bool = {
    val cl = clients.get(s)
    val id = nextConn
    all {
      cl.tabOpen,
      nextConn < MAX_CONNS,
      conns' = {
        val withOld = if (cl.live != NONE) conns.set(cl.live, { ...conns.get(cl.live), abandoned: true }) else conns
        withOld.set(id, { ...UNUSED_CONN, screen: s, attempt: cl.attempt + 1, phase: InFlight })
      },
      nextConn' = nextConn + 1,
      clients' = clients.set(s, { ...cl, attempt: cl.attempt + 1, live: id }),
      registry' = registry,
      arrivals' = arrivals,
    }
  }

  // 画面（タブ）を閉じる。いま使っている接続は見捨てられる。
  action closeTab(s: int): bool = {
    val cl = clients.get(s)
    all {
      cl.tabOpen,
      conns' = if (cl.live != NONE) conns.set(cl.live, { ...conns.get(cl.live), abandoned: true }) else conns,
      nextConn' = nextConn,
      clients' = clients.set(s, { ...cl, live: NONE, tabOpen: false }),
      registry' = registry,
      arrivals' = arrivals,
    }
  }

  // 経路が切れる（VPN の切断、Wi-Fi の切り替え、スリープ）。半開きになる。
  action breakPath(id: int): bool = {
    val c = conns.get(id)
    all {
      c.phase == InFlight or c.phase == Open,
      c.pathUp,
      conns' = conns.set(id, { ...c, pathUp: false }),
      nextConn' = nextConn,
      clients' = clients,
      registry' = registry,
      arrivals' = arrivals,
    }
  }

  // 経路が切れていて、要求がサーバーに届かない。
  action dropRequest(id: int): bool = {
    val c = conns.get(id)
    all {
      c.phase == InFlight,
      not(c.pathUp),
      conns' = conns.set(id, { ...c, phase: Dropped }),
      nextConn' = nextConn,
      clients' = clients,
      registry' = registry,
      arrivals' = arrivals,
    }
  }

  // 要求がサーバーに届き、route.ts の GET が走る。
  action arrive(id: int): bool = {
    val c = conns.get(id)
    val holder = registry.get(c.screen)
    all {
      c.phase == InFlight,
      c.pathUp,
      if (isStale(conns, registry, c)) all {
        // 開かない。backend を購読しないので、閉じる資源もない。
        conns' = conns.set(id, { ...c, phase: Closed, arrived: true, arrivedAt: arrivals, reason: Rejected }),
        registry' = registry,
      } else if (SUPERSEDE and holder != NONE) all {
        conns' = conns.set(holder, release(conns.get(holder), Superseded)).set(id, opened(c, arrivals)),
        registry' = registry.set(c.screen, id),
      } else if (SUPERSEDE) all {
        conns' = conns.set(id, opened(c, arrivals)),
        registry' = registry.set(c.screen, id),
      } else all {
        conns' = conns.set(id, opened(c, arrivals)),
        registry' = registry,
      },
      nextConn' = nextConn,
      clients' = clients,
      arrivals' = arrivals + 1,
    }
  }

  // 後始末（abort・error・end・超過）。開いている接続だけを閉じる（route.ts の finish の finished）。
  action finishWith(id: int, why: CloseReason): bool = {
    val c = conns.get(id)
    all {
      canFinish(c),
      conns' = conns.set(id, release(c, why)),
      registry' = unregister(registry, c, id),
      arrivals' = arrivals,
      nextConn' = nextConn,
      clients' = clients,
    }
  }

  // 画面が閉じたことがサーバーに届き、request.signal が abort する。
  action abortArrives(id: int): bool = {
    val c = conns.get(id)
    all { c.abandoned, c.pathUp, finishWith(id, Aborted) }
  }

  // backend がストリームを終える（backend の停止など）。
  action backendEnds(id: int): bool = finishWith(id, BackendEnded)

  // 経路が切れたまま、OS が TCP をあきらめて接続を閉じる（Linux の既定で十数分）。
  action tcpTimeout(id: int): bool = {
    val c = conns.get(id)
    all { not(c.pathUp), finishWith(id, TcpTimeout) }
  }

  // Next が応答の本体を cancel する。応答が閉じたあと（abort のあと）か、閉じた画面の接続で起きる。
  // FINISH_ONCE でなければ、修正前の route.ts の cancel のように、閉じたかを見ずに閉じる。
  action nextCancels(id: int): bool = {
    val c = conns.get(id)
    all {
      not(c.cancelCalled),
      (c.phase == Closed and c.reason != Rejected) or (c.phase == Open and c.abandoned and c.pathUp),
      if (FINISH_ONCE) all {
        conns' = conns.set(id, if (canFinish(c)) { ...release(c, Cancelled), cancelCalled: true } else { ...c, cancelCalled: true }),
        registry' = if (canFinish(c)) unregister(registry, c, id) else registry,
      } else all {
        conns' = conns.set(id, { ...release(c, Cancelled), cancelCalled: true }),
        registry' = unregister(registry, c, id),
      },
      nextConn' = nextConn,
      clients' = clients,
      arrivals' = arrivals,
    }
  }

  // backend の WatchState が次の状態を送る（変化の通知、または 15 秒ごとの送り直し）。
  action backendEmits(id: int): bool = {
    val c = conns.get(id)
    all {
      c.phase == Open,
      c.produced < MAX_EMITS,
      conns' = conns.set(id, offer(c, c.produced + 1)),
      nextConn' = nextConn,
      clients' = clients,
      registry' = registry,
      arrivals' = arrivals,
    }
  }

  // Next が列から読み、画面へ書く。経路が生きている間だけ（切れると送り先が詰まって読まない）。
  action nextPulls(id: int): bool = {
    val c = conns.get(id)
    all {
      c.phase == Open or c.phase == Closed,
      c.pathUp,
      c.queue.length() > 0,
      conns' = conns.set(id, pullOne(c)),
      nextConn' = nextConn,
      clients' = clients,
      registry' = registry,
      arrivals' = arrivals,
    }
  }

  action step = {
    nondet s = SCREENS.oneOf()
    nondet id = CONN_IDS.oneOf()
    any {
      connect(s),
      closeTab(s),
      breakPath(id),
      dropRequest(id),
      arrive(id),
      abortArrives(id),
      backendEnds(id),
      tcpTimeout(id),
      nextCancels(id),
      backendEmits(id),
      nextPulls(id),
    }
  }

  // ===== 不変条件 =====

  def openConnsOf(s: int): Set[int] =
    CONN_IDS.filter(id => conns.get(id).screen == s and conns.get(id).phase == Open)

  // 同じ接続の資源を二度閉じない。
  val noDoubleRelease: bool = CONN_IDS.forall(id => conns.get(id).releases <= 1)

  // 画面ごとに、サーバーで開いている接続は 1 本まで。
  val atMostOneOpenPerScreen: bool = SCREENS.forall(s => openConnsOf(s).size() <= 1)

  // 同じ画面の次の接続がサーバーに届いて開いたら、それより前に届いた接続は閉じている
  // （見捨てられた接続は、画面が開き直した接続が届いた時点で閉じる）。
  val closedOnNextArrival: bool =
    CONN_IDS.forall(id => {
      val c = conns.get(id)
      c.phase == Open implies CONN_IDS.forall(other => {
        val d = conns.get(other)
        not(d.screen == c.screen and d.arrivedAt > c.arrivedAt and d.reason != Rejected)
      })
    })

  // 同じ画面のもっと新しい試行がサーバーで開いている間は、遅れて届いた古い試行を開かない。
  val staleNeverOpensBesideNewer: bool =
    CONN_IDS.forall(id => {
      val c = conns.get(id)
      c.phase == Open implies CONN_IDS.forall(other => {
        val d = conns.get(other)
        not(d.screen == c.screen and d.phase == Open and d.attempt > c.attempt)
      })
    })

  // 動いている接続（画面がいま使っている接続）を、サーバーの都合で閉じない。
  // 閉じてよいのは backend の終了と、経路が切れて OS が閉じた場合だけ。
  val liveNotClosedByRelay: bool =
    SCREENS.forall(s => {
      val live = clients.get(s).live
      live != NONE and conns.get(live).phase == Closed implies
        (conns.get(live).reason == BackendEnded or conns.get(live).reason == TcpTimeout)
    })

  // registry が指すのは、開いている同じ画面の接続だけ。
  val registryPointsToOpen: bool =
    SCREENS.forall(s => {
      val held = registry.get(s)
      held == NONE or (conns.get(held).phase == Open and conns.get(held).screen == s)
    })

  // backend のタスクは、開いている接続の分だけ生きている。
  val backendTaskOnlyWhileOpen: bool =
    CONN_IDS.forall(id => conns.get(id).backendTask == (conns.get(id).phase == Open))

  // ReadableStream の列は highWaterMark を超えて積まない。
  val queueBounded: bool = CONN_IDS.forall(id => conns.get(id).queue.length() <= HIGH_WATER_MARK)

  // 古い版を新しい版の後に送らない。
  val deliveredInOrder: bool = CONN_IDS.forall(id => not(conns.get(id).outOfOrder))

  // 開いている接続では、backend から受けた最新の版を捨てない（送ったか、列にあるか、持っている）。
  val latestNeverDropped: bool =
    CONN_IDS.forall(id => {
      val c = conns.get(id)
      c.phase == Open and c.produced > 0 implies
        (c.lastDelivered == c.produced or listMax(c.queue) == c.produced or c.pending == c.produced)
    })

  val safety: bool = all {
    noDoubleRelease,
    atMostOneOpenPerScreen,
    closedOnNextArrival,
    staleNeverOpensBesideNewer,
    liveNotClosedByRelay,
    registryPointsToOpen,
    backendTaskOnlyWhileOpen,
    queueBounded,
    deliveredInOrder,
    latestNeverDropped,
  }

  // ===== 到達の確認（--witnesses で数える） =====

  def anyConn(p: Conn => bool): bool = CONN_IDS.exists(id => p(conns.get(id)))

  val witnessSuperseded: bool = anyConn(c => c.reason == Superseded)
  val witnessRejected: bool = anyConn(c => c.reason == Rejected)
  val witnessSkipped: bool = anyConn(c => c.skipped > 0 and c.lastDelivered == c.produced)
  val witnessTcpTimeout: bool = anyConn(c => c.reason == TcpTimeout)
  val witnessAbortThenCancel: bool = anyConn(c => c.reason == Aborted and c.cancelCalled)
  val witnessAbandonedHalfOpenSuperseded: bool =
    anyConn(c => c.reason == Superseded and not(c.pathUp))
  val witnessLiveDelivers: bool =
    SCREENS.exists(s => clients.get(s).live != NONE and conns.get(clients.get(s).live).lastDelivered >= 2)
}

// 修正前の main。画面ごとの付け替えも、詰まったときの間引きも、cancel の守りもない。
module current {
  import stateStreamRelay(SUPERSEDE = false, REJECT_STALE = false, CONFLATE = false, FINISH_ONCE = false).*
}

// 付け替えはするが、遅れて届いた古い試行を開いてしまう変異。
module noRejectStale {
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = false, CONFLATE = true, FINISH_ONCE = true).*
}

// 付け替えだけで、詰まったときに列へ積み続ける変異。
module noConflate {
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = true, CONFLATE = false, FINISH_ONCE = true).*
}

// cancel が後始末の守りを通らない変異。
module noFinishOnce {
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = true, CONFLATE = true, FINISH_ONCE = false).*
}

// 修正案。
module fixed {
  import stateStreamRelay(SUPERSEDE = true, REJECT_STALE = true, CONFLATE = true, FINISH_ONCE = true).*
}
