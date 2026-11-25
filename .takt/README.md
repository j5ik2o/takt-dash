# .takt の終了通知

TAKT のタスクが終わったことを Orca の担当ボット（`takt-dash中将`）へ知らせる仕組みと、
TAKT の版を戻すときの手順を書く。

> いまは `task_finish_hook` を外してある。理由と戻す条件は下の「いまの状態」に書く。
> ここから下の「仕組み」は、このキーを戻したときの動きになる。

## 仕組み

- `.takt/config.yaml` の `task_finish_hook` が、キューのタスクの最終状態（完了・失敗・中断・
  PR 失敗・工程の上限）を `.takt/tasks.yaml` に保存した後、`python3 .takt/scripts/notify_orca.py`
  を 1 回呼ぶ。作業ディレクトリはプロジェクトルート。`timeout_ms` は 15000（TAKT の上限は
  60000、スクリプトが orca を打ち切るのは 10 秒）。
- TAKT が渡すのは `PATH` などの少数の変数と `TAKT_FINISH_*` だけ。`HOME` や認証情報は渡らない。
  `~` を含むパスはスクリプトが自分で展開する。
- 宛先は `.takt/notify-orca.json` の `default_bot`（`takt-dash中将`）。指示文の
  `orca: bot=<名前> item=<番号>` の行、`map` が指す対応表のエントリの順に、先に見つかったものを
  使う。
- 結果は run のディレクトリの `notify-orca.log` に 1 行残る（run のディレクトリが無ければ
  `<プロジェクト>/.takt/notify-orca.log`）。送れたときは orca が返したメッセージ ID
  （`msg_...`）も同じ行に入る。失敗も同じファイルに 1 行残るので、届いたかどうかは行の中身で
  見る。orca の標準エラーは `notify-orca.stderr` に分けて残し、本文にも記録にも貼らない。
- 通知はタスクの終わりに 1 回だけ。ワークフローの工程からは送らない（`taktdash-change` に
  通知の工程は無い）。成功の通知を二重にしないため、工程を足すときは注意する。
- スクリプトはどんな失敗でも終了コード 0 で終わる。TAKT は終了コードで状態も再開位置も
  変えないので、通知の失敗でタスクの結果を変えない。
- 質問待ち・ログの停止・プロセスの消失は、終了の出口では扱わない。台帳の見張りが受け持つ。

## いまの状態

`task_finish_hook` は**いま外してある**（2026-11-25）。

理由は、動いている実体が PATH の **takt 0.77.0** で、このキーを知らないため（TAKT 本体の
`.takt/config.yaml` スキーマ、`src/core/models/config-schemas.ts`）。キーが残っていると、次に
誰かが TAKT を流した瞬間に
`Error: Configuration error: invalid (root) ... Unrecognized key: "task_finish_hook"` で設定を
読めず、全タスクが即失敗する。その地雷を先に消してある。

### 戻す条件

フォーク（`j5ik2o/takt`）の `integration/takt-mcp`（`task_finish_hook` を知る版）を実体として
配備してから、キーを戻す。順序は下の「古い TAKT へ戻すとき」の逆で、先にキーを戻し、そのあと
実際に送れたかを見る。

1. `.takt/config.yaml` に `task_finish_hook` のブロック（`command: [python3,
   .takt/scripts/notify_orca.py]` と `timeout_ms: 15000`）を書き戻す。
2. 次に終わるタスクで、run のディレクトリの `notify-orca.log` の最後の行が
   `送信した（msg_...）` になっていることを見る。失敗も同じファイルに 1 行残るので、行数だけでは
   送れたかを判断できない。

`.takt/scripts/notify_orca.py` と `.takt/notify-orca.json` は残してある。キーが無ければ呼ばれない
だけで、古い TAKT もこの 2 つを読まない。

台帳の見張り（`~/.orca/bots/takt-dash/state/takt-orca-notify.py`）は戻さない。`j5ik2o/takt#126`
と #355 の条件が生きているため。

下の「古い TAKT へ戻すとき」の手順は消さない。将来また版を戻すときに使う。

## 古い TAKT へ戻すとき

古い TAKT は `task_finish_hook` を知らず、このキーを設定の誤りとして扱う。キーを残したまま
版を戻すと、そのプロジェクトの設定を読めなくなる。TAKT の切替と戻しの手順そのものは、
フォーク（`~/local/takt-custom`）の `docs/shared-takt-switch.md` にある。このキーについての
順序は次のとおり。

1. **キーを消す。** `.takt/config.yaml` から `task_finish_hook` のブロックを消す。次に終わる
   タスクから呼ばれなくなる。manager や daemon の起動し直しは要らない。
2. **TAKT の版を戻す。** フォーク（`~/local/takt-custom`）の `docs/shared-takt-switch.md` の
   「戻し方」の順で行う（常駐するコマンドを終了 → `takt daemon stop` → リンクを戻す →
   `takt manager` を起動）。
3. **確認する。** `~/.takt/daemon.json` の `cliPath` が戻した版を指すことと、`takt list` で
   タスクの状態がこれまでどおり保存されることを見る。
4. **上げ直したらキーを戻す。** `task_finish_hook` のブロックを書き戻し、次に終わるタスクで
   `notify-orca.log` の最後の行が `送信した（msg_...）` になっていることを見る。失敗も同じ
   ファイルに 1 行残るので、行数だけでは送れたかを判断できない。

`notify_orca.py` と `notify-orca.json` は残してよい。キーが無ければ呼ばれないだけで、古い
TAKT もこの 2 つを読まない。

## 既知の制限

- SIGINT（Ctrl-C）で止めたタスクは、「中断」ではなく「失敗」で届く。止めたときの理由が
  `Workflow interrupted by user (SIGINT)` で、スクリプトが使う中断判定（理由の文言との照合）に
  当たらないため。文言に頼る判定の置き換えは #355 で扱う。

## 触らないもの

- 台帳の見張り（`~/.orca/bots/takt-dash/state/takt-orca-notify.py`）は、この通知とは別に
  動いている。このリポジトリの作業では触らない。以前の形に戻さない。
