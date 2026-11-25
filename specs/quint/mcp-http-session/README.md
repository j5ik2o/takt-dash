# takt-mcp の HTTP セッションの寿命（issue #310）

backend が takt-mcp の HTTP サーバーに作るセッションが、画面の読み書きを続けても増え続けないことを、Quint の仕様で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/screen_http_test.rs`、`backend/tests/write_grpc_test.rs`、`backend/tests/http_read_test.rs` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `httpSession.qnt` | 状態機械、不変条件、進行の条件 |
| `httpSession_test.qnt` | issue の筋書きを修正前と修正案でたどるテスト |

`httpSession.qnt` は遷移を 2 つの定数で切り替えます。

| モジュール | `SHARED` | `CLASSIFY` | 表すもの |
| --- | --- | --- | --- |
| `current` | false | false | 修正前。要求ごとに initialize でセッションを作り、閉じない |
| `sharedOnly` | true | false | セッションの共有だけを入れた版（PR の最初の版） |
| `fixed` | true | true | 修正案。共有に加えて、要求が届いていない失敗を「送る前の失敗」として分ける |

## モデルの範囲

- 要求（画面の読み取りか書き込みの 1 回の RPC）は 6 件まで、takt-mcp の停止は 2 回までです。
- takt-mcp は、セッションに期限を持ちません。生きたセッションが消えるのは停止のときだけです。
- 未知セッション（404）を受けた要求は処理されておらず、セッションを 1 回だけ作り直します。slot を空けるのは、slot が自分の使ったセッションのままのときだけです。
- 道具を送る段の失敗は 2 種類あります。takt-mcp が止まっていて接続を張れない失敗（`call` の停止中の枝）は、要求が届いていません。応答を受け取れなかった失敗（`lostResponse`）は、要求が届いて処理されています。
- 読み取りは、どちらの失敗でも同じセッションで 1 回だけ送り直します。404 でセッションを作り直したあとは、送り直さずに失敗として返します。書き込みは送り直しません。どちらも slot には触れません。
- セッションの確立は 1 つの遷移です（実装は確立の間だけ鍵を持ち、同時に 2 つは作りません）。
- 次のものは含めていません。置き場の持ち主がいなくなったときと backend の停止時に送る DELETE（確立の途中なら終わるのを待つこと）、確立の途中で失敗したセッションへの DELETE、確立の鍵を待つ時間の上限、poller がプロジェクトごとに持つ読み取りハンドル、接続先の設定の変更、冪等キーの台帳（`specs/quint/decision-write-retry` が扱います）。

## 条件

不変条件（安全性）

- `sessionsDoNotGrowWithRequests`：takt-mcp に生きているセッションは、要求の数によらず 1 つまで。
- `onlySlotSessionLives`：生きているセッションは、slot のセッションだけ。
- `writeDeliveredAtMostOnce`：書き込みが takt-mcp に届くのは 1 回まで。
- `unknownMeansDelivered`：「結果は不明」と伝えた書き込みは、takt-mcp に届いている。
- `rejectedWriteNotDelivered`：「失敗した」と伝えた書き込みは、takt-mcp に届いていない。
- `readDeliveredAtMostThreeTimes`：1 件の読み取りが takt-mcp に届くのは 3 回まで（最初の送信、送り直し、作り直したあとの再試行）。
- `safety`：上の 6 つをすべて満たす。

進行の条件（到達できること。`--witnesses` で数えます）

- `reconnectsAfterRestart`：停止と起動のあと、404 を受けた要求が作り直して成功する。
- `sessionIsReused`：2 つ以上の要求が同じセッションで成功する。
- `readSurvivesConnectionFailure`：送り直した読み取りが成功する。
- `writeRejectedWhileDown`：届いていない書き込みが、失敗として返る。
- `writeUnknownAfterLostResponse`：応答を失った書き込みが、結果不明として返る。
- `allRequestsSettle`：6 件の要求がすべて終わる。

## 結果

quint 0.32.0 で実行しました。実行したのは `quint typecheck`、`quint run`、`quint test` だけです。

### 修正前の遷移：セッションが増える反例あり

```sh
quint run httpSession.qnt --main=current --invariant=sessionsDoNotGrowWithRequests \
  --max-steps=15 --max-samples=10000 --seed=0x310
