// -*- mode: Bluespec; -*-
// offlineActions.qnt の筋書きテスト。issue #307 の筋書きを修正前と修正後の遷移でたどる。
module offlineActions_test {
  import offlineActions(GUARD = false, FOCUS_GUARD = false) as before from "./offlineActions"
  import offlineActions(GUARD = true, FOCUS_GUARD = true) as after from "./offlineActions"
  import offlineActions(GUARD = true, FOCUS_GUARD = false) as unguarded from "./offlineActions"

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

  // 修正後。リンクにあるフォーカスは、切断しても動かさない。
  run linkFocusKeptOnDisconnectTest =
    after::init
      .then(after::open)
      .then(all {
        after::focus' = "link",
        after::connected' = after::connected, after::busy' = after::busy, after::dialog' = after::dialog,
        after::pendingRestore' = after::pendingRestore, after::sent' = after::sent, after::stolen' = after::stolen,
      })
      .then(after::disconnect)
      .then(all {
        assert(after::focus == "link"),
        assert(after::safety),
        after::cancel,
      })

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
}
