// -*- mode: Bluespec; -*-
// offlineActions.qnt の筋書きテスト。issue #307 の筋書きを修正前と修正後の遷移でたどる。
module offlineActions_test {
  import offlineActions(GUARD = false) as before from "./offlineActions"
  import offlineActions(GUARD = true) as after from "./offlineActions"

  // 修正前。切断中でも、最後に読めた判断待ちから確認を開いて確定でき、書き込みが送られる。
  run sendWhileDisconnectedBeforeFixTest =
    before::init
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
        assert(after::focus == "confirm" or after::focus == "heading" or after::focus == "cancel"),
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
}
