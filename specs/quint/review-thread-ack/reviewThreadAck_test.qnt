// -*- mode: Bluespec; -*-
// reviewThreadAck.qnt の筋書きテスト。issue #134・#443・j5ik2o/ci#44 の実例をたどる。
module reviewThreadAck_test {
  import reviewThreadAck(RULE = "v1") as v1 from "./reviewThreadAck"
  import reviewThreadAck(RULE = "window") as w from "./reviewThreadAck"
  import reviewThreadAck(RULE = "unique") as u from "./reviewThreadAck"

  // ---- v1 ----

  // 項目が 2 つある PR で、無関係な一言を 1 つ投稿するだけで両方が承認になる（#134 の穴）。
  // v1 は確認をどの項目にも結び付けず、それより前の全項目を承認する。
  run v1UnrelatedTurnsGreenTest =
    v1::initWith(Set(), Set())
      .then(v1::postAck(true, false, false, Set(), Set(), Set()))
      .then(all {
        assert(v1::gateGreen),
        assert(not(v1::noFalseGreen)),
        v1::editItem(1),
      })

  // ---- ci#43（442d679）の 20 文字の窓 ----

  // j5ik2o/ci#44：両方の項目がリポジトリへのリンクを持ち、作成者が別件の URL を貼って
  // 「確認しました」と書く。URL の頭だけが一致して、全項目が承認になる。
  run windowUrlHeadTurnsGreenTest =
    w::initWith(Set(w::URL_HEAD), Set(w::URL_HEAD))
      .then(w::postAck(true, false, true, Set(), Set(w::URL_HEAD), Set()))
      .then(all {
        assert(w::gateGreen),
        assert(not(w::noFalseGreen)),
        w::editItem(1),
      })

  // 項目 1 を指して定型文を引用した確認が、同じ定型文を持つ項目 2 も承認する（#443）。
  run windowSharedQuoteApprovesOtherTest =
    w::initWith(Set(w::SHARED), Set(w::SHARED))
      .then(w::postAck(true, false, true, Set(), Set(w::SHARED), Set(1)))
      .then(all {
        assert(w::gateGreen),
        assert(not(w::noFalseGreen)),
        w::editItem(1),
      })

  // ---- この設計 ----

  // ci#44 の反例。URL の頭は引用の照合に使わないので、緑にならない。
  run uniqueUrlHeadStaysRedTest =
    u::initWith(Set(u::URL_HEAD), Set(u::URL_HEAD))
      .then(u::postAck(true, false, true, Set(), Set(u::URL_HEAD), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 2 つの項目に共通の定型文の引用は、どちらの項目も承認しない。
  run uniqueSharedQuoteStaysRedTest =
    u::initWith(Set(u::SHARED), Set(u::SHARED))
      .then(u::postAck(true, false, true, Set(), Set(u::SHARED), Set(1)))
      .then(u::postAck(true, false, true, Set(2), Set(), Set(2)))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 定型文でも、PR の項目の中でその項目だけが持つなら、引用で承認できる（#131 の実際の確認）。
  run uniqueSharedQuoteOnSingleItemTurnsGreenTest =
    u::initWith(Set(u::SHARED), Set())
      .then(u::postAck(true, false, true, Set(), Set(u::SHARED), Set(1)))
      .then(u::postAck(true, false, true, Set(2), Set(), Set(2)))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 1 つの確認で、それぞれの項目の固有の文を引用すれば、両方を承認できる。
  run uniqueOwnQuotesTurnGreenTest =
    u::initWith(Set(u::URL_HEAD), Set(u::SHARED))
      .then(u::postAck(true, false, true, Set(), Set("U1", "U2"), Set(1, 2)))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // コメントの id（URL の全体かアンカー）は、定型文の有無によらず承認に使える。
  run uniqueIdsTurnGreenTest =
    u::initWith(Set(u::SHARED, u::URL_HEAD), Set(u::SHARED, u::URL_HEAD))
      .then(u::postAck(true, false, true, Set(1, 2), Set(u::URL_HEAD), Set(1, 2)))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        assert(u::idAckTurnsGreen),
        u::editItem(1),
      })

  // 自動報告（#131 の、本文全体がフェンスの失敗報告）は、引用と確認の語を含んでも承認にならない。
  run uniqueAutoReportStaysRedTest =
    u::initWith(Set(u::SHARED), Set())
      .then(u::postAck(true, true, true, Set(), Set(u::SHARED, u::URL_HEAD), Set(1)))
      .then(u::postAck(true, false, true, Set(2), Set(), Set(2)))
      .then(all {
        assert(not(u::gateGreen)),
        u::editItem(1),
      })

  // 引用だけで確認の語が無ければ緑にならない。
  run uniqueQuoteWithoutAckWordStaysRedTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false, false, Set(), Set("U1", "U2"), Set(1, 2)))
      .then(all {
        assert(not(u::gateGreen)),
        u::editItem(1),
      })

  // 緑のあとで項目が編集されると、承認は無効になり赤へ戻る。確認し直すと緑に戻る。
  run uniqueEditInvalidatesAckTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false, true, Set(1, 2), Set(), Set(1, 2)))
      .then(all { assert(u::gateGreen), u::editItem(1) })
      .then(all {
        assert(not(u::gateGreen)),
        u::postAck(true, false, true, Set(), Set("U1"), Set(1)),
      })
      .then(all {
        assert(u::gateGreen),
        u::editItem(2),
      })
}
