// -*- mode: Bluespec; -*-
// 画面が求める更新間隔の登録簿（RefreshDemand）と、それに従う WatchState の送信と poller の待ちの
// 状態機械（issue #40）。設計は docs/design/issue-40.md の「Quint」の節。
//
// 画面（SCREENS）は WatchState を 1 本ずつ開き、選んだ間隔（RATES、秒）を要求する。backend は
// 画面ごとに要求を 1 つだけ登録簿に持ち、poller は生きている要求の最小値で読み、各接続は
// 自分の間隔で状態を送る。要求は寿命（最後の送信から max(TTL_FACTOR × 適用間隔, TTL_MIN)）で外れる。
// 期限を過ぎた要求は、登録簿に残っていても poller に数えない（refresh.rs の effective）。
//
// 時計は注入した時計 now（1 目盛り = 1 秒）だけを使う。期限（要求の寿命と poller の次の読み取り）は
// 素通りできない: tick は最も近い期限で止まり、期限の来た遷移が先に起きる（tokio::select! の
// sleep_until を表す）。ただし送信タスクが定期の組み立て（assemble_state_for）をしている間は、
// そのタスクは select! の外にいるので、その接続の寿命の期限を時計が越えられる。
//
// 定数による遷移の切り替え（どれも true が設計の案。false はわざと壊した版）:
// - ACQUIRE_AFTER_INITIAL: 初期状態を送り終えてから要求を登録する（false は接続の受け付けで登録）。
// - REJECT_STALE: 同じ画面のもっと大きい試行が登録済みなら、小さい試行を登録しない。
// - CLOSE_SUPERSEDED: 置き換えられた古い接続の送信タスクを閉じる。
// - OWNER_CHECK: guard の解放は、登録がまだ自分を指しているときだけ外す。
// - RELEASE_ON_CLOSE: 接続の終わり（クライアントの切断）で guard を落とす。
// - EXPIRY_URGENT: 寿命の期限を時計が素通りしない（組み立て中の接続を除く）。
// - EXPIRY_CLOSES: 寿命切れで登録を外すだけでなく送信タスクも閉じる。
// - CHECK_AFTER_ASSEMBLY: 定期の組み立ての後、送る前に寿命の期限を確かめ、過ぎていれば送らずに閉じる。
// - RECOMPUTE: 送るたびに、今の下限（minRead）で適用間隔を計算し直す（false は接続の開始で固定）。
// - POLLER_REWAKE: 要求が変わったら poller の待ちを今の実効間隔で計算し直す。
// - SHARED_ATTEMPTS: 画面の試行番号は Provider が持ち、開き直し（レート変更・hidden→visible）でも
//   戻らない（false は openStateStream を呼ぶたびに 1 から数え直す、今の createAttemptUrls）。
module refreshDemand {
  const ACQUIRE_AFTER_INITIAL: bool
  const REJECT_STALE: bool
  const CLOSE_SUPERSEDED: bool
  const OWNER_CHECK: bool
  const RELEASE_ON_CLOSE: bool
  const EXPIRY_URGENT: bool
  const EXPIRY_CLOSES: bool
  const CHECK_AFTER_ASSEMBLY: bool
  const RECOMPUTE: bool
  const POLLER_REWAKE: bool
  const SHARED_ATTEMPTS: bool

  type Phase = Unused | Requested | Assembling | Live | Closed
  type CloseReason = NotClosed | ClientGone | Superseded | Rejected | Expired

