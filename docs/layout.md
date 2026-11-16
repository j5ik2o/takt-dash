# レイアウトの基本部品と規則

issue #119（画面のレイアウトの土台作り）の一部として、frontend/web の画面を組み立てるときに使うレイアウトの基本部品と、その使い方の規則をまとめます。この文書と部品の定義は土台だけです。概要の画面への適用は issue #120、判断待ちの画面への適用は issue #121 で行います。

- 部品の定義: `frontend/web/app/globals.css` の末尾にある「レイアウトの基本部品」の節
- 変数の定義: `frontend/web/app/globals.css` の `:root`
- 検査: `frontend/web/test/pages/layout.test.ts`（部品）、`frontend/web/test/pages/tokens.test.ts`（余白・文字サイズ・角丸の変数）、`tools/repo-gate` の `frontend-style`（色の直書きと列数の固定。「機械の検査」の節を参照）

## 基本部品の一覧

部品のクラス名は、既存の画面用のクラスと区別するため、先頭に `l-` を付けます。部品はレイアウト（並べ方と間隔）だけを受け持ち、文字の色や状態の表示は画面用のクラスに任せます。

| クラス | 役割 | 使う場面 | 使わない場面 |
|--------|------|----------|--------------|
| `.l-stack` | 子要素を縦に積み、間隔をそろえる | カードの中身、一覧の行の並び、ダイアログの中身のように、要素を上から下へ並べるとき | 要素を横に並べたいとき（`.l-cluster` を使う） |
| `.l-cluster` | 子要素を横に並べ、幅が足りなければ次の行へ折り返す | ボタンの並び、ラベルやバッジの並び、見出しと補足情報の並びのとき | 主と従の 2 つの領域を並べるとき（`.l-split` を使う） |
| `.l-split` | 最初の子要素を主の領域、最後の子要素を従の領域として横に並べる。幅が足りなければ従の領域を下へ折り返して 1 列にする | 一覧と詳細、本文と補足のように、広い領域と狭い領域を並べるとき | 同じ種類の要素を 3 つ以上並べるとき（`.l-cluster` か `.l-stack` を使う） |
| `.l-table` | 表を全幅の枠で囲み、横にはみ出す表を枠の中でスクロールさせる | `<table>` で行と列を持つデータを見せるとき | 行ごとに形が違う一覧を見せるとき（`.l-stack` を使う） |
| `.l-section` | 節の見出し（`h2`）と本文を縦に組み、見出しと本文の間隔をそろえる | 画面の中の 1 つの節を作るとき | 節の中の小さなまとまりを作るとき（`.l-stack` を使う） |

### `.l-stack`

子要素の間隔は `gap: var(--space-12)` です。子要素の外側の margin で間隔を作らないでください。

```html
<div class="l-stack">
  <p>1 行目</p>
  <p>2 行目</p>
</div>
```

### `.l-cluster`

子要素の間隔は `gap: var(--space-8)` で、縦方向は中央にそろえます。両端に寄せたいときは、画面用のクラスで `justify-content: space-between` を追加します。

```html
<div class="l-cluster">
  <button type="button">承認</button>
  <button type="button">却下</button>
</div>
```

### `.l-split`

主の領域は残りの幅をすべて使い、従の領域は `:root` の `--layout-aside-width` を基準の幅にします。主の領域に全体の半分の幅を残したまま、従の領域に基準の幅を確保できないときは、従の領域が下へ折り返します。メディアクエリで列数を切り替える必要はありません。子要素は 2 つだけにしてください。

```html
<div class="l-split">
  <div>主の領域</div>
  <aside>従の領域</aside>
</div>
```

### `.l-table`

枠は `background: var(--surface)`、`border: 1px solid var(--line)`、`border-radius: var(--radius-12)` で描きます。表の行は `var(--line2)` の線で区切り、最後の行だけ線を消します。`<table>` は `<thead>` と `<tbody>` を使って書いてください。

```html
<div class="l-table">
  <table>
    <thead>
      <tr><th>サーバー</th><th>状態</th></tr>
    </thead>
    <tbody>
      <tr><td>srv-1</td><td>稼働中</td></tr>
    </tbody>
  </table>
</div>
```

### `.l-section`

見出しと本文の間隔は `gap: var(--space-12)` で、見出しの文字サイズは `var(--font-size-16)` です。見出しの下の余白を、見出しの margin で作らないでください。

```html
<section class="l-section" aria-labelledby="h-runs">
  <h2 id="h-runs">実行中の run</h2>
  <div class="l-stack">…</div>
</section>
```

## 余白・文字サイズ・角丸の変数の使い方

余白、文字サイズ、角丸の値は、`:root` で定義した変数を `var()` で参照して書きます。変数の名前の数値は px の値と同じです（例: `--space-12` は `12px`）。

| 変数 | 使う宣言 | 定義されている値 |
|------|----------|------------------|
| `--space-*` | `margin`、`padding`、`gap`（`row-gap`、`column-gap` を含む） | 2、4、6、8、10、12、14、16、18、20、22、24、36 |
| `--font-size-*` | `font-size` | 12、13、14、15、16 |
| `--radius-*` | `border-radius` | 4、6、8、12、`pill`（999px） |

- 変数は決められた宣言だけで使います。たとえば `--space-*` を `width` や `border-radius` に使いません。
- 必要な値の変数が無いときは、規則に数値を直書きせず、`:root` に変数を追加します。追加した変数の値は名前の数値と同じにします。
- 部品の規則には、罫線の `1px` を除いて、長さを直書きしません。部品が必要とする幅（`--layout-aside-width`）も `:root` に置きます。

## 列数を固定しない原則

