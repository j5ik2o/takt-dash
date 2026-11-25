// -*- mode: Bluespec; -*-
// issue #310 の筋書きを、修正前と修正案でたどる。
module httpSession_test {
  import httpSession(SHARED = false, CLASSIFY = false) as before from "./httpSession"
  import httpSession(SHARED = true, CLASSIFY = false) as shared from "./httpSession"
  import httpSession(SHARED = true, CLASSIFY = true) as after from "./httpSession"

  // 修正前：3回の画面の読み取りが、3つのセッションを残す。
  run screenReadsLeaveSessionsBeforeFixTest =
    before::init
      .then(before::arrive(0, false)).then(before::acquire(0)).then(before::call(0))
      .then(before::arrive(1, false)).then(before::acquire(1)).then(before::call(1))
      .then(before::arrive(2, false)).then(before::acquire(2)).then(before::call(2))
      .expect(before::live.size() == 3 and not(before::sessionsDoNotGrowWithRequests))

  // 修正案：3回の画面の読み取りが、同じ1つのセッションを使う。
  run screenReadsShareOneSessionTest =
    after::init
      .then(after::arrive(0, false)).then(after::acquire(0)).then(after::call(0))
      .then(after::arrive(1, false)).then(after::acquire(1)).then(after::call(1))
      .then(after::arrive(2, false)).then(after::acquire(2)).then(after::call(2))
      .expect(after::live.size() == 1 and after::safety and after::sessionIsReused)

  // 修正案：takt-mcp の再起動のあと、次の要求は 404 を受けて作り直し、成功する。
  run reconnectsAfterRestartTest =
    after::init
      .then(after::arrive(0, false)).then(after::acquire(0)).then(after::call(0))
      .then(after::stop).then(after::start)
      .then(after::arrive(1, false)).then(after::acquire(1)).then(after::call(1))
      .expect(after::requests.get(1).phase == after::Retrying and after::slot == after::NONE)
      .then(after::acquire(1)).then(after::call(1))
      .expect(after::requests.get(1).phase == after::Succeeded and after::live.size() == 1 and after::safety)

  // 修正案：同じ古いセッションで 404 を受けた2つの要求は、作り直したセッションを1つだけ共有する。
  run concurrentRecoveryCreatesOneSessionTest =
    after::init
      .then(after::arrive(0, false)).then(after::acquire(0)).then(after::call(0))
      .then(after::arrive(1, false)).then(after::acquire(1))
      .then(after::arrive(2, false)).then(after::acquire(2))
      .then(after::stop).then(after::start)
      .then(after::call(1)).then(after::acquire(1))
      .then(after::call(2)).then(after::acquire(2))
      .then(after::call(1)).then(after::call(2))
      .expect(after::live.size() == 1 and after::requests.get(1).sid == after::requests.get(2).sid and after::safety)

  // 修正案：応答を失った読み取りは、同じセッションで送り直す。セッションは作り直さない。
  run readResendsOnTheSameSessionTest =
    after::init
      .then(after::arrive(0, false)).then(after::acquire(0))
      .then(after::lostResponse(0)).then(after::call(0))
      .expect(after::requests.get(0).phase == after::Succeeded and after::nextSid == 1 and after::safety)

  // 修正案：応答を失った書き込みは、送り直さず結果不明として返す。セッションは残して使い回す。
  run writeIsUnknownWithoutResendAfterLostResponseTest =
    after::init
      .then(after::arrive(0, true)).then(after::acquire(0))
      .then(after::lostResponse(0))
      .expect(after::requests.get(0).phase == after::Unknown and after::live.size() == 1 and after::safety)
      .then(after::arrive(1, true)).then(after::acquire(1)).then(after::call(1))
      .expect(after::requests.get(1).phase == after::Succeeded and after::nextSid == 1 and after::safety)

  // セッションの共有だけを入れた版：確立済みのセッションがあると、止まっている takt-mcp への
  // 書き込みは、何も届いていないのに結果不明になる（独立レビューの指摘）。
  run unsentWriteBecomesUnknownWithoutClassificationTest =
    shared::init
      .then(shared::arrive(0, false)).then(shared::acquire(0)).then(shared::call(0))
      .then(shared::stop)
      .then(shared::arrive(1, true)).then(shared::acquire(1)).then(shared::call(1))
      .expect(shared::requests.get(1).phase == shared::Unknown and shared::requests.get(1).delivered == 0
        and not(shared::unknownMeansDelivered))

  // 修正案：同じ筋書きで、届いていない書き込みは失敗として返る。
  run unsentWriteIsRejectedTest =
    after::init
      .then(after::arrive(0, false)).then(after::acquire(0)).then(after::call(0))
      .then(after::stop)
      .then(after::arrive(1, true)).then(after::acquire(1)).then(after::call(1))
      .expect(after::requests.get(1).phase == after::Rejected and after::safety)

  // 修正案：404 のあと、セッションを作り直せなかった書き込みも、失敗として返る。
  run writeWhoseSessionCannotBeRecreatedIsRejectedTest =
    after::init
      .then(after::arrive(0, true)).then(after::acquire(0))
      .then(after::stop).then(after::start)
      .then(after::call(0))
      .expect(after::requests.get(0).phase == after::Retrying)
      .then(after::stop)
      .then(after::acquire(0))
      .expect(after::requests.get(0).phase == after::Rejected and after::safety)
}
