// -*- mode: Bluespec; -*-
// issue #364 の 2 点目の筋書きを、修正前と修正案でたどる。
module sessionEndpoint_test {
  import sessionEndpoint(MATCH_ENDPOINT = false) as before from "./sessionEndpoint"
  import sessionEndpoint(MATCH_ENDPOINT = true) as after from "./sessionEndpoint"

  // 修正前：接続先 1 で解決済みの要求が、接続先 2 の反映のあとに来ると、旧接続先の置き場を
  // 登録し直し、新しい接続先の置き場を追い出す。
  run staleRequestReregistersOldEndpointBeforeFixTest =
    before::init
      .then(before::reflect)
      .then(before::resolve(0))
      .then(before::editConfig(2)).then(before::reflect)
      .then(before::resolve(1)).then(before::acquire(1))
      .expect(before::slot.endpoint == 2 and before::safety)
      .then(before::acquire(0))
      .expect(before::slot.endpoint == 1 and not(before::slotMatchesReflected))

  // 修正案：同じ筋書きで、旧設定の要求はその要求だけのセッションを使い、slot は新しい接続先のまま。
  run staleRequestUsesItsOwnSessionTest =
    after::init
      .then(after::reflect)
      .then(after::resolve(0))
      .then(after::editConfig(2)).then(after::reflect)
      .then(after::resolve(1)).then(after::acquire(1))
      .then(after::acquire(0))
      .expect(after::slot.endpoint == 2 and after::requests.get(0).sid != after::slot.sid and after::safety)
      // 旧設定の要求が終わると、そのセッションは閉じる。
      .then(after::finish(0))
      .expect(not(after::sessions.get(after::requests.get(0).sid).live) and after::slot.endpoint == 2 and after::safety)

  // 修正案：新しい設定の要求は、反映のあとも同じ slot を共有する。
  run requestsWithReflectedEndpointShareTheSlotTest =
    after::init
      .then(after::editConfig(2)).then(after::reflect)
      .then(after::resolve(0)).then(after::acquire(0))
      .then(after::resolve(1)).then(after::acquire(1))
      .expect(after::requests.get(0).sid == after::requests.get(1).sid and after::slotShared and after::safety)

  // 修正案：反映の前は照合する設定が無いので、要求の接続先で slot を作る（修正前と同じ）。
  run beforeTheFirstReflectionTheRequestRegistersTest =
    after::init
      .then(after::resolve(0)).then(after::acquire(0))
      .expect(after::slot.sid == after::requests.get(0).sid and after::safety)

  // 修正案：反映より先に新しい設定で解決した要求は、反映の前の接続先と違うので、その要求だけの
  // セッションを使う。次の反映のあとの要求から slot を共有する。
  run newConfigRequestBeforeReflectionIsEphemeralTest =
    after::init
      .then(after::reflect)
      .then(after::resolve(0)).then(after::acquire(0))
      .then(after::editConfig(2))
      .then(after::resolve(1)).then(after::acquire(1))
      .expect(after::slot.endpoint == 1 and after::requests.get(1).sid != after::slot.sid and after::safety)
      .then(after::reflect)
      .expect(after::slot.sid == after::NONE and after::safety)
      .then(after::resolve(2)).then(after::acquire(2))
      .expect(after::slot.endpoint == 2 and after::safety)

  // 到達条件 staleRequestServed は、設定を変える前に終わった要求を「旧設定の要求」に数えない。
  run requestFinishedBeforeTheEditIsNotStaleTest =
    after::init
      .then(after::reflect)
      .then(after::resolve(0)).then(after::acquire(0)).then(after::finish(0))
      .then(after::editConfig(2)).then(after::reflect)
      .expect(not(after::staleRequestServed) and after::safety)

  // 到達条件 staleRequestServed は、反映のあとに旧接続先のまま置き場を得て終わった要求を数える。
  run staleRequestServedCountsAcquisitionAfterReflectTest =
    after::init
      .then(after::reflect)
      .then(after::resolve(0))
      .then(after::editConfig(2)).then(after::reflect)
      .then(after::acquire(0)).then(after::finish(0))
      .expect(after::staleRequestServed and after::safety)

  // 到達条件 oldSlotClosedOnReflect は、旧設定の要求だけのセッションを閉じても数えない
  // （反映の時点で slot が無ければ、反映は何も手放していない）。
  run ephemeralCloseIsNotASlotReleaseTest =
    after::init
      .then(after::reflect)
      .then(after::resolve(0))
      .then(after::editConfig(2)).then(after::reflect)
      .then(after::acquire(0)).then(after::finish(0))
      .expect(not(after::oldSlotClosedOnReflect) and after::safety)

  // 到達条件 oldSlotClosedOnReflect は、反映が手放した slot のセッションが閉じたことを数える。
  run reflectReleasesTheOldSlotTest =
    after::init
      .then(after::reflect)
      .then(after::resolve(0)).then(after::acquire(0)).then(after::finish(0))
      .then(after::editConfig(2)).then(after::reflect)
      .expect(after::oldSlotClosedOnReflect and after::slot.sid == after::NONE and after::safety)

  // 修正案：設定から消えたサーバーは、これまでどおり登録し直さない。
  run removedServerIsNotReregisteredTest =
    after::init
      .then(after::reflect)
      .then(after::resolve(0))
      .then(after::editConfig(after::REMOVED)).then(after::reflect)
      .then(after::acquire(0))
      .expect(after::slot.sid == after::NONE and after::safety)
}
