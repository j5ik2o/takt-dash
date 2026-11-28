// -*- mode: Bluespec; -*-
// reviewThreadAck.qnt の筋書きテスト。issue #134・#443・j5ik2o/ci#44・j5ik2o/ci#43 の残る穴 4 つの実例をたどる。
module reviewThreadAck_test {
  import reviewThreadAck(RULE = "v1") as v1 from "./reviewThreadAck"
  import reviewThreadAck(RULE = "window") as w from "./reviewThreadAck"
  import reviewThreadAck(RULE = "unique") as q from "./reviewThreadAck"
  import reviewThreadAck(RULE = "scoped") as u from "./reviewThreadAck"

  // ---- v1 ----

  // 項目が 2 つある PR で、無関係な一言を 1 つ投稿するだけで両方が承認になる（#134 の穴）。
  // v1 は確認をどの項目にも結び付けず、それより前の全項目を承認する。
  run v1UnrelatedTurnsGreenTest =
    v1::initWith(Set(), Set())
      .then(v1::postAck(true, false, Set(v1::unit(v1::NoWord, Set(), Set())), Set(), Set()))
      .then(all {
        assert(v1::gateGreen),
        assert(not(v1::noFalseGreen)),
        v1::editItem(1),
      })

  // ---- ci#43（442d679・86a443c）の 20 文字の窓 ----

  // j5ik2o/ci#44：両方の項目がリポジトリへのリンクを持ち、作成者が別件の URL を貼って
  // 「確認しました」と書く。URL の頭だけが一致して、全項目が承認になる。
  run windowUrlHeadTurnsGreenTest =
    w::initWith(Set(w::UrlHead), Set(w::UrlHead))
      .then(w::postAck(true, false, Set(w::unit(w::Affirmed, Set(), Set(w::UrlHead))), Set(), Set()))
      .then(all {
        assert(w::gateGreen),
        assert(not(w::noFalseGreen)),
        w::editItem(1),
      })

  // 項目 1 を指して定型文を引用した確認が、同じ定型文を持つ項目 2 も承認する（#443）。
  run windowSharedQuoteApprovesOtherTest =
    w::initWith(Set(w::Shared), Set(w::Shared))
      .then(w::postAck(true, false, Set(w::unit(w::Affirmed, Set(), Set(w::Shared))), Set(1), Set()))
      .then(all {
        assert(w::gateGreen),
        assert(not(w::noFalseGreen)),
        w::editItem(1),
      })

  // ---- 設計 b808a29 に残る穴 4 つ（ci#43 の 86a443c も同じ形） ----

  // 穴 1：まとめ返信「#1 は確認しました。#2 は未対応です。」。語と参照を単位で結ばないので、
  // 未対応と書いた 2 まで承認になる。
  run uniqueScopeLeaksTest =
    q::initWith(Set(), Set())
      .then(q::postAck(true, false,
        Set(q::unit(q::Affirmed, Set(1), Set()), q::unit(q::Pending, Set(2), Set())), Set(1), Set(2)))
      .then(all {
        assert(q::gateGreen),
        assert(not(q::noFalseGreen)),
        q::editItem(1),
      })

