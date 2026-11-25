// -*- mode: Bluespec; -*-
// backend が takt-mcp の HTTP サーバーに作るセッションの寿命の状態機械（issue #310）。
//
// 登場するのは、画面の読み書きの要求（1回の gRPC の RPC）、backend が接続先ごとに持つ
// セッションの置き場（slot）、takt-mcp が持つ生きたセッションの集合の3者。
// takt-mcp はセッションに期限を持たず、DELETE を受けるか停止するまで持ち続ける。
//
// SHARED = false は修正前の遷移（要求ごとにセッションを作り、閉じない）。
// SHARED = true は修正案の遷移（接続先ごとの slot のセッションを使い回す）。
// CLASSIFY = false は、道具を送る段の失敗を、届いたかどうかで分けない遷移。
// CLASSIFY = true は、要求が届いていない失敗（接続を張れない、消えたセッションを
// 作り直せない）を「送る前の失敗」として、結果が不明な失敗と分ける遷移。
module httpSession {
  const SHARED: bool
  const CLASSIFY: bool

  // 要求の進み。
  type Phase =
    | Arrived     // backend に届いた。まだセッションを持たない
    | Ready       // セッションを持った。まだ道具を送っていない（または送り直すところ）
    | Retrying    // セッションが消えていたので、作り直すところ（1回だけ）
    | Succeeded
    | Rejected    // 失敗として返した。書き込みなら「適用されていない」と伝えた
    | Unknown     // 結果が不明として返した（書き込みだけ）

  // write は書き込みの要求。resent は接続の失敗で送り直したか。retried は 404 で作り直したか。
  // delivered は、道具が takt-mcp に届いて処理された回数。
  type Request = { phase: Phase, sid: int, write: bool, resent: bool, retried: bool, delivered: int }

  pure val REQUEST_IDS: Set[int] = 0.to(5)
  pure val NONE: int = -1
  // 停止の回数の上限（状態空間を閉じるため）。
  pure val MAX_STOPS: int = 2

  var requests: int -> Request
  // takt-mcp が動いているか。
  var up: bool
  // takt-mcp が持つ生きたセッション。
  var live: Set[int]
  // backend が接続先ごとに持つセッション（修正案だけが使う）。
  var slot: int
  var nextSid: int
  var stops: int

  action init = all {
    requests' = Map(),
    up' = true,
    live' = Set(),
    slot' = NONE,
    nextSid' = 0,
    stops' = 0,
  }

