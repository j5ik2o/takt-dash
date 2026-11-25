// -*- mode: Bluespec; -*-
// issue #310 の筋書きを、修正前と修正案でたどる。
module httpSession_test {
  import httpSession(SHARED = false) as before from "./httpSession"
  import httpSession(SHARED = true) as after from "./httpSession"

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
      .then(after::restart)
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
      .then(after::restart)
      .then(after::call(1)).then(after::acquire(1))
      .then(after::call(2)).then(after::acquire(2))
      .then(after::call(1)).then(after::call(2))
      .expect(after::live.size() == 1 and after::requests.get(1).sid == after::requests.get(2).sid and after::safety)

  // 修正案：接続の失敗では、読み取りは同じセッションで送り直す。セッションは作り直さない。
  run readResendsOnTheSameSessionAfterIoErrorTest =
    after::init
      .then(after::arrive(0, false)).then(after::acquire(0))
      .then(after::ioError(0)).then(after::call(0))
      .expect(after::requests.get(0).phase == after::Succeeded and after::nextSid == 1 and after::safety)

  // 修正案：接続の失敗では、書き込みは送り直さず失敗として返す。セッションは残して使い回す。
  run writeFailsWithoutResendAfterIoErrorTest =
    after::init
      .then(after::arrive(0, true)).then(after::acquire(0))
      .then(after::ioError(0))
      .expect(after::requests.get(0).phase == after::Failed and after::live.size() == 1 and after::safety)
      .then(after::arrive(1, true)).then(after::acquire(1)).then(after::call(1))
      .expect(after::requests.get(1).phase == after::Succeeded and after::nextSid == 1 and after::safety)
}
