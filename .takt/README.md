# .takt の終了通知

TAKT のタスクが終わったことを Orca の担当ボット（`takt-dash中将`）へ知らせる仕組みと、
TAKT の版を戻すときの手順を書く。

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

## 版を替えたあとに確かめる（任意）

`make workflow-inspect`（`tools/workflow-inspect/workflow_inspect.py`）は `.takt/workflows`
の各 workflow に `takt workflow inspect` を流し、解決できない `workflow_call` を名指しで
出す（`takt workflow doctor` は同じ状態でも `Workflow OK` と言う）。TAKT を流す前に人が
確かめる入口で、CI と `make done` には入れていない（CI に takt が無いため）。takt が無けれ
ば検査をスキップし、名前を渡すと 1 件だけ見る（`make workflow-inspect WORKFLOW=taktdash-change`）。
使う takt は `TAKT_BIN` で指定でき、既定は PATH 上の `takt`。

## 既知の制限

- SIGINT（Ctrl-C）で止めたタスクは、「中断」ではなく「失敗」で届く。止めたときの理由が
  `Workflow interrupted by user (SIGINT)` で、スクリプトが使う中断判定（理由の文言との照合）に
  当たらないため。文言に頼る判定の置き換えは #355 で扱う。

## 触らないもの

- 台帳の見張り（`~/.orca/bots/takt-dash/state/takt-orca-notify.py`）は、この通知とは別に
  動いている。このリポジトリの作業では触らない。以前の形に戻さない。
