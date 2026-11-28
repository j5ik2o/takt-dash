// -*- mode: Bluespec; -*-
// 「Check unresolved comments」の承認判定（issue #134、#443、j5ik2o/ci#44、j5ik2o/ci#43 の残る穴 4 つ）の最小モデル。
//
// 対象は、未解決スレッドが 0 件のときに残る「未承認のトップレベルコメント／レビュー要約」（項目）。
// 判定はコメントの集合に対する純関数で、ここで遷移として表すのは、コメントの投稿・項目の編集という
// 入力の並びに対して、公開される関門の結論が赤と緑の間を動くことだけ。
//
// 確認の語の現れ方（Word）には、4 回目の直し（2026-11-28）で、URL か HTML コメントの中だけにある語の 2 つを足した
// （MaskedWord・AffirmedMaskedCue。ci#43 のレビューで見つかった、設計 06770ed の jq の穴 2 つ）。
//
// 確認（ack）は投稿の時点ではどの項目にも結び付けない。確認は 1 つか 2 つの単位（段落・リストの項目・節。
// 引用とフェンスは付く先の単位に入る）に分かれ、単位ごとに、確認の語の現れ方（Word）と、含むコメントの id、
// 含む文の断片を持つ。どの項目を承認するかは規則 RULE が項目の側の本文と突き合わせて計算する。
// 作成者が「どの項目を指すつもりだったか」（intended）、「まだ」と書いた項目（pending）、項目が編集された
// 時刻の記録（editLog）は規則からは見えず、仕様と環境の前提だけが読む。
//
// RULE:
//   "v1"     いまの v1。項目より後ろの、承認できる人のコメントなら内容によらず承認。
//   "window" j5ik2o/ci#43（442d679・86a443c）。本文のどこかの確認の語と、id の完全一致か 20 文字の窓の
//            どれかが項目の本文に含まれれば承認。窓は URL の頭や、複数の項目に共通の定型文にも当たる。
//   "unique" 設計 b808a29。本文のどこかの確認の語と、id の完全一致か、URL を除いた本文の断片で、
//            PR の項目の中でその項目だけが持つもの。語と参照を単位で結ばず、引用・助動詞を挟む否定・
//            後ろに付く否定の語も確認と数える（ci#43 の残る穴 4 つ）。
//   "scoped" この設計（2 回目の直し）。打ち消されない確認の語と参照が同じ単位にあり、同じ確認のどの
//            打ち消しの単位もその項目に触れていないときだけ承認。
//   それ以外の値では関門は緑にならない（定数の綴りの誤りを witness の gateGreen が 0 件で拾う）。
//
// 項目の種類（Kind、3 回目の直し）。v2 は Codex の利用上限の通知を共有の既定で外す。規則が読むのは、作者が
// Codex か（byCodex）と、本文が通知の頭で始まるか（noticeHead）の 2 つの観測だけで、両方が真のときだけ外す。
// 仕様は真の種類（kind）を読む。外すのは scoped（v2）だけで、v1・window・unique には既定の除外が無い。
module reviewThreadAck {
  const RULE: str

