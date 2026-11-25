# 判断待ちの書き込みと、判断待ちの一覧との照合（issue #319）

画面から送る書き込み（問い合わせへの回答、ゴール登録の承認と却下）を、backend が判断待ちの一覧と照合してから takt へ送ることを、Quint の仕様で確かめた記録です。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/tests/write_grpc_test.rs`、`backend/tests/write_retry_test.rs`、`frontend/web/test/api/decisions-routes.test.ts` が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `pendingCheck.qnt` | 状態機械、不変条件、進行の条件 |
| `pendingCheck_test.qnt` | issue の筋書きを修正前と修正案でたどるテスト |

`pendingCheck.qnt` は遷移を 2 つの定数で切り替えます。

| モジュール | `CHECK` | `TAKT_GUARD` | 表すもの |
| --- | --- | --- | --- |
| `current` | false | true | 修正前。backend は revision が正の数か、goal_id の形が正しいかだけを見る |
| `fixed` | true | true | 修正案。backend は状態ストアの判断待ちと照合し、食い違えば takt へ送らない |
| `fixedWithoutTaktGuard` | true | false | 修正案で、takt-mcp が照合しないと仮定したもの |

`TAKT_GUARD = true` は実物の takt-mcp の挙動です。承認と却下は、保留中の登録案の revision が要求と違えば `stale_summary` で断ります。回答は、指定したゴールにその問い合わせが無ければ `Question does not exist` で断ります。

## モデルの範囲

- 登場するのは takt、backend の状態ストア、画面です。状態ストアは定期読み取り（`poll`）で takt の値を写し、画面は描画（`render`）で状態ストアの値を写します。どちらも遅れます。
- 登録案の revision は 1 から 3 まで、1 プロジェクト分です。takt は作り直しのたびに番号を増やし、承認・却下のあとも戻しません（takt-custom の `conversationService.ts` の `nextRevision`）。モデルも同じで、同じ番号が別の登録案に振られることはありません。承認待ちの有無は `taktPending` で持ちます。状態ストアと画面の 0 は、承認待ちが無いことを表します。承認と却下は区別しません。
- 接続先の切替は含めていません。revision は会話（cwd）ごとに振られるため、設定で cwd や接続先を切り替えると、切替前の一覧の番号が、切替後の接続先の別の登録案の番号と重なりえます。実装は、登録案を読んだ接続先を一覧に持たせ、書き込み先と同じ接続先から読んだ一覧とだけ照合します（`backend/tests/write_grpc_test.rs` の `goal_registration_writes_reject_a_list_read_before_the_connection_changed`）。
- 照合と送信は 1 つの遷移（`decideRegistration`）です。実装は、書き込みゲートを取ってから照合し、定期読み取りは同じゲートを保持して一覧を反映するので、照合から送信までのあいだに一覧は変わりません（同じテストファイルの `goal_registration_approval_checks_the_list_after_taking_the_write_gate`）。
- 問い合わせは 1 件で、ゴール `g1` に属します。要求の goal_id は、無し、`g1`、`g2` のどれかです。
- 冪等キーと台帳（issue #305、`../decision-write-retry/`）、書き込みゲート、複数のプロジェクト、マージの承認（issue #112）は含めていません。

## 条件

不変条件（安全性）

- `sentRevisionIsListed`：backend が takt へ送る承認・却下は、その時点の判断待ちの一覧にある revision だけ。
- `sentAnswerGoesToListedGoal`：backend が takt へ送る回答は、判断待ちの一覧が示すゴールへのものだけ。
- `writesMatchPendingList`：上の 2 つをどちらも満たす。issue の受け入れ条件です。
- `appliedRevisionIsCurrent`：適用された承認・却下は、適用した時点の takt の登録案へのもの。

進行の条件（到達できること。`--witnesses` で数えます）

- `freshRegistrationApplied`：画面が最新なら、承認・却下は適用される。
- `correctAnswerSent`：正しい回答は takt へ届く。
- `staleWriteRejected`：backend が食い違いで断ることがある。

## 結果

quint 0.32.0 で実行しました。実行したのは `quint typecheck`、`quint run`、`quint test` だけです。

### 修正前の遷移：反例あり

```sh
quint run pendingCheck.qnt --main=current --invariant=writesMatchPendingList \
  --max-steps=12 --max-samples=10000 --seed=0x356
```

7 手で `writesMatchPendingList` に違反します。反例の要点は次のとおりです。

1. takt に登録案（revision 1）ができ、状態ストアが読み、画面が表示する。
2. takt で登録案が作り直され（revision 2）、状態ストアが読む。画面は revision 1 のまま。
3. 利用者が画面から承認する。backend は revision 1 を、一覧（revision 2）と照合せずに takt へ送る。

同じ遷移で `appliedRevisionIsCurrent` は、10000 サンプル、max-steps 12、seed 0x319 で反例なし。takt-mcp が自分の値と照合して断るためです。修正前でも、古い revision の承認が新しい登録案に適用されることは、このモデルでは起きません。

### 修正案の遷移：反例なし

```sh
quint run pendingCheck.qnt --main=fixed \
  --invariant="writesMatchPendingList and appliedRevisionIsCurrent" \
  --witnesses freshRegistrationApplied correctAnswerSent staleWriteRejected \
  --max-steps=20 --max-samples=50000 --seed=0x319
```

50000 サンプル、max-steps 20、seed 0x319 で反例なし。`writesMatchPendingList` は修正前の検査と同じものです。trace の長さはすべて 21 で、進める遷移が無くなって止まった trace はありません。

進行の条件は次の数の trace で到達しました。

| 条件 | 到達した trace |
| --- | --- |
| `freshRegistrationApplied` | 30215 / 50000（60.43%） |
| `correctAnswerSent` | 48823 / 50000（97.65%） |
| `staleWriteRejected` | 49602 / 50000（99.20%） |

### 修正案の限界：takt-mcp が照合しなければ反例あり

```sh
quint run pendingCheck.qnt --main=fixedWithoutTaktGuard --invariant=appliedRevisionIsCurrent \
  --max-steps=20 --max-samples=50000 --seed=0x3ea
```

`appliedRevisionIsCurrent` に違反します。状態ストアが takt より古いあいだ（takt は revision 2、状態ストアと画面は revision 1）、backend の照合は通り、revision 1 の承認が revision 2 の登録案に適用されます。

backend の照合は、状態ストアより古い画面からの書き込みを早く断るためのものです。状態ストアが takt より古いときの最終の判定は、takt-mcp の照合が受け持ちます。backend は、書き込みの前に takt を読み直しません。読み直しても、読み取りと書き込みのあいだに登録案が作り直されうるため、takt-mcp の照合の代わりにはなりません。

### 筋書きテスト

```sh
quint test pendingCheck_test.qnt --main=pendingCheck_test
```

7 件すべて成功。`staleRevisionIsSentBeforeFixTest` と `answerToAnotherGoalIsSentBeforeFixTest` は、修正前の遷移で食い違う書き込みが takt へ送られることを固定します。`approvedRevisionIsNotReusedTest` は、承認済みの revision を表示したままの画面からの承認が、あとからできた登録案に適用されないことを固定します。
