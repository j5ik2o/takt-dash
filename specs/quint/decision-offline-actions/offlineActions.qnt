// -*- mode: Bluespec; -*-
// 判断待ち 1 件の操作と、SSE の切断・再接続の状態機械（issue #307）。
//
// 画面は SSE で backend の状態を受け取る。切断中も最後に読めた判断待ちを表示し続ける。
// GUARD = false は修正前（ボタンは送信中だけ無効になり、接続の有無を見ない）。
// GUARD = true は修正後（切断中は、確認を開くトリガーと確定ボタンも無効にし、操作の入口でも
// 止める。「やめる」は送信中だけ押せない）。
// FOCUS_GUARD = true は、フォーカスの復帰を、フォーカスが body にあるときだけ行うこと
// （decideDialogRestore の focusElsewhere）。false は無条件にトリガーへ戻すと仮定したもの。
//
// フォーカスは、トリガー（マージを承認）、ダイアログの見出し・確定・やめる・リンク（プルリクエスト）、
// 項目の外の要素（elsewhere）、どこでもない（body）の 7 つで表す。
module offlineActions {
  const GUARD: bool
  const FOCUS_GUARD: bool

  type Focus = str
  // 送った書き込み。connected は、送った時点で画面が接続中だったか。
  type Sent = { connected: bool }

  pure val DIALOG_FOCUS: Set[Focus] = Set("heading", "confirm", "cancel", "link")

  var connected: bool
  var busy: bool
  var dialog: bool
  var focus: Focus
  // ダイアログを閉じたときトリガーが無効だったら、押せるようになるまでフォーカスの復帰を待つ。
  var pendingRestore: bool
  var sent: List[Sent]
  // 復帰が、利用者が置いた項目の外のフォーカスを上書きしたか。
  var stolen: bool

  // コードの disabled={busy || disconnected} に当たる。修正前は busy だけ。
  def locked(c: bool, b: bool): bool = b or (GUARD and not(c))
  val triggerEnabled: bool = not(dialog) and not(locked(connected, busy))
  val confirmEnabled: bool = dialog and not(locked(connected, busy))
  // 「やめる」と Escape。送信中だけ取り消せない。
  val cancelEnabled: bool = dialog and not(busy)

  // 閉じたダイアログのフォーカスの戻し先（decideDialogRestore）。ダイアログの中にあった
  // フォーカスは、閉じると body へ落ちる。
  action restoreAfterClose(c: bool, b: bool): bool = all {
    dialog' = false,
    stolen' = stolen,
    if (locked(c, b)) all { focus' = "body", pendingRestore' = true }
    else all { focus' = "trigger", pendingRestore' = false },
  }

  // 待っていた復帰を、トリガーが押せるようになった時点で行う。フォーカスが項目の外にあれば
  // 上書きせず、待ちをやめる（FOCUS_GUARD）。
  action resumeRestore(c: bool, b: bool): bool = {
    val due = pendingRestore and not(dialog) and not(locked(c, b))
    val keep = FOCUS_GUARD and focus != "body"
    all {
      focus' = if (due and not(keep)) "trigger" else focus,
      pendingRestore' = if (due) false else pendingRestore,
      stolen' = (stolen or (due and not(keep) and focus == "elsewhere")),
    }
  }

  action init = all {
    connected' = true,
    busy' = false,
    dialog' = false,
    focus' = "body",
    pendingRestore' = false,
    sent' = List(),
    stolen' = false,
  }

  // SSE が切れる。ダイアログの中で無効になった確定ボタンのフォーカスは見出しへ移す。
  // 押せる「やめる」とリンクのフォーカスは動かさない。ダイアログの外で無効になった
  // トリガーのフォーカスは外して body にそろえる。
  action disconnect = all {
    connected,
    connected' = false,
    focus' =
      if (GUARD and dialog and focus == "confirm") "heading"
      else if (GUARD and not(dialog) and focus == "trigger") "body"
      else focus,
    busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent, stolen' = stolen,
  }

  // 次の状態を受け取る。
  action reconnect = all {
    not(connected),
    connected' = true,
    resumeRestore(true, busy),
    busy' = busy, dialog' = dialog, sent' = sent,
  }

  action focusTrigger = all {
    triggerEnabled,
    focus' = "trigger",
    connected' = connected, busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent, stolen' = stolen,
  }

  // 利用者が項目の外（別の判断待ちなど）へフォーカスを移す。ダイアログは閉じ込めるので、
  // 開いている間は外へ出られない。
  action focusElsewhere = all {
    not(dialog),
    focus' = "elsewhere",
    connected' = connected, busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent, stolen' = stolen,
  }

  // 開く。GUARD のときは、操作の入口（acceptsDecisionIntent）でも切断中は受け付けない。
  action open = all {
    triggerEnabled,
    dialog' = true, focus' = "heading", pendingRestore' = false,
    connected' = connected, busy' = busy, sent' = sent, stolen' = stolen,
  }

  action focusInDialog = all {
    dialog,
    nondet target = DIALOG_FOCUS.filter(t =>
      t == "heading" or t == "link" or (t == "confirm" and confirmEnabled) or (t == "cancel" and cancelEnabled)
    ).oneOf()
    focus' = target,
    connected' = connected, busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent, stolen' = stolen,
  }

  // 確定。書き込みを送り、送信中は見出しへフォーカスを移す。
  action confirm = all {
    confirmEnabled,
    busy' = true, focus' = "heading",
    sent' = sent.append({ connected: connected }),
    connected' = connected, dialog' = dialog, pendingRestore' = pendingRestore, stolen' = stolen,
  }

  // 「やめる」か Escape。
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

  action step = any { disconnect, reconnect, focusTrigger, focusElsewhere, open, focusInDialog, confirm, cancel, settle }

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
  val focusInsideOpenDialog: bool = dialog implies DIALOG_FOCUS.contains(focus)
  // 復帰を待つのは、ダイアログが閉じていてトリガーが押せない間だけ。
  val restoreWaitsOnlyWhileLocked: bool = pendingRestore implies (not(dialog) and locked(connected, busy))
  // 復帰は、利用者が項目の外に置いたフォーカスを上書きしない。
  val noFocusOverwrite: bool = not(stolen)

  val safety: bool = all {
    noSendWhileDisconnected,
    focusNotOnDisabled,
    focusInsideOpenDialog,
    restoreWaitsOnlyWhileLocked,
    noFocusOverwrite,
  }
}

module fixed {
  import offlineActions(GUARD = true, FOCUS_GUARD = true).*
}

module current {
  import offlineActions(GUARD = false, FOCUS_GUARD = false).*
}

module fixedWithoutFocusGuard {
  import offlineActions(GUARD = true, FOCUS_GUARD = false).*
}