  type ItemId = int
  // 本文の断片。20 文字以上の連続部分を 1 つの記号に畳む。
  //   Own(i): 項目 i だけが持つ散文。
  //   Shared: 定型文。Codex の要約の見出しや利用上限の通知など、複数の項目に現れうる散文。
  //   UrlHead: URL の頭（`https://github.com/j` の 20 文字）。リポジトリへのリンクを持つ本文なら必ず含む。
  type Frag = Own(ItemId) | Shared | UrlHead
  // 項目の真の種類。規則は読まない（仕様と、観測を決める init だけが読む）。
  //   Plain:       作成者以外の人・ボットの、ふつうの項目。
  //   CodexNotice: Codex の利用上限の通知（作者が Codex で、本文が通知の頭で始まる）。
  //   CodexOther:  Codex のほかのコメント（本物のレビュー・要約・指摘）。本文の途中に通知の文があってもよい。
  //   HeadByOther: ほかの作者の、通知の文で始まるコメント（通知を写して投稿した人・ボット）。
  type Kind = Plain | CodexNotice | CodexOther | HeadByOther
  type Item = { updatedAt: int, frags: Set[Frag], kind: Kind, byCodex: bool, noticeHead: bool }
  // 単位の中の確認の語の現れ方。文字列の水準での分類（jq の clauses・polarity）の結果を観測として持つ。
  //   Affirmed:  作成者が自分の文で書いた、打ち消されない確認の語がある。
  //   Quoted:    確認の語が引用（行頭 `>` の行、「」『』“” の中、インラインのコード）の中にだけある（穴 2）。
  //   NegatedEn: 英語の語が、助動詞や副詞を挟んだ否定・条件・未来で打ち消されている
  //              （`has not been fixed`、`hasn't been fixed`、`once this is fixed`。穴 3）。
  //   NegatedJa: 日本語の語が、後ろに付く否定で打ち消されている（`確認済みではありません`。穴 4）。
  //   Pending:   確認の語は無く、まだだと書いている（`未対応です`、`まだです`、`has not reviewed`）。
  //   NoWord:    どれも無い。
  //   MaskedWord: 確認の語が URL か HTML コメントの中にだけある（`<!-- fixed --> #10`、
  //              `https://github.com/acme/fixed/pull/7#issuecomment-10`。穴 5。4 回目の直し）。
  //   AffirmedMaskedCue: 作成者が自分の文で書いた打ち消されない確認の語があり、打ち消し・まだ・条件の語は
  //              URL か HTML コメントの中にだけある（`https://github.com/acme/pending/pull/7#issuecomment-10 を確認しました`、
  //              ボットの印 `<!-- review-state: pending -->` を含む確認。穴 6。4 回目の直し）。
  type Word = Affirmed | Quoted | NegatedEn | NegatedJa | Pending | NoWord | MaskedWord | AffirmedMaskedCue
  type Unit = { word: Word, ids: Set[ItemId], frags: Set[Frag] }
  type Ack = {
    at: int,
    // 承認できる人（作成者、Codex の確認、作成者が Bot のときの OWNER 等）による投稿か。
    eligible: bool,
    // 自動報告（既知の自動投稿、本文がフェンスだけ、フェンスの外のマーカー）か。
    autoReport: bool,
    // 本文の単位（1 つか 2 つ）。
    units: Set[Unit],
    // 作成者が承認するつもりだった項目。規則は読まない。仕様だけが読む。
    intended: Set[ItemId],
    // 作成者が「まだ」と書いた項目。規則は読まない。環境の前提だけが読む。
    pending: Set[ItemId],
  }
  // 単位の極性。規則が Word をどう読むか。
  type Pol = Pos | Neg | Neutral
  // 直前に起きた遷移。witness で各 action に届くことを確かめるためだけに持つ。
  type Step = Started | Posted | Edited

  pure val ITEMS: Set[ItemId] = Set(1, 2)
  pure val FRAGS: Set[Frag] = Set(Own(1), Own(2), Shared, UrlHead)
  pure val WORDS: Set[Word] = Set(Affirmed, Quoted, NegatedEn, NegatedJa, Pending, NoWord, MaskedWord, AffirmedMaskedCue)
  pure val BOOLS: Set[bool] = Set(true, false)
  // 項目が固有の文のほかに持ちうる断片の組。
  pure val EXTRA_FRAGS: Set[Set[Frag]] = Set(Shared, UrlHead).powerset()
  pure val KINDS: Set[Kind] = Set(Plain, CodexNotice, CodexOther, HeadByOther)

  pure def unit(w: Word, ids: Set[ItemId], fs: Set[Frag]): Unit = { word: w, ids: ids, frags: fs }

  var clock: int
  var items: ItemId -> Item
  var acks: Set[Ack]
  // 項目の編集の記録（項目と時刻）。仕様だけが読む。規則は items の updatedAt を読む。
  var editLog: Set[(ItemId, int)]
  var lastStep: Step

  // ---- 現れ方の前提（確認の本文に、何が意図と無関係に現れうるか） ----
  // 規則の polarity は使わず、Word を直接読む（規則を壊しても前提が一緒に動かないように）。
  pure def saysNotYet(w: Word): bool = w == NegatedEn or w == NegatedJa or w == Pending
  // 作成者が自分の文で、打ち消されない確認の語を書いた単位。URL・HTML コメントの中の打ち消しは、作成者の打ち消しではない。
  pure def authorWord(w: Word): bool = w == Affirmed or w == AffirmedMaskedCue

