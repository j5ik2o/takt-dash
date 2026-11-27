// -*- mode: Bluespec; -*-
// offlineActions.qnt の筋書きテスト。issue #307 と #308 の筋書きを修正前と修正後の遷移でたどる。
module offlineActions_test {
  import offlineActions(GUARD = false, FOCUS_GUARD = false, CONFIRM_REPORT = false) as before from "./offlineActions"
  import offlineActions(GUARD = true, FOCUS_GUARD = true, CONFIRM_REPORT = true) as after from "./offlineActions"
  import offlineActions(GUARD = true, FOCUS_GUARD = false, CONFIRM_REPORT = true) as unguarded from "./offlineActions"
  import offlineActions(GUARD = true, FOCUS_GUARD = true, CONFIRM_REPORT = false) as noConfirm from "./offlineActions"

  // 修正前。切断中でも、最後に読めた判断待ちから確認を開いて確定でき、書き込みが送られる。
  run sendWhileDisconnectedBeforeFixTest =
    before::initWith("merge")
      .then(before::disconnect)
      .then(before::open)
      .then(before::confirm)
      .then(all {
        assert(not(before::noSendWhileDisconnected)),
        before::settle,
      })

  // 修正後。切断中はトリガーを押せない。
  run triggerLockedWhileDisconnectedTest =
    after::init
      .then(after::disconnect)
      .then(all {
        assert(not(after::triggerEnabled)),
        after::open,
      })
      .fail()

  // 修正後。確認を開いたあとで切れたら、確定は押せず、やめるは押せる。確定にあった
  // フォーカスは見出しへ移る。
  run confirmLockedAfterOpenTest =
    after::init
      .then(after::open)
      .then(after::focusInDialog)
      .then(all {
        assert(after::DIALOG_FOCUS.contains(after::focus)),
        after::disconnect,
      })
      .then(all {
        assert(not(after::confirmEnabled)),
        assert(after::cancelEnabled),
        assert(after::focus != "confirm"),
        after::cancel,
      })
      .then(all {
        assert(after::pendingRestore),
        assert(after::focus == "body"),
        after::reconnect,
      })
      .then(all {
        assert(after::triggerEnabled),
        assert(after::focus == "trigger"),
        assert(not(after::pendingRestore)),
        assert(after::sent == List()),
        after::open,
      })

  // 修正後。再接続すれば、確定して書き込みを送れる。
  run sendAfterReconnectTest =
    after::init
      .then(after::disconnect)
      .then(after::reconnect)
      .then(after::open)
      .then(after::confirm)
      .then(all {
        assert(after::sent == List({ connected: true })),
        assert(after::safety),
        after::settle,
      })

  // 修正後。送信中に切れたら、結果が返ってもトリガーは押せず、再接続でフォーカスが戻る。
  run settleWhileDisconnectedTest =
    after::init
      .then(after::open)
      .then(after::confirm)
      .then(after::disconnect)
      .then(after::settle)
      .then(all {
        assert(not(after::dialog)),
        assert(after::pendingRestore),
        after::reconnect,
      })
      .then(all {
        assert(after::focus == "trigger"),
        after::disconnect,
      })

  // 修正後。マージの確認のリンクにあるフォーカスは、切断しても動かさない。リンクはマージの確認に
  // だけある。
  run linkFocusKeptOnDisconnectTest =
    after::initWith("merge")
      .then(after::open)
      .then(after::focusInDialogTo("link"))
      .then(after::disconnect)
      .then(all {
        assert(after::focus == "link"),
        assert(after::safety),
        after::cancel,
      })

  // 作業の依頼の確認にはリンクが無い（対象の行はプロジェクト名だけ）。リンクへは移れない。
  run reportDialogHasNoLinkTest =
    after::initWith("report")
      .then(after::open)
      .then(all {
        assert(not(after::hasLink)),
        after::focusInDialogTo("link"),
      })
      .fail()

