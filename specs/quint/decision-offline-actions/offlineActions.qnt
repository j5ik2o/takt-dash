// -*- mode: Bluespec; -*-
// 判断待ち 1 件の操作と、SSE の切断・再接続の状態機械（issue #307）。
//
// 画面は SSE で backend の状態を受け取る。切断中も最後に読めた判断待ちを表示し続ける。
// GUARD = false は修正前（ボタンは送信中だけ無効になり、接続の有無を見ない）。
// GUARD = true は修正後（切断中は、確認を開くトリガーと確定ボタンも無効にし、操作の入口でも
// 止める。「やめる」は送信中だけ押せない）。
// FOCUS_GUARD = true は、フォーカスの復帰を、フォーカスが body にあるときだけ行うこと
// （decideDialogRestore の focusElsewhere）。false は無条件にトリガーへ戻すと仮定したもの。
// CONFIRM_REPORT = true は、作業の依頼の2択（実施した・対応できない）も確認ダイアログを開き、
// 確定したときだけ送ること（issue #308）。false は、押すとすぐに送る（main 54b10a6 まで）。
//
// 項目は、マージの承認（merge）か、作業の依頼の2択（report）のどちらか1件。トリガーは、
// merge なら「マージを承認」、report なら押した方の2択のボタンを表す。
//
// フォーカスは、トリガー（マージを承認）、ダイアログの見出し・確定・やめる・リンク（プルリクエスト）、
// 項目の外の要素（elsewhere）、どこでもない（body）の 7 つで表す。
module offlineActions {
  const GUARD: bool
  const FOCUS_GUARD: bool
  const CONFIRM_REPORT: bool

  type Focus = str
  // 送った書き込み。connected は、送った時点で画面が接続中だったか。
  type Sent = { connected: bool }

  pure val DIALOG_FOCUS: Set[Focus] = Set("heading", "confirm", "cancel", "link")
  pure val ITEMS: Set[str] = Set("merge", "report")

  var connected: bool
  var busy: bool
  var dialog: bool
  var focus: Focus
  // ダイアログを閉じたときトリガーが無効だったら、押せるようになるまでフォーカスの復帰を待つ。
  var pendingRestore: bool
  var sent: List[Sent]
  // 復帰が、利用者が置いた項目の外のフォーカスを上書きしたか。
  var stolen: bool
  // 項目の種類（ITEMS）。初めに決まり、変わらない。
  var kind: str
  // 確定を押した回数（履歴の変数）。
  var confirms: int
  // 押すと確認ダイアログを開く項目か。CONFIRM_REPORT = false では、作業の依頼は開かずに送る。
  val opensDialog: bool = kind == "merge" or CONFIRM_REPORT

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

  action initWith(k: str): bool = all {
    kind' = k,
    confirms' = 0,
    connected' = true,
    busy' = false,
    dialog' = false,
    focus' = "body",
    pendingRestore' = false,
    sent' = List(),
    stolen' = false,
  }

  action init = {
    nondet k = ITEMS.oneOf()
    initWith(k)
  }

  // SSE が切れる。ダイアログの中で無効になった確定ボタンのフォーカスは見出しへ移す。
  // 押せる「やめる」とリンクのフォーカスは動かさない。ダイアログの外で無効になった
  // トリガーのフォーカスは外して body にそろえる。
  action disconnect = all {
    kind' = kind, confirms' = confirms,
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
    kind' = kind, confirms' = confirms,
    not(connected),
    connected' = true,
    resumeRestore(true, busy),
    busy' = busy, dialog' = dialog, sent' = sent,
  }

  action focusTrigger = all {
    kind' = kind, confirms' = confirms,
    triggerEnabled,
    focus' = "trigger",
    connected' = connected, busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent, stolen' = stolen,
  }

  // 利用者が項目の外（別の判断待ちなど）へフォーカスを移す。ダイアログは閉じ込めるので、
  // 開いている間は外へ出られない。
  action focusElsewhere = all {
    kind' = kind, confirms' = confirms,
    not(dialog),
    focus' = "elsewhere",
    connected' = connected, busy' = busy, dialog' = dialog, pendingRestore' = pendingRestore, sent' = sent, stolen' = stolen,
  }

  // 作業の依頼の2択を押すと、確認を開かずにすぐ送る（CONFIRM_REPORT = false）。押したボタンは
  // 送信中で無効になり、コードがフォーカスを外して body にそろえる。
  action pressReport = all {
    not(opensDialog),
    triggerEnabled,
    busy' = true, focus' = "body",
    sent' = sent.append({ connected: connected }),
    kind' = kind, confirms' = confirms,
    connected' = connected, dialog' = dialog, pendingRestore' = pendingRestore, stolen' = stolen,
  }

  // 開く。GUARD のときは、操作の入口（acceptsDecisionIntent）でも切断中は受け付けない。
  action open = all {
    kind' = kind, confirms' = confirms,
    opensDialog,
    triggerEnabled,
    dialog' = true, focus' = "heading", pendingRestore' = false,
    connected' = connected, busy' = busy, sent' = sent, stolen' = stolen,
  }

  action focusInDialog = all {
    kind' = kind, confirms' = confirms,
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
    kind' = kind, confirms' = confirms + 1,
    busy' = true, focus' = "heading",
    sent' = sent.append({ connected: connected }),
    connected' = connected, dialog' = dialog, pendingRestore' = pendingRestore, stolen' = stolen,
  }

  // 「やめる」か Escape。
  action cancel = all {
    kind' = kind, confirms' = confirms,
    cancelEnabled,
    restoreAfterClose(connected, busy),
    connected' = connected, busy' = busy, sent' = sent,
  }

  // 送った書き込みの結果が返る。送信中に切れても、送った分は最後まで待つ（範囲の外）。
  // ダイアログを開かずに送った書き込み（pressReport）は、閉じるものが無く、フォーカスも動かさない。
  action settle = all {
    kind' = kind, confirms' = confirms,
    busy,
    busy' = false,
    if (dialog) restoreAfterClose(connected, false)
    else all { dialog' = false, focus' = focus, pendingRestore' = pendingRestore, stolen' = stolen },
    connected' = connected, sent' = sent,
  }

  action step = any { disconnect, reconnect, focusTrigger, focusElsewhere, open, pressReport, focusInDialog, confirm, cancel, settle }

  // 不変条件

  // 切断中の画面からは書き込みを送らない。issue の受け入れ条件。
  val noSendWhileDisconnected: bool = sent.foldl(true, (ok, s) => ok and s.connected)
  // 書き込みは確定を押したときだけ出て、1回の確定で1回だけ出る（issue #308）。
  val sendOnlyOnConfirm: bool = sent.length() == confirms
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
    sendOnlyOnConfirm,
  }
}

module fixed {
  import offlineActions(GUARD = true, FOCUS_GUARD = true, CONFIRM_REPORT = true).*
}

module current {
  import offlineActions(GUARD = false, FOCUS_GUARD = false, CONFIRM_REPORT = false).*
}

module fixedWithoutFocusGuard {
  import offlineActions(GUARD = true, FOCUS_GUARD = false, CONFIRM_REPORT = true).*
}

// main 54b10a6。issue #307 の修正は入り、作業の依頼の2択は押すとすぐに送る（issue #308 の前）。
module withoutReportConfirm {
  import offlineActions(GUARD = true, FOCUS_GUARD = true, CONFIRM_REPORT = false).*
}
