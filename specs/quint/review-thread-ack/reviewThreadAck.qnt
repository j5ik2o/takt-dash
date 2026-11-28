// -*- mode: Bluespec; -*-
// 「Check unresolved comments」の承認判定（issue #134、#443、j5ik2o/ci#44）の最小モデル。
//
// 対象は、未解決スレッドが 0 件のときに残る「未承認のトップレベルコメント／レビュー要約」（項目）。
// 判定はコメントの集合に対する純関数で、ここで遷移として表すのは、コメントの投稿・項目の編集という
// 入力の並びに対して、公開される関門の結論が赤と緑の間を動くことだけ。
//
// 確認（ack）は投稿の時点ではどの項目にも結び付けない。確認が持つのは本文から観測できるものだけ
// （含むコメントの id、含む文の断片、確認の語の有無など）で、どの項目を承認するかは規則 RULE が
// 項目の側の本文と突き合わせて計算する。作成者が「どの項目を指すつもりだったか」（intended）は
// 規則からは見えず、仕様（noFalseGreen）だけが読む。
//
// RULE:
//   "v1"     いまの v1。項目より後ろの、承認できる人のコメントなら内容によらず承認。
//   "window" j5ik2o/ci#43（442d679）。確認の語と、id の完全一致か 20 文字の窓のどれかが項目の
//            本文に含まれれば承認。窓は URL の頭や、複数の項目に共通の定型文にも当たる。
//   "unique" この設計。確認の語と、id の完全一致か、URL を除いた本文の断片で、PR の項目の中で
//            その項目だけが持つもの。
module reviewThreadAck {
  const RULE: str

  type ItemId = int
  // 本文の断片。20 文字以上の連続部分を 1 つの記号に畳む。
  //   "U1" / "U2": その項目だけが持つ散文（項目 1 / 2 の固有の文）。
  //   "S": 定型文。Codex の要約の見出しや利用上限の通知など、複数の項目に現れうる散文。
  //   "H": URL の頭（`https://github.com/j` の 20 文字）。リポジトリへのリンクを持つ本文なら必ず含む。
  type Frag = str
  type Item = { updatedAt: int, frags: Set[Frag] }
  type Ack = {
    at: int,
    // 承認できる人（作成者、Codex の確認、作成者が Bot のときの OWNER 等）による投稿か。
    eligible: bool,
    // 自動報告（既知の自動投稿、本文がフェンスだけ、フェンスの外のマーカー）か。
    autoReport: bool,
    // フェンスの外に、否定・疑問の付かない確認の語があるか。
    ackWord: bool,
    // 本文に完全な形で含むコメントの id（コメントの URL の全体、または #issuecomment-N 等のアンカー）。
    ids: Set[ItemId],
    // 本文に含む断片。
    frags: Set[Frag],
    // 作成者が指すつもりだった項目。規則は読まない。仕様だけが読む。
    intended: Set[ItemId],
  }

  pure val ITEMS: Set[ItemId] = Set(1, 2)
  pure val URL_HEAD: Frag = "H"
  pure val SHARED: Frag = "S"
  pure def own(i: ItemId): Frag = if (i == 1) "U1" else "U2"
  pure val FRAGS: Set[Frag] = Set("U1", "U2", SHARED, URL_HEAD)
  pure val BOOLS: Set[bool] = Set(true, false)
  // 項目が固有の文のほかに持ちうる断片の組。
  pure val EXTRA_FRAGS: Set[Set[Frag]] = Set(SHARED, URL_HEAD).powerset()

  var clock: int
  var items: ItemId -> Item
  var acks: Set[Ack]

  // ---- 現れ方の前提（確認の本文に、何が意図と無関係に現れうるか） ----
  // - コメントの id は、その項目を指すつもりのときにしか貼られない（id は GitHub 全体で一意で、
  //   偶然一致しない）。
  // - 項目に固有の散文は、その項目を指すつもりのときにしか引用されない。
  // - 定型文は、それを持つ項目のどれかを指すつもりのときにしか引用されない（どれを指すかは定まらない）。
  // - URL の頭は、別件の URL を貼るだけで現れる。意図と無関係。
  pure def consistent(its: ItemId -> Item, intended: Set[ItemId], ids: Set[ItemId], fs: Set[Frag]): bool = and {
    ids.subseteq(intended),
    ITEMS.forall(i => fs.contains(own(i)) implies intended.contains(i)),
    fs.contains(SHARED) implies ITEMS.exists(i => intended.contains(i) and its.get(i).frags.contains(SHARED)),
  }