  // 単位 u が項目 i に触れている（id、i の固有の文、i が持つ定型文のどれか）。
  pure def mentions(its: ItemId -> Item, i: ItemId, u: Unit): bool =
    u.ids.contains(i)
      or u.frags.contains(Own(i))
      or (u.frags.contains(Shared) and its.get(i).frags.contains(Shared))

  // 単位 u の id と固有の文は集合 s の項目のもので、定型文は s のどれかが持つ。
  pure def refersWithin(its: ItemId -> Item, u: Unit, s: Set[ItemId]): bool = and {
    u.ids.subseteq(s),
    ITEMS.forall(i => u.frags.contains(Own(i)) implies s.contains(i)),
    u.frags.contains(Shared) implies ITEMS.exists(i => s.contains(i) and its.get(i).frags.contains(Shared)),
  }

  // 「まだ」と書いた項目の前提。
  // - 打ち消しの単位（NegatedEn・NegatedJa・Pending）の id と固有の文は、まだと書いた項目のもの。
  // - まだと書いた項目は、打ち消しの単位のどれかで触れている。
  // - 定型文は、それを持つ項目がある PR にしか現れない（どの単位でも）。
  pure def pendingOk(its: ItemId -> Item, units: Set[Unit], pending: Set[ItemId]): bool = and {
    units.forall(u => saysNotYet(u.word) implies refersWithin(its, u, pending)),
    pending.forall(i => units.exists(u => saysNotYet(u.word) and mentions(its, i, u))),
    units.forall(u => u.frags.contains(Shared) implies ITEMS.exists(i => its.get(i).frags.contains(Shared))),
  }

  // 承認するつもりの項目の前提。
  // - 作成者が自分の文で確認の語を書いた単位（Affirmed・AffirmedMaskedCue）の id と固有の文は、承認するつもりの項目か、
  //   同じ確認で「まだ」と書いた項目のもの（「#1 と #2 を確認しました。ただし #2 はまだ」）。
  // - まだと書いた項目は、承認するつもりの項目ではない。
  // - 引用だけ（Quoted）、URL か HTML コメントの中だけの語（MaskedWord）、語の無い単位（NoWord）の id と断片は、
  //   意図と無関係に現れうる（指摘を引用して質問する、別件として触れる）。URL の頭は、どの単位でも意図と無関係に現れる。
  pure def consistent(its: ItemId -> Item, units: Set[Unit], intended: Set[ItemId], pending: Set[ItemId]): bool = and {
    intended.intersect(pending) == Set(),
    units.forall(u => authorWord(u.word) implies refersWithin(its, u, intended.union(pending))),
  }

  // 観測は種類から決まる（環境の前提）。作者が Codex で本文が通知の頭で始まるのは、通知だけ。
  pure def mkItem(fs: Set[Frag], k: Kind): Item = {
    updatedAt: 0, frags: fs, kind: k,
    byCodex: k == CodexNotice or k == CodexOther,
    noticeHead: k == CodexNotice or k == HeadByOther,
  }

