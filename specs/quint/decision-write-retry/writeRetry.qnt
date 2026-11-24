// -*- mode: Bluespec; -*-
// 判断待ちの書き込み（回答・マージ承認・ゴール登録の承認と却下）の再試行の状態機械（issue #305）。
//
// 登場するのは、利用者の試行（Web か CLI からの1回の要求）、backend、takt の3者。
// takt は要求を受け取ると書き込みを適用し、そのあとで応答を返す。応答が呼び出し側の期限や
// backend の期限に間に合わないと、書き込みは適用済みなのに試行は結果を知らない。
//
// DEDUP = false は修正前の遷移（backend は試行を区別せず、届いた要求を毎回 takt へ送る）。
// DEDUP = true は修正案の遷移（要求は論理操作の鍵を持ち、backend は鍵ごとの台帳を引く）。
// EXPIRY = true は、台帳が完了した記録を忘れること（保持期限・backend の再起動）を許す。
module writeRetry {
  const DEDUP: bool
  const EXPIRY: bool

  type Key = str

  // backend から見た試行の進み。
  type Phase =
    | Arrived     // backend に届いた。まだ takt へ送っていない
    | Writing     // takt へ送った。takt はまだ適用していない
    | Unacked     // takt が適用した。応答はまだ backend に届いていない
    | Orphaned    // backend は takt を待つのをやめた。takt はまだ適用していないが、あとで適用しうる
    | Joined      // 同じ鍵の進行中の書き込みの結果を待っている（修正案だけ）
    | Finished

  // 利用者に見える結果。
  type View = Waiting | SawOk | SawRejected | SawUnknown

  // 鍵ごとの台帳（修正案だけが使う）。
  type Ledger = Absent | InFlight | Succeeded | Unknown

  type Attempt = { key: Key, phase: Phase, view: View, wrote: bool }

  pure val KEYS: Set[Key] = Set("k1", "k2")
  pure val ATTEMPT_IDS: Set[int] = 0.to(3)

  var attempts: int -> Attempt
  var applied: Key -> int
  var ledger: Key -> Ledger

  pure def seen(a: Attempt, v: View): Attempt =
    if (a.view == Waiting) { ...a, view: v } else a

  // 鍵 k の書き込みが決着したとき、結果を待っていた試行へ同じ結果を配る。
  pure def settleJoined(atts: int -> Attempt, k: Key, v: View): int -> Attempt =
    atts.keys().mapBy(i => {
      val a = atts.get(i)
      if (a.key == k and a.phase == Joined) seen({ ...a, phase: Finished }, v) else a
    })

  action init = all {
    attempts' = Map(),
    applied' = KEYS.mapBy(_ => 0),
    ledger' = KEYS.mapBy(_ => Absent),
  }

  // 利用者が操作を送る。同じ鍵の試行は、同じ論理操作の再試行か同時送信を表す。
  action submit(id: int, k: Key): bool = all {
    not(attempts.keys().contains(id)),
    attempts' = attempts.put(id, { key: k, phase: Arrived, view: Waiting, wrote: false }),
    applied' = applied,
    ledger' = ledger,
  }

  // backend が要求を処理し始める。
  action receive(id: int): bool = {
    val a = attempts.get(id)
    all {
      a.phase == Arrived,
      applied' = applied,
      if (not(DEDUP)) all {
        attempts' = attempts.set(id, { ...a, phase: Writing }),
        ledger' = ledger,
      } else match ledger.get(a.key) {
        | Absent => all {
            attempts' = attempts.set(id, { ...a, phase: Writing }),
            ledger' = ledger.set(a.key, InFlight),
          }
        | InFlight => all {
            attempts' = attempts.set(id, { ...a, phase: Joined }),
            ledger' = ledger,
          }
        | Succeeded => all {
            attempts' = attempts.set(id, seen({ ...a, phase: Finished }, SawOk)),
            ledger' = ledger,
          }
        | Unknown => all {
            attempts' = attempts.set(id, seen({ ...a, phase: Finished }, SawUnknown)),
            ledger' = ledger,
          }
      },
    }
  }

  // takt が書き込みを適用する。backend が待つのをやめたあとでも起こりうる。
  action taktApply(id: int): bool = {
    val a = attempts.get(id)
    all {
      a.phase == Writing or a.phase == Orphaned,
      attempts' = attempts.set(id, { ...a, phase: if (a.phase == Writing) Unacked else Finished, wrote: true }),
      applied' = applied.set(a.key, applied.get(a.key) + 1),
      ledger' = ledger,
    }
  }

  // takt の成功の応答が backend に届く。
  action taktAck(id: int): bool = {
    val a = attempts.get(id)
    all {
      a.phase == Unacked,
      attempts' = settleJoined(attempts.set(id, seen({ ...a, phase: Finished }, SawOk)), a.key, SawOk),
      applied' = applied,
      ledger' = if (DEDUP) ledger.set(a.key, Succeeded) else ledger,
    }
  }

  // takt が書き込みを適用せずに拒否する（通常の失敗）。
  action taktReject(id: int): bool = {
    val a = attempts.get(id)
    all {
      a.phase == Writing,
      attempts' = settleJoined(attempts.set(id, seen({ ...a, phase: Finished }, SawRejected)), a.key, SawRejected),
      applied' = applied,
      ledger' = if (DEDUP) ledger.set(a.key, Absent) else ledger,
    }
  }