  action initWith(extra1: Set[Frag], extra2: Set[Frag]): bool = all {
    clock' = 0,
    items' = Map(
      1 -> { updatedAt: 0, frags: extra1.union(Set(own(1))) },
      2 -> { updatedAt: 0, frags: extra2.union(Set(own(2))) },
    ),
    acks' = Set(),
  }

  action init = {
    nondet extra1 = EXTRA_FRAGS.oneOf()
    nondet extra2 = EXTRA_FRAGS.oneOf()
    initWith(extra1, extra2)
  }

  // 確認を投稿する。at は投稿の時刻で、単調に増える。
  action postAck(eligible: bool, autoReport: bool, ackWord: bool, ids: Set[ItemId], fs: Set[Frag], intended: Set[ItemId]): bool = all {
    clock' = clock + 1,
    acks' = acks.union(Set({
      at: clock + 1, eligible: eligible, autoReport: autoReport, ackWord: ackWord,
      ids: ids, frags: fs, intended: intended,
    })),
    items' = items,
  }

  // 項目が編集される。最終編集の時刻が進む。
  action editItem(id: ItemId): bool = all {
    clock' = clock + 1,
    items' = items.set(id, { ...items.get(id), updatedAt: clock + 1 }),
    acks' = acks,
  }

  action postAnyAck = {
    nondet intended = ITEMS.powerset().oneOf()
    nondet ids = intended.powerset().oneOf()
    nondet fs = FRAGS.powerset().filter(fs => consistent(items, intended, ids, fs)).oneOf()
    nondet eligible = BOOLS.oneOf()
    nondet autoReport = BOOLS.oneOf()
    nondet ackWord = BOOLS.oneOf()
    postAck(eligible, autoReport, ackWord, ids, fs, intended)
  }

  action step = any {
    postAnyAck,
    nondet id = ITEMS.oneOf()
    editItem(id),
  }

  // ---- 規則（観測できるものだけで、どの確認がどの項目を承認するかを計算する） ----
  pure def isAfter(it: Item, a: Ack): bool = a.at > it.updatedAt

  pure def candidate(a: Ack): bool = a.eligible and not(a.autoReport) and a.ackWord

  // ci#43: id の完全一致か、確認の断片のどれかが項目の本文にある。
  pure def refsWindow(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    a.ids.contains(i) or a.frags.intersect(its.get(i).frags) != Set()

  // この設計: id の完全一致か、URL でなく、PR の項目の中でその項目だけが持つ断片。
  pure def uniqueTo(its: ItemId -> Item, i: ItemId, f: Frag): bool =
    f != URL_HEAD
      and its.get(i).frags.contains(f)
      and ITEMS.forall(j => j != i implies not(its.get(j).frags.contains(f)))

  pure def refsUnique(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    a.ids.contains(i) or a.frags.exists(f => uniqueTo(its, i, f))

  pure def approves(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    if (RULE == "v1") a.eligible and isAfter(its.get(i), a)
    else if (RULE == "window") candidate(a) and isAfter(its.get(i), a) and refsWindow(its, i, a)
    else candidate(a) and isAfter(its.get(i), a) and refsUnique(its, i, a)

  // 公開される関門の結論。
  val gateGreen: bool = ITEMS.forall(i => acks.exists(a => approves(items, i, a)))

  // ---- 仕様（作成者の意図を読む。規則とは別に定める） ----
  // 項目 i に、i を指すつもりで書かれた有効な確認がある。
  pure def intendedAck(its: ItemId -> Item, i: ItemId, a: Ack): bool =
    candidate(a) and isAfter(its.get(i), a) and a.intended.contains(i)

  // 緑なら、すべての項目に、その項目を指すつもりの確認がある。
  // 1 つの確認で指していない項目が承認されたら、ここで反例になる。
  val noFalseGreen: bool =
    gateGreen implies ITEMS.forall(i => acks.exists(a => intendedAck(items, i, a)))

  // I6: すべての項目に、その項目の id を含む有効な確認があれば緑（正当な確認を赤にしない）。
  val idAckTurnsGreen: bool =
    ITEMS.forall(i => acks.exists(a => candidate(a) and isAfter(items.get(i), a) and a.ids.contains(i)))
      implies gateGreen

  val safety: bool = noFalseGreen and idAckTurnsGreen
}

// いまの v1（内容を見ない）。
module legacy {
  import reviewThreadAck(RULE = "v1").*
}

// j5ik2o/ci#43 の 442d679（20 文字の窓が URL の頭や定型文に当たる）。
module window {
  import reviewThreadAck(RULE = "window").*
}

// この設計（id の完全一致か、URL を除いた、その項目だけが持つ断片）。
module fixed {
  import reviewThreadAck(RULE = "unique").*
}