  // 修正後。ダイアログの外で、フォーカスのあるトリガーが切断で無効になったら、body へ外す。
  run triggerFocusDroppedOnDisconnectTest =
    after::init
      .then(after::focusTrigger)
      .then(after::disconnect)
      .then(all {
        assert(after::focus == "body"),
        assert(not(after::pendingRestore)),
        after::reconnect,
      })

  // 修正後。切断中に閉じ、別の判断待ちへフォーカスを移してから再接続しても、フォーカスを奪わない。
  run restoreDoesNotOverwriteElsewhereTest =
    after::init
      .then(after::open)
      .then(after::disconnect)
      .then(after::cancel)
      .then(after::focusElsewhere)
      .then(after::reconnect)
      .then(all {
        assert(after::focus == "elsewhere"),
        assert(not(after::pendingRestore)),
        assert(after::noFocusOverwrite),
        after::disconnect,
      })

  // 無条件に戻すと仮定すると、同じ筋書きでフォーカスを奪う。
  run unguardedRestoreOverwritesElsewhereTest =
    unguarded::init
      .then(unguarded::open)
      .then(unguarded::disconnect)
      .then(unguarded::cancel)
      .then(unguarded::focusElsewhere)
      .then(unguarded::reconnect)
      .then(all {
        assert(unguarded::focus == "trigger"),
        assert(not(unguarded::noFocusOverwrite)),
        unguarded::disconnect,
      })

  // 修正後。送信中は「やめる」も Escape も取り消せない。
  run cancelLockedWhileBusyTest =
    after::init
      .then(after::open)
      .then(after::confirm)
      .then(all {
        assert(not(after::cancelEnabled)),
        after::cancel,
      })
      .fail()

  // issue #308 の前（main 54b10a6）。作業の依頼の2択は、押すと確認を開かずにすぐ送る。
  // 確定を1回も押していないのに書き込みが出る。
  run reportSendsWithoutConfirmBeforeFixTest =
    noConfirm::initWith("report")
      .then(noConfirm::pressReport)
      .then(all {
        assert(noConfirm::sent == List({ connected: true })),
        assert(noConfirm::confirms == 0),
        assert(not(noConfirm::sendOnlyOnConfirm)),
        assert(noConfirm::noSendWhileDisconnected),
        noConfirm::settle,
      })

  // issue #308 の前でも、マージは確認を開く。破れるのは作業の依頼だけ。
  run mergeStillConfirmsBeforeFixTest =
    noConfirm::initWith("merge")
      .then(noConfirm::pressReport)
      .fail()

  // 修正後。作業の依頼の2択は、すぐには送らず確認を開く。
  run reportOpensDialogAfterFixTest =
    after::initWith("report")
      .then(all {
        assert(after::opensDialog),
        after::pressReport,
      })
      .fail()

  // 修正後。作業の依頼の確認をやめれば送らず、押したボタンへフォーカスが戻る。
  run reportCancelSendsNothingTest =
    after::initWith("report")
      .then(after::open)
      .then(after::cancel)
      .then(all {
        assert(after::sent == List()),
        assert(after::focus == "trigger"),
        assert(after::safety),
        after::open,
      })

  // 修正後。作業の依頼の確定は1回だけ送り、送信中は確定をもう一度押せない。
  run reportConfirmSendsOnceTest =
    after::initWith("report")
      .then(after::open)
      .then(after::confirm)
      .then(all {
        assert(after::sent == List({ connected: true })),
        assert(after::confirms == 1),
        assert(after::safety),
        after::confirm,
      })
      .fail()

  // 修正後。作業の依頼の確認を開いたまま切れたら確定できず、再接続すれば確定できる。
  run reportConfirmLockedWhileDisconnectedTest =
    after::initWith("report")
      .then(after::open)
      .then(after::disconnect)
      .then(all {
        assert(not(after::confirmEnabled)),
        assert(after::cancelEnabled),
        after::reconnect,
      })
      .then(after::confirm)
      .then(all {
        assert(after::sent == List({ connected: true })),
        assert(after::safety),
        after::settle,
      })
}