  // 1 本の WatchState の接続（backend の送信タスクと、それが持つ DemandGuard）。
  type Conn = {
    screen: int,
    attempt: int,
    rate: int,              // 要求した間隔（秒）
    phase: Phase,
    clientClosed: bool,     // 画面がこの接続を閉じた（backend はまだ気づいていないことがある）
    stalled: bool,          // 送信が詰まっている（Next が読まない・h2 の窓が埋まった）
    assembling: bool,       // 定期の送信の組み立て中（送信タスクは select! の外）
    held: bool,             // DemandGuard を取ったことがある
    sends: int,
    lastSentAt: int,
    applied: int,           // 最後に送った状態の applied_refresh_interval_ms（秒）
    floorEpochAtSend: int,  // 最後に送ったときの minRead の世代
    lateSends: int,         // 寿命の期限を過ぎてから送った回数（poller が数えなくなった後の applied）
    reason: CloseReason,
    releases: int,          // guard を落とした回数
    supersededBy: int,      // 置き換えた接続の試行番号（置き換えられていなければ NONE）
  }

  // 登録簿の 1 件（画面ごと）。conn が NONE なら要求なし。
  type Entry = { conn: int, attempt: int, rate: int, deadline: int }

  // 画面（Provider）の状態。
  type Client = { visible: bool, rate: int, attempt: int, live: int }

  // 遷移ごとに起きた回数（witness 用）。
  type Fired = {
    connect: int, arrive: int, initialSend: int, beginSend: int, deliver: int, stall: int,
    releaseArrives: int, expire: int, changeRate: int, hide: int, show: int, configChange: int,
    pollerRead: int, tick: int,
  }

  pure val NONE: int = -1
  pure val SCREENS: Set[int] = Set(0, 1)
  pure val RATES: Set[int] = Set(1, 2, 5, 10)
  // config の minReadIntervalMs の候補（秒）。範囲は選択肢の最小〜最大。
  pure val MIN_READS: Set[int] = Set(1, 3, 10)
  pure val MAX_RATE: int = 10
  // config の intervalSeconds（画面がないときの読み取り間隔）の候補。2 は下限（3・10）より短い設定。
  pure val CONFIG_INTERVALS: Set[int] = Set(2, 30)
  pure val DEFAULT_CONFIG_INTERVAL: int = 30
  // 要求の寿命 = max(TTL_FACTOR × 適用間隔, TTL_MIN)。TTL_MIN は要求なしの再送の間隔（15 秒）。
  pure val TTL_FACTOR: int = 3
  pure val TTL_MIN: int = 15
  // 画面の無音の見張り（STATE_STREAM_STALE_TIMEOUT_MS = 40 秒）。
  pure val STALE_TIMEOUT: int = 40
  pure val TICKS: Set[int] = Set(1, 4, 15)
  pure val MAX_CONNS: int = 6
  pure val MAX_TIME: int = 200
  pure val CONN_IDS: Set[int] = 0.to(MAX_CONNS - 1)

  pure val EMPTY: Entry = { conn: NONE, attempt: 0, rate: 0, deadline: 0 }
  pure val UNUSED_CONN: Conn = {
    screen: NONE, attempt: 0, rate: 0, phase: Unused, clientClosed: false, stalled: false, assembling: false,
    held: false, sends: 0, lastSentAt: 0, applied: 0, floorEpochAtSend: 0, lateSends: 0, reason: NotClosed,
    releases: 0, supersededBy: NONE,
  }
  pure val NO_FIRES: Fired = {
    connect: 0, arrive: 0, initialSend: 0, beginSend: 0, deliver: 0, stall: 0, releaseArrives: 0,
    expire: 0, changeRate: 0, hide: 0, show: 0, configChange: 0, pollerRead: 0, tick: 0,
  }

  var now: int
  var minRead: int
  var configInterval: int
  var floorEpoch: int
  var clients: int -> Client
  var conns: int -> Conn
  var nextConn: int
  var registry: int -> Entry
  var lastReadAt: int
  // POLLER_REWAKE = false のとき、poller が前回の読み取りで決めた待ち時間。
  var capturedInterval: int
  var lastDemandChangeAt: int
  var fired: Fired

  // ===== 関数 =====

  pure def max2(a: int, b: int): int = if (a > b) a else b
  pure def min2(a: int, b: int): int = if (a < b) a else b

  pure def liveEntries(reg: int -> Entry): Set[Entry] =
    SCREENS.map(s => reg.get(s)).filter(e => e.conn != NONE)

