// -*- mode: Bluespec; -*-
// 判断待ち 1 件の操作と、SSE の切断・再接続の状態機械（issue #307）。
//
// 画面は SSE で backend の状態を受け取る。切断中も最後に読めた判断待ちを表示し続ける。
// GUARD = false は修正前（ボタンは送信中だけ無効になり、接続の有無を見ない）。
// GUARD = true は修正後（切断中は、確認を開くトリガーと確定ボタンも無効にする。「やめる」は押せる）。
//
// フォーカスは、トリガー（マージを承認）、ダイアログの見出し・確定・やめる、どこでもない（body）の
// 5 つで表す。無効になった要素にあるフォーカスは、ブラウザでは body へ落ちる。
module offlineActions {
  const GUARD: bool

  type Focus = str
  // 送った書き込み。connected は、送った時点で画面が接続中だったか。
  type Sent = { connected: bool }

  var connected: bool
  var busy: bool
  var dialog: bool
  var focus: Focus
  // ダイアログを閉じたときトリガーが無効だったら、押せるようになるまでフォーカスの復帰を待つ。
  var pendingRestore: bool
  var sent: List[Sent]

  // コードの disabled={busy || disconnected} に当たる。修正前は busy だけ。
  def locked(c: bool, b: bool): bool = b or (GUARD and not(c))
  val triggerEnabled: bool = not(locked(connected, busy))
  val confirmEnabled: bool = dialog and not(locked(connected, busy))
  val cancelEnabled: bool = dialog and not(busy)

  // 閉じたダイアログのフォーカスの戻し先（decideDialogRestore）。
  action restoreAfterClose(c: bool, b: bool): bool = all {
    dialog' = false,
    if (locked(c, b)) all { focus' = "body", pendingRestore' = true }
    else all { focus' = "trigger", pendingRestore' = false },
  }

  action init = all {
    connected' = true,
    busy' = false,
    dialog' = false,
    focus' = "body",
    pendingRestore' = false,
    sent' = List(),
  }

  // SSE が切れる。ダイアログの中で無効になった確定ボタンのフォーカスは見出しへ移す。
  // ダイアログの外で無効になったトリガーのフォーカスは body へ落ちる。
  action disconnect = all {
    connected,
    connected' = false,
    focus' =
      if (GUARD and dialog and focus == "confirm") "heading"
      else if (GUARD and not(dialog) and focus == "trigger") "body"
      else focus,
    busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent,
  }

  // 次の状態を受け取る。待っていたフォーカスの復帰は、トリガーが押せるようになった時点で行う。
  action reconnect = all {
    not(connected),
    connected' = true,
    if (pendingRestore and not(dialog) and not(busy)) all { focus' = "trigger", pendingRestore' = false }
    else all { focus' = focus, pendingRestore' = pendingRestore },
    busy' = busy, dialog' = dialog, sent' = sent,
  }

  action focusTrigger = all {
    not(dialog), triggerEnabled,
    focus' = "trigger",
    connected' = connected, busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent,
  }

  action open = all {
    not(dialog), triggerEnabled,
    dialog' = true, focus' = "heading", pendingRestore' = false,
    connected' = connected, busy' = busy, sent' = sent,
  }

  action focusInDialog = all {
    dialog,
    nondet target = Set("heading", "confirm", "cancel").filter(t =>
      t == "heading" or (t == "confirm" and confirmEnabled) or (t == "cancel" and cancelEnabled)
    ).oneOf()
    focus' = target,
    connected' = connected, busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent,
  }

  // 確定。書き込みを送り、送信中は見出しへフォーカスを移す。
  action confirm = all {
    confirmEnabled,
    busy' = true, focus' = "heading",
    sent' = sent.append({ connected: connected }),
    connected' = connected, dialog' = dialog, pendingRestore' = pendingRestore,
  }

  action cancel = all {
    cancelEnabled,
    restoreAfterClose(connected, busy),
    connected' = connected, busy' = busy, sent' = sent,
  }

  // 送った書き込みの結果が返る。送信中に切れても、送った分は最後まで待つ（範囲の外）。
  action settle = all {
    busy,
    busy' = false,
    restoreAfterClose(connected, false),
    connected' = connected, sent' = sent,
  }

  action step = any { disconnect, reconnect, focusTrigger, open, focusInDialog, confirm, cancel, settle }

  // 不変条件

  // 切断中の画面からは書き込みを送らない。issue の受け入れ条件。
  val noSendWhileDisconnected: bool = sent.foldl(true, (ok, s) => ok and s.connected)
  // 無効なボタンにフォーカスが残らない。
  val focusNotOnDisabled: bool = all {
    focus == "trigger" implies triggerEnabled,
    focus == "confirm" implies confirmEnabled,
    focus == "cancel" implies cancelEnabled,
  }
  // ダイアログが開いている間、フォーカスはダイアログの中にある。
  val focusInsideOpenDialog: bool = dialog implies Set("heading", "confirm", "cancel").contains(focus)
  // 復帰を待つのは、ダイアログが閉じていてトリガーが押せない間だけ。
  val restoreWaitsOnlyWhileLocked: bool = pendingRestore implies (not(dialog) and not(triggerEnabled))

  val safety: bool = all { noSendWhileDisconnected, focusNotOnDisabled, focusInsideOpenDialog, restoreWaitsOnlyWhileLocked }
}

module fixed {
  import offlineActions(GUARD = true).*
}

module current {
  import offlineActions(GUARD = false).*
}
