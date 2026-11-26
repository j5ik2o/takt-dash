# issue #162: tasks.yaml の読み取りを Python 側の reader と揃える

対象: [j5ik2o/takt-dash#162](https://github.com/j5ik2o/takt-dash/issues/162)

## 現状（main 8fabf28 で確かめた）

`backend/src/run_log.rs` の `parse_task_entries`（766-830 行）は、Python 側の `tools/project-status/task_context.py` の `parse_tasks_yaml` と次の点で結果が違う。

- `list_item_content` は `"  -"`（2 空白）の行だけを項目の先頭と見る。`item_field_content` は 4 空白ちょうどの行だけを項目のキーと見る。字下げ 0 の `- run_slug: x` は、項目ごと読まれない。
- `split_field` は値を trim するだけで、引用符を外さない。`run_slug: "run-a"` は `"run-a"` のまま持つ。
- `~`・`null` を値として持つ（Python は空として扱う）。キーの引用符を外さない（Python は外す）。

再現: `backend/tests/run_log_test.rs` に、引用符付きの値と字下げ 0 の項目で `resolve` を呼ぶテストを足した。どちらも `Found` を期待して `NotListed` になる（失敗を確かめた）。

Python 側にも、2 つの癖がある（同じ main で確かめた）。

- 項目の先頭の行の値がブロックスカラー（`- retry_note: >-`）のとき、本文を読み飛ばす基準を `-` の字下げにする。そのため、同じ項目の残りのキー（`    run_slug: run-x`）まで読み飛ばす。いまの Rust は、このキーを読む。
- ブロックスカラーの印（`>-` など）を値として持つ（`run_slug: >-` は `">-"`）。いまの Rust は、その項目を持たない（`None`）。

## YAML のライブラリで読む案を採らない理由

- backend の依存に YAML のライブラリは無い。serde_yaml は 2024 年に保守を終えている。
- YAML の仕様どおりに読むと、Python の reader と結果が食い違う（二重引用符の中のエスケープ、`''`、行末のコメント、数値や真偽値のスカラー、フロー形式、アンカー）。この issue の受け入れ条件は「Python 側の reader と同じ結果」なので、仕様どおりのライブラリを入れると、別の食い違いを作る。Python 側は PyYAML に依存しないことを意図して書かれている（docstring）。
- ライブラリは 1 か所の構文の誤りでファイル全体を読めなくする。いまの行単位の reader は、壊れた行だけを飛ばす。
- 両方を YAML のライブラリに替えるのは、Python 側の依存と挙動を変える別の判断になる。範囲の外とする。

代わりに、Python の `parse_tasks_yaml` の手順を、Rust にそのまま移す。食い違いは、Python の reader を正として PBT で比べて捕まえる。同じ規則を 2 か所で直し続ける形を、比べる検査で守る。

## 変えること

### Rust（`backend/src/run_log.rs`）

`parse_task_entries` を、Python の `parse_tasks_yaml` と同じ手順に書き直す。`list_item_content`・`item_field_content`・`split_field` は消す。

- 行の区切りは Python の `str.splitlines` と同じにする（`\n`、`\r\n`、`\r`、`\x0b`、`\x0c`、`\x1c`、`\x1d`、`\x1e`、`\x85`、`U+2028`、`U+2029`）。Python はファイルを universal newlines で読み、そのあと `splitlines` で切る。この組み合わせの結果は、元の文字列を上の区切りで切った結果と同じになる。
- 前後の空白の除去は Python の `str.strip` と同じにする（Rust の `char::is_whitespace` に `\x1c`〜`\x1f` を足した集合）。
- 字下げは、行の先頭の半角空白の数（Python の `_indent_of`）。
- 項目の先頭: 除いた行が `- ` で始まり、まだ基準が無いか、字下げが基準と同じ行。最初の項目の字下げを基準にする。
- 項目のキーの行: 項目の中で、字下げが基準より深い行。
- 本文は最初の `:` で切る。キーは前後の空白を除き、さらに前後の `'` と `"` を除く（`_clean_key`）。値は `_clean_value`（前後の空白を除く、`""`・`~`・`null`・`Null`・`NULL` は空、前後が同じ引用符なら外す）。
- 空でない値だけを `run_slug`・`source_run_slug`・`worktree_path` に持つ。空の値は `None`（Python の `find_task_entry` は空の値を一致に使わない）。同じキーが続けば、後のものが勝つ（Python の辞書と同じ）。
- 値がブロックスカラーの印（`>`、`>-`、`>+`、`|`、`|-`、`|+`）なら、値は空にし、キーの桁より深い行と空行を本文として読み飛ばす（下の Python の直しと同じ）。
- 空行と `#` で始まる行は飛ばす。除いた行が `tasks:` の行は飛ばす。
- `tasks:` の行が 1 つも無いときは、いまと同じく `Err`（`RunLocation::Unreadable`）にする。Python は空の一覧を返すが、空のファイルと壊れたファイルを「一覧に無い」と区別するための、Rust だけの決まりとして残す。

### Python（`tools/project-status/task_context.py`）

上の 2 つの癖を直す。Rust に癖をそのまま移すと、いま読めているキーを読まなくなるため。

- ブロックスカラーの本文を読み飛ばす基準を、行の字下げではなく、キーの桁にする（項目の先頭の行なら、`-` の字下げ + 2 + `- ` の後の空白の数）。
- ブロックスカラーの印を値にせず、空にする。

`tools/project-status/task_context_test.py` に、この 2 点のテストを足す。このテストはいまどこからも流れていないので、`Makefile` の `test` に `python3 tools/project-status/task_context_test.py` を足す（下の「守りの仕組み」）。

### 変えないこと（範囲の外）

- 入れ子のマッピングの中のキーも、項目のキーとして読む（Python と同じ）。takt のスキーマ（`src/infra/task/taskRecordSchemas.ts`）では 3 つのキーは項目の直下にだけあり、実物の tasks.yaml（takt、takt-dash、fraktor-ng-rs）でも入れ子に 3 つのキーは無い。両方を YAML どおりにするなら別の issue にする。
- 二重引用符の中のエスケープ、複数行にわたる引用符の値、行末のコメントは、Python と同じく解釈しない。
- `resolve` の候補の組み立て、許可された置き場の検査（issue #164 の範囲）は変えない。

## 守る不変条件

- I1: `tasks:` の行を持つ任意の文字列について、Rust の項目の列は、Python の `parse_tasks_yaml` の結果を 3 つのキーに写したもの（空は `None`）と、項目の数と順まで一致する。
- I2: `tasks:` の行を持たない任意の文字列について、Rust は `Err` を返す。持つなら `Ok` を返す（`tasks:` の行の有無は Python の `splitlines` と `strip` で判定したものと一致する）。
- I3: 引用符付きの `run_slug` と `worktree_path` を持つ項目は、引用符の無い項目と同じ run に解決する。
- I4: 字下げの幅（0〜任意）が違っても、項目の構造が同じなら、同じ run に解決する。

## PBT の性質（除外なし）

proptest で生成した文字列を一時ファイルに書き、Rust の `parse_task_entries` と、同じファイルを読む Python の `parse_tasks_yaml` に渡して比べる。Python は 1 本の子プロセス（`python3 -I`）を立て、標準入出力で 1 件ずつ問い合わせる。`python3` が無ければテストは失敗する（飛ばさない）。CI の `done`・`coverage` ジョブは Python 3.12 を入れている。

- P1（構造を持つ文書）: `tasks:` の見出し（前後の空白、コメント、空行の変種）のあとに、生成した項目を並べる。各項目は、項目の字下げ（0〜6）、キーの字下げ（項目より 1〜4 深い）、キー（3 つのキー、ほかのキー、引用符付きのキー、空白を含むキー）、値の形（引用符なし、単一引用符、二重引用符、片側だけの引用符、引用符 1 文字、空、`~`・`null` の変種、ブロックスカラーの印と本文、複数行の値の続きの行、`:`・`#`・`- ` を含む値）、値の文字（ASCII、日本語、絵文字、全角空白、NBSP、`\x1c`〜`\x1f`、`U+2028`、`\x85`、タブ）を持つ。改行は `\n`・`\r\n`・`\r` から選ぶ。I1 を確かめる。
- P2（任意の文字列）: 構造を作る文字（`-`、`:`、空白、改行の各種、引用符、`#`、`>`、`|`、タブ）と Unicode に寄せた任意の文字列の前に `tasks:\n` を付けて、I1 を確かめる。
- P3（任意の文字列の `tasks:` の有無）: 任意の文字列（`tasks:` を含むかどうかを問わない）で、Rust の `Ok`／`Err` が、Python 側で判定した `tasks:` の行の有無と一致することと、`Ok` のときは項目が Python と一致することを確かめる。I2 を確かめる。
- 代表ケースの例のテスト（受け入れ条件）: 引用符付きの `run_slug`・`worktree_path` で `resolve` が `Found` になる（I3）。字下げ 0 の項目で `Found` になる（I4）。issue のケース、ブロックスカラーが先頭のキーのケース、`>-` の値のケースで、Rust と Python が同じ項目を返す。

## Quint

要らない。ファイルの文字列から項目の列を返す純粋な関数の修正で、状態遷移（反復、再試行、再開、ロック、並行）に関わらないため。

## 壊しうる入力

- 境界: 字下げ 0、項目ごとに違う字下げ、基準より浅い `- ` 行、基準より深い `- ` 行、タブの字下げ、空のファイル、`tasks:` だけのファイル、最後に改行の無いファイル。
- 区切り: 値の中の `:`（`worktree_path: C:/x`、`a: b: c`）、値の中の `#`、`- ` で始まる値、`-` だけの行、改行の各種（CRLF、CR、`U+2028`、`\x85`、`\x0b`）。
- 入れ子: ブロックスカラーの本文の中の `- run_slug:` 行、先頭のキーがブロックスカラーの項目、入れ子のマッピング、項目の中の配列。
- 引用符: 両側の `"`／`'`、片側だけ、引用符 1 文字、`''`、中に引用符を含む値、引用符付きのキー。
- 空の値: 空、`~`、`null`／`Null`／`NULL`、空白だけ、引用符だけ（`""`）。
- 長さ: 長い行、多数の項目。
- Unicode: 日本語のパス、絵文字、全角空白、NBSP、BOM、`\x1c`〜`\x1f`（Python だけが空白とみなす文字）。
- 並行・再開: 無い（`resolve` のたびにファイルを 1 回読む純粋な関数）。

## 影響

- 上流（takt）: 変えない。takt が書く tasks.yaml の形はそのまま読める。
- 保存済みのデータ: 読むだけで、書かない。
- 公開 API: 変えない（proto も gRPC も同じ）。`RunLocation` の結果は、引用符付きの値と字下げの違う項目で `NotListed` から `Found` に変わる。値が `>foo` のような印でない `>`／`|` 始まりのとき、いままで `None` だったものを値として持つ（Python と同じ）。
- 守りの仕組み: `Makefile` の `test` に `task_context_test.py` を足す（検査を増やすだけ）。`cargo test` が `python3` を要るようになる（CI の `done`・`coverage` ジョブは導入済み）。CI の定義とルールセットは変えない。