  // 期限の来ていない登録（poller が数える要求）。期限ちょうどの登録は切れたものとして数えない。
  pure def unexpiredEntries(reg: int -> Entry, t: int): Set[Entry] = liveEntries(reg).filter(e => t < e.deadline)

  // poller の実効間隔（refresh.rs の effective）。生きている要求が無ければ設定の間隔。あれば、要求の最小値を
  // 下限（floor）で切り上げ、設定の間隔より遅くはしない（下限は要求にだけ掛かる。D16）。
  pure def effective(reg: int -> Entry, floor: int, config: int, t: int): int = {
    val entries = unexpiredEntries(reg, t)
    if (entries.size() == 0) config
    else min2(config, max2(floor, entries.fold(MAX_RATE, (acc, e) => min2(acc, e.rate))))
  }

  // 接続の送信間隔（refresh.rs の applied_for）。
  pure def appliedFor(rate: int, floor: int): int = max2(rate, floor)

  pure def ttlOf(applied: int): int = max2(TTL_FACTOR * applied, TTL_MIN)

  // guard を落とす（DemandGuard の Drop）。OWNER_CHECK なら登録がまだ自分を指すときだけ外す。
  pure def dropGuard(reg: int -> Entry, c: Conn, id: int): int -> Entry =
    if (not(c.held)) reg
    else if (OWNER_CHECK) { if (reg.get(c.screen).conn == id) reg.set(c.screen, EMPTY) else reg }
    else reg.set(c.screen, EMPTY)

  pure def closed(c: Conn, why: CloseReason): Conn =
    { ...c, phase: Closed, reason: why, releases: if (c.held) c.releases + 1 else c.releases }

  // 登録の判定: 同じ画面にもっと大きい試行が登録済みなら断る。
  pure def isStale(reg: int -> Entry, c: Conn): bool =
    REJECT_STALE and reg.get(c.screen).conn != NONE and reg.get(c.screen).attempt > c.attempt

  // 時計が進んでよい上限（最も近い期限）。組み立て中の接続の寿命は数えない（その送信タスクは
  // select! の外にいて、期限の分岐を待っていない）。
  pure def nextDue(reg: int -> Entry, cs: int -> Conn, readAt: int, eff: int, captured: int): int = {
    val pollerDue = if (POLLER_REWAKE) readAt + eff else readAt + captured
    if (EXPIRY_URGENT)
      liveEntries(reg).filter(e => not(cs.get(e.conn).assembling)).fold(pollerDue, (acc, e) => min2(acc, e.deadline))
    else pollerDue
  }


  // ===== 状態機械 =====

  action init = all {
    now' = 0,
    minRead' = 1,
    configInterval' = DEFAULT_CONFIG_INTERVAL,
    floorEpoch' = 0,
    clients' = SCREENS.mapBy(_ => { visible: true, rate: 1, attempt: 0, live: NONE }),
    conns' = CONN_IDS.mapBy(_ => UNUSED_CONN),
    nextConn' = 0,
    registry' = SCREENS.mapBy(_ => EMPTY),
    lastReadAt' = 0,
    capturedInterval' = DEFAULT_CONFIG_INTERVAL,
    lastDemandChangeAt' = 0,
    fired' = NO_FIRES,
  }

