# 接続断の出来事の記録と比較基準（#26）

`backend/src/poller.rs` の、接続断の出来事の記録と比較基準の進め方を Quint で写したモデルです。仕様とコードは機械的にはつながっていません。コードの回帰テストは `backend/tests/poller_test.rs` にあります。

- `pollerEventRecording.qnt`: モデル。`current` が修正前、`fixed` が修正後の遷移です。違いは定数 `HOLD_UNRECORDED_LOST` だけで、不変条件は同じです。
- `pollerEventRecording_test.qnt`: 筋書きを固定したテスト。

## モデルの範囲

1プロジェクトだけを扱います。遅延（Delayed）としきい値は省き、読み取りの失敗はすぐ応答なしへ跨ぎます。記録の照会の保留（`seed_pending`、`deferred_lost`）とプロセスの再起動は扱いません。

## 条件

不変条件 `safety` は次の3つの論理積です。

- `atMostOnce`: 接続断を、検出した回数より多く記録しない。読取先を作り直すと勘定を0へ戻すため、前の世代の接続断を次の世代で記録するとこの条件に反します。
- `restoredIsPaired`: 復帰は、記録済みの接続断に対応する分だけ記録する。
- `noLoss`: 未記録の接続断があるなら、保持している出来事か、再検出できる差分（基準が接続中で状態が応答なし）が残っている。

進行は `--witnesses recoveredAndRecorded` と、筋書きを固定したテストで確かめます。`quint run` は時相の性質を検査しないため、「復旧と再試行が進めば必ず1回記録される」ことの網羅的な確認ではありません。

## 結果（quint 0.32.0）

修正前は反例が出ます。

```
quint run pollerEventRecording.qnt --main current --invariant safety --max-samples 10000 --max-steps 30 --seed 26
[violation] Found an issue
```

反例の末尾は、コードで再現した不具合と同じ形です。記録先が失敗している間に接続断を検出し（`detected: 1`、`notified: Connected`、`store: Unresponsive`）、その後 takt-mcp が復旧した周期で `store: Connected` になります。記録する出来事が空なので記録は成功し、`recordedLost: 0`、`held: false` のまま、記録する手がかりが無くなります（`noLoss` に反する）。

修正後は、同じ不変条件と同じ引数で、10000 サンプル、max-steps 30、seed 26 で反例なしです。

```
quint run pollerEventRecording.qnt --main fixed --invariant safety --max-samples 10000 --max-steps 30 --seed 26 --witnesses recoveredAndRecorded held
[ok] No violation found
recoveredAndRecorded was witnessed in 1196 trace(s) out of 10000 explored (11.96%)
held was witnessed in 5604 trace(s) out of 10000 explored (56.04%)
```

テストは次のとおりです。

```
quint test pollerEventRecording_test.qnt --main currentTest   # 1 passing（修正前に0件で失われる筋書き）
quint test pollerEventRecording_test.qnt --main fixedTest     # 3 passing
```
