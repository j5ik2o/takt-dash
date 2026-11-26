// -*- mode: Bluespec; -*-
// writeRetry.qnt の筋書きテスト。issue #305 の筋書きを修正前と修正案の遷移でたどる。
module writeRetry_test {
  import writeRetry(DEDUP = false, EXPIRY = false, EMPTY_REPLY_UNKNOWN = false) as before from "./writeRetry"
  import writeRetry(DEDUP = true, EXPIRY = false, EMPTY_REPLY_UNKNOWN = true) as after from "./writeRetry"
  import writeRetry(DEDUP = true, EXPIRY = false, EMPTY_REPLY_UNKNOWN = false) as emptyBefore from "./writeRetry"

  // 修正前。適用済みの書き込みの応答を失い、やり直すと書き込みが重なる。
  run lostResponseRetryDuplicatesBeforeFixTest =
    before::init
      .then(before::submit(0, "k1"))
      .then(before::receive(0))
      .then(before::taktApply(0))
      .then(before::callerTimeout(0))
      .then(before::submit(1, "k1"))
      .then(before::receive(1))
      .then(before::taktApply(1))
      .then(before::taktAck(1))
      .then(all {
        assert(before::applied.get("k1") == 2),
        assert(not(before::noDuplicateWrite)),
        before::stutter,
      })

  // 修正案。同じ筋書きで書き込みは1回のまま、やり直しは適用済みの結果を受け取る。
  run lostResponseRetryWritesOnceAfterFixTest =
    after::init
      .then(after::submit(0, "k1"))
      .then(after::receive(0))
      .then(after::taktApply(0))
      .then(after::callerTimeout(0))
      .then(after::submit(1, "k1"))
      .then(after::receive(1))
      .then(after::taktAck(0))
      .then(all {
        assert(after::applied.get("k1") == 1),
        assert(after::attempts.get(0).view == after::SawUnknown),
        assert(after::attempts.get(1).view == after::SawOk),
        assert(after::safety),
        after::stutter,
      })

  // 修正案。同時に届いた同じ操作は、進行中の書き込みの結果を待ち、送り直さない。
  run concurrentSameOperationWritesOnceAfterFixTest =
    after::init
      .then(after::submit(0, "k1"))
      .then(after::submit(1, "k1"))
      .then(after::receive(0))
      .then(after::receive(1))
      .then(after::taktApply(0))
      .then(after::taktAck(0))
      .then(all {
        assert(after::applied.get("k1") == 1),
        assert(after::attempts.get(1).view == after::SawOk),
        after::stutter,
      })

  // 修正案。takt が応答しなかった操作は結果不明として残り、同じ鍵では送り直さない。
  // takt があとから適用しても1回のまま。別の鍵の操作は通る。
  run unknownOutcomeIsNotResentAfterFixTest =
    after::init
      .then(after::submit(0, "k1"))
      .then(after::receive(0))
      .then(after::taktTimeout(0))
      .then(after::submit(1, "k1"))
      .then(after::receive(1))
      .then(after::taktApply(0))
      .then(after::submit(2, "k2"))
      .then(after::receive(2))
      .then(after::taktApply(2))
      .then(after::taktAck(2))
      .then(all {
        assert(after::applied.get("k1") == 1),
        assert(after::attempts.get(1).view == after::SawUnknown),
        assert(after::applied.get("k2") == 1),
        assert(after::attempts.get(2).view == after::SawOk),
        after::stutter,
      })

  // 修正案。takt が拒否した操作は適用されておらず、同じ鍵でやり直せる。
  run rejectedOperationIsRetriedAfterFixTest =
    after::init
      .then(after::submit(0, "k1"))
      .then(after::receive(0))
      .then(after::taktReject(0))
      .then(after::submit(1, "k1"))
      .then(after::receive(1))
      .then(after::taktApply(1))
      .then(after::taktAck(1))
      .then(all {
        assert(after::applied.get("k1") == 1),
        assert(after::attempts.get(0).view == after::SawRejected),
        assert(after::attempts.get(1).view == after::SawOk),
        after::stutter,
      })

  // issue #395 の修正前。適用済みの書き込みに本文の無い応答が返ると「失敗」と伝えて記録を消し、
  // 同じ鍵のやり直しで書き込みが重なる。
  run emptyReplyRetryDuplicatesBeforeFixTest =
    emptyBefore::init
      .then(emptyBefore::submit(0, "k1"))
      .then(emptyBefore::receive(0))
      .then(emptyBefore::taktApply(0))
      .then(emptyBefore::taktEmptyReply(0))
      .then(emptyBefore::submit(1, "k1"))
      .then(emptyBefore::receive(1))
      .then(emptyBefore::taktApply(1))
      .then(emptyBefore::taktAck(1))
      .then(all {
        assert(emptyBefore::attempts.get(0).view == emptyBefore::SawRejected),
        assert(not(emptyBefore::rejectedMeansNotWritten)),
        assert(emptyBefore::applied.get("k1") == 2),
        assert(not(emptyBefore::noDuplicateWrite)),
        emptyBefore::stutter,
      })

  // issue #395 の修正案。本文の無い応答は結果不明として残り、同じ鍵では送り直さない。
  run emptyReplyIsNotResentAfterFixTest =
    after::init
      .then(after::submit(0, "k1"))
      .then(after::receive(0))
      .then(after::taktApply(0))
      .then(after::taktEmptyReply(0))
      .then(after::submit(1, "k1"))
      .then(after::receive(1))
      .then(all {
        assert(after::applied.get("k1") == 1),
        assert(after::attempts.get(0).view == after::SawUnknown),
        assert(after::attempts.get(1).view == after::SawUnknown),
        assert(after::ledger.get("k1") == after::Unknown),
        assert(after::safety),
        after::stutter,
      })
}
