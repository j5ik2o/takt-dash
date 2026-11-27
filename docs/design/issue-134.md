# issue #134 の設計：未解決コメント関門の承認判定を厳しくする

対象：[j5ik2o/takt-dash#134](https://github.com/j5ik2o/takt-dash/issues/134)。実装は j5ik2o/ci の `review-thread-resolution`（再利用ワークフローと `scripts/review-thread-resolution.sh`）になる見込みである。この文書は設計の下書きで、takt-dash と j5ik2o/ci のどちらも変更しない。日付は 2026-11-27。

## いまの状態（実測つき）

### 参照した実物

- j5ik2o/ci のリモートのタグ `v1` は `2ead62a`（`fix(review-thread-resolution): 評価の失敗で error を発行し、壊れた応答で成功を出さない (#39)`）。`git ls-remote origin refs/tags/v1` で確認した。
- 同じ日の `origin/main` の木は `v1` と同一（`git diff v1..origin/main` が空）。`0a7037b`（Codex の利用上限の知らせを未承認に数えない）は一度 main に入ったあと、`acb57ba`（`Revert "Merge pull request #41 ..."`）で戻され、いまの main には入っていない。ただし呼び出し側の一部（takt-dash 等）は、`extra_ignored_auto_report_author_patterns` で同じ通知をリポジトリ単位に外している（takt-dash は #423）。v2 の既定には入れず、呼び出し側の設定をそのまま使う。
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
3. `a` は自動報告ではない。自動報告は判定の前に候補から外す（「判定の順序」）。自動報告かどうかは文面の語句ではなく、**著者が作成者で本文がフェンスの形**（本文全体がフェンス、または一文＋フェンス）であること、または**著者が作成者で `<!-- takt:auto-report -->` を持つ**ことで決める。確認の語や 20 文字以上の引用を含んでいても、承認にしない。**いまは作成者名義の自動投稿が承認として通るので、ここを塞ぐ。**
4. `a` は**確認の語**を含む。`確認しました` / `確認いたしました` / `確認済み` / `承知しました` / `了解しました` / `対応しました` / `対応いたしました` / `対応済み` / `対処しました` / `修正しました` / `直しました` / `acknowledged` / `addressed` / `fixed`（英語は単語境界、大小文字を問わない）。ただし、確認の語の**直前**に否定の語（`未`、`まだ`、`未だに`、`not`、`not yet`、`un-`）が付く場合と、直後が疑問（`か` のあとに `?`・`？`・`。` が続く場合、または文末の `か`）の場合は数えない。`未確認しました`、`確認しましたか？`、`未対応済み`、`not fixed` は承認にならない。`対処しました`、`直しました`、`対応いたしました`、`確認いたしました` は v1 が受けていた正当な語なので一覧に残す。コードフェンス（バッククォート 3 つ、またはチルダ 3 つで囲んだ部分）の中の語は数えない。
5. `a` は**対象を指す参照**を含む。次のいずれか。
   - 対象の URL（`https://github.com/o/r/pull/N#issuecomment-123` や `#pullrequestreview-4`）そのもの、またはそのアンカー（`#issuecomment-123` / `#pullrequestreview-4`）。**URL とアンカーは本文の長さを問わず、そのまま承認に使える。**
   - 対象本文の正規化後の連続部分で **20 文字以上**。引用符の有無は問わない。実装は、`a` の正規化後の 20 文字の窓をすべて取り、`c` の正規化後の本文に含まれる窓が 1 つでもあれば一致とする。**20 文字の下限は、この本文の引用のときにだけ掛かる。**

正規化は、HTML コメント（`<!--` から `-->` まで。途中に `>` があってもそこで切らない）と HTML タグを空白にし、行頭の `>` を外し、Markdown の強調・見出し・表の記号（`*`、`_`、バッククォート、`#`、`>`、`|`）を空白にし、空白を 1 つに畳んで前後を除き、ASCII を小文字にする。コードフェンス（バッククォート 3 つ、またはチルダ 3 つ）は残す（コードの引用は正当な参照なので消さない）。確認の語の判定だけは、先に両方のフェンスを空白にする。**URL とアンカーはこの正規化を通さない。** `#` を空白に落とす正規化を通すと、`#issuecomment-123` が本文側で消えて一致しなくなるため、正規化前の本文（大小文字だけ揃えたもの）に対して照合する。

補足すると、この条件は「明示の印」と「対象への参照」の連言である。参照だけでは #131 の失敗報告が通り、確認の語だけでは対象を指せない（後述）。20 文字は、実測で観測した最短の正当な引用（`Action not completed` の 20 文字）に合わせた。これより短いと、一般的な語が偶然一致しやすくなる。URL・アンカーにはこの下限がないので、対象本文が 20 文字未満でも、URL かアンカーを貼れば承認できる。

### 判定の順序（自動報告を先に外す）

自動報告は、承認の条件を 1 つずつ当てる前の**候補選別**で外す。順序を固定しないと、確認の語と引用を含む失敗報告が自動報告の除外を通り抜け、条件 4 と 5 を満たして通ってしまう。

前の案の whole-fence の枝は、`ack_phrase` が先にフェンスの中を空白にするため、フェンス内に語があれば ack にならず、フェンス外に語があれば枝が成立しない。つまり判定を変えていなかった（no-op）。ここでは除外を **ack の判定より前**に置き、除外した投稿が項目にも承認にも数えられないことを順序で保証する。

1. 取得したトップレベルコメントとレビュー本文のすべてから、自動報告を除く（`$candidates`）。
2. 自動報告は、既存の `ignored_auto_report`（v1 の著者＋本文パターン。CodeRabbit の要約、`extra_ignored_auto_report_author_patterns` で足した Codex の利用上限など）に、**TAKT の失敗報告の種別**を足したものである。既存のパターン一覧は、いまの項目の選び方を変えないためにそのまま使う。新しく足すのは次の 2 つの種別だけで、どちらも本文の語句（`対応しました`、`確認しました` など）では判定しない。
   - **フェンスの形**：著者が作成者（`pr_author`）で、本文がコードフェンス（``` または ~~~）で囲まれた部分だけでできている投稿（#131 の実測形）。フェンスの直前に一文だけ散文がある形（`対応しました\n```\n…引用…\n``` `）も同じ種別として扱う。行数とフェンスの形だけで決め、文面は見ない。
   - **マーカー**：著者が作成者で、本文に `<!-- takt:auto-report -->` を持つ投稿。TAKT が付ける将来の印で、フェンスの外に自由な文を書ける。TAKT は作成者名義で投稿するので、著者条件で足りる。第三者が付けたマーカーは効かない（著者が作成者のときだけ種別と認める）。
3. 残った候補から、項目（`is_item`）と承認の候補（`is_acknowledger`）を選ぶ。
4. 承認は、残った候補のうち条件 1〜5 をすべて満たすものだけを数える。`ack_phrase` はここで初めて評価する。

代表例：作成者が `対応しました\n```\n…対象の引用…\n``` ` を投稿しても、これはフェンスの形の自動報告として `$candidates` から落ちる。`ack_phrase` は評価されず、この投稿は赤（承認でも未承認項目でもない）で、項目からも消える。対象の未承認項目が別に残っていれば関門は赤のまま、残っていなければ緑になる。マーカー付きの散文報告（`<!-- takt:auto-report -->\n対応しました\n…引用…`）も同じである。

自動報告は項目にも承認にもならない（I3）。TAKT の失敗報告が将来フェンスの外に 2 行以上足すときは、`<!-- takt:auto-report -->` を付けることを TAKT 側の移行条件にする（「適用と移行」）。

Codex の利用上限の通知の扱いは、リポジトリごとに次の 3 行で固定する。

- `extra_ignored_auto_report_author_patterns` に入れているリポジトリ（takt-dash 等）では、そのコメントは `ignored_auto_report` として外れ、**項目にならない**（関門は緑）。
- 入れていないリポジトリ（modern-ddd-book 等）では、**未承認の項目として残る**（赤のまま。返信して承知扱いにする運用）。
- どちらの場合も**承認には数えない**。

### 判定の式（jq の形）

````jq
def ack_words:
  "確認しました|確認いたしました|確認済み|承知しました|了解しました|対応しました|対応いたしました|対応済み|対処しました|修正しました|直しました|\\backnowledged\\b|\\baddressed\\b|\\bfixed\\b";

def ack_hits($s):
  [ $s | match("(?i)(" + ack_words + ")"; "g") ];

# 確認の語の直前の否定と、直後の疑問。直前だけを見るので、
# 「Action not completed」を引用した正当な確認まで巻き込まない。
def negated($s; $m):
  ($s[([0, ($m.offset - 12)] | max):$m.offset]
   | test("(?i)(未|まだ|未だに?|\\bnot\\b|\\bnot\\s+yet\\b|\\bun-?)\\s*$"))
  or ($s[($m.offset + ($m.string | length)):($m.offset + ($m.string | length) + 3)]
      | test("^\\s*(か\\s*[?？。]|[?？]|か\\s*$)"));

def ack_phrase:
  (.body // "")
  | gsub("(?s)```.*?```"; " ")
  | gsub("(?s)~~~.*?~~~"; " ")
  | ascii_downcase as $s
  | (ack_hits($s)) as $hits
  | (($hits | length) > 0)
    and ([ $hits[] | select(negated($s; .) | not) ] | length > 0);

def norm_text($body):
  $body
  | gsub("(?s)<!--.*?-->"; " ")
  | split("\n")
  | map(gsub("^\\s*>\\s?"; ""))
  | map(gsub("<[^>]+>"; " "))
  | map(gsub("[*_`#>|]+"; " "))
  | map(gsub("\\s+"; " ") | sub("^ "; "") | sub(" $"; ""))
  | map(select(length > 0))
  | join("\n")
  | ascii_downcase;

def raw_body:
  (.body // "") | ascii_downcase;

def windows($n):
  . as $s | [range(0; (($s | length) - $n + 1)) as $i | $s[$i:$i + $n]];

def anchor($u):
  ($u // "" | split("#") | if length > 1 then "#" + .[-1] else "" end);

def report_author_login:
  (.author.login // "") | sub("\\[bot\\]$"; "");

def is_author:
  (.author.login // "") == ($pr_author // "");

# 既存の ignored_auto_report（著者＋本文パターン）。v1 のまま使う。
def ignored_auto_report_pattern_list:
  $ignored_auto_report_patterns
  | split("\n")
  | map(select(length > 0));

def ignored_auto_report_author_pattern_list:
  $ignored_auto_report_author_patterns
  | split("\n")
  | map(select(length > 0))
  | map(split("\t") as $parts | select($parts | length >= 2) | {
      author: $parts[0],
      pattern: ($parts[1:] | join("\t"))
    });

def ignored_auto_report:
  report_author_login as $author
  | (.body // "") as $body
  | any(ignored_auto_report_pattern_list[]; . as $p | $body | contains($p))
    or any(ignored_auto_report_author_pattern_list[];
           . as $e | $e.author == $author and ($body | contains($e.pattern)));

# TAKT の失敗報告の種別。本文の語句ではなく、マーカーと著者・本文の形で決める。
def has_takt_marker:
  (.body // "") | test("<!--\\s*takt:auto-report\\s*-->");

# 本文全体がフェンスの現在の形と、フェンスの直前に一文だけ散文がある
# 将来の形（代表例）を同じ種別にする。行数とフェンスの形だけを見る。
def fenced_takt_report:
  (.body // "") as $b
  | ($b | test("(?s)^\\s*(?:[^\\n]*\\n)?\\s*```[^\\n]*\\n.*\\n```\\s*$"))
    or ($b | test("(?s)^\\s*(?:[^\\n]*\\n)?\\s*~~~[^\\n]*\\n.*\\n~~~\\s*$"));

# マーカーは作成者の投稿に限って効く。第三者が付けたマーカーは無視する。
def takt_report:
  is_author and (has_takt_marker or fenced_takt_report);

# 自動報告か。v1 の ignored_auto_report（著者＋本文パターン）に、
# TAKT の失敗報告の種別を足す。既存の項目の選び方は変えない。
def is_auto_report:
  ignored_auto_report or takt_report;

# Codex の承知。語の一覧は条件 4 の ack_words と同じものを使う。
def trusted_acknowledgement:
  report_author_login == "chatgpt-codex-connector" and ack_phrase;

def references($target; $ack):
  norm_text($target.body) as $tb
  | norm_text($ack.body) as $b
  | ($ack | raw_body) as $raw
  | (($target.url // "") as $u
     | ($u != "" and ($raw | contains($u | ascii_downcase)))
       or (anchor($u) as $an | $an != "" and ($raw | contains($an | ascii_downcase)))
       or ($b | any(windows(20)[]; . as $w | $tb | contains($w))));

def is_item:
  (trusted_acknowledgement | not)
  and ((.author.login // "") != $pr_author)
  and (
    $pr_author_type != "Bot"
    or (
      .authorAssociation != "OWNER"
      and .authorAssociation != "MEMBER"
      and .authorAssociation != "COLLABORATOR"
    )
  );

def is_acknowledger:
  (.author.login // "") == $pr_author
  or trusted_acknowledgement
  or (
    $pr_author_type == "Bot"
    and (
      .authorAssociation == "OWNER"
      or .authorAssociation == "MEMBER"
      or .authorAssociation == "COLLABORATOR"
    )
  );

# 判定の順序: 先に自動報告を外し、残った候補だけで項目と承認を数える。
# 自動報告は is_item にも is_acknowledger にも入らない（ack_phrase より前）。
$items as $all
| ($all | map(select(is_auto_report))) as $reports
| ($all - $reports) as $candidates
| [ $candidates[]
    | select(is_item)
    | . as $item
    | [ $candidates[]
        | select(is_acknowledger)
        | select(.createdAt > ($item.updatedAt // $item.createdAt))
        | select(ack_phrase and references($item; .))
      ] as $acks
    | select($acks | length == 0)
  ]
````

`is_item` と `is_acknowledger` は v1 の `select` を分けただけである（自動報告の除外は `is_item` の中ではなく、その手前の `$candidates` で済ませる）。この順序なので、自動報告に対して `ack_phrase` は評価されない。`trusted_acknowledgement` は「Codex のコメントを項目から外す」ためだけに残し、**ほかの項目の承認としては、上の 4 と 5 を満たすときだけ**数える。これで「Codex の 1 つの `Acknowledged.` が全項目を承認する」穴も塞がる。`trusted_acknowledgement` の語の一覧は条件 4 の `ack_words` と同じものを使う（`確認いたしました`、`対応いたしました`、`Fixed` などを含む）。v1 の一覧のままだと、Codex の「確認いたしました」が項目から外れず、未承認の項目として残る。`is_auto_report` は v1 の `ignored_auto_report` に TAKT の種別判定を足しただけなので、Codex の明示の確認は自動報告に落ちない。

### 赤のときの案内

いまの `build_summary` は未承認の項目を列挙するだけで、解消の仕方を書いていない。次を足す。

```text
### 未承認のトップレベルコメント／レビュー要約
- by <author> updated at <time>: <url>
  確認するには、返信にこの URL を含めるか、本文を 20 文字以上引用し、
  「確認しました」などの語を添えてください（URL は長さを問いません）。

自動投稿（TAKT の失敗報告など）は確認として数えません。
```

URL をそのまま貼れるので、人が迷わない。TAKT の失敗報告は種別で外れるので、失敗報告に確認の語を入れないという規約は要らない（後述）。

### 変えないこと

- 未解決スレッドの判定（`isResolved`）は変えない。スレッドの返信は承認ではなく、解決（resolve）だけが関門を外す。
- ステータスの context 名（`Check unresolved comments`）、公開方法、branch protection / ruleset は変えない。
- GitHub API の呼び出しは増やさない。コメントとレビューはすでに取得しており、判定は手元の jq だけで完結する。
- 自動投稿として外す対象（CodeRabbit の要約、TAKT の失敗報告など）は変えない。既存の `ignored_auto_report`（著者＋本文パターン）もそのまま使う。変えるのは当て方の一部で、TAKT の失敗報告は文面パターンではなく種別（`<!-- takt:auto-report -->`、または著者が作成者で本文がフェンスの形）で外す（「判定の順序」）。
- Codex の利用上限の通知は、v2 の既定の `ignored_auto_report` には入れない（`0a7037b` は main で revert 済み）。呼び出し側が `extra_ignored_auto_report_author_patterns` で足しているリポジトリ（takt-dash 等）では、その通知は**項目にならない**（関門は緑）。足していないリポジトリ（modern-ddd-book 等）では、**未承認の項目として残る**（赤のまま。返信して承知扱いにする運用）。どちらの場合も**承認には数えない**。v2 はこの呼び出し側の設定を変えない。

## 採らなかった案と理由

| 案 | なぜ採らないか |
| --- | --- |
| 参照だけ（URL か引用があれば承認） | **#131 の失敗報告がそのまま通る。** 失敗報告は「You have reached your Codex usage limits for code reviews.」を本文に引用しており、参照だけの判定では 02:46:31 の時点で緑になる（手元の jq で、02:49:20 の実際の確認を除いても緑になることを確認）。#134 の事象を直せない。 |
| 確認の語だけ（対象を指さなくてよい） | どの項目への確認か分からない。「確認しました」は別件にも書けるし、自動投稿にも入れられる。項目が複数あるとき、1 つの確認で全部が通る。 |
| URL だけ（引用を認めない） | #131 の実際の確認は URL を含まず、引用と「確認しました」だけだった。いまの緑を赤に戻す。運用も「必ず URL を貼る」に変わり、移行の負担が大きい。 |
| スレッド返信があること | 対象はトップレベルの issue コメントとレビュー本文で、これらにスレッドは無い（レビューのスレッドは別の `reviewThreads` で、未解決ならすでに赤）。スレッド返信は解決（resolve）を意味しないので、承認の代わりにもならない。 |
| 作成者のコメントを一切承認に数えない | Bot の要約や通知は解決（resolve）できないため、正当に確認しても永久に赤になる。除外リストを編集するしか解消手段が無くなる。 |
| 自動投稿の文面パターンに TAKT の失敗報告を足す | 文面が変わると壊れる許可リストで、「内容によらず数えない」という要求に答えていない。リポジトリごとに TAKT の文面が違い、j5ik2o/ci の既定に入れると他リポジトリに副作用が出る。種別（マーカー・フェンスの形）で外すのが本設計である。 |
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

1. **j5ik2o/ci の main に実装を入れる。** スクリプト、スモークテスト、README の例。`ci_ref` の既定は **v1 のまま**にし、v2 は明示されたときだけ使う。`quint` はこのリポジトリの対象外なので、モデルは takt-dash 側の設計に置く。
2. **タグを切る前に、open PR の新旧比較を回す。** 22 リポジトリの open PR について、v1 の判定と提案の判定を並べ、差が 0 件であることを確認する（この文書の実測と同じ手順）。差があれば、その PR のコメントを読んで、引用の長さ・URL・確認の語のどれで落ちたかを分類し、誤って赤にしない条件を直してからタグを切る。
3. **`v2` を切る。** `git tag -a v2 -m "review-thread-resolution: require an explicit acknowledgement" <main のコミット>`、`git push origin v2`。`uses: ...@v2` が解決できることを、砂場の 1 本で確かめる。
4. **ci-gate-sandbox を v2 に上げる。** すでに機能ブランチを指している砂場で、無関係コメント・URL・引用・失敗報告の 4 例を実際の PR で流す。
5. **takt-dash を v2 に上げる。** #134 の出所であり、TAKT の自動 PR が最も多い。先に TAKT が失敗報告へ `<!-- takt:auto-report -->` を付ける変更を入れる（付け忘れても、本文全体がコードフェンス、または一文＋フェンスなら種別で外れるが、フェンスの外に 2 行以上足す形に備える）。1 週間（日次の schedule を 1 回以上含む）観察する。赤になった PR があれば、その理由が「本当に未承認」か「誤って赤」かを分類する。
6. **自動化の多い 1 リポジトリ（cqrs-es-example-java）を上げる。** 3 日観察する。
7. **残りを 5 本ずつ上げる。** 各 PR は 2 行で、`uses` と `ci_ref` を**同じ PR で** v2 に上げる。マージ後に open PR のステータスが再評価されることを確認する。
8. **最後に ci の README を整え、v1 を「フォールバック」として案内する。** v2 ワークフローの `ci_ref` の既定が v1 のままであることも、ここで確認する。v1 の削除や廃止は、22 が v2 に移ってから別途決める。

### 呼び出し側の変更（各リポジトリ 2 行）

```yaml
uses: j5ik2o/ci/.github/workflows/review-thread-resolution.yml@v2
...
ci_ref: v2
```

- 2 行は**同じ PR** で変える。`uses` だけ v2 にして `ci_ref: v1` のままだと、v2 のワークフローが v1 のスクリプトを checkout し、**古い判定のまま黙って動く**。`ci_ref` だけ v2 にすると、v1 のワークフローが v2 のスクリプトを動かす。どちらも避ける。
- `ci_ref` の既定は **v1 のまま**にする。v2 は明示的に `ci_ref: v2` を書いたときだけ使う。既定を v2 に動かすと、`uses` だけを上げたリポジトリがどちらの判定で動いたかを PR から読めなくなるため、既定は動かさない。`uses` と `ci_ref` を同じ PR で上げることを移行の必須手順にする。
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

`tests/review-thread-resolution-smoke.sh` の偽 `gh` を広げ、コメントの配列（作成者・本文・`createdAt`・`updatedAt`）を環境変数で差し替えられるようにする。既存の検査はそのまま残す。足す観点は次の 20 である。

1. 無関係な作成者コメント（「ありがとう」「別件です」）では通らない（#134 の核心）。
2. フェンスの失敗報告（対象の引用を本文全体のコードフェンスに入れ、その外に「対応しました」を足したもの）では通らない。この投稿は**項目にも承認にもならない**（報告自体は項目から消える。対象の未承認項目が別に残っていれば赤のまま、残っていなければ緑）。確認の語をフェンスの外に置くので、`ack_phrase` が先に落とすのではなく、自動報告の除外が効いていることを確かめられる。
3. 対象の URL と確認の語で通る（対象本文が 20 文字未満でも通る。URL は正規化前の本文で照合するので `#issuecomment-N` の `#` が消えない）。
4. 対象のアンカー（`#issuecomment-N` / `#pullrequestreview-N`）と確認の語で通る（本文の長さを問わない）。
5. 対象本文の 20 文字以上の引用と確認の語で通る（`Action not completed` の実例）。
6. 確認の語だけで、参照が無ければ通らない。
7. 参照だけで、確認の語が無ければ通らない。
8. 別のコメントの URL では通らない（対象の URL / アンカーだけ）。
9. 対象より前に投稿された確認では通らない。
10. 対象の最終編集より前の確認では通らない。編集後に確認し直すと通る。
11. コードフェンス（3 つのバッククォート）の中の確認の語は数えない（`unacknowledged` のような部分一致もしない）。
12. `~~~` のフェンスの中の確認の語も数えない。
13. HTML コメントの除去は `<!--` から `-->` まで。コメントの中に `>` が含まれていても、そこで切れて引用の照合が崩れない。
14. 確認の語の直前に否定（未・まだ・not・not yet・un-）が付く場合と、直後が疑問（`確認しましたか？`）の場合は通らない。`未確認しました` / `未対応済み` / `not fixed` を固定する。
15. v1 が受けていた `対処しました` / `直しました` / `対応いたしました` / `確認いたしました` で通る（正当な緑を赤にしない）。
16. 自動報告の除外を、項目と承認の両方で確かめる。(a) `<!-- takt:auto-report -->` 付きの散文報告（+ 確認の語 + 20 文字以上の引用）は項目にも承認にもならない。(b) 第三者が付けた同じマーカーは承認にならない（他人のマーカーで緑にできない。著者が作成者のときだけマーカーを種別と認める）。(c) 作成者名義の `対応しました\n```\n…対象の引用…\n``` ` が、`ack_phrase` より前の除外で落ちる（代表例）。
17. 作成者以外の第三者の確認は数えない。作成者が Bot の PR では、OWNER / MEMBER / COLLABORATOR の人間の確認だけ数える。
18. #131 の実データをそのまま固定し、02:49:20 を含むと通り、02:48 までだと通らない（失敗報告だけでは通らない）。
19. いま緑の PR が赤に戻らない。`event-store-adapter-java#871` の実データで緑を維持し、いま赤の `event-store-adapter-kotlin#926` と `event-store-adapter-rs#381` は赤のまま。open PR 15 件の新旧比較で差が 0 件。
20. `ci_ref` を省略した呼び出しは v1 のスクリプトを使い、`ci_ref: v2` を明示したときだけ v2 を使う。

補足すると、19 はスモークテストに実データを置くというより、タグを切る前の使い捨ての比較スクリプトで確かめるのが向いている。固定のフィクスチャとしては 1〜18 と 20 を置き、19 は移行手順のチェックリストに残す。

## 影響と戻し方

### 影響するリポジトリ（2026-11-27 時点）

上の 22 リポジトリ。すべてで `Check unresolved comments` が必須。v2 に上げたリポジトリは、次の評価から新しい判定になる。

- 既存の open PR で、内容によらない作成者コメントだけに頼って緑だったものは、赤に戻る可能性がある。2026-11-27 の 15 件では 0 件だったが、将来の PR では起こり得る。
- 新しく赤になった PR は、作成者が URL（長さを問わない）か 20 文字以上の引用に「確認しました」を添えて返信すれば緑になる。関門の要約にその案内を出す。
- TAKT の失敗報告は、種別（`<!-- takt:auto-report -->`、または著者が作成者で本文がフェンスの形）で自動報告として外れるので承認に数えられない。TAKT が失敗報告を投稿しても、関門は赤のまま人を待つ。これは #134 の意図どおりである。失敗報告に確認の語を入れない規約は要らなくなった。代わりに、フェンスの外に 2 行以上足す形にするときは `<!-- takt:auto-report -->` を付ける必要がある（別 PR、未確認）。
- Codex の利用上限の通知は、`extra_ignored_auto_report_author_patterns` に入れているリポジトリ（takt-dash 等）では項目にならず（緑）、入れていないリポジトリ（modern-ddd-book 等）では未承認の項目として残る（赤のまま）。どちらの場合も承認には数えない。
- SHA 固定の 2 つ（fraktor-rs、g2g）とブランチ固定の 1 つ（ci-gate-sandbox）は、v2 に上げるまで変わらない。同じ穴が残る。
- 追加の API 呼び出し、保存データ、公開 API、ブランチ保護の設定変更は無い。GitHub のレート制限への影響も無い。

### 戻し方

「適用と移行」の表のとおり。もっとも速いのは、そのリポジトリの 2 行を v1 に戻すこと。全体を一瞬で戻す必要があるときだけ、v2 タグの付け替えを例外として使う。

## 守る不変条件

- I1（緑の意味）：関門が緑になるのは、未解決スレッドが 0 件で、かつすべての未承認項目に有効な承認があるときだけ。
- I2（#134 の修正）：確認の語も対象への参照も無い作成者コメントは、決して承認にならない。
- I3（自動報告）：自動報告（既知の自動投稿、著者が作成者で `<!-- takt:auto-report -->` を持つ投稿、著者が作成者で本文がフェンスの形の TAKT の失敗報告）は、項目にも承認の候補にもならない。確認の語や引用を含んでいても承認にならない。TAKT の失敗報告かどうかは著者と種別で決め、本文の語句では決めない。第三者が付けたマーカーは種別と認めない（他人のマーカーで項目を消したり緑にしたりできない）。
- I4（順序）：対象の最終編集より前のコメントは承認にならない。承認のあとで対象が編集されたら、承認し直しが必要。
- I5（純関数）：判定は取得済みのコメントとレビューの集合だけで決まる。実行順、並行性、リトライ、ページ取得の順に依存しない。同じ入力なら同じ結論。
- I6（正当な確認）：対象の URL / アンカーと確認の語（URL・アンカーは長さを問わない）、または 20 文字以上の引用と確認の語がそろえば、緑になる。
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
// STRICT = true が提案（対象への参照と確認の語の両方を要求し、自動報告（フェンスの形・マーカー）と否定付きの語は候補から外す）。
module reviewThreadAck {
  const STRICT: bool

  type ItemId = int
  // Unrelated: 無関係な一言。MarkerOnly: 確認の語だけ。QuoteOnly: 引用だけで確認の語がない投稿。
  // UrlAndMarker / QuoteAndMarker: 参照と確認の語の両方。
  // AutoReport: 自動報告（本文全体がフェンス、一文＋フェンス、または `<!-- takt:auto-report -->`。
  // 確認の語と引用を含むが、著者・種別で候補から外れる）。第三者が付けたマーカーは
  // 著者条件で無効なので、モデルでは Unrelated と同じ（承認にならない）として扱う。
  // NegatedMarker: 否定付きの確認の語（未確認しました等）。
  type AckKind = Unrelated | MarkerOnly | QuoteOnly | UrlAndMarker | QuoteAndMarker | AutoReport | NegatedMarker
  type Item = { updatedAt: int }
  type Ack = { item: ItemId, kind: AckKind, at: int }

  pure val ITEMS: Set[ItemId] = Set(1, 2)
  pure val MARKED: Set[AckKind] = Set(UrlAndMarker, QuoteAndMarker)
  pure val UNRELATED: AckKind = Unrelated
  pure val MARKER_ONLY: AckKind = MarkerOnly
  pure val QUOTE_ONLY: AckKind = QuoteOnly
  pure val URL_AND_MARKER: AckKind = UrlAndMarker
  pure val QUOTE_AND_MARKER: AckKind = QuoteAndMarker
  pure val AUTO_REPORT: AckKind = AutoReport
  pure val NEGATED_MARKER: AckKind = NegatedMarker

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
    postAck(1, AUTO_REPORT),
    postAck(1, NEGATED_MARKER),
    postAck(2, UNRELATED),
    postAck(2, QUOTE_ONLY),
    postAck(2, URL_AND_MARKER),
    postAck(2, AUTO_REPORT),
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
  // 自動報告は ack の判定より前に候補から外れる（モデルでは AutoReport を MARKED に入れない）。
  // 否定付きの確認の語も MARKED に入れない。
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

  // 提案。自動報告（フェンスの形・マーカー）は確認の語と引用を含んでいても、
  // ack の判定より前に種別で外れて緑にならない。
  run strictAutoReportStaysRedTest =
    after::init
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::postAck(1, after::AUTO_REPORT))
      .then(all {
        assert(not(after::gateGreen)),
        after::postAck(1, after::URL_AND_MARKER),
      })

  // 提案。否定付きの確認の語（未確認しました等）では緑にならない。
  run strictNegatedMarkerStaysRedTest =
    after::init
      .then(after::postAck(2, after::URL_AND_MARKER))
      .then(after::postAck(1, after::NEGATED_MARKER))
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
- `quint test reviewThreadAck_test.qnt`: 10 passing。
- `quint run reviewThreadAck.qnt --main=strict --invariant=noFalseGreen --max-steps=8 --max-samples=300`: 違反なし。
- `quint run reviewThreadAck.qnt --main=legacy --invariant=noFalseGreen --max-steps=8 --max-samples=300`: 違反あり。反例は、項目 1 に引用だけ、項目 2 に無関係な一言を投稿した時点で `gateGreen` が真になる、という #134 と同じ形。

モデルが表さないもの：GitHub のイベント配信、ステータスの公開遅れ（新しいコメントが来てから再評価されるまでの数秒）、API の失敗、ページ取得の順。これらはこの変更で変えない（v1 と同じ）ので、モデルの範囲外にした。文字列の正規化（URL の `#`、HTML コメントの `-->`、`~~~` のフェンス）と 20 文字の下限も、モデルではなく jq の実装とスモークテストで確かめる。

## 改訂の記録（レビュー 2 回目で直した 7 点）

| 指摘 | 直した場所 | どう直したか |
| --- | --- | --- |
| 高1. URL・アンカーが正規化で一致しない | 「採用する承認の条件」5、正規化の段落、判定の式の `raw_body` / `references` | URL とアンカーは正規化前の本文（大小文字だけ揃える）で照合する。`#` を空白に落とす `norm_text` を通さない。 |
| 高2. 失敗報告が確認の語＋引用で承認になる | 条件 3、「判定の順序」、jq の `$candidates`、I3 | 自動報告を先に候補から外し、残った候補だけで項目と承認を数える。TAKT の失敗報告は文面ではなく、`<!-- takt:auto-report -->` か「著者が作成者で本文がフェンスの形」という種別で決める。Codex の利用上限はリポジトリごとの `extra_ignored_auto_report_author_patterns` で扱い、どちらの場合も承認には数えない（3 回目で修正）。 |
| 中1. 否定文が確認語に当たる | 条件 4、`ack_phrase` / `negated` | 確認の語の直前に `未` / `まだ` / `not` / `un-` などが付く場合と、直後が疑問（`確認しましたか？`）の場合は数えない。`対処しました` / `直しました` / `対応いたしました` / `確認いたしました` を一覧に足す。 |
| 中2. 本文が 20 文字未満だと承認できない | 条件 5、正規化の段落、赤のときの案内、I6 | URL・アンカーは長さを問わず承認に使えると明記。20 文字の下限は本文の引用だけに掛ける。 |
| 中3. `ci_ref` の既定を v2 にすると黙って v1 判定になる | 移行の順 1・5・7・8、呼び出し側の変更 | `ci_ref` の既定は v1 のまま。v2 は明示したときだけ。`uses` と `ci_ref` を同じ PR で上げることを必須手順にした。 |
| 低1. `>` を含む HTML コメントで除去が崩れる | 正規化の段落、`norm_text` | 除去の規則を `<!--` から `-->` までに直した（途中の `>` で切らない）。 |
| 低2. `~~~` のフェンスを除去していない | 条件 4、`ack_phrase` | 3 連バッククォートに加えて `~~~` のフェンスも空白にする。 |

テストの観点には、3・4（URL / アンカーの長さと `#`）、12・13（`~~~` と HTML コメント）、14・15（否定と v1 の語）、16（自動報告の順序）、20（`ci_ref` の既定）を足した。Quint には `AutoReport` と `NegatedMarker` の種別を足し、どちらも緑にならないことを 2 本のテストで確かめた。

## 改訂の記録（レビュー 3 回目で直した 4 点）

- **高2 のやり直し（自動報告の除外を効く形にする）**：「判定の順序」を書き直し、自動報告を `$candidates` を作る時点で外すようにした。前の案の whole-fence の枝が no-op だった理由（`ack_phrase` が先にフェンスを空白にする）も明記した。判定は文面の語句ではなく、著者が作成者であることと、フェンスの形（本文全体がフェンス、または一文＋フェンス）または `<!-- takt:auto-report -->` で行う。代表例 `対応しました\n```\n…対象の引用…\n``` ` が項目にも承認にもならないことを書いた。jq は `is_auto_report` → `$candidates` → `is_item` / `is_acknowledger` → 条件 4・5 の順に並べ、自動報告に対して `ack_phrase` を評価しないことをコメントした。
- **Codex の上限コメントの記述の矛盾**：「判定の順序」「変えないこと」「影響と戻し方」を、リポジトリごとの 3 行（`extra_ignored_auto_report_author_patterns` に入れているリポジトリ＝項目にならず緑／入れていないリポジトリ＝未承認の項目として赤のまま／どちらの場合も承認には数えない）に揃えた。旧 93 行目と旧 201 行目の「項目に残る」「外す対象」の矛盾を解消し、v2 の既定には入れず呼び出し側の設定をそのまま使うことを明記した。
- **`trusted_acknowledgement` の語彙**：条件 4 の `ack_words` と同じ一覧を使う形に直した。`確認いたしました` / `対応いたしました` / `Fixed` なども Codex の承知として扱うので、v1 の一覧のままで残っていた「確認いたしました」が未承認の項目として残らない。ほかの項目の承認としては、条件 4 と 5 を満たすときだけ数える点は変えない。
- **テストの観点 2 と 16**：2 はフェンスの失敗報告に確認の語をフェンスの外へ足した形（`ack_phrase` が先に落とす形ではない）で、項目にも承認にもならないことを固定する。16 はマーカー付きの散文報告、第三者が付けたマーカー、作成者名義の `対応しました` ＋フェンス引用の 3 つを固定する。第三者のマーカーは著者条件で無効にし、他人のマーカーで緑にできないことを書いた。
- **Quint**：`AutoReport` のコメントを新しい種別（本文全体がフェンス、一文＋フェンス、マーカー）に合わせ、ack の判定より前に候補から外れることを記述した。第三者のマーカーはモデルでは `Unrelated` と同じ扱いとした。モデルの構造とテストの筋書きは変えていない。
- **適用・移行・戻し方**：v2 タグ、移行順、戻し方の骨格は変えていない。TAKT の失敗報告の説明だけ、フェンスの外に 2 行以上足すときはマーカーが必要になる形に合わせた。

## 未確認のこと

- 手元の clone のタグ `v1` が `db3bfd0` を指す理由。リモートの `v1` が過去に付け替えられたのか、手元だけで付けたのかは分からない。
- TAKT の merge-prepare が、赤のまま止まる挙動を受け入れられるか。失敗報告へ `<!-- takt:auto-report -->` を付ける変更を、どのワークフロー（takt-dash の `.takt/` か、takt 本体か）に入れるかは未確認。マーカーが付くまでは、本文全体がコードフェンスの旧来の形と、フェンスの直前に一文だけ置く形を種別で外せる。フェンスの外に 2 行以上置く形は、マーカーが付くまで自動報告として外れない。
- `extra_ignored_auto_report_author_patterns` に Codex の利用上限の通知を入れているリポジトリの完全な一覧は未確認（この文書では takt-dash を確認した）。入れていないリポジトリでは、通知が未承認の項目として残る。
- 22 リポジトリのすべてを v2 に上げるか、SHA 固定の 2 つも同時に上げるかは、オーナーの判断。
- レビューの `updatedAt` が、レビュー本文の編集でコメントと同じように進むかは、GraphQL の実データで確認していない（コメントでは確認済み）。
- 2026-11-27 の open PR 15 件は、その日のスナップショットである。タグを切る前に同じ比較をもう一度回す。
- 既存の `ignored_auto_report` の本文パターンが、今後も Codex / CodeRabbit の通知を十分に覆うかは、別 issue の範囲である。