  // 穴 2：指摘「Once this is fixed, please rerun …」を引用して質問しただけ。引用の中の fixed を数える。
  run uniqueQuotedWordTest =
    q::initWith(Set(), Set())
      .then(q::postAck(true, false, Set(q::unit(q::Quoted, Set(), Set(q::Own(1)))), Set(), Set()))
      .then(q::postAck(true, false, Set(q::unit(q::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(q::gateGreen),
        assert(not(q::noFalseGreen)),
        q::editItem(1),
      })

  // 穴 3：`This has not been fixed. #issuecomment-1`。
  run uniqueNegatedEnTest =
    q::initWith(Set(), Set())
      .then(q::postAck(true, false, Set(q::unit(q::NegatedEn, Set(1), Set())), Set(), Set(1)))
      .then(q::postAck(true, false, Set(q::unit(q::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(q::gateGreen),
        assert(not(q::noFalseGreen)),
        q::editItem(1),
      })

  // 穴 4：`確認済みではありません #issuecomment-1`。
  run uniqueNegatedJaTest =
    q::initWith(Set(), Set())
      .then(q::postAck(true, false, Set(q::unit(q::NegatedJa, Set(1), Set())), Set(), Set(1)))
      .then(q::postAck(true, false, Set(q::unit(q::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(q::gateGreen),
        assert(not(q::noFalseGreen)),
        q::editItem(1),
      })

  // ---- この設計 ----

  // 穴 1 の同じ入力。2 は「まだ」の単位にしか無いので承認しない。あとで 2 を確認すれば緑。
  run scopedMixedReplyTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false,
        Set(u::unit(u::Affirmed, Set(1), Set()), u::unit(u::Pending, Set(2), Set())), Set(1), Set(2)))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::approvedByMixedAck),
        u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()),
      })
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 「#1 と #2 を確認しました。ただし #2 はまだ直していません。」。肯定の単位が 2 を指していても、
  // 打ち消しの単位が 2 に触れているので 2 は承認しない。
  run scopedVetoTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false,
        Set(u::unit(u::Affirmed, Set(1, 2), Set()), u::unit(u::Pending, Set(2), Set())), Set(1), Set(2)))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::vetoFired),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 穴 2〜4 の同じ入力。引用の中だけの語、打ち消された語では承認しない。
  run scopedQuotedAndNegatedStayRedTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false, Set(u::unit(u::Quoted, Set(), Set(u::Own(1)))), Set(), Set()))
      .then(u::postAck(true, false, Set(u::unit(u::NegatedEn, Set(1), Set())), Set(), Set(1)))
      .then(u::postAck(true, false, Set(u::unit(u::NegatedJa, Set(1), Set())), Set(), Set(1)))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 確認の文に引用が付く形（「確認しました」＋その下の `>` の引用）。引用は付く先の単位に入るので、
  // 引用の中の固有の文で承認できる。
  run scopedQuoteAttachedToAckTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(), Set(u::Own(1), u::Own(2)))), Set(1, 2), Set()))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // ci#44 の反例。URL の頭は引用の照合に使わないので、緑にならない。
  run scopedUrlHeadStaysRedTest =
    u::initWith(Set(u::UrlHead), Set(u::UrlHead))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(), Set(u::UrlHead))), Set(), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 2 つの項目に共通の定型文の引用は、どちらの項目も承認しない。
  run scopedSharedQuoteStaysRedTest =
    u::initWith(Set(u::Shared), Set(u::Shared))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(), Set(u::Shared))), Set(1), Set()))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 定型文でも、PR の項目の中でその項目だけが持つなら、引用で承認できる（#131 の実際の確認）。
  run scopedSharedQuoteOnSingleItemTurnsGreenTest =
    u::initWith(Set(u::Shared), Set())
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(), Set(u::Shared))), Set(1), Set()))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 1 つの確認で、それぞれの項目の固有の文を引用すれば、両方を承認できる。
  run scopedOwnQuotesTurnGreenTest =
    u::initWith(Set(u::UrlHead), Set(u::Shared))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(), Set(u::Own(1), u::Own(2)))), Set(1, 2), Set()))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // コメントの id（URL の全体かアンカー）は、定型文の有無によらず承認に使える。
  // リストの項目ごとに確認の語を書く形（「- #1 を確認しました」「- #2 を確認しました」）。
  run scopedIdsTurnGreenTest =
    u::initWith(Set(u::Shared, u::UrlHead), Set(u::Shared, u::UrlHead))
      .then(u::postAck(true, false,
        Set(u::unit(u::Affirmed, Set(1), Set(u::UrlHead)), u::unit(u::Affirmed, Set(2), Set(u::UrlHead))), Set(1, 2), Set()))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        assert(u::idAckTurnsGreen),
        u::editItem(1),
      })

  // 自動報告（#131 の、本文全体がフェンスの失敗報告）は、引用と確認の語を含んでも承認にならない。
  run scopedAutoReportStaysRedTest =
    u::initWith(Set(u::Shared), Set())
      .then(u::postAck(true, true, Set(u::unit(u::Affirmed, Set(), Set(u::Shared, u::UrlHead))), Set(1), Set()))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        u::editItem(1),
      })

  // 引用だけで確認の語が無ければ緑にならない。
  run scopedQuoteWithoutAckWordStaysRedTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false, Set(u::unit(u::NoWord, Set(), Set(u::Own(1), u::Own(2)))), Set(1, 2), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        u::editItem(1),
      })

  // 緑のあとで項目が編集されると、承認は無効になり赤へ戻る。確認し直すと緑に戻る。
  run scopedEditInvalidatesAckTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(1, 2), Set())), Set(1, 2), Set()))
      .then(all { assert(u::gateGreen), u::editItem(1) })
      .then(all {
        assert(not(u::gateGreen)),
        u::postAck(true, false, Set(u::unit(u::Affirmed, Set(), Set(u::Own(1)))), Set(1), Set()),
      })
      .then(all {
        assert(u::gateGreen),
        u::editItem(2),
      })

  // ---- 3 回目の直し：Codex の利用上限の通知を既定で外す ----

  // 通知（作者と本文の頭の両方が一致）は項目にならない。もう 1 つの項目を id で確認すれば緑。
  run scopedCodexNoticeExcludedTest =
    u::initWithKinds(Set(u::Shared), Set(), u::CodexNotice, u::Plain)
      .then(all {
        assert(not(u::gateGreen)),
        u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()),
      })
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        assert(u::greenWithNoticeExcluded),
        u::editItem(1),
      })

  // Codex のほかのコメント（作者だけ一致）と、ほかの作者が通知の文で始めたコメント（本文だけ一致）は外さない。
  // どちらも確認するまで赤。
  run scopedCodexOtherAndHeadByOtherStayItemsTest =
    u::initWithKinds(Set(), Set(), u::CodexOther, u::HeadByOther)
      .then(all {
        assert(not(u::gateGreen)),
        u::postAck(true, false, Set(u::unit(u::Affirmed, Set(1), Set())), Set(1), Set()),
      })
      .then(all {
        assert(not(u::gateGreen)),
        u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()),
      })
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        assert(u::greenWithCodexOtherItem),
        assert(u::greenWithHeadByOtherItem),
        u::editItem(1),
      })

  // 外した通知と同じ定型文を持つ項目（通知を引用したレビュー）は、通知を指した定型文の引用では承認しない。
  // 外した通知も「ほかの項目」に数えるので、定型文はその項目だけのものにならない。URL を貼れば緑。
  run scopedNoticeQuoteDoesNotApproveOtherTest =
    u::initWithKinds(Set(u::Shared), Set(u::Shared), u::CodexNotice, u::Plain)
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(), Set(u::Shared))), Set(1), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()),
      })
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // b808a29 の規則（unique）には既定の除外が無い。通知は項目のまま残り、確認するまで赤。
  run uniqueKeepsNoticeAsItemTest =
    q::initWithKinds(Set(), Set(), q::CodexNotice, q::Plain)
      .then(q::postAck(true, false, Set(q::unit(q::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(not(q::gateGreen)),
        q::editItem(1),
      })

  // ---- 4 回目の直し：URL と HTML コメントの中の語を判定から除く ----

  // 穴 5：`<!-- fixed --> #issuecomment-1`。HTML コメント（か URL）の中だけの語では承認しない。
  // 同じ確認を作成者の文の語で書き直せば緑になる。
  run scopedMaskedWordStaysRedTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false, Set(u::unit(u::MaskedWord, Set(1), Set())), Set(), Set()))
      .then(u::postAck(true, false, Set(u::unit(u::Affirmed, Set(2), Set())), Set(2), Set()))
      .then(all {
        assert(not(u::gateGreen)),
        assert(u::noFalseGreen),
        u::postAck(true, false, Set(u::unit(u::Affirmed, Set(1), Set())), Set(1), Set()),
      })
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        u::editItem(1),
      })

  // 穴 6：`https://github.com/acme/pending/pull/7#issuecomment-1 を確認しました`。打ち消しの語が URL（か
  // HTML コメントのボットの印）の中にだけある確認は、作成者の確認として承認する。
  run scopedMaskedCueTurnsGreenTest =
    u::initWith(Set(), Set())
      .then(u::postAck(true, false,
        Set(u::unit(u::AffirmedMaskedCue, Set(1), Set()), u::unit(u::AffirmedMaskedCue, Set(2), Set())), Set(1, 2), Set()))
      .then(all {
        assert(u::gateGreen),
        assert(u::noFalseGreen),
        assert(u::idAckTurnsGreen),
        u::editItem(1),
      })
}
