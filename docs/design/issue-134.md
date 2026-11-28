# issue #134 の設計：未解決コメント関門の承認判定を厳しくする

対象：[j5ik2o/takt-dash#134](https://github.com/j5ik2o/takt-dash/issues/134)。実装は j5ik2o/ci の `review-thread-resolution`（再利用ワークフローと `scripts/review-thread-resolution.sh`）になる見込みである。この文書は設計の下書きで、takt-dash の実装と j5ik2o/ci のどちらも変更しない（takt-dash には Quint のモデル `specs/quint/review-thread-ack/` だけを置く）。日付は 2026-11-27。2026-11-28 の点検で見つかった穴 3 件（[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44)、[#443](https://github.com/j5ik2o/takt-dash/issues/443)、[j5ik2o/ci#45](https://github.com/j5ik2o/ci/issues/45)）を反映した。同じ日の 2 回目の直しで、[j5ik2o/ci#43](https://github.com/j5ik2o/ci/pull/43)（先端 86a443c）のレビューで見つかった、b808a29 の規則に残る穴 4 つ（まとめ返信で確認の語が別の項目にも効く、行頭 `>` の引用の中の `fixed` を数える、`not been fixed` を確認と数える、`確認済みではありません` を確認と数える）を塞いだ（「確認の語の効く範囲と打ち消し」）。

## 何に触れるか

| 対象 | 触れるか | 中身 |
| --- | --- | --- |
| 公開 API | 触れない | takt-dash の API は変えない。j5ik2o/ci の再利用ワークフローの inputs も変えない（新しいタグ `v2` を足すだけ）。 |
| 保存データ | 触れない | 判定は取得済みのコメントだけで完結し、何も保存しない。 |
| 秘密 | 触れない | 使うトークンと権限は v1 と同じ。 |
| 守りの仕組み | **触れる** | 22 リポジトリで必須チェックの `Check unresolved comments` の判定を厳しくする。誤ると、緑にしてはいけない PR がマージできる（偽の緑）か、正当な PR が止まる（偽の赤）。 |

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
- 2026-11-27 時点の 22 リポジトリの open PR は 15 件。後述の比較で、v1 と提案の判定が違った項目は 0 件だった。この比較は 2026-11-28 の直し（対象を一意に指す値）の前の提案で行った。

### 現在の緑を赤に戻さないことの下見（実測）

open PR 15 件のコメントとレビューを取得し、v1 の判定とこの設計の判定を jq で並べて比べた。

- 未承認の項目は 3 件だけだった。
  - `event-store-adapter-java#871`: CodeRabbit の「Action not completed / Review rate limited」の通知。v1 は緑（00:31 の作成者コメントが他の項目の URL を列挙し、01:42 の「…「Review rate limited / Action not completed」を確認しました。」が引用で承認）。この比較の時点の提案も緑。**2 回目の直しのあとの規則では赤になる**（下の 2026-11-28 の測り直し）。
  - `event-store-adapter-kotlin#926`: Codex の利用上限の通知。v1 も提案も赤（通知のあとに作成者のコメントが無い）。
  - `event-store-adapter-rs#381`: CodeRabbit の利用上限の通知。v1 も提案も赤。
- 15 件のうち、v1 が緑で提案が赤になる PR は 0 件。v1 が赤で提案が緑になる PR も 0 件。
- 直近の merged 48 件（takt-dash、dotfiles、event-store-adapter-java、cqrs-es-example の各 12 件）でも、未承認の項目は 0 件で、新旧の差は出なかった。ただし merged の履歴は「その時点の判定」ではなく「いま取得できるコメント」での比較であり、削除済みコメントは見えない。
- 2026-11-28 の 1 回目の直しのあとの規則（b808a29 の jq）で、実データを 2 件だけ測り直した。`event-store-adapter-java#871` は緑のまま（01:42 の引用「Review rate limited / Action not completed」は、この PR の項目の中で CodeRabbit の通知だけが持つ）。takt-dash#131 は 02:48 までだと利用上限の通知が未承認で赤、02:49:20 の確認を含めると緑。ほかの 14 件は測り直していない（「移行の順」2 で、承認の経路まで分けて回す）。
- 2 回目の直し（「確認の語の効く範囲と打ち消し」）のあとの規則（この文書の jq をそのまま切り出したもの）で、同じ 2 件を測り直した。takt-dash#131 は変わらない（02:48 までは赤、02:49:20 を含めると緑）。**`event-store-adapter-java#871` は赤になる。** 項目は 2 つで、CodeRabbit の通知は 01:42 の引用で承認されたままだが、Codex の利用上限の通知（[#issuecomment-7206241589](https://github.com/j5ik2o/event-store-adapter-java/pull/871#issuecomment-7206241589)。このリポジトリは `extra_ignored_auto_report_author_patterns` で外していない）が未承認に戻る。b808a29 でこの通知を承認していたのは 00:31 のまとめ返信で、通知の URL を含むリストの項目は `I noted it. Codex did not review this round.` と書くだけで、確認の語を含まない。b808a29 は、別のリストの項目の `Five are fixed in 834cb5b.` の `fixed` をこの URL にも効かせていた（穴 1 そのもの）。`noted` を確認の語に足せば緑のまま残せるが、この設計では足さない（「未確認のこと」）。

## 何をどう変えるか

### 採用する承認の条件（1 つ）

後のコメント `a` が、次を**すべて**満たすときだけ、項目 `c` を承認したとみなす。

1. `a` は対象より後である（`a.createdAt > c.updatedAt`。対象の最終編集より後）。
2. `a` は承認できる人による。作成者（`pr_author`）、または Codex の確認コメント、または作成者が Bot のときの OWNER / MEMBER / COLLABORATOR。**作成者以外の第三者のコメントは承認に数えない**（いまも数えていない）。
3. `a` は自動報告ではない。自動報告は判定の前に候補から外す（「判定の順序」）。自動報告かどうかは文面の語句ではなく、**著者が作成者で本文がフェンスだけ**（散文が無い）であること、または**著者が作成者で、フェンスの外に `<!-- takt:auto-report -->` を持つ**ことで決める。散文がある投稿（`対応しました\n```\n…引用…\n``` ` や、マーカー付きの報告を引用した確認）は自動報告にしない。確認の語や 20 文字以上の引用を含んでいても、自動報告なら承認にしない。**いまは作成者名義の自動投稿が承認として通るので、ここを塞ぐ。**
4. `a` は**確認の語**を含む。`確認しました` / `確認いたしました` / `確認済み` / `承知しました` / `了解しました` / `対応しました` / `対応いたしました` / `対応済み` / `対処しました` / `修正しました` / `直しました` / `acknowledged` / `addressed` / `fixed`（英語は単語境界、大小文字を問わない）。`対処しました`、`直しました`、`対応いたしました`、`確認いたしました` は v1 が受けていた正当な語なので一覧に残す。語として数えるのは**作成者が自分の文で書いた、打ち消されていない語**だけである（2 回目の直し）。コードフェンス（バッククォート 3 つ、またはチルダ 3 つで囲んだ部分）・行頭 `>` の引用・「」『』“” の中・インラインのコード・項目の本文を写した部分の中の語は数えない。直前の否定（`未`、`まだ`、`not` など）、英語の助動詞を挟む否定（`has not been fixed`、`hasn't been fixed`）と条件・未来（`once this is fixed`、`will be fixed`）、日本語の後ろに付く否定（`確認済みではありません`、`対応済みでない`）、疑問（`確認しましたか？`、`確認済みですか`、`Is it fixed?`）の語も数えない。規則の全体は「確認の語の効く範囲と打ち消し」に書く。フェンスの境界は「フェンスの境界」の規則で決める。
5. `a` は**対象を一意に指す値**を、完全な形で含む。次のいずれか。**4 の語と 5 の値は、`a` の同じ単位（段落・リストの項目・節）になければならず、`a` のどこかが対象を「まだ」と打ち消していてはならない**（2 回目の直し。「確認の語の効く範囲と打ち消し」）。
   - **対象のコメントの id**。対象の URL（`https://github.com/o/r/pull/N#issuecomment-123` や `…#pullrequestreview-4`）の全体、またはそのアンカー（`#issuecomment-123` / `#pullrequestreview-4`）。アンカーの直後に数字が続く場合は一致としない（`#issuecomment-10` は `#issuecomment-101` に当たらない）。URL の全体はアンカーを含むので、照合はアンカーで足りる。**id は本文の長さを問わず、そのまま承認に使える。**
   - **対象だけが持つ本文の断片**。URL を両方の本文から外して正規化したあとの、連続する **20 文字以上**の部分で、対象の本文にあり、**この PR のほかの項目のどれにも無い**もの。引用符の有無は問わず、**コードフェンスで囲んだ引用も対象にする**（人が対象を引用するときにフェンスを使うのは自然なので、フェンス内の本文も照合する）。実装は、`a` の 20 文字の窓をすべて取り、対象の本文に含まれ、ほかの項目の本文に含まれない窓が 1 つでもあれば一致とする。**20 文字の下限は、この断片のときにだけ掛かる。**

正規化は、HTML コメント（`<!--` から `-->` まで。途中に `>` があってもそこで切らない）と HTML タグを空白にし、行頭の `>` を外し、Markdown の強調・見出し・表の記号（`*`、`_`、バッククォート、`#`、`>`、`|`）を空白にし、空白を 1 つに畳んで前後を除き、ASCII を小文字にする。コードフェンス（バッククォート 3 つ、またはチルダ 3 つ）は残す（コードの引用は正当な参照なので消さない）。確認の語の判定だけは、先に両方のフェンスを空白にする。つまり、**引用の照合はフェンス内の本文を対象にし、フェンスを外すのは確認の語の判定だけ**である。**id はこの正規化を通さない。** `#` を空白に落とす正規化を通すと、`#issuecomment-123` が本文側で消えて一致しなくなるため、正規化前の本文（大小文字だけ揃えたもの）に対して照合する。

**断片の照合の前に、URL を両方の本文から外す**（[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44)）。外すのは `https?://` から空白・`)`・`>`・`]` の手前までと、scheme の無い `ホスト名/パス`（`github.com/j5ik2o/ci` の形）である。URL は id の照合にだけ使う。外さないと、`https://github.com/j`（ちょうど 20 文字）のような**どの URL にも共通の頭**が窓として一致し、作成者が別件の URL を貼って「確認しました」と書くだけで、リポジトリへのリンクを含む項目（Codex の要約など）がすべて承認になる。ci#43 の 442d679 で実際に起きた（項目に `…/g2g/blob/abc123/src/retry.rs#L10`、確認に `別件の https://github.com/j5ik2o/ci/pull/43 を確認しました` で `got 0 / success`）。項目の URL の中のパス（`src/retry.rs#L10`）も照合に使えなくなるので、その項目を指すには項目の URL を貼るか、URL の外の文を引用する。

**ほかの項目にもある断片は使わない**（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。Codex の要約の見出しや利用上限の通知のような定型文は、同じ PR の複数の項目に現れる。その引用はどの項目を指すかが定まらないので、1 つの確認で別の項目まで承認しないよう、どの項目の承認にも使わない。同じ文の項目が複数あるときは、それぞれの URL（アンカー）を貼る。「ほかの項目」は、自動報告を外した候補のうち項目（`is_item`）であるもので、承認済みかどうかは問わない（判定が承認の順に依存しないため。I5）。

補足すると、この条件は「明示の印」と「対象を一意に指す値」の連言で、2 回目の直しでその連言を確認の全体ではなく単位ごとに取る形にした。参照だけでは #131 の失敗報告が通り、確認の語だけでは対象を指せない（後述）。20 文字は、実測で観測した最短の正当な引用（`Action not completed` の 20 文字）に合わせた。これより短いと、一般的な語が偶然一致しやすくなる。id にはこの下限がないので、対象本文が 20 文字未満でも、URL かアンカーを貼れば承認できる。

### 一意に指す値として何を使うか

項目（トップレベルコメントとレビュー要約）を一意に指す値の候補を比べた。

| 候補 | 一意か | 作成者が書けるか | 採否 |
| --- | --- | --- | --- |
| コメントの URL の全体 | 一意（アンカーに id を含む） | 関門の要約がそのまま出すので、貼るだけ | **採る**（照合はアンカーで足りる） |
| アンカー（`#issuecomment-N` / `#pullrequestreview-N`） | 一意（N は GitHub の databaseId で、種別ごとに GitHub 全体で一意。PR を書かなくても別の PR と取り違えない） | URL の末尾を写すだけ | **採る** |
| 項目だけが持つ 20 文字以上の断片（URL を除く） | この PR の項目の中で一意 | 引用するだけ。いまの確認の大半がこの形（下の実測） | **採る**（ほかの項目にある断片と URL は使わない） |
| 20 文字の窓（URL の頭や定型文を含む。ci#43） | 一意でない | — | 採らない（ci#44 と #443 の穴） |
| スレッドの id | — | — | 対象外。レビューのスレッドは解決（resolve）で外れ、承認の判定の対象ではない |
| GraphQL の node id（`IC_kwDO…`） | 一意 | 画面に出ず、人が書けない | 採らない |
| PR 番号と項目の順番（「#431 の項目 2」） | 一意でない | 書けるが、項目の順番はコメントが増えると変わる | 採らない |
| 接頭辞の無い databaseId（`comment6767269753`、`レビュー 5841855907`） | 数字だけでは run id や SHA の数字部分と区別できない | 実際に書かれている（event-store-adapter-kotlin#906） | 採らない。アンカーの形にしてもらう |

id（URL・アンカー）を第一の手段にする。引用を残すのは、いまの確認の大半が引用だけで書かれているためである（下の実測）。引用をやめて id だけにすると、#131 の実際の確認（02:49:20）と event-store-adapter-java#871 の確認がどちらも赤になり、「採らなかった案」の「URL だけ」と同じ移行の負担が出る。断片を「その項目だけが持つもの」に限れば、断片も PR の中で項目を一意に指す値になり、完全一致（部分文字列としての一致）で照合できる。

作成者が書く確認の文の形（例）。

```text
# 推奨：URL（またはアンカー）を貼る。本文の長さ、定型文の有無を問わない。
https://github.com/j5ik2o/takt-dash/pull/131#issuecomment-6905873562 を確認しました。

# 複数の項目を 1 つのコメントで承認する。項目ごとに URL を並べる。
# 前置きの段落に確認の語があり、リストの項目に語も打ち消しも無ければ、前置きの語が項目に効く。
次の 2 件を確認しました。
- #issuecomment-6905873562
- #pullrequestreview-5841855907

# 項目ごとに書き分ける。未対応の項目には確認の語を付けない（その項目は承認されない）。
- #issuecomment-6905873562 を確認しました。
- #pullrequestreview-5841855907 は未対応です。次の PR で直します。

# 引用：その項目だけが持つ 20 文字以上の文を引用する（URL の外の文）。
Codex の「You have reached your Codex usage limits for code reviews.」を確認しました。

# 承認にならない例
別件の https://github.com/j5ik2o/ci/pull/43 を確認しました。      # 対象の id でも、対象の文でもない
「Codex Review: Here are some automated review suggestions」を確認しました。  # 同じ見出しの要約が 2 つあるとき
#issuecomment-10 は確認しました。#issuecomment-11 は未対応です。          # 11 は承認しない（10 は承認する）
This has not been fixed. #issuecomment-10                                  # 打ち消された語
#issuecomment-108 は確認済みではありません。                                # 後ろに付く否定
> Once this is fixed, please rerun the integration tests.                  # 引用の中の fixed だけで、
Which tests should I run?                                                  # 作成者の文に確認の語が無い
```

利用者の確認の書き方（2026-11-28 時点、各リポジトリの直近 15 PR の、作成者名義で確認の語を含むコメント）。

| リポジトリ | 確認 | アンカーを含む | URL を含みアンカーは無い |
| --- | --- | --- | --- |
| takt-dash | 6 | 0 | 0 |
| pekko-persistence-dynamodb | 2 | 0 | 1（#1040。run の URL） |
| event-store-adapter-kotlin | 6 | 1 | 2（#906。compare の URL と、接頭辞の無い id） |
| dotfiles | 3 | 0 | 0 |
| modern-ddd-book | 0 | 0 | 0 |

17 件のうちアンカーを含むのは 1 件だけで、13 件は URL も含まない（引用か、項目を指さない記録。どちらかはこの集計では分けていない）。いまの v1 は内容を見ないので、どれも緑のままである。v2 では、引用の確認はその文が項目に固有なら通り続ける。URL を含みアンカーの無い 3 件は、ci#43 の規則なら URL の頭で通りえた形で、この設計では URL の外の引用か、項目のアンカーが要る。どれもタグを切る前の比較（「移行の順」2）で、承認の経路まで分けて確かめる。

2 回目の直しの影響は、同じ 5 リポジトリで測り直した。上の 17 件は、集計に使った一覧が残っておらず、同じ件をそろえ直せなかった。代わりに 2026-11-28 に、同じ 5 リポジトリの直近 15 PR（`pullRequests(last: 15)`）の、作成者名義で確認の語を含むコメントとレビュー本文を取り直した。30 件（takt-dash 7、pekko-persistence-dynamodb 9、event-store-adapter-kotlin 7、dotfiles 4、modern-ddd-book 3）で、上の表の pekko-persistence-dynamodb#1040 と event-store-adapter-kotlin#906 を含む。各リポジトリの実際の `extra_ignored_auto_report_author_patterns`（modern-ddd-book のほかの 4 つは Codex の利用上限の通知を外す）で項目を選び、各コメントが承認する項目を、b808a29 の規則とこの文書の規則で並べた。あわせて takt-dash#131 と event-store-adapter-java#871 も流した。

| 標本 | 件数 | b808a29 で承認する項目がある | 2 回目の規則で承認しなくなる |
| --- | --- | --- | --- |
| 項目が無い（PR の項目がすべて自動投稿として外れる） | 24 | 0 | 0 |
| 項目がある | 6 | 1（modern-ddd-book#695） | 1（modern-ddd-book#695） |
| takt-dash#131（#131 の時点の設定）、event-store-adapter-java#871 | 3 | 3 | 1（event-store-adapter-java#871 の 00:31） |

- **modern-ddd-book#695（マージ済み）**：まとめ返信が見出しで 2 節に分かれる。1 節目（Devin のレビュー）は `…すべて直しました` で承認する。2 節目は `対象: <Codex の 2 つの URL>` と `指摘と対応: どちらも Codex の利用上限の知らせで、指摘はありません。直すものはありません。` で、確認の語が無い。b808a29 は 1 節目の `直しました` を 2 節目の URL に効かせて、Codex の利用上限の通知を承認していた。2 回目の規則では承認しない（いまこの PR を評価し直せば赤）。
- **event-store-adapter-java#871（open）**：上の「下見」のとおり、00:31 のリストの項目 `I noted it.` に確認の語が無く、Codex の利用上限の通知が未承認に戻る。PR は赤になる。
- 残りの 4 件は、b808a29 でも承認する項目が無く（項目を名前で書くだけで、id も固有の引用も無い）、2 回目の直しで変わらない。b808a29 の規則そのものが v1 より多くを赤にすることは、「移行の順」2 の新旧比較で扱う。

2 件とも、赤になるのは「確認の語の無い単位で項目を指す」形で、作成者がその単位に `確認しました` を足せば緑に戻る。どちらも、確認の語の効く範囲を単位に限ったこと（穴 1 の直し）だけによる。引用・否定の直し（穴 2〜4）で承認しなくなった確認は、30 件の中に無かった。

### フェンスの境界

確認の語（条件 4）、フェンスの外のマーカー、フェンスだけの本文（条件 3）の 3 つの判定は、**同じ 1 つのフェンスの境界**で決める（jq の `fence_split`）。境界を判定ごとに別の正規表現で取ると、同じ本文で「フェンスの中」の範囲が食い違う。

- 行頭（空白 3 つまで）のバッククォートかチルダが 3 つ以上並ぶ行でフェンスが開く。情報文字列（```` ```text ```` の `text`）があってもよい。
- 開きと同じ文字が、開きと同じ数以上だけ並ぶ行で閉じる。4 つのバッククォートで開いたフェンスは、中の 3 つのバッククォートの行では閉じない（マーカー付きの報告を 4 つのバッククォートで引用した確認で、中のマーカーをフェンスの外とみなさない）。
- 閉じないフェンスは本文の終わりまで続く（CommonMark と同じ）。閉じないフェンスの中の確認の語は数えない。
- Codex の文頭一致（`trusted_acknowledgement`）だけは、フェンスを外す前の本文の先頭で見る（v1 と同じ）。先頭のフェンスを外してから見ると、ログのあとの `Acknowledged` が文頭扱いになり、未修正の指摘が続く要約が項目から消える。

### 確認の語の効く範囲と打ち消し（2 回目の直し）

b808a29 の規則は、確認の語を「本文のどこかにあるか」で見て、対象への参照も本文の全体で探していた。[j5ik2o/ci#43](https://github.com/j5ik2o/ci/pull/43) のレビューで、次の 4 つの穴が見つかった（どれも ci#43 の 86a443c の `scripts/review-thread-resolution.sh` と同じ形で、設計の jq でも残っていた）。

| 穴 | 反例（b808a29 では承認になる） | 規則 |
| --- | --- | --- |
| 1. まとめ返信の語が別の項目にも効く | `- #issuecomment-10 は未対応です\n- #issuecomment-11 は対応しました` で 10 も承認。`#issuecomment-1 は確認しました。#issuecomment-2 は未対応です。` で 2 も承認 | 語と参照を同じ単位で結ぶ。打ち消しの単位が触れた項目は承認しない |
| 2. 行頭 `>` の引用の中の `fixed` | `> Once this is fixed, please rerun …` を引用して `Which tests should I run?` とだけ書くと承認（引用が参照も満たす） | 引用・項目の写しの中の語は数えない。引用は参照にだけ使う |
| 3. `not been fixed` などの英語の否定 | `This has not been fixed. #issuecomment-10` で承認（直前だけを見るので `been` を挟むと漏れる） | 同じ節の前に否定・条件・未来の語があれば、英語の語を数えない |
| 4. `確認済みではありません` などの日本語の否定 | `確認済みではありません #issuecomment-108` で承認（後ろを見ない） | 語の直後に付く否定と疑問を見る |

#### 単位に分ける

確認の本文を、行の種別で単位に分ける。フェンスの境界は「フェンスの境界」の規則のまま（`fence_split` と同じ開きと閉じ）。

- 行の種別：フェンスの中、引用（フェンスの外で、行頭の空白 3 つまでのあとが `>`）、空行、見出し（`#` が 1〜6 個と空白）、リストの項目の頭（`-`・`*`・`+`・`1.`・`1)` と空白）、それ以外の文。
- **散文の単位**は、段落（空行・見出し・引用・フェンス・リストの項目の頭で切れる、文の行の続き）、リストの項目（頭の行と、それに続く文の行。空行のあとでも、空白 2 つ以上で字下げした文の行は項目の続き。GitHub の表示と同じ）、見出しの行の 3 つ。見出しで節が切れるので、別の見出しの節の語は効かない。
- **かたまり**は、続く引用の行の並びと、1 つのフェンス。かたまりは次の順で、**同じ見出しの節の中の** 1 つの散文の単位に付く（見出しを越えて付かない。`確認しました。\n\n## B\n> #10` の引用は、どの単位にも付かない）。(1) 空行を挟まずにすぐ上にある段落かリストの項目（すぐ上がかたまりなら、その付く先）、(2) なければ下にある最初の段落かリストの項目（GitHub の「Quote reply」は引用を先に置く）、(3) なければ上にある最後の段落かリストの項目、(4) どれも無ければどこにも付かない。付いたかたまりの本文は、付く先の単位の参照（id と断片）にだけ使い、確認の語は数えない。
- CommonMark の怠惰な続き（`>` の行のすぐ下の、`>` の無い行も引用に含める規則）は採らない。`> 引用\n確認しました` の 2 行目は作成者の文として数える。画面では引用の中に見えるが、作成者が書いた文なので、赤にしない側に倒す。

#### 節に分けて極性を決める

散文の単位を、句点・感嘆符・疑問符（全角と半角）・読点（`、`・`，`・`,`）・セミコロン・改行と、空白か終わりが続くピリオドで節に分ける。読点の無い逆接（`ましたが`・`ですが`・`でしたが`・`ませんが`・`けれども`・`けど`・`ものの`、英語の `but`・`however`・`although`・`whereas` の前）でも切る（`#10 は確認しましたが #11 は未対応です`、`#10 is fixed but #11 is not fixed`）。「」『』“”（）() の中と、インラインのコードの中では切らない（`「You have reached your Codex usage limits for code reviews.」` のピリオドで切らない）。インラインのコードは CommonMark と同じく、同じ長さのバッククォートの列で開いて閉じる（``` ``fixed`` ``` も 2 つの列で囲んだコード）。閉じる列の無いバッククォートは、ただの文字として扱う。

節ごとに、確認の語を探す文字列（masked）を作る。「」『』“” の中身とインラインのコードを空白にし、さらに**項目の本文を写した部分**を空白にする。写しは、単位の本文（小文字）の 20 文字の窓のうち、項目のどれかの `quote_text` に含まれるものとする（単位の全体で印を付けてから節に分ける。読点で切ってから見ると、`Once this is fixed,` のような 20 文字未満の断片を写しと認められない）。`Action not completed / Review rate limited\n確認しました。` のように項目の文を括弧なしで写しても、写しの中の `not` を作成者の打ち消しと読まず、写しの中の `fixed` を作成者の確認と読まない。

節の極性は 3 つ。

- **打ち消し**：masked に、打ち消された確認の語があるか、「まだ」の表明がある。打ち消されない確認の語と同じ節にあっても打ち消しにする（`#10 is fixed and #11 is not fixed` は、どの参照がどちらかを決められないので、節の全体を打ち消しにして赤の側に倒す）。
- **肯定**：打ち消しでなく、masked に打ち消されない確認の語がある。「まだ」の表明は閉じた一覧で、`未対応`・`未確認`・`未修正`・`未対処`・`未解決`・`未反映`・`未着手`・`未完了`・`まだ`・`ていません`／`ていない`／`てません`／`でいない`・`できません`／`できない`・`ではありません`／`じゃない`、英語の `not`・`never`・`cannot`・`pending`・`todo`・`tbd`・`n't`・`no longer`。`問題ありません`・`ない` 単独・`ません` 単独は入れない（`影響がないことを確認しました` を打ち消しにしない）。直後が `こと`・`の`・`と`・`旨` の一致も数えない（`エラーが出ていないことを確認しました` の `ていない` は確認した内容で、まだの表明ではない）。
- **どちらでもない**：それ以外。

確認の語の打ち消しは、語ごとに次のどれかで決める。

1. **直前の否定**（いまの規則のまま）：語の前 12 文字が `未`・`まだ`・`未だに`・`not`・`not yet`・`un-` で終わる（`未確認しました`、`not fixed`）。3 と 4 も含め、語と否定の間の強調の記号（`*`・`_`・`~`）は飛ばして見る（`**確認済み**ではありません`）。
2. **英語の語（`acknowledged`・`addressed`・`fixed`）だけ**：同じ節の、語より前に `not`・`never`・`cannot`・`no longer`・`n't` があるか（`has not been fixed`、`hasn't been fixed`、`isn't fixed`、`was not actually addressed`）、条件・未来の `if`・`once`・`until`・`unless`・`whether`・`will`・`'ll`・`going to` がある（`once this is fixed`、`will be fixed in a follow-up`）。日本語の語には当てない（`Action not completed を確認しました` の `確認しました` を打ち消さない）。節は読点とピリオドで切れるので、`Not a bug, but fixed` や `No regressions; fixed` は打ち消さない。
3. **日本語の後ろに付く否定**：語の直後が `ではありません`・`ではない`・`でない`・`じゃない`・`ではなく`・`ではなかった`・`とは言えません` など（`確認済みではありません`、`対応済みでない`）。`確認しました` などの過去形には後ろから付く否定が無いので、実際に効くのは `済み` の語である。
4. **疑問**：節が `?`・`？` で終わるか、語の直後が `か`（`です`・`でした`・`でしょう` を挟んでもよい。`から` は除く）である（`確認しましたか？`、`確認済みですか`、`Is #10 fixed?`）。b808a29 の規則は語の直後 3 文字だけを見ていたので、`確認済みですか` を承認していた。

`should be fixed now` のような推量は打ち消しにしない（作成者が直したと伝える普通の言い方なので）。

単位の極性は、節の極性を集めて決める。肯定の節だけ（とどちらでもない節）なら**肯定**、打ち消しの節だけなら**打ち消し**、両方なら**混在**、どちらも無ければ**どちらでもない**。

#### どの項目を承認するか

確認 `a` は、次のどれかで項目を指したとき、その項目を承認する（条件 1〜3 も満たすとき）。参照（id か、その項目だけが持つ 20 文字以上の断片）は「採用する承認の条件」5 のまま。

- **肯定の単位**：単位の本文と、付いたかたまりの本文のどこかが項目を指す（`Fixed. See #issuecomment-10.` のように、語と参照が別の文でもよい）。
- **混在の単位**：肯定の節の本文が項目を指す（付いたかたまりは使わない）。`#10 は確認しましたが、#11 は未対応です。` は 10 だけを承認する。
- **前置きに続くリストの項目**：どちらでもないリストの項目は、項目の並び（間の空行とかたまりは許す）のすぐ上の段落（前置き）が肯定なら、項目の本文と付いたかたまりで指した項目を承認する（`次の 2 件を確認しました。\n- #10\n- #11`）。項目に打ち消しがあれば前置きは効かない（`- #11 はまだです`）。リストのあとの締めの文（`以上を確認しました。`）は効かない。

そのうえで、**打ち消しの節が触れた項目は承認しない**（拒否）。触れるとは、打ち消しの節の本文が項目の id を含むか、URL を外した本文の 20 文字の窓が項目の本文にあること（その項目だけが持つ断片でなくてよい）。打ち消しの節を含む単位（打ち消しか混在）に付いたかたまりが触れた項目も拒否する（`この件はまだです。\n> <項目の引用>` のあとに、その項目の URL を確認しても承認しない）。`#10 と #11 を確認しました。ただし #11 はまだ直していません。` は 10 だけを承認する。定型文に打ち消しで触れると、その定型文を持つ項目はどれもその確認では承認されない（どの項目のことか定まらないので、赤の側に倒す）。

#### 誤って赤にする側

この規則が正当な確認を赤にする形と、実データでの影響は次のとおり。

- 確認の語の無い単位で項目を指す（`対象: <URL>\n\n指摘はありません。`、`<URL>: I noted it.`）。実データでは modern-ddd-book#695 と event-store-adapter-java#871 の 2 件（「一意に指す値として何を使うか」の測り直し）。その単位に `確認しました` を足せば緑になる。
- リストのあとの締めの文だけに確認の語がある（`- #10\n- #11\n\n以上を確認しました。`）。30 件の標本には無かった。
- 1 つの節で、英語の否定のあとに別の英語の確認を続ける（`This doesn't affect X and is fixed`）。1 つの節で肯定と打ち消しを `and` でつなぐ（`#10 is fixed and #11 is not fixed` は 10 も承認しない）。30 件の標本には無かった。
- 1 つの確認で、承認した項目の文を打ち消しの節でも写す（矛盾する返信）。拒否が勝つ。

逆に、この規則でも見逃す形（偽の緑の側）は「未確認のこと」に書く。

### 判定の順序（自動報告を先に外す）

自動報告は、承認の条件を 1 つずつ当てる前の**候補選別**で外す。順序を固定しないと、確認の語と引用を含む失敗報告が自動報告の除外を通り抜け、条件 4 と 5 を満たして通ってしまう。

ここでは除外を **ack の判定より前**に置き、除外した投稿が項目にも承認にも数えられないことを順序で保証する。

1. 取得したトップレベルコメントとレビュー本文のすべてから、自動報告を除く（`$candidates`）。
2. 自動報告は、既存の `ignored_auto_report`（v1 の著者＋本文パターン。CodeRabbit の要約、`extra_ignored_auto_report_author_patterns` で足した Codex の利用上限など）に、**TAKT の失敗報告の種別**を足したものである。既存のパターン一覧は、いまの項目の選び方を変えないためにそのまま使う。新しく足すのは次の 2 つの種別だけで、どちらも本文の語句（`対応しました`、`確認しました` など）では判定しない。
   - **フェンスだけの本文**：著者が作成者（`pr_author`）で、本文からコードフェンス（``` または ~~~）を取り除くと空白だけになる投稿（散文が無い。#131 の実測形）。散文がある投稿は、フェンスの直前に一文だけ置く形（`対応しました\n```\n…引用…\n``` `）も含めて自動報告にしない。行数とフェンスの形だけで決め、文面は見ない。
   - **マーカー**：著者が作成者で、**フェンスの外に** `<!-- takt:auto-report -->` を持つ投稿。TAKT が付ける将来の印で、フェンスの外に自由な文を書ける。TAKT は作成者名義で投稿するので、著者条件で足りる。第三者が付けたマーカーは効かない（著者が作成者のときだけ種別と認める）。マーカー付きの報告をフェンスで引用した確認では、マーカーがフェンスの中に入るので種別と認めない。
3. 残った候補から、項目（`is_item`）と承認の候補（`is_acknowledger`）を選ぶ。項目選別の中では `trusted_acknowledgement`（文頭一致）だけを見る。
4. 承認は、残った候補のうち条件 1〜5 をすべて満たすものだけを数える。確認の語は承認の判定（`acked_keys`。単位ごとの語と参照）で評価し、項目選別では見ない。

代表例：作成者が `対応しました\n```\n…対象の引用…\n``` ` を投稿すると、散文があるので自動報告にしない。フェンス内の引用が対象を指し、フェンスの外の `対応しました` が確認の語なので、**承認として通る（赤にしない）**。自動報告として赤くなるのは「本文全体がフェンス」の形だけである。`<!-- takt:auto-report -->` 付きの散文報告（`<!-- takt:auto-report -->\n対応しました\n…引用…`）は、フェンスの外のマーカーがあるので自動報告として `$candidates` から落ちる。マーカー付きの報告をフェンスで引用した確認は、マーカーがフェンスの中なので自動報告にならず、散文の確認の語とフェンス内の引用で通る。

自動報告は項目にも承認にもならない（I3）。TAKT の失敗報告が将来散文を伴う形（フェンスの外に文を足す形）になるときは、`<!-- takt:auto-report -->` を付けることを TAKT 側の移行条件にする（「適用と移行」）。

Codex の利用上限の通知の扱いは、リポジトリごとに次の 3 行で固定する。

- `extra_ignored_auto_report_author_patterns` に入れているリポジトリ（takt-dash 等）では、そのコメントは `ignored_auto_report` として外れ、**項目にならない**（関門は緑）。
- 入れていないリポジトリ（modern-ddd-book 等）では、**未承認の項目として残る**（赤のまま。返信して承知扱いにする運用）。
- どちらの場合も**承認には数えない**。

### 判定の式（jq の形）

````jq
def ack_words:
  "確認しました|確認いたしました|確認済み|承知しました|了解しました|対応しました|対応いたしました|対応済み|対処しました|修正しました|直しました|\\backnowledged\\b|\\baddressed\\b|\\bfixed\\b";

# Codex の承知の語彙。v1 と同じ狭い一覧だけを使う（条件 4 の ack_words は使わない）。
# `Fixed`・`確認済み`・`修正しました`・`直しました`・`確認いたしました`・`対応いたしました` は
# 著者自身の確認（条件 4）でだけ承認に使う。
def trusted_ack_words:
  "acknowledged|addressed|確認しました|承知しました|了解しました|対応済み|対応しました|対処しました";

# コードフェンスの境界。確認の語・マーカー・フェンスだけの本文の判定が、この 1 つを共有する。
# CommonMark と同じく、行頭（空白 3 つまで）のバッククォートかチルダ 3 つ以上で開き、
# 同じ文字で、開きと同じ長さ以上の行で閉じる。閉じないフェンスは本文の終わりまで続く。
# 4 つのバッククォートで開いたフェンスは、中の 3 つのバッククォートの行では閉じない。
def fence_open($line):
  [ $line | match("^ {0,3}(`{3,}|~{3,})") ]
  | if length > 0 then .[0].captures[0].string else null end;

