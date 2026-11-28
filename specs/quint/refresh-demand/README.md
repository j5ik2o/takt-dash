# 画面が求める更新間隔の登録簿（issue #40）

backend の `RefreshDemand`（予定：`backend/src/refresh.rs`）が、画面ごとに 1 つの要求だけを持ち、閉じた・置き換えられた・寿命の切れた要求を残さないこと、各接続が送る `applied_refresh_interval_ms` が予定した送信間隔と一致し、寿命の切れた要求では送らないこと、poller が期限前の要求の最小値で読むことを Quint で確かめた記録です。設計は `docs/design/issue-40.md` の「Quint」の節です。

仕様とコードは機械的にはつながっていません。実装の挙動は、設計の「テストにできる性質」の表にある `backend/tests/refresh_demand_test.rs`（PBT を含む）、`backend/tests/watch_state_test.rs`、`backend/tests/poller_test.rs`、`frontend/web/test/components/live-provider-wiring.test.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `refreshDemand.qnt` | 画面 2 つ・接続 6 本・注入した時計の状態機械、10 の安全不変条件（`safety`）、14 の遷移の発火の記録と 8 つの到達条件（witness） |
| `refreshDemand_test.qnt` | 設計の案と変異の筋書きをたどる 21 件のテスト |
| `mutants.sh` | 変異を流し直す道具（下の「変異」）。CI には載せず、仕様を直したときに手元で流す |

遷移は 11 の定数で切り替えます。`fixed` はすべて `true`、ほかのモジュールは 1 つだけ `false` にした変異です。

| モジュール | `false` にした定数 | 表すもの |
| --- | --- | --- |
| `fixed` | なし | 設計の案 |
| `acquireEarly` | `ACQUIRE_AFTER_INITIAL` | 接続の受け付けで要求を登録し、初期の組み立ての間に寿命が切れうる |
| `noRejectStale` | `REJECT_STALE` | 遅れて届いた古い試行が新しい要求を置き換える |
| `keepSuperseded` | `CLOSE_SUPERSEDED` | 置き換えた古い接続の送信タスクを閉じない |
| `noOwnerCheck` | `OWNER_CHECK` | 古い guard の解放が、置き換え後の新しい要求を消す |
| `leakOnClose` | `RELEASE_ON_CLOSE` | 接続が終わっても guard を落とさない |
| `lazyExpiry` | `EXPIRY_URGENT` | 寿命の期限を時計が素通りする（期限を周期の掃除でしか見ない） |
| `expiryKeepsTask` | `EXPIRY_CLOSES` | 寿命切れで登録を外すだけで、送信タスクは送り続ける |
| `noPostAssemblyCheck` | `CHECK_AFTER_ASSEMBLY` | 定期の組み立ての後に寿命を確かめず、期限を過ぎた要求で `applied` を付けて送り、`touch` で寿命を延ばし直す |
| `fixedAtStart` | `RECOMPUTE` | 適用間隔を接続の開始で固定し、`minReadIntervalMs` の変更が開いている接続に効かない |
| `noRewake` | `POLLER_REWAKE` | poller が前回の読み取りで決めた待ちのまま、要求の増加で起きない |
| `resetAttempts` | `SHARED_ATTEMPTS` | `openStateStream` を呼ぶたびに試行番号を 1 から数え直す（今の `createAttemptUrls`） |

## モデルの範囲と前提

- 時計は注入した `now` だけです（1 目盛り = 1 秒、`tick` は 1・4・15 秒）。要求の寿命と poller の次の読み取りは素通りできません。`tick` は最も近い期限で止まり、期限の来た遷移（`expire`、`pollerRead`）が先に起きます。実装の `tokio::select!` の `sleep_until` を表します。
- ただし、定期の送信の組み立て中（`beginSend` から `deliver` まで）は、その送信タスクが `select!` の外にいるので、その接続の寿命の期限で時計は止まらず、`expire` も起きません。組み立ての後の `deliver` が寿命を確かめます。
- 間隔は秒で、選択肢は 1・2・5・10 秒、`minReadIntervalMs` は 1・3・10 秒、`intervalSeconds` は 2・30 秒（2 秒は下限より短い設定）、寿命は `max(3 × 適用間隔, 15 秒)`、画面の見張りは 40 秒です。
- poller の実効間隔は、期限が `now` より後の登録だけを数えます（期限ちょうどの登録は切れたものとして数えない）。
- 要求を出す接続だけを表します。`refresh_rate` を付けない接続（古い画面、古い BFF、不正な `refresh_ms`）は登録簿に触れないので含めていません。
- Next の登録簿（`screen-streams.ts`）と backend の登録簿は同じ規則（同じ画面のもっと大きい試行が登録済みなら断る）なので、backend の 1 つで代表させています。断る時点は、Next は受け付け、backend は初期状態の送信の後です。
- 画面が接続を閉じたこと（`clientClosed`）は backend にすぐには届きません。届くのが `releaseArrives`（`sender.closed()`）です。閉じた画面の要求が遅れて届き、登録されることもあります。その要求は `releaseArrives` か寿命で外れます。
- 置き換えは、新しい試行の登録の時点で古い接続を閉じます。実装は、古い送信タスクが組み立て中なら組み立ての後に置き換えを確かめて閉じます。どちらでも古い接続は置き換えの後に送りません。
- 送信の詰まり（`stall`）は戻りません。遅れて終わる送信は、遅れて起きる `beginSend`・`deliver` で表します（送信は急ぎの遷移ではない）。
- 変化の合流（間隔の中で何度変化が来ても最後の状態を 1 回だけ送る）、`DashboardState` の中身、組み立ての失敗、停止の合図、poller の読み取りにかかる時間は含めていません。設計の P3（送信スケジュールの PBT）と `watch_state_test.rs`、設計の「負荷の合格線」が受け持ちます。
- 接続を使い切ると（6 本）`connect` が止まり、時刻は 200 秒で止まります。

## 条件

不変条件（安全性）

- `liveConnsRegistered`：生きている接続は、どれもその画面の登録の持ち主である。
- `registryPointsToOpen`：登録は、閉じていない接続だけを指す。
- `supersededOnlyByNewer`：登録を置き換えるのは、同じ画面のもっと新しい試行だけ。
- `latestNeverRejected`：画面の最新の試行は断られない（204 で EventSource が固まらない）。
- `noOverdueEntry`：寿命を過ぎた登録が残るのは、その送信タスクが組み立て中の間だけ。
- `noExpiryBeforeFirstSend`：寿命切れは、初期状態を送った接続にしか起きない。
- `appliedTracksFloor`：今の下限の世代で送った状態の `applied` は max(要求, 今の下限)。
- `pollerNotLate`：poller は、前回の読み取りから実効間隔と、要求が最後に変わった時点の遅いほうを越えて待たない。
- `closedReleasedGuard`：閉じた接続のうち guard を取ったものは、guard を落としている。
- `noSendAfterExpiry`：寿命の期限を過ぎた（poller がもう数えていない）要求で、`applied` を付けて送らない。
- `safety`：上の 10 をすべて満たす。

到達条件（`--witnesses` で数えます）

- `firedConnect` … `firedTick`：14 の遷移がそれぞれ少なくとも 1 回起きる。
- `witnessSuperseded`、`witnessRejectedStale`、`witnessExpiredAfterStall`、`witnessExpiredAfterAssembly`、`witnessFloorRaised`、`witnessOneSecondPoller`、`witnessTwoScreensMin`、`witnessConfigFasterThanFloor`。

## 結果

quint 0.32.0 で実行しました。

```sh
cd specs/quint/refresh-demand
quint typecheck refreshDemand.qnt
quint typecheck refreshDemand_test.qnt
quint test refreshDemand_test.qnt --main=refreshDemand_test
```

- `quint typecheck`：2 ファイルともエラーなし。
- `quint test`：21 件すべて成功。

設計の案は反例なし:

```sh
quint run refreshDemand.qnt --main=fixed --invariant=safety --max-steps=150 --max-samples=10000
```

`--seed=0x40` で 10,000 サンプル・150 ステップで反例なし（quint の「Trace length statistics」は max=151・min=151 で、すべての跡が 151 状態まで伸びました）。seed を固定しない 200,000 サンプル × 5 回（合わせて 100 万本、150 ステップ）でも反例なし。同じ条件で `--witnesses` に 22 の到達条件を渡すと、すべて 1% を超えて到達しました（いちばん薄いのは `witnessExpiredAfterAssembly` 208 件（2.08%）、`witnessRejectedStale` 671 件、`witnessExpiredAfterStall` 916 件、`firedDeliver` 918 件、`firedExpire` 1,291 件）。`witnessExpiredAfterAssembly` は `deliver` の閉じる分岐だけを数えます（`expire` は組み立て中に起きないので、`Expired` で `assembling` のままの接続はこの分岐からしか来ません）。

寿命切れの跡は手数で増えます。1 秒の要求の下では `tick` が 1 秒ずつしか進まず、寿命の下限 15 秒に届くまで約 30 ステップかかるためです。同じ seed・10,000 本で、手数 60 では `firedExpire` 99 件・`witnessExpiredAfterStall` 56 件、100 では `witnessExpiredAfterAssembly` が 87 件（0.87%）で 1% を切りました。150 では 208 件（2.08%）、200 では 258 件（2.58%）です。そこで手数を 150 にし、CI（`specs/quint/ci-specs.json`）の `max_steps` も 150 にしました（`fixed` の 10,000 本は手元で約 7 秒。100 手で約 6 秒）。

### 変異

変異は `mutants.sh` で流し直せます（リポジトリの根から）。

```sh
specs/quint/refresh-demand/mutants.sh           # 下の表の 16 組、原本の safety、resetAttemptsRejectsLatestTest（約 1 分半）
specs/quint/refresh-demand/mutants.sh --matrix  # 変異 11 × 不変条件 10 の 110 回。表に無い違反が出ても失敗する
```

既定は `--max-steps=150 --max-samples=20000 --seed=0x40`（`STEPS`・`SAMPLES`・`SEED` で変えられます）。違反した組は、再現の seed を出します（`--seed=<値> --max-samples=1 --backend=rust` で 1 本目に違反が出ます）。`resetAttemptsRejectsLatestTest` は、`quint test --match` が何も選ばなくても 0 で終わるので、そのテストが 1 件だけ流れて通ったことも確かめます。

`--matrix` の結果（150 手。負荷の高い手元で約 1 時間半）：違反した組は次の表と過不足なく一致し、表に無い違反はありませんでした。10 の不変条件は、どれも少なくとも 1 つの変異で違反し、11 の変異は、どれも少なくとも 1 つの不変条件に違反します。原本の `safety` は違反なし。

| モジュール | 違反する不変条件 |
| --- | --- |
| `acquireEarly` | `noExpiryBeforeFirstSend`、`liveConnsRegistered` |
| `noRejectStale` | `supersededOnlyByNewer` |
| `keepSuperseded` | `liveConnsRegistered` |
| `noOwnerCheck` | `liveConnsRegistered` |
| `leakOnClose` | `registryPointsToOpen`、`closedReleasedGuard` |
| `lazyExpiry` | `noOverdueEntry` |
| `expiryKeepsTask` | `liveConnsRegistered` |
| `noPostAssemblyCheck` | `noSendAfterExpiry`、`pollerNotLate`（期限後の送信の `touch` が要求を生き返らせ、poller が通知なしに速くなる） |
| `fixedAtStart` | `appliedTracksFloor` |
| `noRewake` | `pollerNotLate` |
| `resetAttempts` | `supersededOnlyByNewer`（同じ番号の試行どうしが置き換え合う）。`latestNeverRejected` は無作為の探索では見つかりません（150 ステップ・20,000 サンプル、前の版の 100 ステップ・100,000 サンプル）。決まった手順の `resetAttemptsRejectsLatestTest` で違反を確かめます |

### 4 つの確かめ（元帥の決まり）

Quint が通ったことだけを合格にしません。設計の「Quint」の節に、次の 4 つを書いています。

- (a) 不変条件が恒真でない：上の「変異」。
- (b) 各 action が少なくとも 1 回起きる：上の witness（14 の遷移と 8 つの意味のある状態が、どれも 1% を超える）。
- (c) 状態・action と本番のコードの対応：設計の対応表。古い試行は handler が `Err(Status::aborted)` を返す形、組み立てを止める試験の口 `with_state_assembler` を、3 回目の読みの後に書き足しました。
- (d) 検査の大きさと、それで見られないもの：設計の一覧。「反例が見つからなかった」のは上の大きさでサンプルした跡の中だけで、「成り立つ」とは言えません。

モデルと本番のコードの対応表、見られないものの一覧は、設計の「Quint」の節の (c)・(d) にあります。

## 仕様を直すとき

`RefreshDemand`、WatchState の送信の間隔と寿命、poller の待ち、画面の試行番号のどれかを変えるときは、先にこの仕様を直して `quint test` と `quint run` と `mutants.sh` を流し、そのあとでコードを変えます。