列の数は、画面の幅と要素の大きさから決まるようにします。サーバーの台数、ゴールの件数、項目の数のように、データの数から列の数を決めません。

- `grid-template-columns: repeat(3, …)` のように列数を数で固定しません。並べる要素には `.l-cluster` や `.l-split` の折り返しを使います。グリッドが必要なときは `repeat(auto-fit, minmax(…))` のように、幅から列数が決まる書き方にします。
- 台数や件数に合わせて列数を変える分岐を、コンポーネントに書きません。
- 既存の `.tiles`（3 列）、`.cards` と `.scards`（2 列）は、列数を固定してメディアクエリで切り替えています。これらは部品を適用する前の書き方であり、新しい画面では同じ書き方をしません。

## 色を直書きしない原則

色は `:root` で定義した色の変数（`--text`、`--muted`、`--surface`、`--line`、`--teal`、`--orange` など）を `var()` で参照して書きます。

- 規則に `#fff` のような 16 進数、`rgb()`、`rgba()`、`hsl()` を直書きしません。
- 必要な色の変数が無いときは、`:root` に変数を追加してから参照します。
- 部品の規則は、枠や罫線の色にも変数（`--surface`、`--line`、`--line2`）を使います。

## 機械の検査

「列数を固定しない原則」と「色を直書きしない原則」は、`tools/repo-gate` の `frontend-style` 検査で機械的に確かめます。判定は `tools/repo-gate/gate_frontend_style.py` にあります。判定を変えるときは、この節も合わせて直してください。

### 実行の経路

- `make done` は `make test` を実行し、`make test` は `python3 tools/repo-gate/repo_gate_test.py` を実行します。このテストの中で実リポジトリの `frontend/web` を検査し、違反があると `make test` と `make done` が失敗します。
- `python3 tools/repo-gate/repo_gate.py --root .`（`--skip-make` を付けた場合も）でも `frontend-style` として実行され、違反は stderr の `frontend-style:` の 1 行に、`ファイル:行` と文脈、宣言、種別（`色` または `列数`）を並べて出ます。

### 対象のファイル

- `frontend/web` の下の `*.css` と `*.tsx`（コンポーネント）です。
- 次は対象外です。
  - ディレクトリ `node_modules`、`.next`、`out`、`coverage`（どの深さでも）
  - `frontend/web/src/proto`（生成物）、`frontend/web/test`（テスト）、`frontend/web/public`（静的ファイル）
  - ファイル名が `*.test.tsx` のもの
- `.ts` のファイル（状態や API の処理）は、拡張子が対象外のため検査しません。
- CSS が 1 件も見つからないときは、探索先の誤りとして検査が失敗します。

### 色として検出するもの

CSS では、コメントを除いたすべての宣言の値を調べます。

- 16 進数の色（`#fff`、`#fffcf9` など）
- 色の関数（`rgb()`、`rgba()`、`hsl()`、`hsla()`、`hwb()`、`lab()`、`lch()`、`oklab()`、`oklch()`、`color()`）
- CSS の色の名前（`red`、`white`、`orange` など 148 色）

コンポーネントでは、コメントを除いた各行を調べます。

- 引用符または backtick で囲んだ文字列の中にある 16 進数の色
- 色の関数
- 色を持つ style のキー（`color`、`background`、`backgroundColor`、`border*`、`outline*`、`fill`、`stroke`、`boxShadow`、`textShadow`、`caretColor`、`accentColor`、`textDecorationColor`、`columnRuleColor`）の文字列の値にある色の名前

次は検出しません。

- `:root` の規則の中の変数の定義（`--surface: #fff` など）
- `var(--teal)` のような変数の参照（`--teal` は `teal` として一致しません）
- `transparent`、`currentcolor`、`inherit`、`initial`、`unset`
- コメント、CSS の値の中の引用符で囲んだ文字列と `url()`
- コンポーネントの JSX の本文（`issue #119` など）、クラス名などの style 以外の文字列にある色の名前、URL の `//`

### 列数の固定として検出するもの

- CSS の `grid-template-columns`、`grid-template`、`grid` の値にある `repeat(数, …)`（`@media` の中も含みます）
- CSS の `columns` と `column-count` の値が数で始まるもの
- コンポーネントの `repeat(` の直後が数か `${…}`（式の値で列数を決めるもの）、`columnCount:`、数で始まる `columns:`

次は検出しません。

- `repeat(auto-fit, minmax(…))` と `repeat(auto-fill, minmax(…))`
- `96px minmax(0, 1fr) 200px` のように個々の幅を並べたトラック指定。1 つの要素の中の区画を決めるもので、データの数から列数を決めるものではないためです。
- `className` の三項演算子などで、件数によってクラスを切り替える分岐。静的には判別できないため、レビューで確かめます。

### 既存の例外

部品を適用する前の既存の宣言（`.tiles`、`.cards`、`.scards`、`.sum-grid` の列数と、`.run-row .bar.live > i`、`.dot.live`、`@keyframes pulse`、`@keyframes flash`、`.scard.down`、`.dialog-backdrop` の色、計 14 件）は、`tools/repo-gate/gate_frontend_style.py` の `EXISTING_EXCEPTIONS` に載せて通しています。

- 例外は、ファイル、文脈（`@media` などの入れ子とセレクタ）、プロパティ、値（空白を 1 つに詰めたもの）が完全に一致するときだけ通します。値を変えた宣言や、同じセレクタに新しく書いた違反は検出されます。
- 部品の適用（issue #120・#121）などで宣言が消えたり値が変わったりしたら、一覧から外してください。一覧に残った項目が CSS に見つからないと、検査が失敗します。
- 新しい例外は足しません。色は `:root` の変数で、列数は部品か `auto-fit` で書いてください。