  // 画面の読み書きの要求が backend に届く。
  action arrive(id: int, write: bool): bool = all {
    not(requests.keys().contains(id)),
    requests' = requests.put(id,
      { phase: Arrived, sid: NONE, write: write, resent: false, retried: false, delivered: 0 }),
    up' = up, live' = live, slot' = slot, nextSid' = nextSid, stops' = stops,
  }

  // 要求がセッションを得る。修正前は毎回 initialize で作る。修正案は slot にあれば何も送らずに
  // 使い、無ければ作って slot に置く（作る間は slot の鍵を持つので、同時に2つは作らない）。
  // takt-mcp が止まっていれば作れない。道具を送る前の確立（Arrived）の失敗は、失敗として返す。
  // 404 のあとの作り直し（Retrying）の失敗は、CLASSIFY = false だと、書き込みでは結果不明になる。
  action acquire(id: int): bool = {
    val r = requests.get(id)
    all {
      r.phase == Arrived or r.phase == Retrying,
      if (SHARED and slot != NONE) all {
        requests' = requests.set(id, { ...r, phase: Ready, sid: slot }),
        live' = live, slot' = slot, nextSid' = nextSid,
      } else if (up) all {
        requests' = requests.set(id, { ...r, phase: Ready, sid: nextSid }),
        live' = live.union(Set(nextSid)),
        slot' = if (SHARED) nextSid else slot,
        nextSid' = nextSid + 1,
      } else all {
        requests' = requests.set(id,
          { ...r, phase: if (r.phase == Retrying and r.write and not(CLASSIFY)) Unknown else Rejected }),
        live' = live, slot' = slot, nextSid' = nextSid,
      },
      up' = up, stops' = stops,
    }
  }

  // 道具を送る。
  // - takt-mcp が止まっていれば、接続を張れない。要求は届かない。読み取りは1回だけ送り直す。
  //   書き込みは送り直さない。CLASSIFY = false だと結果不明、true だと失敗として返す。
  // - セッションが生きていれば、届いて処理され、成功する。
  // - セッションが消えていれば 404（処理されない）で、1回だけ作り直す。修正案は、slot が
  //   自分の使ったセッションのままのときだけ slot を空ける。
  action call(id: int): bool = {
    val r = requests.get(id)
    all {
      r.phase == Ready,
      if (not(up)) all {
        requests' = requests.set(id,
          if (r.write) { ...r, phase: if (CLASSIFY) Rejected else Unknown }
          else if (r.resent) { ...r, phase: Rejected }
          else { ...r, resent: true }),
        slot' = slot,
      } else if (live.contains(r.sid)) all {
        requests' = requests.set(id, { ...r, phase: Succeeded, delivered: r.delivered + 1 }),
        slot' = slot,
      } else all {
        requests' = requests.set(id, { ...r, phase: if (r.retried) Rejected else Retrying, retried: true }),
        slot' = if (SHARED and slot == r.sid) NONE else slot,
      },
      up' = up, live' = live, nextSid' = nextSid, stops' = stops,
    }
  }

  // 道具は届いて処理されたが、応答を受け取れなかった（応答の前の切断、時間切れ）。
  // 読み取りは同じセッションで1回だけ送り直す。書き込みは送り直さず、結果不明として返す。
  // セッションと slot には触れない（セッションが消えたかどうかは 404 で知る）。
  action lostResponse(id: int): bool = {
    val r = requests.get(id)
    all {
      r.phase == Ready,
      up,
      live.contains(r.sid),
      requests' = requests.set(id,
        if (r.write) { ...r, phase: Unknown, delivered: r.delivered + 1 }
        else if (r.resent) { ...r, phase: Rejected, delivered: r.delivered + 1 }
        else { ...r, resent: true, delivered: r.delivered + 1 }),
      up' = up, live' = live, slot' = slot, nextSid' = nextSid, stops' = stops,
    }
  }

  // takt-mcp が止まる。生きたセッションはすべて消える。backend の slot は古いまま残る。
  action stop = all {
    up,
    stops < MAX_STOPS,
    up' = false,
    live' = Set(),
    stops' = stops + 1,
    requests' = requests, slot' = slot, nextSid' = nextSid,
  }

  // takt-mcp が起動する。
  action start = all {
    not(up),
    up' = true,
    requests' = requests, live' = live, slot' = slot, nextSid' = nextSid, stops' = stops,
  }

  action step = any {
    nondet id = REQUEST_IDS.oneOf()
    any {
      nondet write = Set(true, false).oneOf()
      arrive(id, write),
      all { requests.keys().contains(id), acquire(id) },
      all { requests.keys().contains(id), call(id) },
      all { requests.keys().contains(id), lostResponse(id) },
    },
    stop,
    start,
  }

  // 生きたセッションの数は、要求の数によらず、slot の1つまで。
  val sessionsDoNotGrowWithRequests: bool = live.size() <= 1

  // 生きているセッションは、slot のセッションだけ。
  val onlySlotSessionLives: bool = live.subseteq(Set(slot))

  // 書き込みが takt-mcp に届くのは1回まで（結果が不明な書き込みを送り直さない）。
  val writeDeliveredAtMostOnce: bool =
    requests.keys().forall(i => requests.get(i).write implies requests.get(i).delivered <= 1)

  // 「結果は不明」と伝えた書き込みは、takt-mcp に届いている。届いていない書き込みを
  // 結果不明にしない（結果不明は冪等キーの台帳に残り、やり直しを止める）。
  val unknownMeansDelivered: bool =
    requests.keys().forall(i => requests.get(i).phase == Unknown implies requests.get(i).delivered > 0)

  // 「失敗した」と伝えた書き込みは、takt-mcp に届いていない。
  val rejectedWriteNotDelivered: bool =
    requests.keys().forall(i =>
      (requests.get(i).write and requests.get(i).phase == Rejected) implies requests.get(i).delivered == 0)

  val sessionSafety: bool = sessionsDoNotGrowWithRequests and onlySlotSessionLives
  val writeSafety: bool = writeDeliveredAtMostOnce and unknownMeansDelivered and rejectedWriteNotDelivered
  val safety: bool = sessionSafety and writeSafety

  // 進行の条件（到達できること。--witnesses で数える）。
  // 停止と起動のあと、404 を受けた要求が作り直して成功する。
  val reconnectsAfterRestart: bool =
    stops > 0 and requests.keys().exists(i => requests.get(i).phase == Succeeded and requests.get(i).retried)
  // 2つ以上の要求が同じセッションで成功する。
  val sessionIsReused: bool =
    requests.keys().exists(i => requests.keys().exists(j =>
      i != j and requests.get(i).phase == Succeeded and requests.get(j).phase == Succeeded
        and requests.get(i).sid == requests.get(j).sid))
  // 送り直した読み取りが成功する。
  val readSurvivesConnectionFailure: bool =
    requests.keys().exists(i => requests.get(i).resent and requests.get(i).phase == Succeeded)
  // 止まっている間の書き込みが、届いていない失敗として返る。
  val writeRejectedWhileDown: bool =
    requests.keys().exists(i => requests.get(i).write and requests.get(i).phase == Rejected)
  // 応答を失った書き込みが、結果不明として返る。
  val writeUnknownAfterLostResponse: bool =
    requests.keys().exists(i => requests.get(i).phase == Unknown)
  val allRequestsSettle: bool =
    requests.keys().size() == REQUEST_IDS.size()
      and requests.keys().forall(i => {
        val p = requests.get(i).phase
        p == Succeeded or p == Rejected or p == Unknown
      })
}

module current {
  import httpSession(SHARED = false, CLASSIFY = false).*
}

// セッションの共有だけを入れた遷移（PR の最初の版）。
module sharedOnly {
  import httpSession(SHARED = true, CLASSIFY = false).*
}

module fixed {
  import httpSession(SHARED = true, CLASSIFY = true).*
}
