// -*- mode: Bluespec; -*-
// backend が takt-mcp の HTTP サーバーに作るセッションの寿命の状態機械（issue #310）。
//
// 登場するのは、画面の読み書きの要求（1回の gRPC の RPC）、backend が接続先ごとに持つ
// セッションの置き場（slot）、takt-mcp が持つ生きたセッションの集合の3者。
// takt-mcp はセッションに期限を持たず、DELETE を受けるか再起動するまで持ち続ける。
//
// SHARED = false は修正前の遷移（要求ごとにセッションを作り、閉じない）。
// SHARED = true は修正案の遷移（接続先ごとの slot のセッションを使い回す）。
module httpSession {
  const SHARED: bool

  // 要求の進み。
  type Phase =
    | Arrived     // backend に届いた。まだセッションを持たない
    | Ready       // セッションを持った。まだ道具を送っていない
    | Retrying    // セッションが消えていたので、作り直すところ（1回だけ）
    | Succeeded
    | Failed

  // write は書き込みの要求。resent は接続の失敗で送り直したか。retried は 404 で作り直したか。
  type Request = { phase: Phase, sid: int, write: bool, resent: bool, retried: bool }

  pure val REQUEST_IDS: Set[int] = 0.to(5)
  pure val NONE: int = -1
  // 再起動の回数の上限（状態空間を閉じるため）。
  pure val MAX_RESTARTS: int = 2

  var requests: int -> Request
  // takt-mcp が持つ生きたセッション。
  var live: Set[int]
  // backend が接続先ごとに持つセッション（修正案だけが使う）。
  var slot: int
  var nextSid: int
  var restarts: int

  action init = all {
    requests' = Map(),
    live' = Set(),
    slot' = NONE,
    nextSid' = 0,
    restarts' = 0,
  }

  // 画面の読み書きの要求が backend に届く。
  action arrive(id: int, write: bool): bool = all {
    not(requests.keys().contains(id)),
    requests' = requests.put(id, { phase: Arrived, sid: NONE, write: write, resent: false, retried: false }),
    live' = live, slot' = slot, nextSid' = nextSid, restarts' = restarts,
  }

  // 要求がセッションを得る。修正前は毎回 initialize で作る。修正案は slot にあれば使い、
  // 無ければ作って slot に置く（作る間は slot の鍵を持つので、同時に2つは作らない）。
  action acquire(id: int): bool = {
    val r = requests.get(id)
    all {
      r.phase == Arrived or r.phase == Retrying,
      if (SHARED and slot != NONE) all {
        requests' = requests.set(id, { ...r, phase: Ready, sid: slot }),
        live' = live, slot' = slot, nextSid' = nextSid,
      } else all {
        requests' = requests.set(id, { ...r, phase: Ready, sid: nextSid }),
        live' = live.union(Set(nextSid)),
        slot' = if (SHARED) nextSid else slot,
        nextSid' = nextSid + 1,
      },
      restarts' = restarts,
    }
  }

  // 道具を送り、応答を受け取る。セッションが生きていれば成功。消えていれば 404 で、
  // 1回だけ作り直す。修正案は、slot が自分の使ったセッションのままのときだけ slot を空ける
  // （ほかの要求が先に作り直したセッションを捨てない）。
  action call(id: int): bool = {
    val r = requests.get(id)
    all {
      r.phase == Ready,
      if (live.contains(r.sid)) all {
        requests' = requests.set(id, { ...r, phase: Succeeded }),
        slot' = slot,
      } else all {
        requests' = requests.set(id, { ...r, phase: if (r.retried) Failed else Retrying, retried: true }),
        slot' = if (SHARED and slot == r.sid) NONE else slot,
      },
      live' = live, nextSid' = nextSid, restarts' = restarts,
    }
  }

  // 接続の失敗（I/O エラー）。サーバーが生きたままでも起きる。要求が届いたかは分からない。
  // 読み取りは、同じセッションで1回だけ送り直す。書き込みは送り直さず、失敗として返す。
  // どちらも、セッションと slot には触れない（セッションが消えたかどうかは 404 で知る）。
  action ioError(id: int): bool = {
    val r = requests.get(id)
    all {
      r.phase == Ready,
      requests' = requests.set(id,
        if (r.write or r.resent) { ...r, phase: Failed } else { ...r, resent: true }),
      live' = live, slot' = slot, nextSid' = nextSid, restarts' = restarts,
    }
  }

  // takt-mcp の再起動。生きたセッションはすべて消える。backend の slot は古いまま残る。
  action restart = all {
    restarts < MAX_RESTARTS,
    live' = Set(),
    restarts' = restarts + 1,
    requests' = requests, slot' = slot, nextSid' = nextSid,
  }

  action step = any {
    nondet id = REQUEST_IDS.oneOf()
    any {
      nondet write = Set(true, false).oneOf()
      arrive(id, write),
      all { requests.keys().contains(id), acquire(id) },
      all { requests.keys().contains(id), call(id) },
      all { requests.keys().contains(id), ioError(id) },
    },
    restart,
  }

  // 生きたセッションの数は、要求の数によらず、slot の1つまで。
  val sessionsDoNotGrowWithRequests: bool = live.size() <= 1

  // 生きているセッションは、slot のセッションだけ。
  val onlySlotSessionLives: bool = live.subseteq(Set(slot))

  // 書き込みは、接続の失敗のあとで送り直さない。
  val writesAreNotResent: bool = requests.keys().forall(i => not(requests.get(i).write and requests.get(i).resent))

  val safety: bool = sessionsDoNotGrowWithRequests and onlySlotSessionLives and writesAreNotResent

  // 進行の条件（到達できること。--witnesses で数える）。
  // 再起動のあとに届いた要求が成功する。
  val reconnectsAfterRestart: bool =
    restarts > 0 and requests.keys().exists(i => requests.get(i).phase == Succeeded and requests.get(i).retried)
  // 2つ以上の要求が同じセッションで成功する。
  val sessionIsReused: bool =
    requests.keys().exists(i => requests.keys().exists(j =>
      i != j and requests.get(i).phase == Succeeded and requests.get(j).phase == Succeeded
        and requests.get(i).sid == requests.get(j).sid))
  // 接続の失敗で送り直した読み取りが成功する。
  val readSurvivesIoError: bool =
    requests.keys().exists(i => requests.get(i).resent and requests.get(i).phase == Succeeded)
  val allRequestsSettle: bool =
    requests.keys().size() == REQUEST_IDS.size()
      and requests.keys().forall(i => requests.get(i).phase == Succeeded or requests.get(i).phase == Failed)
}

module current {
  import httpSession(SHARED = false).*
}

module fixed {
  import httpSession(SHARED = true).*
}
