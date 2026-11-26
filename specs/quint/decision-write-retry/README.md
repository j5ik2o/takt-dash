# 判断待ちの書き込みの再試行（issue #305、#395）

判断待ちの書き込み（回答、マージの承認、ゴール登録の承認と却下）で、takt 側では書き込みが済んでいるのに応答を失ったとき、同じ操作の再試行と同時の重複が書き込みを重ねないことを、Quint の仕様で確かめた記録です。issue #395 で、takt が isError ではない本文の無い応答を返す遷移を足しました（設計は `docs/design/issue-395.md`）。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/write_retry_test.rs`、`backend/src/grpc/write_ledger_tests.rs`、`backend/src/grpc/failure_tests.rs`、`frontend/web/test/api/decisions-write-retry.test.ts`、`frontend/web/test/state/decision-retry.test.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `writeRetry.qnt` | 状態機械、不変条件、進行の条件 |
| `writeRetry_test.qnt` | issue の筋書きを修正前と修正案でたどるテスト |

`writeRetry.qnt` は遷移を 3 つの定数で切り替えます。

| モジュール | `DEDUP` | `EXPIRY` | `EMPTY_REPLY_UNKNOWN` | 表すもの |
| --- | --- | --- | --- | --- |
| `current` | false | false | false | #305 の修正前。backend は届いた要求を毎回 takt へ送る |
| `fixed` | true | false | true | 修正案。要求は鍵を持ち、backend は鍵ごとの台帳を引く。本文の無い応答は結果不明 |
| `fixedWithExpiry` | true | true | true | 修正案で、台帳が決着した記録を忘れることを許したもの |
| `emptyReplyAsRejected` | true | false | false | #395 の修正前。台帳はあるが、本文の無い応答を適用されなかった失敗として扱う |

## モデルの範囲

- 利用者の試行は 4 回まで、論理操作（鍵）は `k1` と `k2` の 2 つです。
- 同じ鍵の試行は、同じ論理操作の再試行か同時送信を表します。修正前の backend は鍵を見ません。
- 期限切れは 2 種類あります。`callerTimeout` は Web や CLI が backend を待つのをやめること、`taktTimeout` は backend が takt を待つのをやめることです。
- takt は、backend が待つのをやめたあとでも書き込みを適用できます（`Orphaned` からの `taktApply`）。
- takt は、isError ではない本文の無い応答を返すことがあります（`taktEmptyReply`）。takt は断っていないので、適用の前（`Writing`）と後（`Unacked`）のどちらでも起こります。backend の `ToolCallError { refused: false }` に当たります。
- 書き込みゲートによる直列化、プロジェクトごとの違い、鍵と要求の内容の食い違いは含めていません。

## 条件

不変条件（安全性）

- `noDuplicateWrite`：同じ鍵の書き込みは、takt に 1 回までしか適用されない。
- `noDuplicateAfterLostResponse`：応答を失った試行がある鍵に限った `noDuplicateWrite`。反例が issue の筋書きになります。
- `rejectedMeansNotWritten`：「失敗した」と伝えた試行は、書き込みを適用していない。
- `okMeansWritten`：「成功した」と伝えた操作は、takt に適用されている。
- `safety`：`noDuplicateWrite`、`rejectedMeansNotWritten`、`okMeansWritten` をすべて満たす。

進行の条件（到達できること。`--witnesses` で数えます）

- `retryLearnsSuccess`：応答を失った操作を同じ鍵でやり直すと、適用済みの結果を受け取れる。
- `otherOperationProceeds`：ある操作の結果が不明でも、別の操作は適用できる。
- `rejectedOperationCanBeRetried`：拒否された操作は、同じ鍵でやり直して適用できる。

## 結果

quint 0.32.0 で実行しました。実行したのは `quint typecheck`、`quint run`、`quint test` だけです。数字は #395 で遷移を足したあとに取り直したものです。

### 修正前の遷移：反例あり