def fence_closes($line; $open):
  ($open[0:1]) as $c
  | $line | test("^ {0,3}" + (if $c == "`" then "`" else "~" end)
                 + "{" + ($open | length | tostring) + ",}\\s*$");

def fence_split:
  reduce ((.body // "") | split("\n"))[] as $line (
    { open: null, prose: [], fenced: 0 };
    if .open == null then
      fence_open($line) as $o
      | if $o == null then .prose += [$line] else .open = $o | .fenced += 1 end
    elif fence_closes($line; .open) then .open = null
    else . end);

# フェンスの外の散文だけ。
def strip_fences:
  fence_split | .prose | join("\n");

# 文頭一致。v1 の `^\s*(…)` と同じく、フェンスを外す前の本文の先頭で見る。
# 先頭のフェンス（ログなど）を外してから見ると、その後ろの `Acknowledged` が文頭扱いになり、
# 未修正の指摘が続く Codex の要約が項目から消えるため。語彙も v1 と同じ trusted_ack_words だけ。
def ack_at_start:
  (.body // "")
  | ascii_downcase
  | test("^\\s*(" + trusted_ack_words + ")");

# フェンスは残す（フェンス内の引用も照合の対象）。確認の語は norm_text を通さず、
# 単位と節（clauses）の masked で見る。
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

# URL は id の照合にだけ使う。断片の照合の前に両方の本文から外す（ci#44）。
# 外さないと `https://github.com/j` のような URL の共通の頭が 20 文字の窓として当たる。
def strip_urls($body):
  ($body // "")
  | gsub("(?i)https?://[^\\s)>\\]]+"; " ")
  | gsub("(?i)(?<pre>^|[^a-z0-9_./-])(?:[a-z0-9-]+\\.)+[a-z]{2,}/[^\\s)>\\]]*"; "\(.pre) ");

def quote_text($c):
  norm_text(strip_urls($c.body));

def windows($n):
  . as $s | [range(0; (($s | length) - $n + 1)) as $i | $s[$i:$i + $n]];

def anchor($u):
  ($u // "" | split("#") | if length > 1 then "#" + .[-1] else "" end);

# id の完全一致。直後に数字が続く一致は数えない（#issuecomment-10 は #issuecomment-101 に当たらない）。
def id_match($raw; $needle):
  $needle != ""
  and any(($raw | indices($needle))[];
          . as $i
          | (($raw[$i + ($needle | length):$i + ($needle | length) + 1] // "")
             | test("^[0-9]$") | not));

# ---- 確認の語の効く範囲・引用・否定（2026-11-28 の 2 回目の直し。ci#43 の残る穴 4 つ） ----

# 行の種別。フェンスの境界は fence_split と同じ規則（閉じないフェンスは終わりまで）。
# fence: フェンスの中（開き・閉じの行を含む。開きの行に start: true）
# quote: フェンスの外で、行頭（空白 3 つまで）が `>` の行
# blank / heading / item（リストの項目の頭）/ text
def line_kinds:
  reduce ((.body // "") | split("\n"))[] as $line (
    { open: null, out: [] };
    if .open == null then
      fence_open($line) as $o
      | if $o != null then .open = $o | .out += [{k: "fence", t: $line, start: true}]
        elif ($line | test("^\\s*$")) then .out += [{k: "blank", t: $line}]
        elif ($line | test("^ {0,3}>")) then .out += [{k: "quote", t: $line}]
        elif ($line | test("^ {0,3}#{1,6}(\\s|$)")) then .out += [{k: "heading", t: $line}]
        elif ($line | test("^\\s*([-*+]|[0-9]+[.)])\\s+")) then .out += [{k: "item", t: $line}]
        else .out += [{k: "text", t: $line}] end
    elif fence_closes($line; .open) then .open = null | .out += [{k: "fence", t: $line}]
    else .out += [{k: "fence", t: $line}] end)
  | .out;

# 単位の列。散文の単位（para・item・heading）と、引用・フェンスのかたまり（block）。
# gap: 直前に空行があるか。sec: 見出しで区切った節の番号（見出しの行から新しい節）。
def units:
  reduce line_kinds[] as $l (
    { us: [], gap: false, sec: 0 };
    (.us | length) as $n
    | (if $n > 0 then .us[$n - 1] else null end) as $last
    | (if $l.k == "heading" then .sec += 1 else . end)
    | if $l.k == "blank" then .gap = true
      elif $l.k == "text" and $last != null and (.gap | not)
           and ($last.type == "para" or $last.type == "item") then
        .us[$n - 1].text += "\n" + $l.t
      # 空行のあとでも、字下げ（空白 2 つ以上）した文の行はリストの項目の続き（GitHub の表示と同じ）。
      elif $l.k == "text" and $last != null and $last.type == "item" and ($l.t | test("^ {2,}\\S")) then
        .us[$n - 1].text += "\n" + $l.t | .gap = false
      elif $l.k == "quote" and $last != null and (.gap | not) and $last.type == "block" and $last.q then
        .us[$n - 1].text += "\n" + $l.t
      elif $l.k == "fence" and ($l.start | not) and $last != null and $last.type == "block" and ($last.q | not) then
        .us[$n - 1].text += "\n" + $l.t | .gap = false
      else
        .us += [{
          type: (if $l.k == "text" then "para" elif $l.k == "item" then "item"
                 elif $l.k == "heading" then "heading" else "block" end),
          q: ($l.k == "quote"),
          text: $l.t,
          gap: .gap,
          sec: .sec
        }]
        | .gap = false
      end)
  | .us;

# かたまりの付く先（散文の単位の位置）。探すのは同じ見出しの節の中だけ。
# 1. 空行を挟まずすぐ上にある散文の単位（すぐ上がかたまりなら、その付く先）
# 2. なければ、下にある最初の para か item
# 3. なければ、上にある最後の para か item
# 4. どれも無ければ null（どの単位にも付かない）
def attach_targets($us):
  reduce range(0; $us | length) as $j (
    [];
    . as $acc
    | if $us[$j].type != "block" then $acc + [null]
      elif $j > 0 and ($us[$j].gap | not)
           and ($us[$j - 1].type == "para" or $us[$j - 1].type == "item") then $acc + [$j - 1]
      elif $j > 0 and ($us[$j].gap | not) and $us[$j - 1].type == "block" and $acc[$j - 1] != null
           and $acc[$j - 1] < $j then $acc + [$acc[$j - 1]]
      else
        ([range($j + 1; $us | length)
          | select(($us[.].type == "para" or $us[.].type == "item") and $us[.].sec == $us[$j].sec)] | first) as $next
        | ([range(0; $j)
          | select(($us[.].type == "para" or $us[.].type == "item") and $us[.].sec == $us[$j].sec)] | last) as $prev
        | $acc + [($next // $prev)]
      end);

# リストの項目の前置き：項目の並び（間の空行とかたまりは許す）のすぐ上の para。
def lead_of($us; $j):
  [ range($j - 1; -1; -1) as $k | $us[$k] | {k: $k, type} ]
  | (map(select(.type != "item" and .type != "block")) | first) as $p
  | if $p != null and $p.type == "para" then $p.k else null end;

# 節に分ける。句点・感嘆符・疑問符・読点・セミコロン・改行と、空白か終わりが続くピリオドで切る。
# 「」『』“”（）() の中と、インラインのコードの中では切らない。
# raw: 照合に使う元の文字列。masked: 確認の語を探す文字列（「」『』“” の中身、インラインのコード、
# $cover が印を付けた項目の写しを空白にする）。
# インラインのコードは CommonMark と同じく、同じ長さのバッククォートの列で開いて閉じる
# （``fixed`` も 2 つの列で囲んだコード）。閉じる列が無い列は、ただの文字として扱う。
def backtick_runs($cs):
  ($cs | length) as $len
  | [ range(0; $len) as $i
      | select($cs[$i] == 96 and ($i == 0 or $cs[$i - 1] != 96))
      | { start: $i,
          len: ([ range($i; $len) as $k | select($cs[$k] != 96) | $k ] | first // $len) - $i } ];

def clauses($cover):
  explode as $cs
  | ($cs | length) as $len
  | backtick_runs($cs) as $runs
  | ($runs | map({key: (.start | tostring), value: .len}) | from_entries) as $runat
  | reduce range(0; $len) as $i (
      { out: [], raw: [], masked: [], depth: 0, qdepth: 0, code: null };
      ($cs[$i]) as $c
      | ([$c] | implode) as $ch
      | (if $i + 1 < $len then ([$cs[$i + 1]] | implode) else "" end) as $nx
      | ($runat[$i | tostring]) as $run
      | if $run != null and .code == null
           and any($runs[]; .start > $i and .len == $run) then
          .code = $run | .raw += [$c] | .masked += [32]
        elif $run != null and .code == $run then .code = null | .raw += [$c] | .masked += [32]
        elif .code != null then .raw += [$c] | .masked += [32]
        elif $ch == "`" then .raw += [$c] | .masked += [32]
        else
          (if ($ch | test("^[「『“]$")) then .qdepth += 1 | .depth += 1
           elif ($ch | test("^[」』”]$")) then .qdepth = ([0, .qdepth - 1] | max) | .depth = ([0, .depth - 1] | max)
           elif ($ch | test("^[（(]$")) then .depth += 1
           elif ($ch | test("^[）)]$")) then .depth = ([0, .depth - 1] | max)
           else . end)
          | .raw += [$c]
          | .masked += [(if ($cover[$i] // false) or (.qdepth > 0 and ($ch | test("^[「『“」』”]$") | not)) then 32 else $c end)]
          | if .depth == 0
               and (($ch | test("^[。！？!?；;、，,\n]$"))
                    or ($ch == "." and ($nx == "" or ($nx | test("^\\s$"))))) then
              .out += [{raw: (.raw | implode), masked: (.masked | implode)}] | .raw = [] | .masked = []
            else . end
        end)
  | .out + (if (.raw | length) > 0 then [{raw: (.raw | implode), masked: (.masked | implode)}] else [] end)
  | map(select(.raw | test("\\S")));

def en_ack_words: "\\backnowledged\\b|\\baddressed\\b|\\bfixed\\b";

# 確認の語の打ち消し。$s は節の masked を小文字にしたもの、$m は語の一致。
def negated2($s; $m):
  ($s[0:$m.offset]) as $before
  | ($s[($m.offset + ($m.string | length)):]) as $after
  # 直前の否定（いまの規則のまま）
  | ($s[([0, ($m.offset - 12)] | max):$m.offset]
     | test("(未|まだ|未だに?|\\bnot\\b|\\bnot\\s+yet\\b|\\bun-?)[\\s*_~]*$"))
  # 英語：同じ節の前に否定・条件・未来の語（has not been fixed / hasn't been fixed / once this is fixed）
  or (($m.string | test(en_ack_words))
      and ($before | test("\\b(not|never|cannot|no longer|unless|until|if|once|whether|will|going to)\\b|n['’]t\\b|['’]ll\\b")))
  # 日本語：後ろに付く否定（確認済みではありません / 対応済みでない / とは言えません）
  or ($after | test("^[\\s*_~]*((では|じゃ|で)は?\\s*(ありません|ない|なく|なかった)|とは(言え|いえ)(ません|ない))"))
  # 疑問（確認しましたか / 確認済みですか / Is it fixed?）
  or ($after | test("^[\\s*_~]*(です|でした|でしょう)?\\s*か(?!ら)"))
  or ($s | test("[?？]\\s*$"));

# まだの表明（確認の語が無くても、その節は「打ち消し」になる）。
def pending_cue:
  "未(対応|確認|修正|対処|解決|反映|着手|完了)|まだ|(て|で)い?(ません|ない|なかった)|でき(ません|ない)|(では|じゃ)(ありません|ない)|\\b(not|never|cannot|pending|todo|tbd)\\b|n['’]t\\b|\\bno longer\\b";

# 項目の本文を写した部分の印（$s は単位の本文を小文字にしたもの、$texts は項目の quote_text）。
# 20 文字の窓が項目のどれかの本文にあれば、その 20 文字を作成者の文ではないとみなす。
# 項目の文の `not`・`まだ`・`fixed` を、作成者の打ち消しや確認と読まないため。単位の全体で見てから節に分ける
# （節に分けてから見ると、読点で切れた 20 文字未満の断片を写しと認められない）。
def cover_flags($s; $texts):
  ($s | explode | length) as $len
  | (reduce range(0; ([0, $len - 19] | max)) as $i (
      [range(0; $len) | false];
      if any($texts[]; contains($s[$i:$i + 20])) then reduce range($i; $i + 20) as $k (.; .[$k] = true) else . end));

# まだの表明の一致。直後が `こと`・`の`・`と`・`旨` の一致は数えない（`エラーが出ていないことを確認しました` の
# `ていない` は、確認した内容であって、まだの表明ではない）。
def pending_hits($s):
  [ $s | match(pending_cue; "g")
    | select(($s[(.offset + (.string | length)):] | test("^\\s*(こと|の|と|旨)")) | not) ];

# 節の極性：pos（打ち消されない確認の語があり、打ち消しもまだの表明も無い）/
# neg（打ち消された語かまだの表明がある。打ち消されない語と同じ節にあっても neg）/ none。
# 同じ節に肯定と打ち消しが並ぶとき（`#10 is fixed and #11 is not fixed`）は、どの参照がどちらかを
# 決められないので、節の全体を打ち消しにして赤の側に倒す。
# 語・打ち消し・まだの表明は masked（引用・インラインのコード・項目の写しを空白にしたもの）で見る。
def polarity:
  (.masked | ascii_downcase) as $s
  | [ $s | match("(" + ack_words + ")"; "g") ] as $hits
  | ([ $hits[] | select(negated2($s; .) | not) ] | length) as $pos
  | (($hits | length) - $pos) as $negw
  | (pending_hits($s) | length) as $cue
  | if $negw > 0 or $cue > 0 then "neg"
    elif $pos > 0 then "pos"
    else "none" end;

# 節を切る前に、逆接の接続（`ましたが`・`ですが`・`けど`・`ものの`、英語の `but`・`however`・`although`・
# `whereas`）の位置に改行を入れる。読点の無い `#10 は確認しましたが #11 は未対応です` や
# `#10 is fixed but #11 is not fixed` を 2 つの節に分ける。参照の照合は元の本文で行う。
def split_contrast:
  gsub("(?<a>(ました|です|でした|ません)が|けれども|けれど|けど|ものの)(?![、，,])"; "\(.a)\n")
  | gsub("(?i)(?<a>[ \\t])(?<b>but|however|although|whereas)\\b"; "\(.a)\n\(.b)");

# 文字列 $t が指す項目の位置（$its の key）。id か、その項目だけが持つ 20 文字の断片。
def refs_in($t; $its):
  ($t | ascii_downcase) as $raw
  | (norm_text(strip_urls($t))) as $b
  | ($b | windows(20)) as $ws
  | [ range(0; $its | length) as $k
      | $its[$k] as $it
      | ([ $its | to_entries[] | select(.key != $k) | quote_text(.value) ]) as $others
      | quote_text($it) as $tb
      | select(id_match($raw; (anchor($it.url) | ascii_downcase))
               or any($ws[]; . as $w | ($tb | contains($w)) and all($others[]; contains($w) | not)))
      | $k ];

# 打ち消しの節が触れている項目（id か、URL を外した 20 文字の窓が項目の本文にある。一意でなくてよい）。
def mentions_in($t; $its):
  ($t | ascii_downcase) as $raw
  | (norm_text(strip_urls($t)) | windows(20)) as $ws
  | [ range(0; $its | length) as $k
      | $its[$k] as $it
      | quote_text($it) as $tb
      | select(id_match($raw; (anchor($it.url) | ascii_downcase)) or any($ws[]; . as $w | $tb | contains($w)))
      | $k ];

# 確認 $a が承認する項目の位置の集合。
def acked_keys($its):
  . as $a
  | [ $its[] | quote_text(.) ] as $texts
  | units as $us
  | attach_targets($us) as $tg
  | [ range(0; $us | length) as $j
      | select($us[$j].type != "block")
      | ($us[$j].text | split_contrast) as $st
      | ($st | clauses(cover_flags($st | ascii_downcase; $texts)) | map(. + {pol: polarity})) as $cl
      | { j: $j, type: $us[$j].type, cl: $cl,
          pol: (if any($cl[]; .pol == "pos") and any($cl[]; .pol == "neg") then "mixed"
                elif any($cl[]; .pol == "pos") then "pos"
                elif any($cl[]; .pol == "neg") then "neg" else "none" end),
          blocks: [ range(0; $us | length) as $b | select($tg[$b] == $j) | $us[$b].text ] } ] as $segs
  | ($segs | map({key: (.j | tostring), value: .pol}) | from_entries) as $polof
  # 拒否：打ち消しの節が触れた項目と、打ち消しの節を含む単位（neg・mixed）に付いたかたまりが触れた項目。
  | ([ ($segs[] | .cl[] | select(.pol == "neg") | mentions_in(.raw; $its)[]),
       ($segs[] | select(.pol == "neg" or .pol == "mixed") | .blocks[] | mentions_in(.; $its)[]) ] | unique) as $veto
  | [ $segs[]
      | if .pol == "pos" then refs_in(([$us[.j].text] + .blocks) | join("\n"); $its)[]
        elif .pol == "mixed" then (.cl[] | select(.pol == "pos") | refs_in(.raw; $its)[])
        elif .pol == "none" and .type == "item" then
          lead_of($us; .j) as $l
          | if $l != null and $polof[$l | tostring] == "pos"
            then refs_in(([$us[.j].text] + .blocks) | join("\n"); $its)[] else empty end
        else empty end ]
  | unique
  | map(select(. as $k | $veto | index($k) | not));

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
# マーカーはフェンスの外にあるときだけ効く（引用ではフェンスの中に入る）。
def has_takt_marker:
  strip_fences
  | test("<!--\\s*takt:auto-report\\s*-->");

# フェンスが 1 つ以上あり、フェンスの外が空白だけ（散文が無い）。
# フェンスの直前に一文だけ散文がある形は、散文があるので自動報告にしない。
def fenced_report_body:
  fence_split
  | .fenced > 0 and (.prose | join("\n") | test("^\\s*$"));

# マーカーは作成者の投稿に限って効く。第三者が付けたマーカーは無視する。
def takt_report:
  is_author and (has_takt_marker or fenced_report_body);

# 自動報告か。v1 の ignored_auto_report（著者＋本文パターン）に、
# TAKT の失敗報告の種別を足す。既存の項目の選び方は変えない。
def is_auto_report:
  ignored_auto_report or takt_report;

# Codex の承知。v1 と同じく文頭一致だけを見る。語彙も v1 と同じ trusted_ack_words だけ。
# 本文の途中の `fixed` も、文頭の `Fixed` も承知にしない（Codex の要約を項目から消さない）。
def trusted_acknowledgement:
  report_author_login == "chatgpt-codex-connector" and ack_at_start;

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
# 自動報告は is_item にも is_acknowledger にも入らない（acked_keys より前）。
# 確認ごとに、承認する項目の位置（$its の key）を acked_keys で求め、対象の最終編集より後のものだけを数える。
$items as $all
| ($all | map(select(is_auto_report))) as $reports
| ($all - $reports) as $candidates
| ($candidates | map(select(is_item))) as $its
| [ $candidates[] | select(is_acknowledger) | {at: .createdAt, keys: acked_keys($its)} ] as $acks
| [ range(0; $its | length) as $k
    | $its[$k] as $item
    | select([ $acks[]
               | select(.at > ($item.updatedAt // $item.createdAt))
               | select(.keys | index($k) != null) ] | length == 0)
    | $item
  ]
````

「ほかの項目」は位置（`key`）で分けるので、本文が同じ 2 つの項目（同じ利用上限の通知が 2 回など）は、互いの断片を「ほかの項目にある」とみなし、引用ではどちらも承認できない。それぞれの URL を貼る。

`is_item` と `is_acknowledger` は v1 の `select` を分けただけである（自動報告の除外は `is_item` の中ではなく、その手前の `$candidates` で済ませる）。この順序なので、自動報告に対して `acked_keys` は評価されない。また、**項目選別の中では `trusted_acknowledgement`（文頭一致）だけを見る**。確認の語（`acked_keys` の単位ごとの語）は承認の判定で評価する。`trusted_acknowledgement` は v1 と同じく `^\s*` の文頭一致で、**本文の途中に `fixed` があっても承知にしない**（v1 で赤だった Codex の要約を、項目から消して緑にしない）。`trusted_acknowledgement` は「Codex のコメントを項目から外す」ためと、Codex を承認の候補にするために使い、**ほかの項目の承認としては、上の 4 と 5 を満たすときだけ**数える。これで「Codex の 1 つの `Acknowledged.` が全項目を承認する」穴も塞がる。語彙は **v1 と同じ狭い一覧**（`acknowledged` / `addressed` / `確認しました` / `承知しました` / `了解しました` / `対応済み` / `対応しました` / `対処しました`）だけを使う。4 回目に広げた `Fixed`・`確認済み`・`修正しました`・`直しました`・`確認いたしました`・`対応いたしました` は、**著者自身の確認（条件 4）の `ack_words` にだけ**使い、Codex の承知には使わない。これで Codex の要約が `Fixed …` や `確認済みです。ただし…` で始まっても項目から消えず、v1 と同じ赤になる。文頭に来たときだけ承知なので、`確認しました` が文頭にあれば項目から外れ、本文の途中なら項目として残る。`is_auto_report` は v1 の `ignored_auto_report` に TAKT の種別判定を足しただけなので、Codex の明示の確認は自動報告に落ちない。

### 赤のときの案内

いまの `build_summary` は未承認の項目を列挙するだけで、解消の仕方を書いていない。次を足す。

```text
### 未承認のトップレベルコメント／レビュー要約
- by <author> updated at <time>: <url>
  確認するには、返信にこの URL を含めるか、本文を 20 文字以上引用し、
  「確認しました」などの語を添えてください（URL は長さを問いません）。
  引用はコードフェンスで囲んでもかまいません。
  ほかの項目にもある文（定型文）と、URL の一部は引用として数えません。
  同じ文の項目が複数あるときは、それぞれの URL を貼ってください。
  確認の語は、URL・引用と同じ段落かリストの項目に書いてください。
  引用（行頭の >、「」）の中の語と、「未対応です」「not been fixed」
  「確認済みではありません」のような打ち消しは確認として数えません。

自動投稿（TAKT の失敗報告など）は確認として数えません。
```

URL をそのまま貼れるので、人が迷わない。TAKT の失敗報告は種別で外れるので、失敗報告に確認の語を入れないという規約は要らない（後述）。

### 変えないこと

- 未解決スレッドの判定（`isResolved`）は変えない。スレッドの返信は承認ではなく、解決（resolve）だけが関門を外す。
- ステータスの context 名（`Check unresolved comments`）、公開方法、branch protection / ruleset は変えない。
- GitHub API の呼び出しは増やさない。コメントとレビューはすでに取得しており、判定は手元の jq だけで完結する。
- 自動投稿として外す対象（CodeRabbit の要約、TAKT の失敗報告など）は変えない。既存の `ignored_auto_report`（著者＋本文パターン）もそのまま使う。変えるのは当て方の一部で、TAKT の失敗報告は文面パターンではなく種別（フェンスの外の `<!-- takt:auto-report -->`、または著者が作成者で本文がフェンスだけ）で外す（「判定の順序」）。
- Codex の利用上限の通知は、v2 の既定の `ignored_auto_report` には入れない（`0a7037b` は main で revert 済み）。呼び出し側が `extra_ignored_auto_report_author_patterns` で足しているリポジトリ（takt-dash 等）では、その通知は**項目にならない**（関門は緑）。足していないリポジトリ（modern-ddd-book 等）では、**未承認の項目として残る**（赤のまま。返信して承知扱いにする運用）。どちらの場合も**承認には数えない**。v2 はこの呼び出し側の設定を変えない。

## 採らなかった案と理由

| 案 | なぜ採らないか |
| --- | --- |
| 参照だけ（URL か引用があれば承認） | **#131 の失敗報告がそのまま通る。** 失敗報告は「You have reached your Codex usage limits for code reviews.」を本文に引用しており、参照だけの判定では 02:46:31 の時点で緑になる（手元の jq で、02:49:20 の実際の確認を除いても緑になることを確認）。#134 の事象を直せない。 |
| 確認の語だけ（対象を指さなくてよい） | どの項目への確認か分からない。「確認しました」は別件にも書けるし、自動投稿にも入れられる。項目が複数あるとき、1 つの確認で全部が通る。 |
| URL だけ（引用を認めない） | #131 の実際の確認は URL を含まず、引用と「確認しました」だけだった。いまの緑を赤に戻す。運用も「必ず URL を貼る」に変わり、移行の負担が大きい（実測では、直近の確認 17 件のうちアンカーを含むのは 1 件）。代わりに、引用はその項目だけが持つ断片に限る（「一意に指す値として何を使うか」）。 |
| 20 文字の窓をそのまま使う（ci#43 の 442d679） | 窓が URL の共通の頭（`https://github.com/j`）や、複数の項目に共通の定型文に当たる。別件の URL を貼った「確認しました」で全項目が承認になる（[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44)）。1 つの項目を指した引用で、同じ定型文を持つ別の項目も承認になる（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。 |
| 窓を長くする（30 文字、40 文字） | URL の頭（scheme・ホスト・owner・repo）は 30 文字を超える（`https://github.com/j5ik2o/takt-dash/` で 36 文字）。定型文はもっと長い。長さでは「一意に指す」を保証できず、正当な短い引用（`Action not completed`）だけが落ちる。 |
| スレッド返信があること | 対象はトップレベルの issue コメントとレビュー本文で、これらにスレッドは無い（レビューのスレッドは別の `reviewThreads` で、未解決ならすでに赤）。スレッド返信は解決（resolve）を意味しないので、承認の代わりにもならない。 |
| 作成者のコメントを一切承認に数えない | Bot の要約や通知は解決（resolve）できないため、正当に確認しても永久に赤になる。除外リストを編集するしか解消手段が無くなる。 |
| 自動投稿の文面パターンに TAKT の失敗報告を足す | 文面が変わると壊れる許可リストで、「内容によらず数えない」という要求に答えていない。リポジトリごとに TAKT の文面が違い、j5ik2o/ci の既定に入れると他リポジトリに副作用が出る。種別（フェンスだけの本文・フェンスの外のマーカー）で外すのが本設計である。 |
| 確認の語を本文の全体で見る（b808a29） | まとめ返信の 1 つの語が、別の項目の参照にも効く（ci#43 のレビューの穴 1）。「#10 は確認しました。#11 は未対応です。」で 11 も承認する。 |
| 単位を文（句点まで）だけにする | `Fixed. See #10.` や `対象: <URL>\n指摘と対応: …直しました。` のように、語と参照を別の文に書く正当な確認が赤になる。段落とリストの項目を単位にし、混在のときだけ節に下りる。 |
| CommonMark の怠惰な続きに合わせる（`>` の直下の行も引用） | `> 引用\n確認しました` の作成者の文まで引用になり、正当な確認が赤になる。行頭が `>` の行だけを引用とする。 |
| `noted` などの語を足して #871 を緑のまま残す | `As noted in #10, …` のような地の文の `noted` で承認になる。語の一覧を広げる判断は、この設計の 4 つの穴の外にあるので、元帥に上げる（「未確認のこと」）。 |
| 打ち消しの拒否を、その項目だけが持つ断片に限る | 定型文に打ち消しで触れた確認（「同じ見出しの要約はまだです」）では、どの項目にも拒否が効かず、その定型文を持つ項目を別の単位の id でそのまま承認してしまう（まだと書いたのがその項目かもしれない）。拒否は、項目が持つ断片なら一意でなくても効かせる（赤の側に倒す）。 |
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

- `v2` は切ったら動かさない（force-push で付け替えない）。どのリポジトリがどの判定で走ったかを後から読めるようにする。
- 直すときは `v2.1` を切る（`v2` は残す）。影響するリポジトリだけ `uses` と `ci_ref` を `v2.1` に上げる（2 行は同じ PR で変える）。
- `v1` は削除しない。SHA 固定の逃げ道として `2ead62a` を README に書く。

### タグを切る条件

`v2` は、点検で見つかった次の 4 件が直るまで切らない。どれも、切ったあとに見つかると `v2.1` を切って全リポジトリを上げ直すことになる。

- [j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44)：断片の照合が URL の共通の頭に当たる。この設計の条件 5（URL を外す、ほかの項目にある断片を使わない）を実装し、スモークテストの反例（いまは `got 0 / success`）が赤になること。
- [#443](https://github.com/j5ik2o/takt-dash/issues/443)：Quint のモデルが承認の判定を計算していない。`specs/quint/review-thread-ack/` で直した（「Quint の要否」）。takt-dash の main に入ること。
- [j5ik2o/ci#45](https://github.com/j5ik2o/ci/issues/45)：スモーク 18a が #131 の実物の形（本文全体が ```text のフェンス）でない。テストの観点 18 の形の固定例が入ること。
- [j5ik2o/ci#43](https://github.com/j5ik2o/ci/pull/43) のレビューの残る穴 4 つ（まとめ返信の効く範囲、引用の中の語、英語の否定、日本語の否定）：「確認の語の効く範囲と打ち消し」を実装し、テストの観点 25〜29 の表の行が、b808a29 の列では誤った承認、この規則の列のとおりに固定されること。

**新旧の比較（「移行の順」2）で差が 0 件でも、ci#44 の穴は見つからない。** v1 は対象より後ろの作成者のコメントを内容によらず承認にするので、ci#43 の規則が URL の頭で承認する項目は、v1 でも必ず承認になる。新旧がどちらも緑の偽の緑は、差には出ない（旧の判定に、内容で承認を決める経路が無い）。偽の緑は、新しい判定だけを見て、各項目が**どの経路で承認されたか**を確かめないと見つからない。2026-11-27 の「差 0 件」もこの穴を見ていない。

### 移行の順

1. **j5ik2o/ci の main に実装を入れる。** スクリプト、スモークテスト、README の例。`ci_ref` の既定は **v1 のまま**にし、v2 は明示されたときだけ使う。`quint` はこのリポジトリの対象外なので、モデルは takt-dash の `specs/quint/review-thread-ack/` に置く。
2. **タグを切る前に、open PR の新旧比較を回す。** 22 リポジトリの open PR について、v1 の判定と提案の判定を並べ、差が 0 件であることを確認する（この文書の実測と同じ手順）。差があれば、その PR のコメントを読んで、引用の長さ・URL・確認の語・ほかの項目との共通の断片のどれで落ちたかを分類し、誤って赤にしない条件を直してからタグを切る。確認の語の無い単位で項目を指しているだけの差（2026-11-28 の event-store-adapter-java#871 の形）は、規則の誤りではなく意図した赤として一覧に残し、その PR の作成者に確認の書き方を伝える。**あわせて、新しい判定で承認になった項目ごとに、承認した確認と経路（id か、断片か。断片ならその 20 文字）を出し、その確認が本当にその項目を指しているかを読む。** 差の 0 件だけでは偽の緑が見えないため（「タグを切る条件」）。URL を含みアンカーの無い確認（pekko-persistence-dynamodb#1040、event-store-adapter-kotlin#906 の形）は、とくに読む。
3. **`v2` を切る。** 「タグを切る条件」の 4 件が直っていることを確かめ、元帥が 2 の差の一覧と承認の経路を見てから切る。 `git tag -a v2 -m "review-thread-resolution: require an explicit acknowledgement" <main のコミット>`、`git push origin v2`。`uses: ...@v2` が解決できることを、砂場の 1 本で確かめる。
4. **ci-gate-sandbox を v2 に上げる。** すでに機能ブランチを指している砂場で、無関係コメント・URL・引用・失敗報告の 4 例を実際の PR で流す。
5. **takt-dash を v2 に上げる。** #134 の出所であり、TAKT の自動 PR が最も多い。先に TAKT が失敗報告へ `<!-- takt:auto-report -->` を付ける変更を入れる（散文を足す形に備える）。印を付ける変更が nrslib/takt に当たる場合は、着手前に元帥へ上げる。付け忘れても、本文がコードフェンスだけなら種別で外れる。1 週間（日次の schedule を 1 回以上含む）観察する。赤になった PR があれば、その理由が「本当に未承認」か「誤って赤」かを分類する。
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
| 4. 全リポジトリが止まる緊急時 | 付け替えはしない。影響するリポジトリを 1 または 2 で v1（`2ead62a`）に戻し、修正版の `v2.1` を切ってから、必要なリポジトリだけ上げる | リポジトリごと | 影響数に比例 |

ステータスは次のイベント（コメント、レビュー、日次の schedule、手動 dispatch）で再計算される。戻したあとに即時で確かめたいときは、対象 PR で `workflow_dispatch` を `pr_number` 付きで回す。

## テストの観点

`tests/review-thread-resolution-smoke.sh` の偽 `gh` を広げ、コメントの配列（作成者・本文・`createdAt`・`updatedAt`）を環境変数で差し替えられるようにする。既存の検査はそのまま残す。足す観点は次の 29 である（25〜29 は 2 回目の直しで足した）。

1. 無関係な作成者コメント（「ありがとう」「別件です」）では通らない（#134 の核心）。
2. 本文全体がコードフェンスの失敗報告（対象の引用だけを本文全体のフェンスに入れ、フェンスの外に散文が無い）では通らない。この投稿は**項目にも承認にもならない**（報告自体は項目から消える。対象の未承認項目が別に残っていれば赤のまま、残っていなければ緑）。散文を伴う形（フェンスの外に「対応しました」を足したもの）は自動報告にせず、5 と 16(c) のとおり承認として通ることも固定する。
3. 対象の URL と確認の語で通る（対象本文が 20 文字未満でも通る。URL は正規化前の本文で照合するので `#issuecomment-N` の `#` が消えない）。
4. 対象のアンカー（`#issuecomment-N` / `#pullrequestreview-N`）と確認の語で通る（本文の長さを問わない）。
5. 対象本文の 20 文字以上の引用と確認の語で通る（`Action not completed` の実例）。引用をコードフェンスで囲んだ `確認しました\n```\nAction not completed\n``` ` でも通る（フェンス内の本文を照合する）。
6. 確認の語だけで、参照が無ければ通らない。
7. 参照だけで、確認の語が無ければ通らない。
8. 対象を指さない URL では通らない（[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44)）。(a) 別のコメントの URL・アンカーでは通らない（対象の id だけ）。`#issuecomment-10` の項目は `#issuecomment-101` を含む確認では通らない。(b) **URL の頭だけの一致では通らない。** 項目の本文が `https://github.com/j5ik2o/g2g/blob/abc123/src/retry.rs#L10` を含み、確認が `別件の https://github.com/j5ik2o/ci/pull/43 を確認しました` のとき、項目は未承認のまま（赤）。ci#43 の 442d679 ではこの例が `got 0 / success` になる（red として残す）。(c) scheme の無い `別件の github.com/j5ik2o/ci を確認しました` でも通らない。(d) 項目の URL の中のパスだけを写した確認（`src/retry.rs#L10 を確認しました`）は、URL を外した本文に一致しないので通らない（項目の URL を貼るか、URL の外の文を引用する）。
9. 対象より前に投稿された確認では通らない。
10. 対象の最終編集より前の確認では通らない。編集後に確認し直すと通る。
11. コードフェンス（3 つのバッククォート）の中の確認の語は数えない（`unacknowledged` のような部分一致もしない）。閉じないフェンス（`…引用…\n```\n確認しました` で終わる本文）の中の確認の語も数えない。4 つのバッククォートで開いたフェンスは、中の 3 つのバッククォートの行で閉じない。
12. `~~~` のフェンスの中の確認の語も数えない。
13. HTML コメントの除去は `<!--` から `-->` まで。コメントの中に `>` が含まれていても、そこで切れて引用の照合が崩れない。
14. 確認の語の直前に否定（未・まだ・not・not yet・un-）が付く場合と、直後が疑問（`確認しましたか？`、`確認済みですか`）の場合は通らない。`未確認しました` / `未対応済み` / `not fixed` を固定する。英語の助動詞を挟む否定と日本語の後ろに付く否定は 27・28 で固定する。
15. v1 が受けていた `対処しました` / `直しました` / `対応いたしました` / `確認いたしました` で通る（正当な緑を赤にしない）。
16. 自動報告の除外を、項目と承認の両方で確かめる。(a) 著者が作成者の `<!-- takt:auto-report -->` 付き散文報告（+ 確認の語 + 20 文字以上の引用）は、フェンスの外のマーカーがあるので項目にも承認にもならない。(b) 第三者が付けた同じマーカーは承認にならず（他人のマーカーで緑にできない。著者が作成者のときだけマーカーを種別と認める）、自動報告にもせず**未承認の項目として残る**。(c) 作成者名義の `対応しました\n```\n…対象の引用…\n``` ` は、散文があるので自動報告にせず、フェンス内の引用とフェンス外の確認の語で**承認として通る**（赤にしない）。(d) マーカー付きの報告をフェンスで引用した確認は、マーカーがフェンスの中なので自動報告にならず、承認として通る。引用が 4 つのバッククォートのフェンスで、中に 3 つのバッククォートのフェンスを含む形（`確認しました\n````\n<!-- takt:auto-report -->\n```\n…\n```\n````）でも通る。
17. 作成者以外の第三者の確認は数えない。作成者が Bot の PR では、OWNER / MEMBER / COLLABORATOR の人間の確認だけ数える。
18. #131 の実データをそのまま固定し、02:49:20 を含むと通り、02:48 までだと通らない（失敗報告だけでは通らない）。**この筋書きは、#131 の時点の設定（`extra_ignored_auto_report_author_patterns` が空）で流す。** いまの takt-dash の設定（#423）は Codex の利用上限の通知を外すので、そのまま流すと唯一の項目が消え、02:46 の時点から緑になって何も確かめない。失敗報告（02:46:31、[#issuecomment-6905873562](https://github.com/j5ik2o/takt-dash/pull/131#issuecomment-6905873562)）は**実物の形**で置く（[j5ik2o/ci#45](https://github.com/j5ik2o/ci/issues/45)）。実物は、本文全体が情報文字列 `text` 付きのフェンスで、フェンスの外に散文が 1 文字も無い。中に未承認の項目の引用（`『You have reached your Codex usage limits for code reviews.』`）、`acknowledge` の語（`未acknowledge`、`acknowledgeすること`）、URL（`https://github.com/j5ik2o/takt-dash/actions/runs/43793650983`）を含む。固定例は次の形（中身は実物から抜粋。改行とフェンスは実物のまま）。

    ````text
    ```text
    TAKTのCI回復を停止しました。修正回数: 0、再実行回数: 0。…唯一の未acknowledgeトップレベルコメントは chatgpt-codex-connector[bot] の『You have reached your Codex usage limits for code reviews.』(2026-11-16T01:28:14Z)で、既定のignore対象『chatgpt-codex-connector / Codex Review』に一致しないためブロックしている。…根本修正は当該通知をトップレベルでacknowledgeすること…。最後の失敗チェック・run・job・証拠不足: {"checks":[],"unavailable":[{"name":"Check unresolved comments","url":"https://github.com/j5ik2o/takt-dash/actions/runs/43793650983","reason":"Failed check has no supported Actions log reference"}]}
    ```
    ````

    この報告は (a) 自動報告（フェンスだけの本文）として項目にも承認にもならない、(b) 情報文字列 `text` があってもフェンスと認める、(c) 中の URL が項目の URL の頭と一致しても、自動報告なので承認にならない、の 3 点を確かめる。いまのスモーク 18a は報告をフェンスの無い散文で作っており、フェンスだけの本文の除外を実物の形で確かめていない。18a を実物の形に直すか、実物の形の例を 1 件足す。実際の確認（02:49:20）は散文で、`「You have reached your Codex usage limits for code reviews.」…を確認しました。` の引用と確認の語で通る（URL は含まない）。
19. いま緑の PR が赤に戻るかを、実データで確かめる。いま赤の `event-store-adapter-kotlin#926` と `event-store-adapter-rs#381` は赤のまま。**`event-store-adapter-java#871` は 2 回目の直しで赤になる**（Codex の利用上限の通知を承認していたのは、確認の語の無いリストの項目 `I noted it.` だけ。「いまの状態」の下見）。タグを切る前の open PR の新旧比較で、差の 1 件ごとに、確認の語の無い単位で指しているだけか（意図した赤）、規則の誤りか（直す）を分ける。
20. `ci_ref` を省略した呼び出しは v1 のスクリプトを使い、`ci_ref: v2` を明示したときだけ v2 を使う。
21. Codex の要約 `The first point is fixed, but the retry path is still broken.` は、文頭が確認の語でないので Codex の承知にならず、**未承認の項目として残る**（v1 と同じ赤）。文頭の `Acknowledged.` だけが Codex の承知として項目から外れる。本文がフェンス（ログなど）で始まり、その後ろに `Acknowledged.` がある要約（```` ```\nlog\n```\nAcknowledged. But the retry path is still broken. ````）は、文頭が確認の語でないので承知にならず、未承認の項目として残る。
22. Codex の要約が `Fixed the first point, but the retry path is still broken.` や `確認済みです。ただし、再試行の経路はまだ壊れています。` のように `Fixed`・`確認済み` で**始まっても**、`trusted_acknowledgement` の語彙（v1 と同じ狭い一覧）に無いので Codex の承知にならず、**未承認の項目として残る**（v1 と同じ赤）。`Fixed`・`確認済み`・`修正しました`・`直しました`・`確認いたしました`・`対応いたしました` は、著者自身の確認（条件 4）でだけ承認に使う。

23. 1 つの確認で、指していない項目を承認しない（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。同じ見出し（`Codex Review: Here are some automated review suggestions`）を持つ Codex の要約が 2 つあり、確認がその見出しだけを引用するとき、どちらの項目も未承認のまま。同じ利用上限の通知が 2 回あり、確認がその文を引用するときも、どちらも未承認のまま（それぞれの URL を貼れば通る）。項目が 1 つだけなら、同じ引用で通る（#131 の実際の確認）。
24. 1 つの確認が、項目ごとの id か、項目ごとに固有の断片を並べれば、複数の項目を承認できる（`次の 2 件を確認しました。\n- #issuecomment-10\n- #issuecomment-11`。前置きの段落の語がリストの項目に効く）。
25. **確認の語の効く範囲（穴 1）。** 1 つの確認が複数の項目に触れるとき、語は同じ単位の参照にだけ効く。(a) リストの項目ごとに書き分けた `- #10 は未対応です\n- #11 は対応しました` は 11 だけを承認する（Devin の例）。(b) 同じ段落の 2 文 `#10 は確認しました。#11 は未対応です。` と、読点でつないだ 1 文 `#10 は確認しましたが、#11 は未対応です。` は 10 だけを承認する。(c) 前置き＋リストは、項目に打ち消しが無ければ前置きの語が効き、`- #11 はまだです` の項目には効かない。(d) 肯定で触れた項目でも、同じ確認の打ち消しの節が触れれば承認しない（`ただし #11 はまだ直していません`）。(e) 見出しで分かれた節の語は、別の節の参照に効かない（modern-ddd-book#695 の形）。(g) 同じ節で肯定と打ち消しを並べる形：`but` や読点の無い `ましたが` で切れれば肯定の側だけを承認し、`and` でつなげば節の全体を承認しない。(h) 引用は見出しを越えて付かない。(i) まだの文に付いた引用が触れた項目は、別の単位でその URL を確認しても承認しない。(j) 空行のあとに字下げして続けた確認の文は、リストの項目に入る。(f) 確認の文に付いた引用・フェンス（`確認しました\n> …`、`> …\n\n確認しました。`、`確認しました\n```\n…\n``` `）は、付く先の単位の参照として通る。
26. **引用の中の語（穴 2）。** 行頭 `>` の引用、「」『』“” の中、インラインのコード（``` ``fixed`` ``` のような 2 つ以上の列で囲んだものを含む。閉じる列の無いバッククォートは文字）、項目の本文を写した部分の中の確認の語は数えない。(a) `> Once this is fixed, please rerun the integration tests.\n\nWhich tests should I run?` は通らない（Devin の例）。(b) 同じ引用の外に `確認しました。` を添えると通る。(c) 項目の文を括弧なしで写した確認（`Action not completed / Review rate limited\n確認しました。`）は、写しの中の `not` を打ち消しと読まずに通る。(d) `> 引用\n確認しました` の 2 行目（CommonMark では引用の続き）は作成者の文として数え、通る。
27. **英語の否定（穴 3）。** 同じ節の前の否定・条件・未来で、英語の語を数えない。`This has not been fixed.`（Devin の例）、`hasn't been fixed yet`、`isn't fixed`、`was not actually addressed`、`will be fixed in a follow-up`、`Once this is fixed` は通らない。`Fixed #10 in abc123.`、`Not a bug, but fixed`、`No regressions; fixed`、`should be fixed now` は通る。日本語の語には当てない（`Action not completed を確認しました` は通る）。誤って赤にする形 `doesn't affect X and is fixed` も固定する（赤）。
28. **日本語の否定（穴 4）。** 語の直後の否定と疑問を見る。`確認済みではありません #issuecomment-108`（CodeRabbit の例）、`対応済みでない`、`確認済みですか？`、`確認済みかどうか分かりません`、`**確認済み**ではありません` は通らない。`確認済みです`、`**確認済み**です`、`問題がないことを確認しました`、`エラーが出ていないことを確認しました`、`確認しましたから大丈夫です` は通る。
29. 27・28 は、同じ参照を付けた肯定と否定の対で固定する（CodeRabbit の推奨）。

25〜29 の入力と期待（この文書の jq を切り出して流した結果。7a〜7n は PR #452 のレビューで足した。項目は `#issuecomment-10`＝`Once this is fixed, please rerun the integration tests.`、`#issuecomment-11`＝`Action not completed / Review rate limited`、`#issuecomment-108`＝`You have reached your Codex usage limits for code reviews.` の 3 つ。b808a29 の列は 1 回目の直しの jq で流した結果で、穴の再現になる）。

| 例 | 確認の本文 | b808a29 で承認する項目 | この規則で承認する項目 |
| --- | --- | --- | --- |
| 1a | `- #issuecomment-10 は未対応です\n- #issuecomment-11 は対応しました` | 10・11 | 11 |
| 1b | `#issuecomment-10 は確認しました。#issuecomment-11 は未対応です。` | 10・11 | 10 |
| 1c | `#issuecomment-10 は確認しましたが、#issuecomment-11 は未対応です。` | 10・11 | 10 |
| 1d | `次の 2 件を確認しました。\n- #issuecomment-10\n- #issuecomment-11` | 10・11 | 10・11 |
| 1e | `次の 2 件を確認しました。\n- #issuecomment-10\n- #issuecomment-11 はまだです` | 10・11 | 10 |
| 1f | `#issuecomment-10 と #issuecomment-11 を確認しました。ただし #issuecomment-11 はまだ直していません。` | 10・11 | 10 |
| 1g | `確認しました\n> Action not completed / Review rate limited` | 11 | 11 |
| 1h | `> Action not completed / Review rate limited\n\n確認しました。` | 11 | 11 |
| 1i | ```` 確認しました\n```\nAction not completed\n``` ```` | 11 | 11 |
| 1j | `Fixed. See #issuecomment-10.` | 10 | 10 |
| 2a | `> Once this is fixed, please rerun the integration tests.\n\nWhich tests should I run?` | 10 | なし |
| 2b | `> Once this is fixed, please rerun the integration tests.\n\n確認しました。` | 10 | 10 |
| 2c | `「Once this is fixed, please rerun the integration tests.」とありますが、どのテストですか` | 10 | なし |
| 3a | `This has not been fixed. #issuecomment-10` | 10 | なし |
| 3b | `This hasn't been fixed yet: #issuecomment-10` | 10 | なし |
| 3c | `#issuecomment-10 will be fixed in a follow-up.` | 10 | なし |
| 3d | `Is #issuecomment-10 fixed?` | なし | なし |
| 3e | `Fixed #issuecomment-10 in abc123.` | 10 | 10 |
| 3f | `Action not completed を確認しました #issuecomment-11` | 11 | 11 |
| 4a | `確認済みではありません #issuecomment-108` | 108 | なし |
| 4b | `確認済みです #issuecomment-108` | 108 | 108 |
| 4c | `#issuecomment-108 は対応済みでない` | 108 | なし |
| 4d | `#issuecomment-108 は確認済みですか？` | 108 | なし |
| 4e | `#issuecomment-108 に問題がないことを確認しました。` | 108 | 108 |
| 4f | `#issuecomment-108 は確認しましたから大丈夫です。` | 108 | 108 |
| 5a | `Action not completed / Review rate limited\n確認しました。` | 11 | 11 |
| 5b | `Once this is fixed, please rerun the integration tests. を確認しました` | 10 | 10 |
| 5c | `Action not completed / Review rate limited はまだです。#issuecomment-10 は確認しました。` | 10・11 | 10 |
| 5d | `#issuecomment-10 と #issuecomment-11 を確認しました。Action not completed / Review rate limited の件はまだ直していません。` | 10・11 | 10 |
| 5e | `次の 2 件を確認しました。\n\n- #issuecomment-10\n- #issuecomment-11` | 10・11 | 10・11 |
| 5f | `- #issuecomment-10\n- #issuecomment-11\n\n以上を確認しました。` | 10・11 | なし |
| 5g | `## A\n対象: #issuecomment-10\n指摘と対応: 直しました。\n\n## B\n対象: #issuecomment-11\n指摘と対応: 指摘はありません。` | 10・11 | 10 |
| 5h | `- **Codex usage limit** (#issuecomment-108): I noted it. Codex did not review this round.\n- Five are fixed in 834cb5b.` | 108 | なし |
| 6a | `Not a bug, but fixed: #issuecomment-10` | 10 | 10 |
| 6b | `No regressions; fixed #issuecomment-10` | 10 | 10 |
| 6c | `#issuecomment-10 should be fixed now.` | 10 | 10 |
| 6d | `#issuecomment-10 was not actually addressed.` | 10 | なし |
| 6e | `#issuecomment-10 isn't fixed` | 10 | なし |
| 6f | `#issuecomment-10 doesn't affect X and is fixed` | 10 | なし |
| 6g | `> Action not completed / Review rate limited\n確認しました` | 11 | 11 |
| 6h | `#issuecomment-108 が確認済みかどうか分かりません` | 108 | なし |
| 7a | `#issuecomment-10 is fixed but #issuecomment-11 is not fixed.` | 10・11 | 10 |
| 7b | `#issuecomment-10 is fixed and #issuecomment-11 is not fixed.` | 10・11 | なし |
| 7c | `#issuecomment-10 は確認しましたが #issuecomment-11 は未対応です` | 10・11 | 10 |
| 7d | `#issuecomment-10 でエラーが出ていないことを確認しました。` | 10 | 10 |
| 7e | `確認しました。\n\n## B\n> #issuecomment-10` | 10 | なし |
| 7f | `## 未対応\n> Action not completed / Review rate limited\n\n## 対応済み\n#issuecomment-10 を確認しました。` | 10・11 | 10 |
| 7g | `この件はまだです。\n> Action not completed / Review rate limited\n\n#issuecomment-10 と #issuecomment-11 を確認しました。` | 10・11 | 10 |
| 7h | ``` ``fixed`` #issuecomment-10 ``` | 10 | なし |
| 7i | `` `fixed` #issuecomment-10 `` | 10 | なし |
| 7j | `fixed #issuecomment-10` | 10 | 10 |
| 7k | `` `foo を確認しました #issuecomment-10 `` | 10 | 10 |
| 7l | `**確認済み**ではありません #issuecomment-108` | 108 | なし |
| 7m | `**確認済み**です #issuecomment-108` | 108 | 108 |
| 7n | `- #issuecomment-10\n\n  確認しました。` | 10 | 10 |


補足すると、19 はスモークテストに実データを置くというより、タグを切る前の使い捨ての比較スクリプトで確かめるのが向いている。固定のフィクスチャとしては 1〜18、20〜29 を置き、19 は移行手順のチェックリストに残す。25〜29 は上の表の行をそのままフィクスチャにできる。19 の差 0 件は、8(b)・23 の偽の緑を見つけない（新旧の両方が緑になるため。「タグを切る条件」）。8(b) と 23 はフィクスチャで固定する。

## 影響と戻し方

### 影響するリポジトリ（2026-11-27 時点）

上の 22 リポジトリ。すべてで `Check unresolved comments` が必須。v2 に上げたリポジトリは、次の評価から新しい判定になる。

- 既存の open PR で、内容によらない作成者コメントだけに頼って緑だったものは、赤に戻る可能性がある。2026-11-27 の 15 件では 0 件だったが、2 回目の直し（確認の語の効く範囲を単位に限る）のあとの規則では `event-store-adapter-java#871` が赤になる（Codex の利用上限の通知を指すリストの項目に確認の語が無い）。マージ済みの modern-ddd-book#695 も、評価し直せば同じ理由で赤になる。どちらも、その単位に「確認しました」を足せば緑に戻る。
- 新しく赤になった PR は、作成者が URL（長さを問わない）か 20 文字以上の引用に「確認しました」を添えて返信すれば緑になる。関門の要約にその案内を出す。引用は、その項目だけが持つ文（URL の外）でなければ数えない。同じ文の項目が複数ある PR（同じ利用上限の通知が 2 回、同じ見出しの要約が 2 つ）では、引用の代わりにそれぞれの URL を貼る運用になる。
- 利用者ごとの運用（「一意に指す値として何を使うか」の実測）：
  - takt-dash、dotfiles：確認は URL を含まず、引用か記録の形。引用がその項目に固有なら、そのまま通る。TAKT の窓口が書く確認（`Codex（…）の「You have reached …」を確認しました`）も通る。同じ通知が 2 回ある PR では URL を貼る。
  - pekko-persistence-dynamodb、event-store-adapter-kotlin：確認に run や compare の URL、接頭辞の無い id（`comment6767269753`）を含むことがある。これらは承認の経路にならないので、項目のアンカーを貼るか、URL の外の文を引用する形に変える。ci#43 の規則のまま v2 を切っていれば、この形が URL の頭で黙って通っていた。
  - modern-ddd-book：Codex の利用上限の通知を外していないので、通知は項目として残る。通知が 1 回なら引用で通る。複数回なら URL を貼る。
  - 5 つとも、v1 のあいだは何も変わらない。v2 に上げる PR の本文に、確認の書き方（「一意に指す値として何を使うか」の例）を貼る。
- TAKT の失敗報告は、種別（フェンスの外の `<!-- takt:auto-report -->`、または著者が作成者で本文がフェンスだけ）で自動報告として外れるので承認に数えられない。TAKT が失敗報告を投稿しても、関門は赤のまま人を待つ。これは #134 の意図どおりである。失敗報告に確認の語を入れない規約は要らなくなった。代わりに、散文を伴う報告（フェンスの直前に一文だけ置く形を含む）にするときは `<!-- takt:auto-report -->` を付ける必要がある（別 PR、未確認）。
- Codex の利用上限の通知は、`extra_ignored_auto_report_author_patterns` に入れているリポジトリ（takt-dash 等）では項目にならず（緑）、入れていないリポジトリ（modern-ddd-book 等）では未承認の項目として残る（赤のまま）。どちらの場合も承認には数えない。
- SHA 固定の 2 つ（fraktor-rs、g2g）とブランチ固定の 1 つ（ci-gate-sandbox）は、v2 に上げるまで変わらない。同じ穴が残る。
- 追加の API 呼び出し、保存データ、公開 API、ブランチ保護の設定変更は無い。GitHub のレート制限への影響も無い。

### 戻し方

「適用と移行」の表のとおり。もっとも速いのは、そのリポジトリの 2 行を v1 に戻すこと。`v2` の付け替えはしない（`v2` は切ったら動かさない）。全体に効く不具合でも、各リポジトリを v1（または `2ead62a`）に戻し、修正は `v2.1` として切ってから、必要なリポジトリだけ上げる。

## 守る不変条件

- I1（緑の意味）：関門が緑になるのは、未解決スレッドが 0 件で、かつすべての未承認項目に有効な承認があるときだけ。
- I2（#134 の修正）：作成者が自分の文で書いた打ち消されない確認の語と、対象を一意に指す値（対象の id、または URL を外した本文で対象だけが持つ 20 文字以上の断片）が**同じ単位**にある確認だけが、その対象を承認する。同じ確認の打ち消しの節が対象に触れていれば承認しない。引用の中の語、打ち消された語（英語・日本語）、「まだ」の表明は確認の語にならない（2 回目の直し）。確認の語も対象への参照も無い作成者コメントは、決して承認にならない。**対象を指さない確認は、どの項目も承認しない**：別件の URL（URL の共通の頭 `https://github.com/j` だけが一致する。[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44)）や、ほかの項目にもある定型文の引用では承認にならず、1 つの確認が指していない項目を承認することもない（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。Quint の `noFalseGreen` が、この性質を作成者の意図と突き合わせて確かめる。
- I3（自動報告）：自動報告（既知の自動投稿、著者が作成者でフェンスの外に `<!-- takt:auto-report -->` を持つ投稿、著者が作成者で本文がフェンスだけの TAKT の失敗報告）は、項目にも承認の候補にもならない。確認の語や引用を含んでいても承認にならない。TAKT の失敗報告かどうかは著者と種別で決め、本文の語句では決めない。第三者が付けたマーカーは種別と認めない（他人のマーカーで項目を消したり緑にしたりできない）。第三者が付けたマーカー付きのコメントは、自動報告にせず**未承認の項目として残る**。
- I4（順序）：対象の最終編集より前のコメントは承認にならない。承認のあとで対象が編集されたら、承認し直しが必要。
- I5（純関数）：判定は取得済みのコメントとレビューの集合だけで決まる。実行順、並行性、リトライ、ページ取得の順に依存しない。同じ入力なら同じ結論。
- I6（正当な確認）：対象の URL / アンカーと確認の語（URL・アンカーは長さを問わない）、または対象だけが持つ 20 文字以上の引用と確認の語が同じ単位にそろい、同じ確認の打ち消しの節が対象に触れていなければ、緑になる。
- I7（API 不増）：判定のために新しい GitHub API 呼び出しをしない。
- I8（設定不変）：ステータスの context 名と公開方法を変えない。branch protection / ruleset を触らない。

## Quint の要否

**書く。** ただし、関門の実行そのもの（API 呼び出し、リトライ、並行実行）の状態機械ではなく、承認の判定が赤と緑の間を動く部分だけの最小モデルである。

理由は、#134 が「緑に到達できてしまう」という到達性の問題であり、コメントの投稿・編集の並びに対して、v1 の判定と提案の判定がどこで分かれるかを反例として出せるからである。とくに「対象が編集されると承認が無効になり、緑から赤へ戻る」遷移がある。状態が無い純関数ではあるが、入力の並びに対する関門の結論の遷移はある。

モデルは [`specs/quint/review-thread-ack/`](../../specs/quint/review-thread-ack/README.md) に置いた（`reviewThreadAck.qnt` と `reviewThreadAck_test.qnt`、`ci-specs.json` に登録済みで、CI の `quint` ジョブが流す）。

### 前の下書きの穴（#443）

前の下書き（この文書に直接書いていたもの）は、確認を投稿する時点で `{ item: ItemId, kind: AckKind }` と項目に結び付け、STRICT のとき `implementedAck` を仕様の `isStrictAck` そのものにしていた。そのため `noFalseGreen`（`gateGreen implies allStrictAcked`）は恒真で、strict の「違反なし」は何も確かめていなかった。また、確認がどの項目を承認するかをモデルが計算しないので、「1 つの確認で全項目が通る」形も、ci#44 の「URL の頭だけの一致」も表せなかった。点検は、これが ci#43 の URL の穴を見逃した理由の 1 つと見ている（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）。

### 直したモデル

- 確認は投稿の時点ではどの項目にも結び付けない。確認が持つのは、本文から観測できるもの（承認できる人か、自動報告か、1 つか 2 つの**単位**）と、作成者が承認するつもりだった項目（`intended`）・「まだ」と書いた項目（`pending`）である。単位は、確認の語の現れ方（`Word`）、含むコメントの id、含む断片を持つ（2 回目の直し）。
- `Word` は、文字列の水準の分類（jq の `clauses` と `polarity`）の結果を観測として与える。`Affirmed`（作成者が自分の文で書いた打ち消されない語）、`Quoted`（語が引用・項目の写しの中にだけある。穴 2）、`NegatedEn`（助動詞を挟む英語の否定。穴 3）、`NegatedJa`（後ろに付く日本語の否定。穴 4）、`Pending`（語が無く「まだ」と書く）、`NoWord` の 6 つ。どの文字列がどの `Word` になるかの正しさは、モデルでは確かめない（テストの観点 25〜29 の表で確かめる）。
- 項目の本文は 20 文字以上の断片の集合に畳む。`Own(i)` は項目 i だけが持つ散文、`Shared` は複数の項目に現れうる定型文、`UrlHead` は URL の頭（`https://github.com/j`）。どの項目が `Shared` と `UrlHead` を持つかは初期状態で非決定に選ぶ。
- 規則は定数 `RULE` で切り替える。`v1`（いまの v1）、`window`（ci#43 の 442d679・86a443c の 20 文字の窓）、`unique`（b808a29。語と参照を本文の全体で見て、`Quoted`・`NegatedEn`・`NegatedJa` も確認と数える）、`scoped`（この設計の 2 回目の直し）。`scoped` は、極性 `polarity` が `Pos` の単位の中で語と一意の参照がそろい、`Neg` の単位がその項目に触れていないときだけ承認する。規則は `intended`・`pending` を読まず、**どの確認がどの項目を承認するかを計算する**。
- 仕様 `noFalseGreen` は、関門が緑なら、すべての項目に「その項目を承認するつもりで、最後の編集より後に書かれた、自動報告でない、作成者が自分の文で語を書いた確認」があることを求める。仕様は `intended` と編集の記録 `editLog` を読み、規則が読む `updatedAt` とは別の経路で最後の編集を求める。前の下書きのように仕様と規則が同じ定義を共有すると、規則を壊しても仕様が一緒に動いて違反が出ない。同じ理由で、環境の前提（`consistent`・`pendingOk`）は規則の `polarity` を使わず、`Word` を直接読む。`idAckTurnsGreen`（I6）は、すべての項目に、id を確認の語と同じ単位に含み、打ち消しの単位がその項目に触れていない有効な確認があれば緑であることを求める。
- 確認の本文に何が現れうるかには前提を置く（`consistent`・`pendingOk`）。`Affirmed` の単位の id と固有の散文は、承認するつもりの項目か、同じ確認で「まだ」と書いた項目のもの。打ち消しの単位（`NegatedEn`・`NegatedJa`・`Pending`）の id と固有の散文は、「まだ」と書いた項目のもの。「まだ」と書いた項目は承認するつもりの項目ではない。定型文は、それを持つ項目がある PR にしか現れない。`Quoted` と `NoWord` の単位の id と断片、どの単位の URL の頭も、意図と無関係に現れる（指摘を引用して質問する、別件として触れる）。この前提を外すと `fixed` でも反例が出るので、結論はこの前提に依存する。前提が崩れる入力（別の PR の項目の文を偶然 20 文字以上写すなど）は、モデルの範囲外である。

### 結果（quint 0.32.0）

- `quint typecheck`：2 ファイルとも成功。
- `quint test reviewThreadAck_test.qnt --main reviewThreadAck_test`：19 passing。穴 4 つを、`unique`（b808a29）で緑かつ `noFalseGreen` が偽として固定した（`uniqueScopeLeaksTest`、`uniqueQuotedWordTest`、`uniqueNegatedEnTest`、`uniqueNegatedJaTest`）。同じ入力が `scoped` では赤のままで、まとめ返信のあとに 2 を確認し直せば緑になること（`scopedMixedReplyTest`）、打ち消しの単位で拒否が効くこと（`scopedVetoTest`）、引用が確認の文に付く形は通ること（`scopedQuoteAttachedToAckTest`）も固定した。ci#44 と #443 の反例（`windowUrlHeadTurnsGreenTest`、`windowSharedQuoteApprovesOtherTest`）と v1 の穴（`v1UnrelatedTurnsGreenTest`）は前のまま。
- `quint run --main legacy|window|unique --invariant noFalseGreen --max-steps 10 --max-samples 50000 --seed 134`：3 つとも違反あり。
- `quint run --main fixed --invariant safety --max-steps 10 --max-samples 50000 --seed 134 --witnesses …`：違反なし（50000 の標本を調べた範囲で。証明ではない）。手元で約 4 分（前のモデルより重い。単位が 2 つになり、1 回の投稿の非決定の選び方が増えたため）。CI の `quint` ジョブも同じ数で流す。

### Quint を採る前の 4 つの確かめ

元帥の追加の決まり（2026-11-28、オーナーの注意「Quint ファイルが間違っているとすべてが狂う」）に沿って、quint-modeling の見直しの手順で確かめた。2 回目の直しでもやり直した。詳しい表は [`specs/quint/review-thread-ack/README.md`](../../specs/quint/review-thread-ack/README.md) にある。

- **(a) 不変条件が恒真でない。** `specs/quint/review-thread-ack/mutants.sh` が、原本（違反なし）のほかに 14 の壊した版を作り、すべてで決めた不変条件に違反が出ることを確かめる（手元で全体 3 分 45 秒）。**ci#43 の残る穴 4 つを 1 つずつ開けた版**が 4 つ（語と参照を本文の全体で見る、`Quoted` を肯定と読む、`NegatedEn` を肯定と読む、`NegatedJa` を肯定と読む）、穴 1 の直しの部品を外した版が 2 つ（打ち消しの単位による拒否を消す、`Pending` を打ち消しと読まない）、前からあるガードを 1 つ外した版が 6 つ（URL の頭、ほかの項目に無い断片、1 つの確認の断片で別の項目も承認、id の経路、最終編集より後、自動報告）、遷移を 1 つ変えた版が 2 つ（編集で `updatedAt` を進めない、現れ方の前提を外す）。id の経路を消した版は `idAckTurnsGreen` に、ほかの 13 は `noFalseGreen` に違反が出る。あわせて、穴 4 つを開けたままの b808a29 の規則（`unique`）も `noFalseGreen` に違反が出る。
- **(b) 遷移に届く。** `step` の 2 つの action は witness の `postedAck`（99.92%）と `editedItem`（99.91%）で届く。2 回目の直しで足した形にも届く：まとめ返信（打ち消しの単位を含む確認）が別の項目を承認する（`approvedByMixedAck`、2.76%）、肯定の単位が指した項目を打ち消しの単位が拒否する（`vetoFired`、8.52%）、引用の中だけの語の単位を含む確認がある PR での緑（`greenWithQuotedWordAck`、6.72%）、打ち消された語の単位を含む確認がある PR での緑（`greenWithNegatedAck`、8.34%）、2 つの単位を持つ確認による承認（`approvedByTwoUnitAck`、10.55%）。前からの形にも届く：確認が編集で無効になる遷移（`editInvalidatedAck`、18.86%）、引用だけでの承認（7.84%）、定型文がある PR での緑（9.05%）、URL の頭を含む確認がある PR での緑（10.30%）、緑そのもの（11.52%）。規則がいつも赤を返して違反を避けているのではない。
- **(c) 実物と合う。** 状態と action を、j5ik2o/ci#43 の 86a443c の `scripts/review-thread-resolution.sh` の関数と値に行番号つきで対応させた（判定の部分は 442d679 から変わっておらず、行番号も同じ）。`unique`・`window` の `bodyAckWord` は実物の `ack_phrase`（534 行目。フェンスだけを外し、`negated` は直前と直後 3 文字だけを見る）に、`bodyIds`・`bodyFrags` は `references`（568 行目）が本文の全体を見ることに当たる。`scoped` の `Unit`・`polarity`・`refsUnit`・`vetoed` は実物に無く、この設計の jq の `units`・`clauses`・`polarity`・`refs_in`・`mentions_in`・`acked_keys` に当たる。対応の無いもの（`intended`、`pending`、`editLog`、`clock`、`consistent`・`pendingOk`、`lastStep`）は、仕様・環境の前提・witness のためだけに持つ。
- **(d) 範囲。** 項目 2 つ、断片 4 種、確認の単位は 2 つまで、語の現れ方 6 種、ステップ 10、標本 50000（seed 134）。網羅ではない。見られないもの：項目が 3 つ以上のとき、単位が 3 つ以上の確認、前置きに続くリストの項目（モデルでは前置きの語が効いた項目を `Affirmed` の単位として与える）、定型文が 2 種類以上・引用が固有の文と定型文にまたがるとき、文字列の水準（単位と節への分け方、かたまりの付く先、確認の語と打ち消しの一覧、項目の写しの印、正規化、URL の外し方、フェンスの境界、20 文字の下限）、時刻が同じ秒に並ぶとき（実物は厳密な比較なので、編集と同じ秒の確認は数えない）、前提が崩れる入力、未解決スレッドと項目の選び方。

Quint が通ったことだけを合格の根拠にはしない。モデルが見ない文字列の水準は、テストの観点 3〜5・8・11〜13・18・23・25〜29 のスモークテストで確かめる。モデルの筋書きとテストの観点の対応は次のとおり。

| テストの観点 | Quint の筋書き |
| --- | --- |
| 1（無関係な一言） | `v1UnrelatedTurnsGreenTest`（v1 で緑になる側） |
| 3・4・24（id で通る、複数の項目を 1 つで） | `scopedIdsTurnGreenTest`、`scopedOwnQuotesTurnGreenTest` |
| 5・18 の 02:49:20（項目だけが持つ文の引用） | `scopedSharedQuoteOnSingleItemTurnsGreenTest`、`scopedOwnQuotesTurnGreenTest` |
| 7（参照だけで確認の語が無い） | `scopedQuoteWithoutAckWordStaysRedTest` |
| 8(b)（URL の頭） | `windowUrlHeadTurnsGreenTest`、`scopedUrlHeadStaysRedTest` |
| 10（編集で無効、確認し直しで戻る） | `scopedEditInvalidatesAckTest` |
| 2・16(a)・18 の 02:46:31（自動報告） | `scopedAutoReportStaysRedTest` |
| 23（定型文で別の項目） | `windowSharedQuoteApprovesOtherTest`、`scopedSharedQuoteStaysRedTest` |
| 25（確認の語の効く範囲。穴 1） | `uniqueScopeLeaksTest`（b808a29 で緑）、`scopedMixedReplyTest`、`scopedVetoTest`、`scopedQuoteAttachedToAckTest` |
| 26（引用の中の語。穴 2） | `uniqueQuotedWordTest`（b808a29 で緑）、`scopedQuotedAndNegatedStayRedTest` |
| 27（英語の否定。穴 3） | `uniqueNegatedEnTest`（b808a29 で緑）、`scopedQuotedAndNegatedStayRedTest` |
| 28（日本語の否定。穴 4） | `uniqueNegatedJaTest`（b808a29 で緑）、`scopedQuotedAndNegatedStayRedTest` |

モデルが表さないもの：GitHub のイベント配信、ステータスの公開遅れ（新しいコメントが来てから再評価されるまでの数秒）、API の失敗、ページ取得の順。これらはこの変更で変えない（v1 と同じ）ので、モデルの範囲外にした。文字列の正規化（URL の `#`、URL の外し方、HTML コメントの `-->`、`~~~` のフェンス、情報文字列付きのフェンス）、単位と節への分け方、確認の語の一覧と打ち消し、項目の写しの印、20 文字の下限も、モデルではなく jq の実装とスモークテストで確かめる。承認できる人か、自動報告か、単位の語の現れ方は、観測できる値としてモデルに与えており、その判定の正しさはモデルでは確かめない。

## 改訂の記録（レビュー 2 回目で直した 7 点）

| 指摘 | 直した場所 | どう直したか |
| --- | --- | --- |
| 高1. URL・アンカーが正規化で一致しない | 「採用する承認の条件」5、正規化の段落、判定の式の `raw_body` / `references` | URL とアンカーは正規化前の本文（大小文字だけ揃える）で照合する。`#` を空白に落とす `norm_text` を通さない。 |
| 高2. 失敗報告が確認の語＋引用で承認になる | 条件 3、「判定の順序」、jq の `$candidates`、I3 | 自動報告を先に候補から外し、残った候補だけで項目と承認を数える。TAKT の失敗報告は文面ではなく、フェンスの外の `<!-- takt:auto-report -->` か「著者が作成者で本文がフェンスだけ」という種別で決める。Codex の利用上限はリポジトリごとの `extra_ignored_auto_report_author_patterns` で扱い、どちらの場合も承認には数えない（3 回目と 4 回目で修正）。 |
| 中1. 否定文が確認語に当たる | 条件 4、`ack_phrase` / `negated` | 確認の語の直前に `未` / `まだ` / `not` / `un-` などが付く場合と、直後が疑問（`確認しましたか？`）の場合は数えない。`対処しました` / `直しました` / `対応いたしました` / `確認いたしました` を一覧に足す。 |
| 中2. 本文が 20 文字未満だと承認できない | 条件 5、正規化の段落、赤のときの案内、I6 | URL・アンカーは長さを問わず承認に使えると明記。20 文字の下限は本文の引用だけに掛ける。 |
| 中3. `ci_ref` の既定を v2 にすると黙って v1 判定になる | 移行の順 1・5・7・8、呼び出し側の変更 | `ci_ref` の既定は v1 のまま。v2 は明示したときだけ。`uses` と `ci_ref` を同じ PR で上げることを必須手順にした。 |
| 低1. `>` を含む HTML コメントで除去が崩れる | 正規化の段落、`norm_text` | 除去の規則を `<!--` から `-->` までに直した（途中の `>` で切らない）。 |
| 低2. `~~~` のフェンスを除去していない | 条件 4、`ack_phrase` | 3 連バッククォートに加えて `~~~` のフェンスも空白にする。 |

テストの観点には、3・4（URL / アンカーの長さと `#`）、12・13（`~~~` と HTML コメント）、14・15（否定と v1 の語）、16（自動報告の順序）、20（`ci_ref` の既定）を足した。Quint には `AutoReport` と `NegatedMarker` の種別を足し、どちらも緑にならないことを 2 本のテストで確かめた。

## 改訂の記録（レビュー 3 回目で直した 4 点）

- **高2 のやり直し（自動報告の除外を効く形にする）**：「判定の順序」を書き直し、自動報告を `$candidates` を作る時点で外すようにした。前の案の whole-fence の枝が no-op だった理由（`ack_phrase` が先にフェンスを空白にする）も明記した。判定は文面の語句ではなく、著者が作成者であることと、フェンスの形（本文全体がフェンス、または一文＋フェンス）または `<!-- takt:auto-report -->` で行う。代表例 `対応しました\n```\n…対象の引用…\n``` ` が項目にも承認にもならないことを書いた。jq は `is_auto_report` → `$candidates` → `is_item` / `is_acknowledger` → 条件 4・5 の順に並べ、自動報告に対して `ack_phrase` を評価しないことをコメントした（4 回目で、散文を伴う形は自動報告にしない形に改めた）。
- **Codex の上限コメントの記述の矛盾**：「判定の順序」「変えないこと」「影響と戻し方」を、リポジトリごとの 3 行（`extra_ignored_auto_report_author_patterns` に入れているリポジトリ＝項目にならず緑／入れていないリポジトリ＝未承認の項目として赤のまま／どちらの場合も承認には数えない）に揃えた。旧 93 行目と旧 201 行目の「項目に残る」「外す対象」の矛盾を解消し、v2 の既定には入れず呼び出し側の設定をそのまま使うことを明記した。
- **`trusted_acknowledgement` の語彙**：条件 4 の `ack_words` と同じ一覧を使う形に直した。`確認いたしました` / `対応いたしました` / `Fixed` なども Codex の承知として扱うので、v1 の一覧のままで残っていた「確認いたしました」が未承認の項目として残らない。ほかの項目の承認としては、条件 4 と 5 を満たすときだけ数える点は変えない（4 回目で、文頭一致だけを承知とする形に戻した）。この語彙の拡大は 5 回目で取り消し、v1 の狭い一覧に戻した。
- **テストの観点 2 と 16**：2 はフェンスの失敗報告に確認の語をフェンスの外へ足した形（`ack_phrase` が先に落とす形ではない）で、項目にも承認にもならないことを固定する。16 はマーカー付きの散文報告、第三者が付けたマーカー、作成者名義の `対応しました` ＋フェンス引用の 3 つを固定する。第三者のマーカーは著者条件で無効にし、他人のマーカーで緑にできないことを書いた。
- **Quint**：`AutoReport` のコメントを新しい種別（本文全体がフェンス、一文＋フェンス、マーカー）に合わせ、ack の判定より前に候補から外れることを記述した。第三者のマーカーはモデルでは `Unrelated` と同じ扱いとした。モデルの構造とテストの筋書きは変えていない（4 回目で、`AutoReport` は本文がフェンスだけの形に修正した）。
- **適用・移行・戻し方**：v2 タグ、移行順、戻し方の骨格は変えていない。TAKT の失敗報告の説明だけ、散文を伴う報告にはマーカーが必要になる形に合わせた（4 回目で、自動報告は本文がフェンスだけの形に限ると正確にした）。

## 改訂の記録（レビュー 4 回目で直した 4 点）

- **高（緩む穴）：`trusted_acknowledgement` の文頭アンカー**：v1 の `^\s*(…)` の文頭一致が外れ、本文のどこに語があっても Codex の承知として扱われていた。とくに Codex の要約 `The first point is fixed, but the retry path is still broken.` が項目から消え、緑になっていた（実測）。`ack_at_start` を足し、`trusted_acknowledgement` を v1 と同じ文頭一致に戻した。語彙は `ack_words` のままで、文頭に来たときだけ承知にする（語彙も 5 回目で v1 の狭い一覧に戻した）。
- **中（正当な確認が赤になる）：フェンスで囲んだ引用**：自動報告の判定を「本文からフェンスを除くと空白だけ（散文が無い）」に限り、散文がある投稿（一文＋フェンス）は自動報告にしない。引用の照合はフェンス内の本文も対象にした（`norm_text` はフェンスを残す）。`<!-- takt:auto-report -->` はフェンスの外にあるときだけ種別と認め、マーカー付きの報告をフェンスで引用した確認は通る。前回「代表例」として赤にした `対応しました\n```\n…引用…\n``` ` は、散文があるので承認として通る形に改めた。
- **低（記述の正確さ）**：`ack_phrase` の評価位置の記述を「項目選別の中では `trusted_acknowledgement`（文頭一致）だけを見る。`ack_phrase` は承認の判定で初めて評価する」に直した。
- **低（第三者のマーカー）**：第三者が付けたマーカー付きのコメントは自動報告にならず、未承認の項目として残ることを I3 とテスト 16 に足した。

テストの観点は、2（本文全体がフェンスの報告）、5（フェンス内の引用で承認）、16（散文＋フェンスは自動報告にしない・マーカー引用）、21（文頭でない `fixed` は Codex の承知にしない）で固定する。Quint は `AutoReport` の説明だけを新しい種別（フェンスだけの本文・フェンスの外のマーカー）に合わせ、モデルの構造は変えていない。

## 改訂の記録（レビュー 5 回目で直した 1 点）

- **高（緩む穴）：`trusted_acknowledgement` の語彙**：4 回目に条件 4 の `ack_words` まで広げたため、Codex の要約が `Fixed …` や `確認済みです。ただし…` で始まると、要約自体が項目から消えて緑になっていた（v1 は赤）。`trusted_ack_words` を新設し、`trusted_acknowledgement` の語彙を v1 と同じ狭い一覧（`acknowledged` / `addressed` / `確認しました` / `承知しました` / `了解しました` / `対応済み` / `対応しました` / `対処しました`）に戻した。広げた語は著者自身の確認（条件 4）の `ack_words` にだけ残す。テストの観点 22 で、`Fixed …` と `確認済みです。ただし…` が赤のまま残ることを固定する。

## 改訂の記録（2026-11-28 の点検で直した 3 点と、Devin の指摘 5 点）

- **高（偽の緑）：断片の照合が URL の共通の頭に当たる**（[j5ik2o/ci#44](https://github.com/j5ik2o/ci/issues/44)）：条件 5 を「対象を一意に指す値の完全一致」に改めた。id（URL の全体かアンカー。直後に数字が続く一致は数えない）か、URL を両方の本文から外したうえで、対象だけが持ち、ほかの項目のどれにも無い 20 文字以上の断片。jq の `strip_urls`・`quote_text`・`id_match`・`references($target; $ack; $others)` を直した。一意に指す値の候補の比較、確認の文の形の例、利用者の確認の書き方の実測を足した。I2、テストの観点 8・23・24、赤のときの案内、採らなかった案、影響を直した。
- **高（検証の穴）：Quint の `noFalseGreen` が恒真**（[#443](https://github.com/j5ik2o/takt-dash/issues/443)）：モデルを `specs/quint/review-thread-ack/` に移し、確認を投稿の時点で項目に結び付けない形に書き直した。承認は規則が観測できるものから計算し、仕様は作成者の意図（`intended`）と突き合わせる。`window`（ci#43）で ci#44 と定型文の反例が出て、`fixed` では出ないことを確かめた。
- **中（テストの形）：スモーク 18a が #131 の実物の形でない**（[j5ik2o/ci#45](https://github.com/j5ik2o/ci/issues/45)）：テストの観点 18 に、実物の失敗報告の形（本文全体が情報文字列 `text` 付きのフェンス、中に引用・`acknowledge`・URL）を書いた。
- **Devin の 2026-11-27 の指摘 5 点**：(1) アンカーの前方一致（`#issuecomment-1238` が `#issuecomment-123` に当たる）は `id_match`（直後に数字が続く一致を数えない）とテストの観点 8(a) で塞いだ。(2) フェンスの境界を「フェンスの境界」の 1 つの規則（`fence_split`。閉じないフェンスは終わりまで、4 つのバッククォートは 3 つで閉じない）にまとめ、確認の語・マーカー・フェンスだけの本文で共有した（観点 11・16(d)）。(3) Codex の文頭一致は、フェンスを外す前の本文の先頭で見る形に戻した（観点 21）。(4) #131 の筋書きは #131 の時点の設定（利用上限の通知を外さない）で流すと観点 18 に書いた。(5) v1 のモデルは、確認をどの項目にも結び付けず、1 つの一言で全項目を承認する形にした（`v1UnrelatedTurnsGreenTest`）。
- **Quint を採る前の 4 つの確かめ**（元帥の追加の決まり）：仕様が規則と定義を共有しないよう、最後の編集の時刻を編集の記録 `editLog` から求める形にし、断片と遷移を文字列でなく型で表した。わざと壊した 8 版で違反が出ること（`mutants.sh`）、各 action と主な遷移に witness が届くこと、実物の関数との対応表、検査の大きさと見られないものを「Quint の要否」と `specs/quint/review-thread-ack/README.md` に書いた。直しのあとの規則で #871 と #131 の実データを測り直した。
- **移行**：「タグを切る条件」を足した。`v2` は上の 3 件が直るまで切らない。新旧の比較の差 0 件では ci#44 の偽の緑が見つからない（旧の判定に、内容で承認を決める経路が無い）ので、比較に承認の経路の確認を足した。

## 改訂の記録（2026-11-28 の 2 回目の直し。ci#43 のレビューの残る穴 4 つ）

[j5ik2o/ci#43](https://github.com/j5ik2o/ci/pull/43)（先端 86a443c）でテストを b808a29 の規則に合わせて固めたところ、Devin と CodeRabbit のレビューで、b808a29 の規則にも残る穴が 4 つ見つかった。

- **高（偽の緑）：まとめ返信の確認の語が別の項目にも効く**：確認の語と参照を同じ単位（段落・リストの項目・混在のときは節）で結ぶ形にした。引用とフェンスは付く先の単位に入る。前置きの段落の語は、打ち消しの無いリストの項目に効く。打ち消しの節が触れた項目は承認しない。「確認の語の効く範囲と打ち消し」、条件 4・5、jq の `line_kinds`・`units`・`attach_targets`・`lead_of`・`clauses`・`acked_keys`、テストの観点 25。
- **高（偽の緑）：行頭 `>` の引用の中の `fixed` を数える**：確認の語は、引用（行頭 `>`、「」『』“”、インラインのコード）と項目の本文を写した部分の外だけで数える。引用は参照にだけ使う。jq の `clauses` の masked と `cover_flags`、テストの観点 26。
- **高（偽の緑）：`not been fixed` などの英語の否定**：英語の語は、同じ節の前に否定・条件・未来の語があれば数えない。テストの観点 27。
- **高（偽の緑）：`確認済みではありません` などの日本語の否定**：語の直後の否定と疑問（`ですか` を含む）を見る。b808a29 で漏れていた `確認済みですか` も塞いだ。テストの観点 28。
- **誤って赤にする側**：同じ 5 リポジトリで標本を取り直し（30 件。前の 17 件はそろえ直せなかった）、#131 と event-store-adapter-java#871 も流した。赤に戻るのは modern-ddd-book#695（マージ済み）と event-store-adapter-java#871（open）の 2 件で、どちらも確認の語の無い単位で項目を指す形（穴 1 の直しによる）。「いまの状態」の下見、「一意に指す値として何を使うか」の測り直し、テストの観点 19、影響を直した。
- **Quint**：確認を単位に分け、単位ごとの語の現れ方（`Word`）を持たせた。規則 `unique`（b808a29）を足して穴 4 つの反例を固定し、この設計の規則を `scoped` とした。わざと壊した版を 14 にし（穴 4 つを 1 つずつ開けた版を含む）、witness と実物の対応表と範囲を更新した（「Quint の要否」と README）。
- **v2**：「タグを切る条件」に 4 件目として、この 4 つの穴を足した。v1 は動かさない、v2 は元帥が新旧の比較を見てから切る、は変えていない。
- **PR #452 のレビューで直した 6 点**（Devin 6 件、Cursor Bugbot 1 件、CodeRabbit 2 件。重なりを除いて 6 点）：(1) 同じ節に肯定と打ち消しが並ぶと肯定が勝っていた（`#10 is fixed but #11 is not fixed`）。逆接で節を切り、それでも同じ節に並ぶときは節の全体を打ち消しにした。まだの表明のうち、直後が `こと` などのものは数えない。(2) 引用が見出しを越えて別の節の確認の文に付いていた。付く先を同じ見出しの節に限った。(3) まだの文に付いた引用が拒否に使われていなかった。打ち消しの節を含む単位に付いたかたまりも拒否に使う。(4) ``` ``fixed`` ``` のような 2 つ以上のバッククォートのコードを、1 文字ごとに出入りしていた。CommonMark と同じく、同じ長さの列で開いて閉じる。(5) `**確認済み**ではありません` で、強調の記号が否定の判定を妨げていた。強調の記号を飛ばして見る。(6) 空行のあとに字下げした続きの行を、リストの項目から切っていた。字下げした文の行は項目の続きにした。テストの観点 25・26・28 の表に 7a〜7n の 14 行を足した（全 55 行。14 行のうち 9 行で、b808a29 は指していない項目かまだと書いた項目まで承認する）。30 件の標本と #131・#871 の結果は変わらない。Quint のモデルは変えていない（同じ節の肯定と打ち消しは、文字列の水準で打ち消しの単位になるので、モデルの `Word` の与え方の内側）。

## 未確認のこと

- **`noted` などの語を確認の語に足すか**（元帥の判断）。足せば event-store-adapter-java#871 の `I noted it.` が通り、#871 は緑のまま残る。代わりに `As noted in #10, …` のような地の文で承認になる危険がある。この設計では足さず、#871 は赤になる（作成者が `確認しました` を足せば緑）。
- この規則でも見逃す形（偽の緑の側）：(1) 「まだ」の表明の一覧に無い先送り（`次の PR で直します`、`後で対応します`、`follow-up で`）は打ち消しにならず、前置きの語や同じ段落の語が効く。(2) 英語の推量（`should be fixed now`）は確認と数える。(3) ASCII の `"…"` の中の語は引用とみなさない（対になっているかを決められないため）。(4) HTML の `<blockquote>` は引用とみなさない。どれも ci#43 のレビューの反例ではなく、30 件の標本にも無かった。
- jq の `match` の `offset` と文字列の切り出しは、jq 1.7 以上（コードポイントで数える）を前提にしている。jq 1.6 はバイト位置を返すので、日本語の後ろで位置がずれる（ci#43 の開いたスレッドの `id_match` と同じ類）。実装では `explode` で数えるか、jq 1.7 以上を前提として README に書く。
- `refs_in` と `mentions_in` は、節ごとにすべての項目の `quote_text` と `$others` を作り直す。ci#43 の開いたスレッド（長文の返信で数分かかる）をさらに重くする。実装では、項目ごとの `quote_text` と、窓の集合を一度だけ作る。

- 引用を「その項目だけが持つ断片」に限って残す設計は、元帥の方針（照合を、対象を一意に指す値の完全一致にする）の読み方の 1 つである。引用をやめて id（URL・アンカー）だけにする読み方もありうる。その場合、#131 の実際の確認と event-store-adapter-java#871 の確認が赤になり、テストの観点 5・18・19 と移行の負担が変わる。どちらにするかは元帥の確認が要る。
- URL を含みアンカーの無い確認（pekko-persistence-dynamodb#1040、event-store-adapter-kotlin#906）が、URL の外の引用でも項目を指しているかは、まだ読んでいない（「移行の順」2 で確かめる）。
- 同じ PR に同じ文の項目が並ぶ頻度（同じ利用上限の通知が 2 回など）は数えていない。多い場合は、引用の代わりに URL を貼る運用の負担が増える。
- scheme の無い `ホスト名/パス` を外す正規表現が、ファイル名（`repo_gate.py/…` のような形）を巻き込まないかは、実データで確かめていない。

- 手元の clone のタグ `v1` が `db3bfd0` を指す理由。リモートの `v1` が過去に付け替えられたのか、手元だけで付けたのかは分からない。
- TAKT の merge-prepare が、赤のまま止まる挙動を受け入れられるか。失敗報告へ `<!-- takt:auto-report -->` を付ける変更を、どのワークフロー（takt-dash の `.takt/` か、takt 本体か）に入れるかは未確認。マーカーが付くまでは、本文がコードフェンスだけの旧来の形を種別で外せる。散文を伴う報告（フェンスの直前に一文を置く形を含む）は、マーカーが付くまで自動報告として外れない。
- `extra_ignored_auto_report_author_patterns` に Codex の利用上限の通知を入れているリポジトリの完全な一覧は未確認（この文書では takt-dash を確認した）。入れていないリポジトリでは、通知が未承認の項目として残る。
- 22 リポジトリのすべてを v2 に上げるか、SHA 固定の 2 つも同時に上げるかは、オーナーの判断。
- レビューの `updatedAt` が、レビュー本文の編集でコメントと同じように進むかは、GraphQL の実データで確認していない（コメントでは確認済み）。
- 2026-11-27 の open PR 15 件は、その日のスナップショットである。タグを切る前に同じ比較をもう一度回す。2026-11-28 の直しのあとの規則では、#871 と #131 しか測っていない。
- 既存の `ignored_auto_report` の本文パターンが、今後も Codex / CodeRabbit の通知を十分に覆うかは、別 issue の範囲である。
