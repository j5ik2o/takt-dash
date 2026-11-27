# issue #186 の設計：Web のログの各行に時刻を付ける

対象の issue：[j5ik2o/takt-dash#186](https://github.com/j5ik2o/takt-dash/issues/186)

## いまの状態（main `54b10a6`）

- 本番の Web は launchd のジョブ `com.ideoplus.taktdash-web` で動く。plist（`~/Library/LaunchAgents/com.ideoplus.taktdash-web.plist`）の `ProgramArguments` は `node <配置先>/frontend/web/node_modules/next/dist/bin/next start -H 0.0.0.0 -p 3000` で、`StandardOutPath` が `<配置先>/data/web-launchd.log`、`StandardErrorPath` が `<配置先>/data/web-launchd.err.log`。
- どちらのログにも時刻が無い。Next.js の出力（`▲ Next.js …`、`✓ Ready in …`、警告）も、アプリの出力（`[taktDash][timing] …`、`console.error` のスタック）も、そのまま書かれる。
- そのため `tools/prod-check/prod_log.py` の `log_excerpts` は、Web のエラーを「最後の `Ready` 以降の行」で近似して拾っている。
- backend は `tracing_subscriber::fmt()` の既定で、各行の先頭に UTC の RFC 3339（`2026-11-26T22:48:01.736884Z`、マイクロ秒）を書く。

## 案の比較

| | (A) 起動を包むラッパーで各行に時刻を付ける | (B) Next.js の中（`instrumentation.ts` などで `process.stdout.write` や `console` を差し替え）で付ける |
| --- | --- | --- |
| Next.js 自身の出力（`▲ Next.js`、`✓ Ready`、設定の警告） | 付く。子の出力をすべて通すため | 付かない。`next start` の CLI が出す行は `register()` より前に書かれる |
| Node が直接書く出力（捕まえていない例外のスタック、`--trace-warnings`、プロセスの終わりのメッセージ） | 付く | 付かない。差し替えた `write` を通らない書き出しがある |
| アプリのコード | 変えない | 実行時に全体の出力を差し替える。テストと開発（`next dev`）の出力にも影響する |
| 本番の反映 | plist の `ProgramArguments` を変える | plist は変えない |

(A) を採る。issue の目的は障害の時刻でログを切り出すことで、Next.js の起動の行とスタックにこそ時刻が要る。(B) はそれを取りこぼす。

ラッパーは sh ではなく Node のスクリプトにする。sh の `while read` は改行の無い最後の行と NUL を扱えず、BSD の `date` はミリ秒を出せず、行ごとに `date` を起こすと重い。子への合図の引き継ぎも sh では書きにくい。Node は Next.js を動かすために既にある。

## 何をどう変えるか

`frontend/web/scripts/` に 2 つのファイルを足す。どちらも型の変換なしに `node` で動く ESM（`.mjs`）。

### 1. `frontend/web/scripts/line-stamper.mjs`（時刻を付ける純粋な部分）

```js
/** @param {number} epochMs @returns {string} */
export function formatStamp(epochMs) // new Date(epochMs).toISOString()。例 "2026-11-27T01:02:03.456Z"

/** @param {() => number} [now] 既定は Date.now @returns {(chunk: Uint8Array) => Buffer} */
export function createLineStamper(now)
```

- 時刻の形式は UTC の RFC 3339、ミリ秒まで、末尾 `Z`（`Date.prototype.toISOString()` と同じ 24 文字）。そのあとに半角の空白を 1 つ置いてから行の中身を続ける。backend と精度は違う（ミリ秒とマイクロ秒）が、`prod_log.py` の `jst_stamp` の正規表現（`\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z\s+`）はどちらも読める。
- `createLineStamper` は流れ 1 本ぶんの状態（いまが行の頭か）を持つ関数を返す。受け取ったバイト列を、行の頭にあたる位置へ時刻を差し込んで返す。
  - 行の頭とは、流れの最初のバイトと、`\n`（0x0A）の直後のバイト。
  - 時刻は、その行の最初のバイトを受け取ったときに `now()` を 1 回呼んで決める。行の終わり（`\n`）を待たない。
  - `\n` で終わったバイト列のあとは、次のバイトが来るまで時刻を出さない（流れの終わりに、中身の無い時刻だけの行を作らない）。
  - 空のバイト列には空を返し、`now()` を呼ばない。
- 行を溜めない。受け取ったぶんをすぐに返す。とても長い行でも、記憶の量は受け取った塊の大きさで決まる。
- 文字の符号を解釈しない。`\n` は UTF-8 の多バイト文字の途中に現れないので、塊の切れ目が文字の途中でも、時刻は文字の間に入らない。

### 2. `frontend/web/scripts/with-timestamps.mjs`（起動を包む部分）

```
node scripts/with-timestamps.mjs <program> [args...]
```

- `<program>` を `args` で起こす（`child_process.spawn`、シェルを通さない）。標準入力は引き継ぐ。標準出力と標準エラーは pipe で受け、それぞれ別の `createLineStamper()` を通して、自分の標準出力と標準エラーへ書く。標準出力の行は標準出力へ、標準エラーの行は標準エラーへ。
- 書き出しが詰まったら（`write` が false を返したら）、その子の流れを止め、`drain` で再開する。
- 合図：自分が `SIGTERM`・`SIGINT`・`SIGHUP` を受けたら、同じ合図を子へ送る。自分は終わらず、子の終わりを待つ。
- 終わり方：子の標準出力と標準エラーを読み終え（子の `close`）、自分の書き出しを終えてから終わる。
  - 子が終了コードで終わったら、同じ終了コードで終わる。
  - 子が合図で終わったら、合図の受け手を外して、同じ合図を自分へ送って終わる。launchd から見て、子が直接起こされていたときと同じ終わり方になる。
- 子を起こせなかったら（`spawn` の `error`、例えば `ENOENT`）、時刻を付けた 1 行を標準エラーへ書き、127 で終わる。
- `<program>` が無ければ、使い方を時刻を付けた 1 行で標準エラーへ書き、2 で終わる。
- ラッパーは環境変数を変えない。Next.js の色の判定（出力先が端末か）は、いまと同じく「端末でない」になる。

### 反映のときの plist の変え方（この PR では変えない）

`ProgramArguments` の先頭に、node とラッパーを足す。子の node も絶対パスで渡す。

```
<node> <配置先>/frontend/web/scripts/with-timestamps.mjs <node> <配置先>/frontend/web/node_modules/next/dist/bin/next start -H 0.0.0.0 -p 3000
```

いまの値では、`<node>` は `/Users/j5ik2o/.local/share/mise/installs/node/24.22.3/bin/node`、`<配置先>` は `/Users/j5ik2o/local/takt-dash-app`。`StandardOutPath`、`StandardErrorPath`、`WorkingDirectory`、`EnvironmentVariables`、`KeepAlive`、`ThrottleInterval` は変えない。

手順：

1. 配置先を、ラッパーを含む版へ更新し、`npm --prefix frontend/web run build` を流す。
2. plist の `ProgramArguments` を上の値に書き換える（`plutil -insert ProgramArguments.0 -string <node>` と `plutil -insert ProgramArguments.1 -string <ラッパー>` を、この順で）。`plutil -lint` で確かめる。
3. `launchctl bootout gui/$(id -u)/com.ideoplus.taktdash-web` のあと `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.ideoplus.taktdash-web.plist`。`kickstart -k` は plist を読み直さないので使わない。
4. `tail data/web-launchd.log` で `✓ Ready` の行が時刻で始まることを確かめる。

戻すときは、`plutil -remove ProgramArguments.0` を 2 回流して先頭の 2 つ（node とラッパー）を消し、3 と同じく起こし直す。ログの古い行には時刻が無いままなので、読み手は時刻の無い行も読めるままにする。

`prod_log.py` の「最後の Ready 以降」の近似を、時刻での切り出しに替えるのは別の issue にする（この PR の範囲の外）。

## 変えないこと

- 本番の plist（反映は別）。
- Next.js の設定、アプリのコード、`[taktDash][timing]` の行の形。
- 開発と e2e の起動（`npm run dev`、`npm run start`、`e2e/support/stack.ts`）。ラッパーは本番の plist からだけ使う。
- `tools/prod-check/` の読み方。

## 守る不変条件

- I1（中身）：出力から各行の先頭の「時刻と空白 1 つ」を除くと、子が書いたバイト列と 1 バイトも違わない。文字の符号の誤り、`\r`、NUL、制御文字、ANSI の色もそのまま通す。改行の無い最後の行に改行を足さない。
- I2（各行に時刻）：中身が 1 バイトでもある行は、すべて時刻で始まる。複数行のエラー（スタック）は、最初の行だけでなく各行に時刻を付ける。どの行を切り出しても時刻が分かるようにするため。空の行（`\n` だけ）にも付ける。
- I3（時刻の順）：時刻は単調に増えるとは限らない。壁時計（`Date.now()`）が NTP などで戻ると、後の行の時刻が前の行より早くなる。それを補正しない（時刻を書き換えると、実際の時計とずれて他のログと突き合わせられなくなる）。読み手は時刻の並びではなく、行の並びを出来事の順とする。
- I4（順）：1 本の流れの中では、行とバイトの順を保つ。標準出力と標準エラーの間の順は、いまと同じく保証しない（別のファイルに書かれる）。
- I5（区別）：子の標準出力は自分の標準出力へ、子の標準エラーは自分の標準エラーへ書く。混ぜない。
- I6（終わり方）：子の終了コードと、子を終わらせた合図を、そのまま自分の終わり方にする。受けた合図は子へ送る。子の出力を読み終えるまで終わらない（最後の行を落とさない）。

## 壊しうる入力

| 入力 | 扱い |
| --- | --- |
| 空の行（`\n\n`） | 各行に時刻を付ける（`<時刻> \n<時刻> \n`） |
| 改行の無い最後の行 | 時刻を付けて中身を書き、改行は足さない |
| `\n` で終わった直後に流れが終わる | 時刻だけの行を作らない |
| とても長い行（数 MB） | 溜めずに流す。時刻は行の頭に 1 つだけ |
| UTF-8 の多バイト文字、文字の途中での塊の切れ目 | 符号を解釈しないので、文字は壊れない |
| UTF-8 として正しくないバイト、NUL | そのまま通す |
| `\r\n`、`\r` だけ | `\n` だけを行の区切りとする。`\r` は中身として残す |
| 塊の切れ目で行が割れる（`ab` と `c\n`） | 時刻は最初の塊の `a` の前に 1 つだけ |
| 空の塊 | 何も書かず、時計を読まない |
| 時計が戻る | 戻った時刻をそのまま書く（I3） |
| 子が終了コード 0／0 以外で終わる | 同じ終了コードで終わる |
| ラッパーが SIGTERM を受ける | 子へ SIGTERM を送り、子の終わりを待ち、子の終わり方で終わる。launchd の `ExitTimeOut`（既定 20 秒）を過ぎれば launchd が SIGKILL する |
| 子が合図で終わる | 同じ合図で自分を終わらせる |
| 子を起こせない | 時刻を付けた 1 行を標準エラーに書き、127 で終わる |
| 書き出し先が詰まる | 子の流れを止めて待つ |

## Quint は使わない

ラッパーの状態は「行の頭か」の 1 ビットだけで、並行する遷移も台帳も無く、テストの性質（任意のバイト列と切れ目）で全体を確かめられるため。

## テスト（先に書いて、いまの main で落ちることを確かめる）

`frontend/web/test/scripts/`（`npm --prefix frontend/web run test` の対象）。

- `line-stamper.test.ts`：`createLineStamper` と `formatStamp` を直接呼ぶ。
  - 例：1 行、1 つの塊に複数行、空の行、改行の無い最後の行と続きの塊、多バイト文字の途中の切れ目、空の塊、行ごとに時計を 1 回読むこと、時計が戻る場合、2 本の流れが状態を共有しないこと、`formatStamp` の形。
  - 性質（種つきの乱数、除外なし）：任意のバイト列（0〜255 の全バイト、`\n`・`\r`・多バイト文字を多めに）を任意の切れ目で流し、任意の時刻（戻るものを含む）を返す時計を使っても、(1) 各行の頭の時刻を除いた出力が入力と一致し、(2) 中身のある行の数だけ時刻が付き、それぞれが時計の返した値を呼ばれた順に書いたもので、(3) 時計を呼ぶ回数は行の数と同じ。
- `with-timestamps.test.ts`：ラッパーを子プロセスとして起こす。
  - 標準出力と標準エラーの各行に時刻が付き、混ざらないこと。時刻はテストの前後の時刻の間にあること。
  - 子の任意のバイト列（種つき、多バイト文字と長い行を含む）が、時刻を除いて標準出力と標準エラーのそれぞれで一致すること。
  - 子の終了コード（0 と 0 以外）を引き継ぐこと。
  - SIGTERM を受けると子へ送り、子の終了コード（子が合図を受けて選んだ値）で終わること。SIGINT と SIGHUP も子へ送ること。
  - SIGTERM を送った子が合図で終わると、ラッパーも SIGTERM で終わり、その時点で子が残っていないこと。
  - 子が自分で合図を受けて終わると、ラッパーも同じ合図で終わり、それまでの行を落とさないこと。
  - 子を起こせないと 127 で終わり、標準エラーに時刻つきの行を書くこと。

## #413 の直し

対象の issue：[j5ik2o/takt-dash#413](https://github.com/j5ik2o/takt-dash/issues/413)（#186 のマージ後の点検の指摘）。変えるのは `frontend/web/scripts/with-timestamps.mjs` とこの文書だけ。`line-stamper.mjs`、時刻の形、plist（まだ入れていない）、既存のテストは変えない。

- 合図で終わった子（指摘 1）：子が合図 S（番号 n は `os.constants.signals[S]`）で終わったら、自分の受け手を外す。S が SIGPIPE・SIGXFSZ（Node が既定で無視する）か SIGUSR1（Node が Inspector を起こす）か、外したあとも S の受け手が残っている（`process.listenerCount(S) > 0`、親が無視している SIGHUP など）なら、自分へ送らず `process.exit(128 + n)` で終わる。それ以外は自分へ S を送り、同期に死ななければ `process.exit(128 + n)`（`unref` しない）。どの経路でも 0 では終わらない。
- 引数なし（指摘 2）：`writeStampedLine` は書き込み先の `error` を `once` で捕まえ、書き終わりか失敗のどちらかで解決する。引数なしは標準エラーが壊れていても常に 2。
- 起こせない（指摘 3）：`spawn` を `try` で包み、同期の例外（空の文字列など）も `error` と同じ扱いにする。起こせなければ、標準エラーへ時刻付きの 1 行（`<program を JSON の文字列にしたもの>: <理由>`。空でも `"": …` と分かる）を書き、127 で終わる。子の終わりは `error` と `close` の先に来た方で待ちを抜け（`error` だけで `close` が来なくても抜ける）、流れの転送の終わりを待ったあと、その時点までに `error` が来ていれば 127 とする（`close` が先でも見落とさない）。`forward` は source の `end` でも `close` でも終わる（先に来た方、2 回目は無視）。
- 書き込みの失敗の知らせ（指摘 4）：流れごとに「最初の失敗」の時点で、すぐもう一方の流れへ時刻付きの 1 行 `with-timestamps: <stdout|stderr> への書き込みに失敗しました: <message>` を書く。知らせは流れごとに 1 回だけ。もう一方も既に失敗していれば書かない（両方失敗なら何もしない）。子の終わりのあとの `reportForwardFailure` は無くす（二重に知らせない）。
- 反映の手順（指摘 5）：下の「反映の手順の追記」。

守る不変条件（I6 を言い直す）：

- I6'：ラッパーの終わり方は、子の終わり方の写し。子が終了コード c なら c。子が合図 S なら S か 128 + n。起こせなければ 127。引数なしは 2。合図で終わった子に対して 0 で終わる経路は無い。
- I7：ラッパーは自分へ SIGUSR1 を送らない（Inspector を起こさない。標準エラーに `Debugger listening` を出さない）。
- I8：書き込みの失敗の知らせは、流れごとに高々 1 回、失敗した流れと逆の流れにだけ、子の生死と関係なく失敗の直後に出る。
- I9：ラッパーは、どの経路でも時刻の無い行を標準エラーに書かない（捕まえていない例外のスタックを出さない）。

性質（除外なし）：合図 S を {SIGHUP, SIGINT, SIGTERM, SIGUSR1, SIGUSR2, SIGPIPE, SIGXFSZ, SIGKILL} の全部、親の状態を「受け手なし／SIGHUP の受け手あり」の両方で総当たりし、子（`/bin/sh -c 'kill -s S $$'`）が S で終わるとき、ラッパーは「S で終わる」か「128 + n で終わる」のどちらかで、標準エラーに `Debugger listening` を含まない。乱数でなく総当たりにするのは、領域が有限で小さいため。

壊しうる入力：空の文字列のプログラム、存在しないプログラム、実行の権限の無いファイル、閉じた標準エラー（引数なし・起こせないとき）、閉じた標準出力（常駐中）、両方が閉じている、子が Node の無視する合図で終わる、親が受け手を持つ合図で子が終わる。

## 状態遷移と Quint

状態は「子の生死（動いている／終了コード／合図／起こせない）」と「流れごとの終わり（読み中／終わり、失敗の知らせ済みか）」で、遷移は一方向に数段だけ、並行して書き換える台帳も再試行も無い。総当たりのテストで全部の組を確かめられるので、Quint は使わない。

## 反映の手順の追記（#413 の 5）

上の「反映のときの plist の変え方」の手順を、次のとおり補う。

- 2 の前に `plutil -p ~/Library/LaunchAgents/com.ideoplus.taktdash-web.plist` で `ProgramArguments` の 0 番目がいまの `<node>`、1 番目が `next` の本体であること（ラッパーがまだ入っていないこと）を確かめる。既に入っていれば `plutil -insert` を流さない（二重に入れない）。
- 3 の `bootstrap` は、`bootout` の直後に一時的に `Bootstrap failed: 5: Input/output error` で失敗することがある。`scripts/deploy-backend.sh` と同じく、1 秒おきに 10 回まで流し直す（`until launchctl bootstrap ...; do ...; sleep 1; done`）。10 回とも失敗したら手で流し直す。
- 戻すときも、`plutil -remove` の前に `plutil -p` で先頭の 2 つがラッパーの組であることを確かめ、起こし直しは同じ再試行で行う。

## #413 のテスト

`frontend/web/test/scripts/with-timestamps-413.test.ts`（既存の `with-timestamps.test.ts` は変えない）。いまの main で落ちることを確かめる。

- 子が SIGPIPE で終わる → 0 で終わらない（SIGPIPE か 128 + 13）。
- 子が SIGUSR1 で終わる → `Debugger listening` を出さず、0 で終わらない。
- 親に SIGHUP の受け手がある状態（`--import` で受け手を足す小さなスクリプト）で子が SIGHUP で終わる → 0 で終わらない。
- 上の性質の総当たり。
- 引数なしで標準エラーを閉じたパイプにする → 2。
- プログラムが空の文字列 → 127、標準エラーの全部の行が時刻で始まる。
- 常駐中に標準出力が閉じたパイプ → 子が動いている間に、標準エラーに時刻付きの知らせが 1 行だけ出る（待ちの上限は数秒）。