  action initWithKinds(extra1: Set[Frag], extra2: Set[Frag], k1: Kind, k2: Kind): bool = all {
    clock' = 0,
    items' = Map(
      1 -> mkItem(extra1.union(Set(Own(1))), k1),
      2 -> mkItem(extra2.union(Set(Own(2))), k2),
    ),
    acks' = Set(),
    editLog' = Set(),
    lastStep' = Started,
  }

  action initWith(extra1: Set[Frag], extra2: Set[Frag]): bool = initWithKinds(extra1, extra2, Plain, Plain)

  action init = {
    nondet extra1 = EXTRA_FRAGS.oneOf()
    nondet extra2 = EXTRA_FRAGS.oneOf()
    nondet k1 = KINDS.oneOf()
    nondet k2 = KINDS.oneOf()
    initWithKinds(extra1, extra2, k1, k2)
  }

  // 確認を投稿する。at は投稿の時刻で、単調に増える。
  action postAck(eligible: bool, autoReport: bool, units: Set[Unit], intended: Set[ItemId], pending: Set[ItemId]): bool = all {
    clock' = clock + 1,
    acks' = acks.union(Set({
      at: clock + 1, eligible: eligible, autoReport: autoReport,
      units: units, intended: intended, pending: pending,
    })),
    items' = items,
    editLog' = editLog,
    lastStep' = Posted,
  }

  // 項目が編集される。最終編集の時刻が進む。
  action editItem(id: ItemId): bool = all {
    clock' = clock + 1,
    items' = items.set(id, { ...items.get(id), updatedAt: clock + 1 }),
    acks' = acks,
    editLog' = editLog.union(Set((id, clock + 1))),
    lastStep' = Edited,
  }

  action postAnyAck = {
    nondet two = BOOLS.oneOf()
    nondet w1 = WORDS.oneOf()
    nondet ids1 = ITEMS.powerset().oneOf()
    nondet fs1 = FRAGS.powerset().oneOf()
    nondet w2 = WORDS.oneOf()
    nondet ids2 = ITEMS.powerset().oneOf()
    nondet fs2 = FRAGS.powerset().oneOf()
    val units = if (two) Set(unit(w1, ids1, fs1), unit(w2, ids2, fs2)) else Set(unit(w1, ids1, fs1))
    nondet pending = ITEMS.powerset().filter(p => pendingOk(items, units, p)).oneOf()
    nondet intended = ITEMS.powerset().filter(s => consistent(items, units, s, pending)).oneOf()
    nondet eligible = BOOLS.oneOf()
    nondet autoReport = BOOLS.oneOf()
    postAck(eligible, autoReport, units, intended, pending)
  }

  action step = any {
    postAnyAck,
    nondet id = ITEMS.oneOf()
    editItem(id),
  }

  // ---- 規則（観測できるものだけで、どの確認がどの項目を承認するかを計算する） ----
  pure def isAfter(it: Item, a: Ack): bool = a.at > it.updatedAt

  // 本文全体で見たときの確認の語（ci#43 と b808a29）。引用の中の語も、助動詞を挟む否定も、
  // 後ろに付く否定も、URL・HTML コメントの中の語も確認と数える。
  pure def bodyAckWord(a: Ack): bool =
    a.units.exists(u => u.word == Affirmed or u.word == Quoted or u.word == NegatedEn or u.word == NegatedJa
      or u.word == MaskedWord or u.word == AffirmedMaskedCue)
  pure def bodyIds(a: Ack): Set[ItemId] = a.units.map(u => u.ids).flatten()
  pure def bodyFrags(a: Ack): Set[Frag] = a.units.map(u => u.frags).flatten()

  pure def candidateBody(a: Ack): bool = a.eligible and not(a.autoReport) and bodyAckWord(a)

  // ci#43: id の完全一致か、確認の断片のどれかが項目の本文にある。
  pure def refsWindow(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    bodyIds(a).contains(i) or bodyFrags(a).intersect(its.get(i).frags) != Set()

  // v2 の既定の除外。作者と本文の頭の両方が一致したときだけ外す（片方だけなら外さない）。
  pure def defaultExcluded(it: Item): bool =
    RULE == "scoped" and it.byCodex and it.noticeHead

  // URL でなく、PR の項目の中でその項目だけが持つ断片。「ほかの項目」には、既定で外した通知も数える
  // （外した通知を引用したほかの項目を、通知の引用で承認しない）。
  pure def uniqueTo(its: ItemId -> Item, i: ItemId, f: Frag): bool =
    f != UrlHead
      and its.get(i).frags.contains(f)
      and ITEMS.forall(j => j != i implies not(its.get(j).frags.contains(f)))

  // b808a29: 本文全体の id か、その項目だけが持つ断片。
  pure def refsUnique(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    bodyIds(a).contains(i) or bodyFrags(a).exists(f => uniqueTo(its, i, f))

  // この設計: 単位の極性。穴 2〜5 は、ここで語の現れ方を肯定と読み違えること。穴 6 は打ち消しと読み違えること。
  pure def polarity(w: Word): Pol =
    if (w == Affirmed) Pos
    else if (w == Quoted) Neutral
    else if (w == NegatedEn) Neg
    else if (w == NegatedJa) Neg
    else if (w == Pending) Neg
    else if (w == MaskedWord) Neutral
    else if (w == AffirmedMaskedCue) Pos
    else Neutral

  // 単位 u が項目 i を一意に指す（id か、その項目だけが持つ断片）。
  pure def refsUnit(its: ItemId -> Item, i: ItemId, u: Unit): bool =
    u.ids.contains(i) or u.frags.exists(f => uniqueTo(its, i, f))

  // 打ち消しの単位が項目 i に触れている（id か、URL の頭を除く、i が持つ断片のどれか。一意でなくてよい）。
  pure def vetoed(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    a.units.exists(u => polarity(u.word) == Neg
      and (u.ids.contains(i) or u.frags.exists(f => f != UrlHead and its.get(i).frags.contains(f))))

  // 肯定の単位の中で、語と参照がそろう（穴 1 の効く範囲）。
  pure def scopedRef(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    a.units.exists(u => polarity(u.word) == Pos and refsUnit(its, i, u))

  pure def approvesScoped(its: ItemId -> Item, i: ItemId, a: Ack): bool = and {
    a.eligible,
    not(a.autoReport),
    isAfter(its.get(i), a),
    scopedRef(its, i, a),
    not(vetoed(its, i, a)),
  }

  pure def approves(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    if (RULE == "v1") a.eligible and isAfter(its.get(i), a)
    else if (RULE == "window") candidateBody(a) and isAfter(its.get(i), a) and refsWindow(its, i, a)
    else if (RULE == "unique") candidateBody(a) and isAfter(its.get(i), a) and refsUnique(its, i, a)
    else if (RULE == "scoped") approvesScoped(its, i, a)
    else false

  // 公開される関門の結論。既定で外した項目は承認を待たない。
  val gateGreen: bool = ITEMS.forall(i => defaultExcluded(items.get(i)) or acks.exists(a => approves(items, i, a)))

  // ---- 仕様（作成者の意図と編集の記録を読む。規則とは別に定める） ----
  // 項目 i の最後の編集の時刻（編集が無ければ 0）。
  pure def lastEdit(log: Set[(ItemId, int)], i: ItemId): int =
    log.fold(0, (m, e) => if (e._1 == i and e._2 > m) e._2 else m)

  // 項目 i に、i を承認するつもりで、最後の編集より後に書かれた、自動報告でない、作成者が自分の文で
  // 確認の語を書いた確認がある。
  pure def intendedAck(log: Set[(ItemId, int)], i: ItemId, a: Ack): bool = and {
    a.eligible, not(a.autoReport),
    a.units.exists(u => authorWord(u.word)),
    a.at > lastEdit(log, i),
    a.intended.contains(i),
  }

  // 承認を待たなくてよい項目（真の種類が Codex の利用上限の通知）。規則の defaultExcluded とは別に、kind で決める。
  pure def isNotice(it: Item): bool = it.kind == CodexNotice

  // 緑なら、通知を除くすべての項目に、その項目を承認するつもりの確認がある。
  // 1 つの確認で指していない項目や「まだ」と書いた項目が承認されたら、ここで反例になる。
  // Codex のほかのコメントや、ほかの作者が通知の文で始めたコメントを外しても、ここで反例になる。
  val noFalseGreen: bool =
    gateGreen implies ITEMS.forall(i => isNotice(items.get(i)) or acks.exists(a => intendedAck(editLog, i, a)))

  // I6: すべての項目に、その項目の id を、作成者が確認の語を書いた同じ単位に含む有効な確認があり、
  // その確認のどの打ち消しの単位もその項目に触れていなければ緑（正当な確認を赤にしない）。
  // 通知には確認を求めない（通知が外れずに残って赤になれば、ここで反例になる）。
  val idAckTurnsGreen: bool =
    ITEMS.forall(i => isNotice(items.get(i)) or acks.exists(a => and {
      intendedAck(editLog, i, a),
      a.units.exists(u => authorWord(u.word) and u.ids.contains(i)),
      not(a.units.exists(u => saysNotYet(u.word) and mentions(items, i, u))),
    }))
      implies gateGreen

  val safety: bool = noFalseGreen and idAckTurnsGreen

  // ---- witness（届くことを確かめる状態） ----
  pure def hasNotYetUnit(a: Ack): bool = a.units.exists(u => saysNotYet(u.word))
  // 各 action が起きる。
  val postedAck: bool = lastStep == Posted
  val editedItem: bool = lastStep == Edited
  // 確認のあとで項目が編集され、その確認が無効になった（I4 の遷移）。
  val editInvalidatedAck: bool =
    lastStep == Edited and ITEMS.exists(i => acks.exists(a =>
      a.eligible and not(a.autoReport) and scopedRef(items, i, a) and not(isAfter(items.get(i), a))))
  // 断片の引用だけで承認された項目がある（id の経路だけで緑になっていない）。
  val approvedByQuoteOnly: bool =
    ITEMS.exists(i => acks.exists(a => approves(items, i, a) and not(bodyIds(a).contains(i))))
  // 定型文を持つ項目があり、それでも緑。
  val greenWithSharedText: bool =
    gateGreen and ITEMS.exists(i => items.get(i).frags.contains(Shared))
  // URL の頭を持つ確認があり、それでも緑。
  val greenWithUrlHeadAck: bool =
    gateGreen and acks.exists(a => bodyFrags(a).contains(UrlHead))
  // 「まだ」と書いた単位を含む確認（まとめ返信）が、別の項目を承認している（穴 1 の規則がいつも赤を返していない）。
  val approvedByMixedAck: bool =
    ITEMS.exists(i => acks.exists(a => approves(items, i, a) and hasNotYetUnit(a)))
  // 肯定の単位が項目を指しているのに、同じ確認の打ち消しの単位で拒まれた。
  val vetoFired: bool =
    ITEMS.exists(i => acks.exists(a => a.eligible and not(a.autoReport) and isAfter(items.get(i), a)
      and scopedRef(items, i, a) and vetoed(items, i, a)))
  // 引用の中の語だけの単位を含む確認があり、それでも緑。
  val greenWithQuotedWordAck: bool =
    gateGreen and acks.exists(a => a.units.exists(u => u.word == Quoted))
  // 打ち消された語（英語・日本語）の単位を含む確認があり、それでも緑。
  val greenWithNegatedAck: bool =
    gateGreen and acks.exists(a => a.units.exists(u => u.word == NegatedEn or u.word == NegatedJa))
  // 2 つの単位を持つ確認が項目を承認している。
  val approvedByTwoUnitAck: bool =
    ITEMS.exists(i => acks.exists(a => approves(items, i, a) and a.units.size() == 2))
  // 既定で外した通知があり、それでも（ほかの項目を承認して）緑。
  val greenWithNoticeExcluded: bool =
    gateGreen and ITEMS.exists(i => defaultExcluded(items.get(i)))
      and ITEMS.exists(i => not(defaultExcluded(items.get(i))))
  // Codex のほかのコメント（作者だけ一致）が項目に残り、確認で承認されて緑。
  val greenWithCodexOtherItem: bool =
    gateGreen and ITEMS.exists(i => items.get(i).kind == CodexOther)
  // ほかの作者の、通知の文で始まるコメント（本文だけ一致）が項目に残り、確認で承認されて緑。
  val greenWithHeadByOtherItem: bool =
    gateGreen and ITEMS.exists(i => items.get(i).kind == HeadByOther)
  // URL か HTML コメントの中だけの語の単位を含む確認があり、それでも緑（4 回目）。
  val greenWithMaskedWordAck: bool =
    gateGreen and acks.exists(a => a.units.exists(u => u.word == MaskedWord))
  // 打ち消しの語が URL か HTML コメントの中にだけある単位が、項目を承認する（4 回目。規則がその単位を赤にしていない）。
  val approvedByMaskedCue: bool =
    ITEMS.exists(i => acks.exists(a => approves(items, i, a)
      and a.units.exists(u => u.word == AffirmedMaskedCue and refsUnit(items, i, u))))
}

// いまの v1（内容を見ない）。
module legacy {
  import reviewThreadAck(RULE = "v1").*
}

// j5ik2o/ci#43 の 442d679・86a443c（20 文字の窓が URL の頭や定型文に当たる。語と参照を単位で結ばない）。
module window {
  import reviewThreadAck(RULE = "window").*
}

// 設計 b808a29（一意に指す値。語と参照を単位で結ばず、引用・否定の語を確認と数える）。
module unique {
  import reviewThreadAck(RULE = "unique").*
}

// この設計（2 回目の直し。同じ単位の打ち消されない語と一意の参照、打ち消しの単位による拒否）。
module fixed {
  import reviewThreadAck(RULE = "scoped").*
}
