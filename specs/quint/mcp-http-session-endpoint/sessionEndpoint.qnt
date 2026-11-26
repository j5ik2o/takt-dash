// -*- mode: Bluespec; -*-
// 常駐 HTTP サーバー 1 台のセッションの置き場（slot）と、反映済みの接続先の状態機械（issue #364）。
//
// 登場するのは、設定ファイル、画面の読み書きの要求（1 回の gRPC の RPC）、backend の
// レジストリ（反映済みの接続先と slot）、takt-mcp に残るセッションの 4 者。
// 要求は、届いた時点の設定ファイルから接続先を解決して持ち、あとでレジストリに置き場を求める。
// 定期読み取りは周期ごとに設定ファイルを読んでレジストリへ反映する（retain_configured）。
// 解決と置き場の取得の間に設定が変わって反映されると、要求は旧設定の接続先のまま来る。
//
// MATCH_ENDPOINT = false は修正前の遷移。反映済みの設定と ID だけを照合し、接続先が違う置き場は
// 要求の接続先で作り直して slot を置き換える。
// MATCH_ENDPOINT = true は修正案の遷移。反映済みの接続先と要求の接続先も照合し、違えば
// その要求だけが使うセッションを渡して、slot には触れない。
module sessionEndpoint {
  const MATCH_ENDPOINT: bool

  // 接続先。1 と 2 は HTTP の接続先（URL とトークンの組）。REMOVED は、設定から消えた
  // （または stdio へ変わった）ことを表す。UNCONFIGURED は、レジストリへまだ一度も反映していないこと。
  pure val ENDPOINTS: Set[int] = Set(1, 2)
  pure val REMOVED: int = -1
  pure val UNCONFIGURED: int = -2
  pure val NONE: int = -1

  pure val REQUEST_IDS: Set[int] = 0.to(3)
  // 設定ファイルを書き換える回数の上限（状態空間を閉じるため）。
  pure val MAX_EDITS: int = 2

  type Phase =
    | Resolved   // 設定ファイルから接続先を解決した。まだ置き場を持たない
    | Holding    // 置き場（セッション）を持って、takt-mcp へ要求を送っている
    | Done       // 要求が終わり、置き場を手放した

  // staleAtAcquire は、置き場を求めた時点で、要求の接続先が反映済みの接続先と違ったか。
  type Request = { phase: Phase, endpoint: int, sid: int, staleAtAcquire: bool }

  // セッション。endpoint はどの接続先に作ったか。live は takt-mcp にまだ残っているか。
  type Session = { endpoint: int, live: bool }

  type Slot = { endpoint: int, sid: int }

  var file: int
  var reflected: int
  var slot: Slot
  var sessions: int -> Session
  var requests: int -> Request
  var nextSid: int
  var edits: int
  // 反映（reflect）が slot から手放したセッション。
  var releasedByReflect: Set[int]

  pure val EMPTY_SLOT: Slot = { endpoint: NONE, sid: NONE }

  // ---- 純粋な計算 ----

  // 要求 id が持っているセッションの集合。
  pure def heldSids(rs: int -> Request): Set[int] =
    rs.keys().filter(i => rs.get(i).phase == Holding).map(i => rs.get(i).sid)

  // 持ち主（slot と、置き場を持つ要求）がいなくなったセッションを閉じる。実装では、最後の
  // HttpSession が消えたときに Drop が DELETE を送る。
  pure def closeUnowned(ss: int -> Session, s: Slot, rs: int -> Request): int -> Session = {
    val owned = heldSids(rs).union(if (s.sid == NONE) Set() else Set(s.sid))
    ss.keys().mapBy(sid => {
      val session = ss.get(sid)
      if (owned.contains(sid)) session else { ...session, live: false }
    })
  }

  // 要求の接続先で、slot を共有できるか。
  pure def sharesSlot(s: Slot, endpoint: int): bool = s.sid != NONE and s.endpoint == endpoint

  // 要求に、その要求だけが使うセッションを渡すか（slot に触れない）。
  // 反映の前（UNCONFIGURED）は、照合する設定が無いので登録します。
  pure def ephemeral(reflectedEndpoint: int, endpoint: int): bool =
    reflectedEndpoint != UNCONFIGURED
      and (reflectedEndpoint == REMOVED or (MATCH_ENDPOINT and reflectedEndpoint != endpoint))

  // 反映で slot を残すか。接続先が設定と同じ slot だけを残します。
  pure def keepSlot(s: Slot, endpoint: int): Slot =
    if (s.sid != NONE and s.endpoint == endpoint) s else EMPTY_SLOT

  // ---- 状態機械 ----

  action init = all {
    file' = 1,
    reflected' = UNCONFIGURED,
    slot' = EMPTY_SLOT,
    sessions' = Map(),
    requests' = Map(),
    nextSid' = 0,
    edits' = 0,
    releasedByReflect' = Set(),
  }

  // 設定ファイルの接続先を書き換える（URL・トークンの変更、HTTP から外す）。
  action editConfig(endpoint: int): bool = all {
    edits < MAX_EDITS,
    endpoint != file,
    file' = endpoint,
    edits' = edits + 1,
    reflected' = reflected, slot' = slot, sessions' = sessions, requests' = requests, nextSid' = nextSid,
    releasedByReflect' = releasedByReflect,
  }

  // 定期読み取りが設定ファイルをレジストリへ反映する。接続先が変わった slot は手放す。
  action reflect: bool = {
    val nextSlot = keepSlot(slot, file)
    all {
      reflected' = file,
      slot' = nextSlot,
      sessions' = closeUnowned(sessions, nextSlot, requests),
      releasedByReflect' =
        if (slot.sid != NONE and nextSlot.sid == NONE) releasedByReflect.union(Set(slot.sid)) else releasedByReflect,
      file' = file, requests' = requests, nextSid' = nextSid, edits' = edits,
    }
  }