  // backend が takt の応答を待つのをやめる（backend から takt への期限切れ）。
  action taktTimeout(id: int): bool = {
    val a = attempts.get(id)
    all {
      a.phase == Writing or a.phase == Unacked,
      attempts' = settleJoined(
        attempts.set(id, seen({ ...a, phase: if (a.phase == Writing) Orphaned else Finished }, SawUnknown)),
        a.key,
        SawUnknown
      ),
      applied' = applied,
      ledger' = if (DEDUP) ledger.set(a.key, Unknown) else ledger,
    }
  }

  // 呼び出し側が応答を待つのをやめる（Web や CLI から backend への期限切れ）。
  // 修正前は backend の処理も一緒に打ち切られる。takt へ送る前なら書き込みは起きないが、
  // 送ったあとなら takt は適用を続け、その結果は誰にも届かない。
  // 修正案は backend の書き込みを呼び出しから切り離すので、試行の進みは変わらない。
  action callerTimeout(id: int): bool = {
    val a = attempts.get(id)
    all {
      a.view == Waiting,
      a.phase != Finished,
      attempts' = attempts.set(id, {
        ...a,
        view: SawUnknown,
        phase:
          if (DEDUP) a.phase
          else if (a.phase == Arrived) Finished
          else if (a.phase == Writing) Orphaned
          else if (a.phase == Unacked) Finished
          else a.phase,
      }),
      applied' = applied,
      ledger' = ledger,
    }
  }

  // 台帳が完了した記録を忘れる（保持期限か backend の再起動）。
  action forget(k: Key): bool = all {
    EXPIRY,
    DEDUP,
    ledger.get(k) == Succeeded or ledger.get(k) == Unknown,
    ledger' = ledger.set(k, Absent),
    attempts' = attempts,
    applied' = applied,
  }

  action stutter = all {
    attempts' = attempts,
    applied' = applied,
    ledger' = ledger,
  }

  action step = any {
    nondet id = oneOf(ATTEMPT_IDS)
    nondet k = oneOf(KEYS)
    submit(id, k),
    nondet id = oneOf(attempts.keys())
    any {
      receive(id),
      taktApply(id),
      taktAck(id),
      taktReject(id),
      taktTimeout(id),
      callerTimeout(id),
    },
    nondet k = oneOf(KEYS)
    forget(k),
  }

  // ---- 不変条件（安全性）----

  /// 同じ論理操作（同じ鍵）の書き込みは、何度試行しても takt に1回までしか適用されない。
  val noDuplicateWrite = KEYS.forall(k => applied.get(k) <= 1)

  /// 応答を失った試行があるときに限った noDuplicateWrite。反例が issue の筋書きになる。
  val noDuplicateAfterLostResponse = KEYS.forall(k =>
    attempts.keys().exists(i => attempts.get(i).key == k and attempts.get(i).view == SawUnknown)
      implies applied.get(k) <= 1
  )

  /// 「失敗した」と伝えた試行は、書き込みを適用していない（結果不明と通常の失敗を混ぜない）。
  val rejectedMeansNotWritten = attempts.keys().forall(i =>
    attempts.get(i).view == SawRejected implies not(attempts.get(i).wrote)
  )

  /// 「成功した」と伝えた操作は、takt に適用されている。
  val okMeansWritten = attempts.keys().forall(i =>
    attempts.get(i).view == SawOk implies applied.get(attempts.get(i).key) >= 1
  )

  val safety = all { noDuplicateWrite, rejectedMeansNotWritten, okMeansWritten }

  // ---- 進行の条件（到達できること。--witnesses で数える）----

  /// 応答を失った操作を同じ鍵でやり直すと、適用済みの結果を受け取れる。
  val retryLearnsSuccess = KEYS.exists(k => all {
    applied.get(k) == 1,
    attempts.keys().exists(i => attempts.get(i).key == k and attempts.get(i).view == SawUnknown),
    attempts.keys().exists(i => attempts.get(i).key == k and attempts.get(i).view == SawOk),
  })

  /// ある操作の結果が不明でも、別の正当な操作は適用できる。
  val otherOperationProceeds = all {
    attempts.keys().exists(i => attempts.get(i).key == "k1" and attempts.get(i).view == SawUnknown),
    applied.get("k2") == 1,
  }

  /// 拒否された（適用されなかった）操作は、同じ鍵でやり直して適用できる。
  val rejectedOperationCanBeRetried = KEYS.exists(k => all {
    applied.get(k) == 1,
    attempts.keys().exists(i => attempts.get(i).key == k and attempts.get(i).view == SawRejected),
    attempts.keys().exists(i => attempts.get(i).key == k and attempts.get(i).view == SawOk),
  })
}

// 修正前の遷移。
module current {
  import writeRetry(DEDUP = false, EXPIRY = false).*
}

// 修正案の遷移。台帳は記録を保持し続ける。
module fixed {
  import writeRetry(DEDUP = true, EXPIRY = false).*
}

// 修正案の遷移で、台帳が記録を忘れることを許したもの。修正の限界を確かめるために置く。
module fixedWithExpiry {
  import writeRetry(DEDUP = true, EXPIRY = true).*
}
