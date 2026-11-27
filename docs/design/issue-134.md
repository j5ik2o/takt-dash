# issue #134 の設計：未解決コメント関門の承認判定を厳しくする

対象：[j5ik2o/takt-dash#134](https://github.com/j5ik2o/takt-dash/issues/134)。実装は j5ik2o/ci の `review-thread-resolution`（再利用ワークフローと `scripts/review-thread-resolution.sh`）になる見込みである。この文書は設計の下書きで、takt-dash と j5ik2o/ci のどちらも変更しない。日付は 2026-11-27。

## いまの状態（実測つき）

### 参照した実物

- j5ik2o/ci のリモートのタグ `v1` は `2ead62a`（`fix(review-thread-resolution): 評価の失敗で error を発行し、壊れた応答で成功を出さない (#39)`）。`git ls-remote origin refs/tags/v1` で確認した。
- 同じ日の `origin/main` の木は `v1` と同一（`git diff v1..origin/main` が空）。`0a7037b`（Codex の利用上限の知らせを未承認に数えない）は一度 main に入ったあと、`acb57ba`（`Revert "Merge pull request #41 ..."`）で戻され、いまの main には入っていない。
- 手元の clone のタグ `v1` は `db3bfd0` を指しており、リモートと食い違う。過去に付け替えたのか、手元だけで付けたのかは**未確認**。以下の判定はすべてリモートの `v1`（`2ead62a`）の内容で読んだ。

### 判定のコード

`scripts/review-thread-resolution.sh` の `filter_unacknowledged_comments`（v1 の 440 行目から）は、トップレベルコメントとレビュー本文を同じ一覧にして、次で「未承認」を選ぶ。

- 未承認の候補（項目）は、自動投稿の既知パターンに当たらず、Codex の自分の確認コメントでなく、作成者（`pr_author`）のコメントでないもの。作成者が Bot のときは、OWNER / MEMBER / COLLABORATOR のコメントも項目から外す。
- 項目 `c` は、**`c` の最終更新より後に投稿された作成者名義のコメントが 1 つでもあれば**承認済みになる（`select(.createdAt > ($comment.updatedAt // $comment.createdAt))`）。コメントの本文は見ない。
- 例外として、`chatgpt-codex-connector` のコメントが `acknowledged` / `確認しました` などで**始まる**と、そのコメントは項目から外れ、さらに**すべての項目の承認**として数えられる（`trusted_acknowledgement`）。対象を指していない。

つまり「他人（作成者を含む）のコメントを、内容によらず承認として数えない」という #134 の要求に対して、いまの判定は作成者のコメントを内容によらず数え、Codex の 1 つの確認を全項目に効かせている。

### #131 で起きたこと（実測）

[takt-dash#131](https://github.com/j5ik2o/takt-dash/pull/131)（2026-11-16、マージ済み）のコメントと、head `05b8306` の `Check unresolved comments` の履歴。

| 時刻 (UTC) | 出来事 |
| --- | --- |
| 01:28:11 | Codex のレビュー要約（`codex-pull-request-review-summary`） |
| 01:28:14 | Codex の利用上限の通知（`You have reached your Codex usage limits for code reviews. ...`） |
| 02:44:27 | 関門が `failure` を公開（「Review-thread gate failure.」） |
| 02:46:31 | 作成者名義で takt の失敗報告「TAKTのCI回復を停止しました。…」が投稿される。本文は未承認の実体である利用上限の通知を**そのまま引用**している |
| 02:46:43 | 関門が `success` を公開（失敗報告の 12 秒後） |
| 02:49:20 | 作成者の実際の確認「Codex（…）の「You have reached your Codex usage limits for code reviews.」（…）を確認しました。…」 |

`repos/j5ik2o/takt-dash/commits/05b8306.../statuses` で、02:44:27 の failure のあと 02:46:43 に success が付いたことを確認した。失敗報告は確認の表明ではないが、作成者名義で利用上限の通知より後にあるため、v1 の判定では承認として通った。これが #134 の実体である。

### 2026-11-27 時点の利用リポジトリ

`HEAD:.github/workflows/review-thread-resolution.yml` を 227 の非アーカイブリポジトリについて GraphQL で読み、26 リポジトリにファイルがあることを確認した。先行調査の「26 リポジトリ」と同じ数になった（内訳は上表のとおりで、呼び出し側は 25、定義側の ci を含めて 26）。呼び出し先で分けると次のとおり。

| 区分 | 数 | リポジトリ |
| --- | --- | --- |
| `@v1`（`ci_ref: v1`） | 22 | aislop, cqrs-es-example, cqrs-es-example-go, cqrs-es-example-java, cqrs-es-example-js, cqrs-es-example-kotlin, cqrs-es-example-rs, cqrs-es-example-scala, ddd-principles, dotfiles, event-store-adapter, event-store-adapter-go, event-store-adapter-java, event-store-adapter-js, event-store-adapter-kotlin, event-store-adapter-rs, event-store-adapter-scala, fraktor-ng-rs, modern-ddd-book, pekko-cqrs-es-example, pekko-persistence-dynamodb, takt-dash |
| SHA 固定 | 2 | fraktor-rs（`560170b6…`）、g2g（`9cf0e9a8…`） |
| ブランチ固定 | 1 | ci-gate-sandbox（`feat/review-thread-gate-other-checks`） |
| 定義側（呼び出しなし） | 1 | ci |

- SHA 固定の 2 つは 2026-06 のコミットで、v1 より古い。どちらも少なくとも同じ形の「内容を見ない」判定を含み、v1 を直しても変わらない。
- 22 のすべてで `Check unresolved comments` が必須チェックになっている（20 はクラシックの branch protection、takt-dash と ddd-principles は ruleset）。`gh api repos/<repo>/branches/main/protection` と `rulesets` で確認した。
- 2026-11-27 時点の 22 リポジトリの open PR は 15 件。後述の比較で、v1 と提案の判定が違った項目は 0 件だった。

### 現在の緑を赤に戻さないことの下見（実測）

open PR 15 件のコメントとレビューを取得し、v1 の判定とこの設計の判定を jq で並べて比べた。

- 未承認の項目は 3 件だけだった。
  - `event-store-adapter-java#871`: CodeRabbit の「Action not completed / Review rate limited」の通知。v1 は緑（00:31 の作成者コメントが他の項目の URL を列挙し、01:42 の「…「Review rate limited / Action not completed」を確認しました。」が引用で承認）。提案も緑。
  - `event-store-adapter-kotlin#926`: Codex の利用上限の通知。v1 も提案も赤（通知のあとに作成者のコメントが無い）。
  - `event-store-adapter-rs#381`: CodeRabbit の利用上限の通知。v1 も提案も赤。
- 15 件のうち、v1 が緑で提案が赤になる PR は 0 件。v1 が赤で提案が緑になる PR も 0 件。
- 直近の merged 48 件（takt-dash、dotfiles、event-store-adapter-java、cqrs-es-example の各 12 件）でも、未承認の項目は 0 件で、新旧の差は出なかった。ただし merged の履歴は「その時点の判定」ではなく「いま取得できるコメント」での比較であり、削除済みコメントは見えない。

## 何をどう変えるか

### 採用する承認の条件（1 つ）

後のコメント `a` が、次を**すべて**満たすときだけ、項目 `c` を承認したとみなす。

1. `a` は対象より後である（`a.createdAt > c.updatedAt`。対象の最終編集より後）。
2. `a` は承認できる人による。作成者（`pr_author`）、または Codex の確認コメント、または作成者が Bot のときの OWNER / MEMBER / COLLABORATOR。**作成者以外の第三者のコメントは承認に数えない**（いまも数えていない）。
3. `a` は自動投稿ではない。既知の自動投稿パターン（`ignored_auto_report`）に当たるコメントは、項目にも承認にも数えない。**いまは作成者名義の自動投稿が承認として通るので、ここを塞ぐ。**
4. `a` は**確認の語**を含む。`確認しました` / `確認済み` / `承知しました` / `了解しました` / `対応しました` / `修正しました` / `対応済み` / `acknowledged` / `addressed` / `fixed`（英語は単語境界、大小文字を問わない）。コードフェンス（3 つのバッククォートで囲んだ部分）の中の語は数えない。
5. `a` は**対象を指す参照**を含む。次のいずれか。
   - 対象の URL（`https://github.com/o/r/pull/N#issuecomment-123` や `#pullrequestreview-4`）そのもの、またはそのアンカー（`#issuecomment-123` / `#pullrequestreview-4`）。
   - 対象本文の正規化後の連続部分で **20 文字以上**。引用符の有無は問わない。実装は、`a` の正規化後の 20 文字の窓をすべて取り、`c` の正規化後の本文に含まれる窓が 1 つでもあれば一致とする。

正規化は、HTML コメント（`<!-- ... -->`）と HTML タグを空白にし、行頭の `>` を外し、Markdown の強調・見出し・表の記号（`*`、`_`、バッククォート、`#`、`>`、`|`）を空白にし、空白を 1 つに畳んで前後を除き、ASCII を小文字にする。コードフェンスは残す（コードの引用は正当な参照なので消さない）。確認の語の判定だけは、先にコードフェンスを空白にする。

補足すると、この条件は「明示の印」と「対象への参照」の連言である。参照だけでは #131 の失敗報告が通り、確認の語だけでは対象を指せない（後述）。20 文字は、実測で観測した最短の正当な引用（`Action not completed` の 20 文字）に合わせた。これより短いと、一般的な語が偶然一致しやすくなる。

### 判定の式（jq の形）

````jq
def ack_phrase:
  (.body // "")
  | gsub("(?s)```.*?```"; " ")
  | test("(?i)(確認しました|確認済み|承知しました|了解しました|対応しました|修正しました|対応済み|\\backnowledged\\b|\\baddressed\\b|\\bfixed\\b)");

def norm_text($body):
  $body
  | split("\n")
  | map(gsub("^\\s*>\\s?"; ""))
  | map(gsub("<!--[^>]*-->"; " "))
  | map(gsub("<[^>]+>"; " "))
  | map(gsub("[*_`#>|]+"; " "))
  | map(gsub("\\s+"; " ") | sub("^ "; "") | sub(" $"; ""))
  | map(select(length > 0))
  | join("\n")
  | ascii_downcase;

def windows($n):
  . as $s | [range(0; (($s | length) - $n + 1)) as $i | $s[$i:$i + $n]];

def anchor($u):
  ($u // "" | split("#") | if length > 1 then "#" + .[-1] else "" end);

def references($target; $ack):
  norm_text($target.body) as $tb
  | norm_text($ack.body) as $b
  | (($target.url // "") as $u
     | ($u != "" and ($b | contains($u | ascii_downcase)))
       or (anchor($u) as $an | $an != "" and ($b | contains($an | ascii_downcase)))
       or ($b | any(windows(20)[]; . as $w | $tb | contains($w))));

# 項目ごとの承認の探し方
[ $items[]
  | select(is_item)
  | . as $item
  | [ $items[]
      | select(is_acknowledger)
      | select(.createdAt > ($item.updatedAt // $item.createdAt))
      | select(ignored_auto_report | not)
      | select(ack_phrase and references($item; .))
    ] as $acks
  | select($acks | length == 0)
]
````

`is_item` と `is_acknowledger` は v1 の `select` を分けただけである。`trusted_acknowledgement` は「Codex のコメントを項目から外す」ためだけに残し、**ほかの項目の承認としては、上の 4 と 5 を満たすときだけ**数える。これで「Codex の 1 つの `Acknowledged.` が全項目を承認する」穴も塞がる。

### 赤のときの案内

いまの `build_summary` は未承認の項目を列挙するだけで、解消の仕方を書いていない。次を足す。

```text
### 未承認のトップレベルコメント／レビュー要約
- by <author> updated at <time>: <url>
  確認するには、返信にこの URL を含めるか、本文を 20 文字以上引用し、
  「確認しました」などの語を添えてください。

自動投稿（TAKT の失敗報告など）は確認として数えません。
```

URL をそのまま貼れるので、人が迷わない。TAKT の失敗報告に確認の語を入れない規約も、この案内と対で運用する（後述）。

### 変えないこと

- 未解決スレッドの判定（`isResolved`）は変えない。スレッドの返信は承認ではなく、解決（resolve）だけが関門を外す。
- ステータスの context 名（`Check unresolved comments`）、公開方法、branch protection / ruleset は変えない。
- GitHub API の呼び出しは増やさない。コメントとレビューはすでに取得しており、判定は手元の jq だけで完結する。
- 自動投稿の既知パターン（CodeRabbit の要約など）の一覧は、この設計では変えない。Codex の利用上限の除外（`0a7037b`）は main で revert 済みなので、v2 には入れない。再投入するかは別 issue で決める。

## 採らなかった案と理由

| 案 | なぜ採らないか |
| --- | --- |
| 参照だけ（URL か引用があれば承認） | **#131 の失敗報告がそのまま通る。** 失敗報告は「You have reached your Codex usage limits for code reviews.」を本文に引用しており、参照だけの判定では 02:46:31 の時点で緑になる（手元の jq で、02:49:20 の実際の確認を除いても緑になることを確認）。#134 の事象を直せない。 |
| 確認の語だけ（対象を指さなくてよい） | どの項目への確認か分からない。「確認しました」は別件にも書けるし、自動投稿にも入れられる。項目が複数あるとき、1 つの確認で全部が通る。 |
| URL だけ（引用を認めない） | #131 の実際の確認は URL を含まず、引用と「確認しました」だけだった。いまの緑を赤に戻す。運用も「必ず URL を貼る」に変わり、移行の負担が大きい。 |
| スレッド返信があること | 対象はトップレベルの issue コメントとレビュー本文で、これらにスレッドは無い（レビューのスレッドは別の `reviewThreads` で、未解決ならすでに赤）。スレッド返信は解決（resolve）を意味しないので、承認の代わりにもならない。 |
| 作成者のコメントを一切承認に数えない | Bot の要約や通知は解決（resolve）できないため、正当に確認しても永久に赤になる。除外リストを編集するしか解消手段が無くなる。 |
| 自動投稿だけをパターンで除外する（TAKT の失敗報告を `ignored_auto_report` に足す） | 文面が変わると壊れる許可リストで、「内容によらず数えない」という要求に答えていない。リポジトリごとに TAKT の文面が違い、j5ik2o/ci の既定に入れると他リポジトリに副作用が出る。 |
| リアクション（👍 など）を承認にする | 対象を指す情報が無く、誤クリックでも通る。GraphQL の `reactions` を新たに取る必要があり、API 呼び出しも増える。 |

## 適用と移行

### v2 を切る（採用）

`v1` はそのまま残し、`v2` を新しく切って、リポジトリごとに `uses` と `ci_ref` を v2 に上げる。

| 観点 | v1 を直接書き換える | v2 を切る（採用） |
| --- | --- | --- |
| 影響範囲 | 22 リポジトリの次のイベントで一斉に変わる | 上げたリポジトリから順に変わる |
| 観察 | できない。全 PR が同時に新判定になる | 砂場と takt-dash で先に観察できる |
| 誤って赤にしたとき | 全リポジトリのマージが止まる。戻すには v1 を付け替える（全体操作） | そのリポジトリの 2 行を戻すだけ。次回の評価で戻る |
| 監査 | タグを付け替えた事実が残らず、どの判定がいつからか読めない | `v2` のコミットが固定で読める |
| コスト | 変更 1 回 | 22 の小さな PR（各 2 行）。まとめて自動化できる |
| 既存の緑 | 予告なく判定が変わる | 変わるのは上げたリポジトリだけ |

v1 を直さない理由は、22 のすべてが `Check unresolved comments` を必須にしており、判定の誤りがそのままマージ停止になるためである。とくに TAKT の自動 PR は、失敗報告が承認に数えられなくなると赤のまま止まる。砂場で先に確かめられない一斉変更は、この関門には重すぎる。

### タグの扱い

- `v2` は**固定**する（作ったコミットから動かさない）。どのリポジトリがどの判定で走ったかを後から読めるようにする。
- ただし、v2 自体に全リポジトリを止める不具合が出たときの緊急手段として、`v2` を直前の良いコミットへ付け替える手順を用意する。付け替えたときは、日時・理由・戻したコミットをインシデント記録に残す。通常の修正は `v2.1` を切って、影響するリポジトリだけ上げる。
- `v1` は削除しない。SHA 固定の逃げ道として `2ead62a` を README に書く。

### 移行の順

1. **j5ik2o/ci の main に実装を入れる。** スクリプト、スモークテスト、README の例、`v2` 用の `ci_ref` 既定値。`quint` はこのリポジトリの対象外なので、モデルは takt-dash 側の設計に置く。
2. **タグを切る前に、open PR の新旧比較を回す。** 22 リポジトリの open PR について、v1 の判定と提案の判定を並べ、差が 0 件であることを確認する（この文書の実測と同じ手順）。差があれば、その PR のコメントを読んで、引用の長さ・URL・確認の語のどれで落ちたかを分類し、誤って赤にしない条件を直してからタグを切る。
3. **`v2` を切る。** `git tag -a v2 -m "review-thread-resolution: require an explicit acknowledgement" <main のコミット>`、`git push origin v2`。`uses: ...@v2` が解決できることを、砂場の 1 本で確かめる。
4. **ci-gate-sandbox を v2 に上げる。** すでに機能ブランチを指している砂場で、無関係コメント・URL・引用・失敗報告の 4 例を実際の PR で流す。
5. **takt-dash を v2 に上げる。** #134 の出所であり、TAKT の自動 PR が最も多い。1 週間（日次の schedule を 1 回以上含む）観察する。赤になった PR があれば、その理由が「本当に未承認」か「誤って赤」かを分類する。
6. **自動化の多い 1 リポジトリ（cqrs-es-example-java）を上げる。** 3 日観察する。
7. **残りを 5 本ずつ上げる。** 各 PR は 2 行。マージ後に open PR のステータスが再評価されることを確認する。
8. **最後に ci の README と v2 ワークフローの既定値を整え、v1 を「フォールバック」として案内する。** v1 の削除や廃止は、22 が v2 に移ってから別途決める。

### 呼び出し側の変更（各リポジトリ 2 行）

```yaml
uses: j5ik2o/ci/.github/workflows/review-thread-resolution.yml@v2
...
ci_ref: v2
```

- 2 行は同じコミットで変える。`uses` だけ v2 にして `ci_ref: v1` のままだと、v2 のワークフローが v1 のスクリプトを checkout し、**古い判定のまま黙って動く**。`ci_ref` だけ v2 にすると、v1 のワークフローが v2 のスクリプトを動かす。どちらも避ける。
- v2 のワークフローでは `ci_ref` の既定値を `v2` にする。`ci_ref` を書かない呼び出し側は v2 のスクリプトになる。
- 移行後に、`uses` の ref と `ci_ref` が一致しているかを 22 リポジトリについて監査する小さなスクリプトを ci に置く（GraphQL でファイルを読み、2 つの値を比べるだけ）。これは実装時の追加候補で、この設計では必須にしない。

### 戻し方

| 段階 | 操作 | 効く範囲 | 所要 |
| --- | --- | --- | --- |
| 1. そのリポジトリだけ戻す | v2 に上げたコミットを revert（2 行を v1 に戻す） | そのリポジトリの次の評価から | 1 PR、数分 |
| 2. タグに依存せず戻す | `uses` と `ci_ref` を `2ead62a`（v1 の SHA）にする | そのリポジトリ。将来 v1 が動いても影響を受けない | 1 PR、数分 |
| 3. v2 全体に不具合 | 影響するリポジトリを 1 または 2 で戻す。修正は main に入れて `v2.1` を切る | リポジトリごと | 影響数に比例 |
| 4. 全リポジトリが止まる緊急時 | `v2` を直前の良いコミットへ付け替えて force-push（例外手順） | 全 v2 利用者 | 数分。インシデント記録が必須 |

ステータスは次のイベント（コメント、レビュー、日次の schedule、手動 dispatch）で再計算される。戻したあとに即時で確かめたいときは、対象 PR で `workflow_dispatch` を `pr_number` 付きで回す。

## テストの観点

`tests/review-thread-resolution-smoke.sh` の偽 `gh` を広げ、コメントの配列（作成者・本文・`createdAt`・`updatedAt`）を環境変数で差し替えられるようにする。既存の検査はそのまま残す。足す観点は次の 14 である。

1. 無関係な作成者コメント（「ありがとう」「別件です」）では通らない（#134 の核心）。
2. TAKT の失敗報告（本文を引用し、コードフェンスで囲んだもの）では通らない。
3. 対象の URL と確認の語で通る。
4. 対象のアンカー（`#issuecomment-N` / `#pullrequestreview-N`）と確認の語で通る。
5. 対象本文の 20 文字以上の引用と確認の語で通る（`Action not completed` の実例）。
6. 確認の語だけで、参照が無ければ通らない。
7. 参照だけで、確認の語が無ければ通らない。
8. 別のコメントの URL では通らない（対象の URL / アンカーだけ）。
9. 対象より前に投稿された確認では通らない。
10. 対象の最終編集より前の確認では通らない。編集後に確認し直すと通る。
11. コードフェンスの中の確認の語は数えない（`unacknowledged` のような部分一致もしない）。
12. 作成者以外の第三者の確認は数えない。作成者が Bot の PR では、OWNER / MEMBER / COLLABORATOR の人間の確認だけ数える。
13. #131 の実データをそのまま固定し、02:49:20 を含むと通り、02:48 までだと通らない（失敗報告だけでは通らない）。
14. いま緑の PR が赤に戻らない。`event-store-adapter-java#871` の実データで緑を維持し、いま赤の `event-store-adapter-kotlin#926` と `event-store-adapter-rs#381` は赤のまま。open PR 15 件の新旧比較で差が 0 件。

補足すると、14 はスモークテストに実データを置くというより、タグを切る前の使い捨ての比較スクリプトで確かめるのが向いている。固定のフィクスチャとしては 1〜13 を置き、14 は移行手順のチェックリストに残す。

## 影響と戻し方

### 影響するリポジトリ（2026-11-27 時点）

上の 22 リポジトリ。すべてで `Check unresolved comments` が必須。v2 に上げたリポジトリは、次の評価から新しい判定になる。

- 既存の open PR で、内容によらない作成者コメントだけに頼って緑だったものは、赤に戻る可能性がある。2026-11-27 の 15 件では 0 件だったが、将来の PR では起こり得る。
- 新しく赤になった PR は、作成者が URL か 20 文字以上の引用に「確認しました」を添えて返信すれば緑になる。関門の要約にその案内を出す。
- TAKT の失敗報告は承認に数えられなくなる。TAKT が失敗報告を投稿しても、関門は赤のまま人を待つ。これは #134 の意図どおりだが、TAKT の merge-prepare が同じ PR を再試行し続けるなら、失敗報告に確認の語を入れない・赤のまま止まる、という規約を TAKT 側のワークフローに足す必要がある（別 PR、未確認）。
- SHA 固定の 2 つ（fraktor-rs、g2g）とブランチ固定の 1 つ（ci-gate-sandbox）は、v2 に上げるまで変わらない。同じ穴が残る。
- 追加の API 呼び出し、保存データ、公開 API、ブランチ保護の設定変更は無い。GitHub のレート制限への影響も無い。

### 戻し方

「適用と移行」の表のとおり。もっとも速いのは、そのリポジトリの 2 行を v1 に戻すこと。全体を一瞬で戻す必要があるときだけ、v2 タグの付け替えを例外として使う。

## 守る不変条件

- I1（緑の意味）：関門が緑になるのは、未解決スレッドが 0 件で、かつすべての未承認項目に有効な承認があるときだけ。
- I2（#134 の修正）：確認の語も対象への参照も無い作成者コメントは、決して承認にならない。
- I3（自動投稿）：既知の自動投稿パターンに当たるコメントは、項目にも承認にもならない。TAKT の失敗報告は、引用を含んでいても承認にならない。
- I4（順序）：対象の最終編集より前のコメントは承認にならない。承認のあとで対象が編集されたら、承認し直しが必要。
- I5（純関数）：判定は取得済みのコメントとレビューの集合だけで決まる。実行順、並行性、リトライ、ページ取得の順に依存しない。同じ入力なら同じ結論。
- I6（正当な確認）：対象の URL / アンカーと確認の語、または 20 文字以上の引用と確認の語がそろえば、緑になる。
- I7（API 不増）：判定のために新しい GitHub API 呼び出しをしない。
- I8（設定不変）：ステータスの context 名と公開方法を変えない。branch protection / ruleset を触らない。

## Quint の要否

**書く。** ただし、関門の実行そのもの（API 呼び出し、リトライ、並行実行）の状態機械ではなく、承認の判定が赤と緑の間を動く部分だけの最小モデルである。

理由は、#134 が「緑に到達できてしまう」という到達性の問題であり、コメントの投稿・編集の並びに対して、v1 の判定と提案の判定がどこで分かれるかを反例として出せるからである。とくに「対象が編集されると承認が無効になり、緑から赤へ戻る」遷移がある。状態が無い純関数ではあるが、入力の並びに対する関門の結論の遷移はある。

モデルは `specs/quint/review-thread-ack/` に置く（実装時に README と `ci-specs.json` の登録も足す）。以下は下書きで、quint 0.32.0 で `typecheck`、`test`、`run` を実行して確かめた。

<details>
<summary>reviewThreadAck.qnt（下書き）</summary>

```quint
// -*- mode: Bluespec; -*-
// 「Check unresolved comments」の承認判定（issue #134）の最小モデル。
//
// 対象は、未解決スレッドが 0 件のときに残る「未承認のトップレベルコメント／レビュー要約」。
// 判定はコメントの集合に対する純関数で、実行中の状態機械は無い。ここで遷移として表すのは、
// コメントの投稿・編集という入力の並びに対して、公開される関門の結論が赤と緑の間を動くことだけ。
//
// STRICT = false はいまの v1（作成者名義のコメントが後ろにあれば内容によらず承認）。
// STRICT = true が提案（対象への参照と確認の語の両方を要求）。
module reviewThreadAck {
  const STRICT: bool

  type ItemId = int
  // Unrelated: 無関係な一言。MarkerOnly: 確認の語だけ。QuoteOnly: 引用だけ（TAKT の失敗報告）。
  // UrlAndMarker / QuoteAndMarker: 参照と確認の語の両方。
  type AckKind = Unrelated | MarkerOnly | QuoteOnly | UrlAndMarker | QuoteAndMarker
  type Item = { updatedAt: int }
  type Ack = { item: ItemId, kind: AckKind, at: int }

  pure val ITEMS: Set[ItemId] = Set(1, 2)
  pure val MARKED: Set[AckKind] = Set(UrlAndMarker, QuoteAndMarker)
  pure val UNRELATED: AckKind = Unrelated
  pure val MARKER_ONLY: AckKind = MarkerOnly
  pure val QUOTE_ONLY: AckKind = QuoteOnly
  pure val URL_AND_MARKER: AckKind = UrlAndMarker
  pure val QUOTE_AND_MARKER: AckKind = QuoteAndMarker

  var clock: int
  var items: ItemId -> Item
  var acks: Set[Ack]

  action init = all {
    clock' = 0,
    items' = ITEMS.mapBy(i => { updatedAt: 0 }),
    acks' = Set(),
  }

  // 作成者（または許可された人）がコメントを投稿する。at は単調に増える。
  action postAck(id: ItemId, kind: AckKind): bool = all {
    clock' = clock + 1,
    acks' = acks.union(Set({ item: id, kind: kind, at: clock + 1 })),
    items' = items,
  }

  // シミュレーション用の入力。どれか1つが起きる。
  action step = any {
    postAck(1, UNRELATED),
    postAck(1, MARKER_ONLY),
    postAck(1, QUOTE_ONLY),
    postAck(1, URL_AND_MARKER),
    postAck(1, QUOTE_AND_MARKER),
    postAck(2, UNRELATED),
    postAck(2, QUOTE_ONLY),
    postAck(2, URL_AND_MARKER),
    editItem(1),
    editItem(2),
  }

  // 対象コメントが編集される。最終編集時刻が進む。
  action editItem(id: ItemId): bool = all {
    clock' = clock + 1,
    items' = items.set(id, { ...items.get(id), updatedAt: clock + 1 }),
    acks' = acks,
  }

  pure def isAfter(i: Item, a: Ack): bool = a.at > i.updatedAt

  // 提案: 対象への参照（URL か 20 文字以上の引用）と確認の語の両方がある。
  pure def isStrictAck(i: Item, a: Ack): bool =
    isAfter(i, a) and MARKED.contains(a.kind)

  // v1: 対象より後ろの作成者コメントなら内容を問わない。
  pure def isLegacyAck(i: Item, a: Ack): bool =
    isAfter(i, a)

  pure def implementedAck(i: Item, a: Ack): bool =
    if (STRICT) isStrictAck(i, a) else isLegacyAck(i, a)

  pure def allImplementedAcked: bool =
    ITEMS.forall(i => acks.exists(a => a.item == i and implementedAck(items.get(i), a)))

  pure def allStrictAcked: bool =
    ITEMS.forall(i => acks.exists(a => a.item == i and isStrictAck(items.get(i), a)))

  // 公開される関門の結論。
  pure def gateGreen: bool = allImplementedAcked

  // 緑なら、すべての項目に提案どおりの承認がある。
  val noFalseGreen = gateGreen implies allStrictAcked
}

// いまの v1（内容を見ない）。
module legacy {
  import reviewThreadAck(STRICT = false).*
}

// 提案（参照と確認の語を要求する）。
module strict {
  import reviewThreadAck(STRICT = true).*
}
```

</details>

<details>
<summary>reviewThreadAck_test.qnt（下書き）</summary>

```quint
// -*- mode: Bluespec; -*-
// reviewThreadAck.qnt の筋書きテスト。issue #134 の実例をたどる。
module reviewThreadAck_test {
  import reviewThreadAck(STRICT = false) as before from "./reviewThreadAck"
  import reviewThreadAck(STRICT = true) as after from "./reviewThreadAck"

  // v1。無関係な一言でも、あとから投稿されれば承認になる（#134 の穴）。
  run legacyUnrelatedTurnsGreenTest =
    before::init
      .then(before::postAck(2, before::URL_AND_MARKER))
      .then(before::postAck(1, before::UNRELATED))
      .then(all {
        assert(before::gateGreen),
        before::postAck(2, before::URL_AND_MARKER),
      })

  // 提案。無関係な一言では緑にならない。
  run strictUnrelatedStaysRedTest =
    after::init
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::postAck(1, after::UNRELATED))
      .then(all {
        assert(not(after::gateGreen)),
        after::postAck(2, after::URL_AND_MARKER),
      })

  // 提案。引用だけでは緑にならない（#131 の TAKT 失敗報告は本文を引用していた）。
  run strictQuoteOnlyStaysRedTest =
    after::init
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::postAck(1, after::QUOTE_ONLY))
      .then(all {
        assert(not(after::gateGreen)),
        after::postAck(1, after::URL_AND_MARKER),
      })

  // 提案。確認の語だけでは緑にならない。
  run strictMarkerOnlyStaysRedTest =
    after::init
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::postAck(1, after::MARKER_ONLY))
      .then(all {
        assert(not(after::gateGreen)),
        after::postAck(1, after::URL_AND_MARKER),
      })

  // 提案。URL と確認の語がそろえば緑になる。
  run strictUrlAndMarkerTurnsGreenTest =
    after::init
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::postAck(1, after::URL_AND_MARKER))
      .then(all {
        assert(after::gateGreen),
        after::postAck(2, after::URL_AND_MARKER),
      })

  // 提案。20 文字以上の引用と確認の語がそろえば緑になる（#131 の実際の確認）。
  run strictQuoteAndMarkerTurnsGreenTest =
    after::init
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::postAck(1, after::QUOTE_AND_MARKER))
      .then(all {
        assert(after::gateGreen),
        after::postAck(2, after::URL_AND_MARKER),
      })

  // 提案。緑のあとで対象が編集されると、承認は無効になり赤へ戻る。
  run strictEditInvalidatesAckTest =
    after::init
      .then(after::postAck(1, after::QUOTE_AND_MARKER))
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::editItem(1))
      .then(all {
        assert(not(after::gateGreen)),
        after::postAck(1, after::QUOTE_AND_MARKER),
      })

  // 提案。対象より前の承認は数えない。
  run strictAckBeforeItemStaysRedTest =
    after::init
      .then(after::editItem(1))
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(all {
        assert(not(after::gateGreen)),
        after::postAck(1, after::QUOTE_AND_MARKER),
      })
}
```

</details>

実行結果（quint 0.32.0）。

- `quint typecheck reviewThreadAck.qnt` と `quint typecheck reviewThreadAck_test.qnt`: 成功。
- `quint test reviewThreadAck_test.qnt`: 8 passing。
- `quint run reviewThreadAck.qnt --main=strict --invariant=noFalseGreen --max-steps=8 --max-samples=300`: 違反なし。
- `quint run reviewThreadAck.qnt --main=legacy --invariant=noFalseGreen --max-steps=8 --max-samples=300`: 違反あり。反例は、項目 1 に引用だけ、項目 2 に無関係な一言を投稿した時点で `gateGreen` が真になる、という #134 と同じ形。

モデルが表さないもの：GitHub のイベント配信、ステータスの公開遅れ（新しいコメントが来てから再評価されるまでの数秒）、API の失敗、ページ取得の順。これらはこの変更で変えない（v1 と同じ）ので、モデルの範囲外にした。

## 未確認のこと

- 手元の clone のタグ `v1` が `db3bfd0` を指す理由。リモートの `v1` が過去に付け替えられたのか、手元だけで付けたのかは分からない。
- TAKT の merge-prepare が、赤のまま止まる挙動を受け入れられるか。失敗報告に確認の語を入れない規約を、どのワークフロー（takt-dash の `.takt/` か、takt 本体か）に足すかは未確認。
- 22 リポジトリのすべてを v2 に上げるか、SHA 固定の 2 つも同時に上げるかは、オーナーの判断。
- レビューの `updatedAt` が、レビュー本文の編集でコメントと同じように進むかは、GraphQL の実データで確認していない（コメントでは確認済み）。
- 2026-11-27 の open PR 15 件は、その日のスナップショットである。タグを切る前に同じ比較をもう一度回す。
- `ignored_auto_report` の既知パターンが、今後も Codex / CodeRabbit の通知を十分に覆うかは、別 issue の範囲。