  // 画面が接続を開く（ready かつ visible で接続が無い、または前の接続がサーバー側で閉じた）。
  action connect(s: int): bool = {
    val cl = clients.get(s)
    val id = nextConn
    all {
      cl.visible,
      cl.live == NONE or conns.get(cl.live).phase == Closed,
      nextConn < MAX_CONNS,
      conns' = conns.set(id, { ...UNUSED_CONN, screen: s, attempt: cl.attempt + 1, rate: cl.rate, phase: Requested }),
      nextConn' = nextConn + 1,
      clients' = clients.set(s, { ...cl, attempt: cl.attempt + 1, live: id }),
      fired' = { ...fired, connect: fired.connect + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      registry' = registry, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
      lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  // 登録（acquire）。置き換えた古い接続は CLOSE_SUPERSEDED なら閉じる。
  pure def register(cs: int -> Conn, reg: int -> Entry, id: int, c: Conn, deadline: int): (int -> Conn, int -> Entry) = {
    val old = reg.get(c.screen).conn
    val withNew = reg.set(c.screen, { conn: id, attempt: c.attempt, rate: c.rate, deadline: deadline })
    val heldConn = cs.set(id, { ...c, held: true })
    if (old != NONE and old != id and CLOSE_SUPERSEDED and cs.get(old).phase != Closed) {
      val oc = cs.get(old)
      (heldConn.set(old, { ...closed(oc, Superseded), supersededBy: c.attempt }), dropGuard(withNew, oc, old))
    } else (heldConn, withNew)
  }

  // 要求が backend に届く（届く順は開いた順と限らない）。
  action arrive(id: int): bool = {
    val c = conns.get(id)
    all {
      // 画面が閉じたことは backend にまだ届いていないことがある（clientClosed でも届く）。
      c.phase == Requested,
      if (ACQUIRE_AFTER_INITIAL or isStale(registry, c)) all {
        conns' = conns.set(id, { ...c, phase: Assembling }),
        registry' = registry,
        lastDemandChangeAt' = lastDemandChangeAt,
      } else {
        val r = register(conns, registry, id, { ...c, phase: Assembling }, now + ttlOf(appliedFor(c.rate, minRead)))
        all {
          conns' = r._1,
          registry' = r._2,
          lastDemandChangeAt' = now,
        }
      },
      fired' = { ...fired, arrive: fired.arrive + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, nextConn' = nextConn, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
    }
  }

  // 初期状態を組み立てて送る。ACQUIRE_AFTER_INITIAL ならここで登録する。古い試行なら断って閉じる。
  // 初期の組み立ての時間は、arrive と initialSend の間に tick が入ることで表す。
  action initialSend(id: int): bool = {
    val c = conns.get(id)
    val applied = appliedFor(c.rate, minRead)
    val sent = { ...c, phase: Live, sends: 1, lastSentAt: now, applied: applied, floorEpochAtSend: floorEpoch }
    all {
      c.phase == Assembling,
      if (isStale(registry, c)) all {
        conns' = conns.set(id, closed(c, Rejected)),
        registry' = dropGuard(registry, c, id),
        lastDemandChangeAt' = lastDemandChangeAt,
      } else if (ACQUIRE_AFTER_INITIAL) {
        val r = register(conns, registry, id, sent, now + ttlOf(applied))
        all {
          conns' = r._1,
          registry' = r._2,
          lastDemandChangeAt' = now,
        }
      } else all {
        conns' = conns.set(id, sent),
        registry' =
          if (registry.get(c.screen).conn == id)
            registry.set(c.screen, { ...registry.get(c.screen), deadline: now + ttlOf(applied) })
          else registry,
        lastDemandChangeAt' = lastDemandChangeAt,
      },
      fired' = { ...fired, initialSend: fired.initialSend + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, nextConn' = nextConn, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
    }
  }

  // 送信の期限（前の送信から applied）が来て、最後の状態の組み立てを始める（sleep_until(next_send) の分岐から
  // assemble_state_for へ）。組み立ての間、この送信タスクは select! の外にいる。
  action beginSend(id: int): bool = {
    val c = conns.get(id)
    all {
      c.phase == Live,
      not(c.stalled),
      not(c.assembling),
      now >= c.lastSentAt + c.applied,
      conns' = conns.set(id, { ...c, assembling: true }),
      fired' = { ...fired, beginSend: fired.beginSend + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, nextConn' = nextConn, registry' = registry, lastReadAt' = lastReadAt,
      capturedInterval' = capturedInterval, lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  // 組み立てが終わる。CHECK_AFTER_ASSEMBLY なら、寿命の期限を過ぎていれば送らずに閉じる（guard の Drop）。
  // 期限前なら送り、寿命を延ばす（touch）。
  action deliver(id: int): bool = {
    val c = conns.get(id)
    val e = registry.get(c.screen)
    val expired = e.conn == id and now >= e.deadline
    val applied = if (RECOMPUTE) appliedFor(c.rate, minRead) else c.applied
    all {
      c.phase == Live,
      c.assembling,
      if (CHECK_AFTER_ASSEMBLY and expired) all {
        // assembling は true のまま残す（組み立ての後に閉じた接続を witness で見分けるため）。
        conns' = conns.set(id, closed(c, Expired)),
        registry' = dropGuard(registry, c, id),
        lastDemandChangeAt' = now,
      } else all {
        conns' = conns.set(id, { ...c, assembling: false, sends: c.sends + 1, lastSentAt: now, applied: applied,
          floorEpochAtSend: floorEpoch, lateSends: if (expired) c.lateSends + 1 else c.lateSends }),
        registry' =
          if (e.conn == id) registry.set(c.screen, { ...e, deadline: now + ttlOf(applied) })
          else registry,
        lastDemandChangeAt' = lastDemandChangeAt,
      },
      fired' = { ...fired, deliver: fired.deliver + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, nextConn' = nextConn, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
    }
  }

  // 送信が詰まる（Next が読まない、h2 の窓が埋まる）。詰まりは戻らない（戻る場合は遅れた deliver と同じ）。
  action stall(id: int): bool = {
    val c = conns.get(id)
    all {
      c.phase == Live,
      not(c.stalled),
      not(c.assembling),
      conns' = conns.set(id, { ...c, stalled: true }),
      fired' = { ...fired, stall: fired.stall + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, nextConn' = nextConn, registry' = registry, lastReadAt' = lastReadAt,
      capturedInterval' = capturedInterval, lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  // 画面が今の接続を閉じる（レート変更・hidden の前段）。backend が気づくのは releaseArrives。
  pure def detach(cs: int -> Conn, cl: Client): int -> Conn =
    if (cl.live == NONE) cs
    else if (cs.get(cl.live).phase == Closed) cs
    else cs.set(cl.live, { ...cs.get(cl.live), clientClosed: true })

  // 利用者がレートを変える（storage イベントで別タブから移る場合も同じ）。
  action changeRate(s: int, r: int): bool = {
    val cl = clients.get(s)
    all {
      cl.visible,
      r != cl.rate,
      conns' = detach(conns, cl),
      clients' = clients.set(s, { ...cl, rate: r, live: NONE, attempt: if (SHARED_ATTEMPTS) cl.attempt else 0 }),
      fired' = { ...fired, changeRate: fired.changeRate + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      nextConn' = nextConn, registry' = registry, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
      lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  // タブが hidden になる。接続を閉じる。
  action hide(s: int): bool = {
    val cl = clients.get(s)
    all {
      cl.visible,
      conns' = detach(conns, cl),
      clients' = clients.set(s, { ...cl, visible: false, live: NONE, attempt: if (SHARED_ATTEMPTS) cl.attempt else 0 }),
      fired' = { ...fired, hide: fired.hide + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      nextConn' = nextConn, registry' = registry, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
      lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  // タブが visible に戻る（開き直しは connect）。
  action show(s: int): bool = {
    val cl = clients.get(s)
    all {
      not(cl.visible),
      clients' = clients.set(s, { ...cl, visible: true }),
      fired' = { ...fired, show: fired.show + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      conns' = conns, nextConn' = nextConn, registry' = registry, lastReadAt' = lastReadAt,
      capturedInterval' = capturedInterval, lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  // backend が画面の切断に気づく（sender.closed()）。送信タスクが終わり、guard を落とす。
  action releaseArrives(id: int): bool = {
    val c = conns.get(id)
    all {
      c.clientClosed,
      c.phase != Closed,
      c.phase != Unused,
      conns' = conns.set(id, if (RELEASE_ON_CLOSE) closed(c, ClientGone) else { ...c, phase: Closed, reason: ClientGone }),
      registry' = if (RELEASE_ON_CLOSE) dropGuard(registry, c, id) else registry,
      lastDemandChangeAt' = now,
      fired' = { ...fired, releaseArrives: fired.releaseArrives + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, nextConn' = nextConn, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
    }
  }

  // 寿命切れ（送信タスクの sleep_until(guard.deadline()) の分岐）。登録を外し、EXPIRY_CLOSES なら送信タスクも
  // 閉じる（ストリームが終わり、画面は再接続する）。組み立て中の送信タスクはこの分岐を待っていない。
  action expire(s: int): bool = {
    val e = registry.get(s)
    val c = conns.get(e.conn)
    all {
      e.conn != NONE,
      now >= e.deadline,
      not(c.assembling),
      registry' = registry.set(s, EMPTY),
      conns' = if (EXPIRY_CLOSES) conns.set(e.conn, closed(c, Expired)) else conns,
      lastDemandChangeAt' = now,
      fired' = { ...fired, expire: fired.expire + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, nextConn' = nextConn, lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
    }
  }

  // config が変わる（poller が設定を読み直した周期で RefreshDemand::set_min_read を呼ぶ）。
  // m は minReadIntervalMs、i は intervalSeconds。
  action configChange(m: int, i: int): bool = all {
    m != minRead or i != configInterval,
    minRead' = m,
    configInterval' = i,
    floorEpoch' = if (m != minRead) floorEpoch + 1 else floorEpoch,
    lastDemandChangeAt' = now,
    fired' = { ...fired, configChange: fired.configChange + 1 },
    now' = now, clients' = clients, conns' = conns, nextConn' = nextConn, registry' = registry,
    lastReadAt' = lastReadAt, capturedInterval' = capturedInterval,
  }

  // poller が読む（期限が来たら）。次の待ちは実効間隔。読み取りにかかる時間は 0 として扱う。
  action pollerRead: bool = {
    val eff = effective(registry, minRead, configInterval, now)
    val due = if (POLLER_REWAKE) lastReadAt + eff else lastReadAt + capturedInterval
    all {
      now >= due,
      lastReadAt' = now,
      capturedInterval' = eff,
      fired' = { ...fired, pollerRead: fired.pollerRead + 1 },
      now' = now, minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch,
      clients' = clients, conns' = conns, nextConn' = nextConn, registry' = registry,
      lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  // 時計を進める。最も近い期限を越えない（期限の来た遷移が先に起きる）。
  action tick(d: int): bool = {
    val limit = nextDue(registry, conns, lastReadAt, effective(registry, minRead, configInterval, now), capturedInterval)
    val target = min2(now + d, limit)
    all {
      target > now,
      now < MAX_TIME,
      now' = target,
      fired' = { ...fired, tick: fired.tick + 1 },
      minRead' = minRead, configInterval' = configInterval, floorEpoch' = floorEpoch, clients' = clients,
      conns' = conns, nextConn' = nextConn, registry' = registry, lastReadAt' = lastReadAt,
      capturedInterval' = capturedInterval, lastDemandChangeAt' = lastDemandChangeAt,
    }
  }

  action step = {
    nondet s = SCREENS.oneOf()
    nondet id = CONN_IDS.oneOf()
    nondet r = RATES.oneOf()
    nondet m = MIN_READS.oneOf()
    nondet i = CONFIG_INTERVALS.oneOf()
    nondet d = TICKS.oneOf()
    any {
      connect(s),
      arrive(id),
      initialSend(id),
      beginSend(id),
      deliver(id),
      stall(id),
      changeRate(s, r),
      hide(s),
      show(s),
      releaseArrives(id),
      expire(s),
      configChange(m, i),
      pollerRead,
      tick(d),
    }
  }

  // ===== 不変条件 =====

  pure def isLive(c: Conn): bool = c.phase == Live

  // 1. 生きている（Live の）接続は、どれもその画面の登録の持ち主である。置き換え・寿命切れ・
  //    解放の取り違えで、登録の無い接続が送り続けて applied を偽らない。
  val liveConnsRegistered: bool =
    CONN_IDS.forall(id => isLive(conns.get(id)) implies registry.get(conns.get(id).screen).conn == id)

  // 2. 登録は、閉じていない接続だけを指す（解放の後に要求が残らない）。
  val registryPointsToOpen: bool =
    SCREENS.forall(s => {
      val e = registry.get(s)
      e.conn == NONE or {
        val c = conns.get(e.conn)
        c.phase != Closed and c.screen == s and c.attempt == e.attempt and c.rate == e.rate
      }
    })

  // 3. 登録を置き換えるのは、同じ画面のもっと新しい試行だけ（遅れて届いた古い試行が新しい要求を消さない）。
  //    新しい試行が解放されたあとに古い試行が登録されることはありうる。その接続は画面がもう閉じたもので、
  //    backend が切断に気づく（releaseArrives）か寿命で外れる。
  val supersededOnlyByNewer: bool =
    CONN_IDS.forall(id => conns.get(id).reason == Superseded implies conns.get(id).supersededBy > conns.get(id).attempt)

  // 4. 画面の最新の試行は断られない（204 で EventSource が固まらない）。
  val latestNeverRejected: bool =
    CONN_IDS.forall(id => {
      val c = conns.get(id)
      c.reason == Rejected implies c.attempt < clients.get(c.screen).attempt
    })

  // 5. 寿命を過ぎた登録が残るのは、その送信タスクが組み立て中の間だけ（select! の期限の分岐を待っている
  //    送信タスクの登録は、期限で必ず外れる）。期限を過ぎた登録は poller に数えない（effective）。
  val noOverdueEntry: bool =
    SCREENS.forall(s => {
      val e = registry.get(s)
      e.conn == NONE or now <= e.deadline or conns.get(e.conn).assembling
    })

  // 6. 寿命切れは、少なくとも初期状態を送った接続にしか起きない（初期の組み立て中に切れない）。
  val noExpiryBeforeFirstSend: bool =
    CONN_IDS.forall(id => conns.get(id).reason == Expired implies conns.get(id).sends >= 1)

  // 7. 接続が今の下限の世代で送った状態の applied は、max(要求, 今の下限)。
  val appliedTracksFloor: bool =
    CONN_IDS.forall(id => {
      val c = conns.get(id)
      (isLive(c) and c.floorEpochAtSend == floorEpoch) implies c.applied == appliedFor(c.rate, minRead)
    })

  // 8. poller は、要求が最後に変わった時点か、前回の読み取りから実効間隔のどちらか遅いほうを越えて待たない。
  val pollerNotLate: bool =
    now <= max2(lastReadAt + effective(registry, minRead, configInterval, now), lastDemandChangeAt)

  // 9. 閉じた接続のうち guard を取ったものは、guard を落としている。
  val closedReleasedGuard: bool =
    CONN_IDS.forall(id => {
      val c = conns.get(id)
      (c.phase == Closed and c.held) implies c.releases == 1
    })

  // 10. 寿命の期限を過ぎた（poller がもう数えていない）要求で、applied を付けて送らない。
  val noSendAfterExpiry: bool = CONN_IDS.forall(id => conns.get(id).lateSends == 0)

  val safety: bool = all {
    liveConnsRegistered,
    registryPointsToOpen,
    supersededOnlyByNewer,
    latestNeverRejected,
    noOverdueEntry,
    noExpiryBeforeFirstSend,
    appliedTracksFloor,
    pollerNotLate,
    closedReleasedGuard,
    noSendAfterExpiry,
  }

  // ===== 到達（witness） =====

  // 遷移ごと（各 action が少なくとも 1 回起きる）。
  val firedConnect: bool = fired.connect > 0
  val firedArrive: bool = fired.arrive > 0
  val firedInitialSend: bool = fired.initialSend > 0
  val firedBeginSend: bool = fired.beginSend > 0
  val firedDeliver: bool = fired.deliver > 0
  val firedStall: bool = fired.stall > 0
  val firedChangeRate: bool = fired.changeRate > 0
  val firedHide: bool = fired.hide > 0
  val firedShow: bool = fired.show > 0
  val firedReleaseArrives: bool = fired.releaseArrives > 0
  val firedExpire: bool = fired.expire > 0
  val firedConfigChange: bool = fired.configChange > 0
  val firedPollerRead: bool = fired.pollerRead > 0
  val firedTick: bool = fired.tick > 0

  // 意味のある状態。
  pure def anyConn(cs: int -> Conn, p: Conn => bool): bool = CONN_IDS.exists(id => p(cs.get(id)))
  val witnessSuperseded: bool = anyConn(conns, c => c.reason == Superseded)
  val witnessRejectedStale: bool = anyConn(conns, c => c.reason == Rejected)
  val witnessExpiredAfterStall: bool = anyConn(conns, c => c.reason == Expired and c.stalled)
  // 組み立ての後の確かめで閉じた（deliver の閉じる分岐。expire は組み立て中には起きないので、
  // Expired で assembling のままの接続はこの分岐からしか来ない）。
  val witnessExpiredAfterAssembly: bool = anyConn(conns, c => c.reason == Expired and c.assembling)
  val witnessFloorRaised: bool = anyConn(conns, c => isLive(c) and c.applied > c.rate)
  val witnessOneSecondPoller: bool = effective(registry, minRead, configInterval, now) == 1 and lastReadAt > 0
  val witnessTwoScreensMin: bool =
    SCREENS.forall(s => registry.get(s).conn != NONE) and registry.get(0).rate != registry.get(1).rate
  // 設定の intervalSeconds が下限より短く、生きている要求があっても poller は設定の間隔で読む。
  val witnessConfigFasterThanFloor: bool =
    configInterval < minRead and unexpiredEntries(registry, now).size() > 0
      and effective(registry, minRead, configInterval, now) == configInterval
}

// 設計の案。
module fixed {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 接続の受け付けで登録し、初期の組み立て中に寿命が切れうる変異。
module acquireEarly {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = false, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 遅れて届いた古い試行を登録してしまう変異。
module noRejectStale {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = false, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 置き換えた古い接続の送信タスクを閉じない変異。
module keepSuperseded {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = false, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 古い guard の解放が、置き換え後の新しい要求を消す変異。
module noOwnerCheck {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = false,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 接続が終わっても guard を落とさない変異（guard を送信タスクへ移し忘れる）。
module leakOnClose {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = false, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 寿命の期限を時計が素通りできる変異（期限を周期の掃除でしか見ない）。
module lazyExpiry {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = false, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 寿命切れで登録を外すだけで、送信タスクを閉じない変異（読みの指摘 ③）。
module expiryKeepsTask {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = false, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 定期の組み立ての後に寿命の期限を確かめず、期限を過ぎた要求で送り、touch で延ばし直す変異（2 回目の読みの指摘 5）。
module noPostAssemblyCheck {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = false, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// 適用間隔を接続の開始で固定する変異（minReadIntervalMs の変更が開いている接続に効かない）。
module fixedAtStart {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = false,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = true).*
}

// poller が前回の読み取りで決めた待ちのまま、要求の増加で起きない変異。
module noRewake {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = false, SHARED_ATTEMPTS = true).*
}

// openStateStream を呼ぶたびに試行番号を 1 から数え直す変異（今の createAttemptUrls、読みの指摘 ⑲）。
module resetAttempts {
  import refreshDemand(ACQUIRE_AFTER_INITIAL = true, REJECT_STALE = true, CLOSE_SUPERSEDED = true, OWNER_CHECK = true,
    RELEASE_ON_CLOSE = true, EXPIRY_URGENT = true, EXPIRY_CLOSES = true, CHECK_AFTER_ASSEMBLY = true, RECOMPUTE = true,
    POLLER_REWAKE = true, SHARED_ATTEMPTS = false).*
}