  // 要求が届き、その時点の設定ファイルから HTTP の接続先を解決する。
  action resolve(id: int): bool = all {
    not(requests.keys().contains(id)),
    file != REMOVED,
    requests' = requests.put(id, { phase: Resolved, endpoint: file, sid: NONE, staleAtAcquire: false }),
    file' = file, reflected' = reflected, slot' = slot, sessions' = sessions, nextSid' = nextSid, edits' = edits,
    releasedByReflect' = releasedByReflect,
  }

  // 要求がレジストリに置き場を求める（http_session）。
  action acquire(id: int): bool = {
    val r = { ...requests.get(id), staleAtAcquire: reflected != UNCONFIGURED and reflected != requests.get(id).endpoint }
    all {
      requests.keys().contains(id),
      r.phase == Resolved,
      if (ephemeral(reflected, r.endpoint)) all {
        // その要求だけが使うセッション。slot は変えません。
        requests' = requests.set(id, { ...r, phase: Holding, sid: nextSid }),
        sessions' = sessions.put(nextSid, { endpoint: r.endpoint, live: true }),
        slot' = slot,
        nextSid' = nextSid + 1,
      } else if (sharesSlot(slot, r.endpoint)) all {
        requests' = requests.set(id, { ...r, phase: Holding, sid: slot.sid }),
        sessions' = sessions, slot' = slot, nextSid' = nextSid,
      } else {
        // 置き場を作って slot に入れる。前の slot は手放します。
        val nextSlot = { endpoint: r.endpoint, sid: nextSid }
        val nextRequests = requests.set(id, { ...r, phase: Holding, sid: nextSid })
        all {
          requests' = nextRequests,
          slot' = nextSlot,
          sessions' = closeUnowned(sessions.put(nextSid, { endpoint: r.endpoint, live: true }), nextSlot, nextRequests),
          nextSid' = nextSid + 1,
        }
      },
      file' = file, reflected' = reflected, edits' = edits, releasedByReflect' = releasedByReflect,
    }
  }

  // 要求が終わり、置き場を手放す。
  action finish(id: int): bool = {
    val r = requests.get(id)
    val nextRequests = requests.set(id, { ...r, phase: Done })
    all {
      requests.keys().contains(id),
      r.phase == Holding,
      requests' = nextRequests,
      sessions' = closeUnowned(sessions, slot, nextRequests),
      file' = file, reflected' = reflected, slot' = slot, nextSid' = nextSid, edits' = edits,
      releasedByReflect' = releasedByReflect,
    }
  }

  action step = {
    nondet id = REQUEST_IDS.oneOf()
    nondet endpoint = ENDPOINTS.union(Set(REMOVED)).oneOf()
    any {
      editConfig(endpoint),
      reflect,
      resolve(id),
      acquire(id),
      finish(id),
    }
  }

  // ---- 不変条件 ----

  // 反映済みの設定があれば、slot は反映済みの接続先のセッションだけ。旧接続先の置き場を
  // 登録し直して、新しい置き場を追い出さない。
  val slotMatchesReflected: bool =
    reflected == UNCONFIGURED or slot.sid == NONE or slot.endpoint == reflected

  // takt-mcp に残っているセッションは、slot か、置き場を持つ要求のものだけ。
  val noOrphanSession: bool = {
    val owned = heldSids(requests).union(if (slot.sid == NONE) Set() else Set(slot.sid))
    sessions.keys().forall(sid => sessions.get(sid).live implies owned.contains(sid))
  }

  // 置き場を持つ要求は、自分が解決した接続先のセッションを使う。
  val requestUsesItsEndpoint: bool =
    requests.keys().forall(i => {
      val r = requests.get(i)
      r.phase == Holding implies sessions.get(r.sid).endpoint == r.endpoint
    })

  // slot に登録されたセッションは takt-mcp に残っている。
  val slotSessionIsLive: bool = slot.sid == NONE or sessions.get(slot.sid).live

  val safety: bool = slotMatchesReflected and noOrphanSession and requestUsesItsEndpoint and slotSessionIsLive

  // ---- 進行の条件（--witnesses） ----

  // 旧設定の要求が、反映のあとに（反映済みの接続先と違う接続先のまま）置き場を得て終わる
  // （拒まずに処理する）。
  val staleRequestServed: bool =
    requests.keys().exists(i => requests.get(i).phase == Done and requests.get(i).staleAtAcquire)
  // 2 つの要求が同じ slot のセッションを共有している。
  val slotShared: bool =
    slot.sid != NONE
      and requests.keys().exists(i => requests.keys().exists(j =>
        i != j and requests.get(i).phase == Holding and requests.get(j).phase == Holding
          and requests.get(i).sid == slot.sid and requests.get(j).sid == slot.sid))
  // 接続先の変更が反映され、新しい接続先の slot ができる。
  val slotMovedToNewEndpoint: bool = slot.sid != NONE and slot.endpoint != 1 and reflected == slot.endpoint
  // 反映で旧接続先の slot を手放し、そのセッションを閉じる。
  val oldSlotClosedOnReflect: bool = releasedByReflect.exists(sid => not(sessions.get(sid).live))
}

module current {
  import sessionEndpoint(MATCH_ENDPOINT = false).*
}

module fixed {
  import sessionEndpoint(MATCH_ENDPOINT = true).*
}