```

12 手で `sessionsDoNotGrowWithRequests` に違反します。故障は要りません。2 件の要求がそれぞれ initialize でセッションを作ると、生きたセッションが 2 つになります。作ったセッションを閉じる遷移は無いので、要求が終わっても残ります（筋書きテストの `screenReadsLeaveSessionsBeforeFixTest`）。

同じ条件で `unknownMeansDelivered` も 9 手で違反します。修正前でも、確立のあと道具を送る前に takt-mcp が止まると、届いていない書き込みが結果不明になります。

### 共有だけの版：届いていない書き込みが結果不明になる反例あり

```sh
quint run httpSession.qnt --main=sharedOnly --invariant=unknownMeansDelivered \
  --max-steps=15 --max-samples=10000 --seed=0x310
```

7 手で `unknownMeansDelivered` に違反します。`sessionsDoNotGrowWithRequests` は、同じ条件（10000 サンプル、max-steps 15、seed 0x310）で反例なしです。

共有だけの版では、slot に確立済みのセッションがあると、書き込みの前の接続の確認は何も送らずに通ります。takt-mcp が止まっていると、道具を送る段で初めて接続の失敗が分かり、届いていない書き込みが結果不明になります。修正前は停止が確立と送信の間に挟まったときだけでしたが、共有だけの版では、止まっている間の書き込みがすべてこうなります（筋書きテストの `unsentWriteBecomesUnknownWithoutClassificationTest`）。

### 修正案の遷移：反例なし

```sh
quint run httpSession.qnt --main=fixed --invariant=safety \
  --witnesses reconnectsAfterRestart sessionIsReused readSurvivesConnectionFailure \
    writeRejectedWhileDown writeUnknownAfterLostResponse allRequestsSettle \
  --max-steps=30 --max-samples=50000 --seed=0x310
```

50000 サンプル、max-steps 30、seed 0x310 で `safety` の反例なし。不変条件は、修正前と共有だけの版の検査と同じものです。

trace の長さは最短 17、最長 31、平均 24.97 です。30 に届かない trace は、6 件の要求がすべて終わり、停止の回数も使い切って、進める遷移が無くなったものです。

進行の条件は次の数の trace で到達しました。

| 条件 | 到達した trace |
| --- | --- |
| `reconnectsAfterRestart` | 11076 / 50000（22.15%） |
| `sessionIsReused` | 45777 / 50000（91.55%） |
| `readSurvivesConnectionFailure` | 23360 / 50000（46.72%） |
| `writeRejectedWhileDown` | 10764 / 50000（21.53%） |
| `writeUnknownAfterLostResponse` | 39103 / 50000（78.21%） |
| `allRequestsSettle` | 48533 / 50000（97.07%） |

### 筋書きテスト

```sh
quint test httpSession_test.qnt --main=httpSession_test
```

10 件すべて成功。`screenReadsLeaveSessionsBeforeFixTest` は、修正前の遷移で 3 回の読み取りが 3 つのセッションを残すことを固定します。`unsentWriteBecomesUnknownWithoutClassificationTest` は、共有だけの版で、届いていない書き込みが結果不明になることを固定します。

## 実物で確かめたこと

配備済みの takt-mcp（`serverHttpTransport`）を、隔離したホームと別のポートで起動し、initialize だけを繰り返しました。2000 個のセッションを作ったあとも最初のセッションは 200 を返し、期限では消えません。`DELETE` と `Mcp-Session-Id` を送ると 200 が返り、そのセッションへの次の要求は 404 になります。
