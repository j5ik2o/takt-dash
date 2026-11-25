# takt-mcp の HTTP セッションの寿命（issue #310）

backend が takt-mcp の HTTP サーバーに作るセッションが、画面の読み書きを続けても増え続けないことを、Quint の仕様で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/screen_http_test.rs`、`backend/tests/write_grpc_test.rs`、`backend/tests/http_read_test.rs` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `httpSession.qnt` | 状態機械、不変条件、進行の条件 |
| `httpSession_test.qnt` | issue の筋書きを修正前と修正案でたどるテスト |

`httpSession.qnt` は遷移を定数 `SHARED` で切り替えます。

| モジュール | `SHARED` | 表すもの |
| --- | --- | --- |
| `current` | false | 修正前。要求ごとに initialize でセッションを作り、閉じない |
| `fixed` | true | 修正案。接続先ごとの置き場（slot）のセッションを使い回す |

## モデルの範囲

- 要求（画面の読み取りか書き込みの 1 回の RPC）は 6 件まで、takt-mcp の再起動は 2 回までです。
- takt-mcp は、セッションに期限を持ちません。生きたセッションが消えるのは再起動のときだけです。
- 未知セッション（404）を受けた要求は、セッションを 1 回だけ作り直します。slot を空けるのは、slot が自分の使ったセッションのままのときだけです。
- 接続の失敗（I/O エラー）では、読み取りは同じセッションで 1 回だけ送り直し、書き込みは送り直しません。どちらも slot には触れません。
- セッションの確立は 1 つの遷移です（実装は確立の間だけ鍵を持ち、同時に 2 つは作りません）。
- 次のものは含めていません。置き場の持ち主がいなくなったときと backend の停止時に送る DELETE、確立の途中で失敗したセッションへの DELETE、poller がプロジェクトごとに持つ読み取りハンドル、接続先の設定の変更。

## 条件

不変条件（安全性）

- `sessionsDoNotGrowWithRequests`：takt-mcp に生きているセッションは、要求の数によらず 1 つまで。
- `onlySlotSessionLives`：生きているセッションは、slot のセッションだけ。
- `writesAreNotResent`：書き込みは、接続の失敗のあとで送り直さない。
- `safety`：上の 3 つをすべて満たす。

進行の条件（到達できること。`--witnesses` で数えます）

- `reconnectsAfterRestart`：再起動のあと、404 を受けた要求が作り直して成功する。
- `sessionIsReused`：2 つ以上の要求が同じセッションで成功する。
- `readSurvivesIoError`：接続の失敗で送り直した読み取りが成功する。
- `allRequestsSettle`：6 件の要求がすべて成功か失敗で終わる。

## 結果

quint 0.32.0 で実行しました。実行したのは `quint typecheck`、`quint run`、`quint test` だけです。

### 修正前の遷移：反例あり

```sh
quint run httpSession.qnt --main=current --invariant=sessionsDoNotGrowWithRequests \
  --max-steps=12 --max-samples=10000 --seed=0x310
```

5 手で `sessionsDoNotGrowWithRequests` に違反します。故障も再起動も要りません。2 件の要求がそれぞれ initialize でセッションを作り、生きたセッションが 2 つになります。作ったセッションを閉じる遷移は無いので、要求が終わっても残ります（筋書きテストの `screenReadsLeaveSessionsBeforeFixTest`）。

### 修正案の遷移：反例なし

```sh
quint run httpSession.qnt --main=fixed --invariant=safety \
  --witnesses reconnectsAfterRestart sessionIsReused readSurvivesIoError allRequestsSettle \
  --max-steps=30 --max-samples=50000 --seed=0x310
```

50000 サンプル、max-steps 30、seed 0x310 で `safety` の反例なし。`sessionsDoNotGrowWithRequests` は修正前の検査と同じものです。

trace の長さは最短 21、最長 31、平均 23.45 です。30 に届かない trace は、6 件の要求がすべて終わり、再起動の回数も使い切って、進める遷移が無くなったものです。

進行の条件は次の数の trace で到達しました。

| 条件 | 到達した trace |
| --- | --- |
| `reconnectsAfterRestart` | 9466 / 50000（18.93%） |
| `sessionIsReused` | 46956 / 50000（93.91%） |
| `readSurvivesIoError` | 27419 / 50000（54.84%） |
| `allRequestsSettle` | 49649 / 50000（99.30%） |

### 筋書きテスト

```sh
quint test httpSession_test.qnt --main=httpSession_test
```

6 件すべて成功。`screenReadsLeaveSessionsBeforeFixTest` は、修正前の遷移で 3 回の読み取りが 3 つのセッションを残すことを固定します。

## 実物で確かめたこと

配備済みの takt-mcp（`serverHttpTransport`）を、隔離したホームと別のポートで起動し、initialize だけを繰り返しました。2000 個のセッションを作ったあとも最初のセッションは 200 を返し、期限では消えません。`DELETE` と `Mcp-Session-Id` を送ると 200 が返り、そのセッションへの次の要求は 404 になります。