```sh
quint run writeRetry.qnt --main=current --invariant=noDuplicateAfterLostResponse \
  --max-steps=12 --max-samples=10000 --seed=0x426
```

12 手で `noDuplicateAfterLostResponse` に違反します。`safety` も同じ seed で違反します。反例の要点は次のとおりです。

1. 試行 2 が鍵 `k1` の書き込みを takt へ送る（`Writing`）。
2. 呼び出し側が試行 2 を待つのをやめる。利用者には結果が分からない（`SawUnknown`）。takt は要求を受け取ったままになる（`Orphaned`）。
3. 利用者が同じ操作を試行 0 としてやり直す。backend は試行を区別せず、takt へもう一度送る。
4. takt が試行 2 の書き込みを適用する（`applied[k1] = 1`）。
5. 呼び出し側が試行 0 を待つのもやめる。
6. takt が試行 0 の書き込みも適用する（`applied[k1] = 2`）。

### 修正案の遷移：反例なし

```sh
quint run writeRetry.qnt --main=fixed --invariant=safety \
  --witnesses retryLearnsSuccess otherOperationProceeds rejectedOperationCanBeRetried \
  --max-steps=20 --max-samples=50000 --seed=0x305
```

50000 サンプル、max-steps 20、seed 0x305 で `safety` の反例なし。`noDuplicateAfterLostResponse` も同じ条件で反例なし。不変条件は修正前の検査と同じものです。

trace の長さは最短 10、最長 19、平均 14.76 です。20 に届かない trace は、4 回の試行がすべて終わって進める遷移が無くなったものです。

進行の条件は次の数の trace で到達しました。

| 条件 | 到達した trace |
| --- | --- |
| `retryLearnsSuccess` | 3088 / 50000（6.18%） |
| `otherOperationProceeds` | 23786 / 50000（47.57%） |
| `rejectedOperationCanBeRetried` | 159 / 50000（0.32%） |

### #395 の修正前の遷移：反例あり

```sh
quint run writeRetry.qnt --main=emptyReplyAsRejected --invariant=rejectedMeansNotWritten \
  --max-steps=20 --max-samples=50000 --seed=0x395
quint run writeRetry.qnt --main=emptyReplyAsRejected --invariant=noDuplicateWrite \
  --max-steps=20 --max-samples=50000 --seed=0x395
```

どちらも 14 手で違反します。takt が書き込みを適用したあとに本文の無い応答を返すと、backend は「失敗した」と伝えて台帳の記録を消します（`rejectedMeansNotWritten` の違反）。同じ鍵のやり直しが takt へ送られ、書き込みが 2 回になります（`noDuplicateWrite` の違反）。

### 修正案の限界：台帳が記録を忘れると反例あり

```sh
quint run writeRetry.qnt --main=fixedWithExpiry --invariant=noDuplicateWrite \
  --max-steps=20 --max-samples=50000 --seed=0x434
```

17 手で `noDuplicateWrite` に違反します。結果不明の記録を台帳が忘れたあと、同じ鍵のやり直しが takt へ送られ、先の書き込みと重なります。

実装の台帳は backend のメモリにあり、決着した記録を 1 時間保ちます。backend を再起動するか 1 時間を過ぎると、この反例の状況になります。Web は、成功していない同じ操作のやり直しに、2 分のあいだ同じ鍵を使います。2 分を過ぎたやり直しとページを読み込み直したあとの操作は、新しい鍵を使う別の論理操作です。

### 筋書きテスト

```sh
quint test writeRetry_test.qnt --main=writeRetry_test
```

7 件すべて成功。`lostResponseRetryDuplicatesBeforeFixTest` は、#305 の修正前の遷移で書き込みが 2 回になることを固定します。`emptyReplyRetryDuplicatesBeforeFixTest` は #395 の修正前に本文の無い応答のあとで書き込みが 2 回になることを、`emptyReplyIsNotResentAfterFixTest` は修正案で結果不明として残り送り直さないことを固定します。
