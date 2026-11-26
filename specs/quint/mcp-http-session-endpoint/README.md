# 接続先が変わった直後の HTTP セッションの置き場（[issue #364](https://github.com/j5ik2o/takt-dash/issues/364) の 2 点目）

backend のレジストリが常駐 HTTP サーバーごとに持つセッションの置き場（slot）が、接続先（URL とトークン）を変えた直後に、旧設定で解決済みの要求によって旧接続先へ戻らないことを、Quint の仕様で確かめた記録です。セッションの寿命そのもの（共有、404 での作り直し、届いたかどうかの分け方）は `specs/quint/mcp-http-session/` が扱います。

仕様とコードは機械的にはつながっていません。コードの挙動は `backend/src/mcp/registry.rs` のテスト（性質のテストを含む）が確かめます。

## ファイル

| ファイル | 内容 |
| --- | --- |
| `sessionEndpoint.qnt` | 状態機械、不変条件、進行の条件 |
| `sessionEndpoint_test.qnt` | issue の筋書きを修正前と修正案でたどるテストと、到達条件が意図した筋書きだけを数えることのテスト（10 件） |

`sessionEndpoint.qnt` は遷移を 1 つの定数で切り替えます。

| モジュール | `MATCH_ENDPOINT` | 表すもの |
| --- | --- | --- |
| `current` | false | 修正前。`http_session` は、反映済みの設定を ID だけで照合する（`http_servers` は ID の集合）。接続先が違う置き場は、要求の接続先で作り直して slot を置き換える |
| `fixed` | true | 修正案。反映済みの設定を ID と接続先で照合する。要求の接続先が反映済みの接続先と違えば、その要求だけが使うセッションを渡し、slot には触れない |

## 仕様とコードの対応

| 仕様の要素 | コード |
| --- | --- |
| `file` | 設定ファイル。gRPC の要求は `backend/src/grpc/resolve.rs` の `load_config` で、要求ごとに読む |
| `resolve` | `backend/src/grpc/resolve.rs` の接続先の解決（要求が届いた時点の設定） |
| `reflect` | `backend/src/poller.rs` の `sync_clients` → `McpClientRegistry::retain_configured` |
| `reflected` | `RegistryInner` の反映済みの HTTP の接続先（修正前は ID の集合 `http_servers`） |
| `acquire` | `McpClientRegistry::http_session` |
| `finish` と `closeUnowned` | 要求が `HttpSession` を手放す。最後の持ち主が消えると `SessionInner` の `Drop` が DELETE を送る |

## モデルの範囲

- 常駐 HTTP サーバーは 1 台（ID は 1 つ）、接続先は 2 種類と「設定から消えた」、要求は 4 件、設定ファイルの書き換えは 2 回までです。
- セッションは置き場を作った時点で takt-mcp に残るものとします（実装では最初の要求で確立します。確立の細部は `mcp-http-session` が扱います）。
- 次のものは含めていません。確立・404・再試行、stdio のレーン（`client_for_key`）、定期読み取りのプロジェクトごとの読み取りハンドル（`poller.rs` の `sync_reads` は接続先まで比べています）、backend の停止（`specs/quint/backend-shutdown-deadline/` が扱います）。

## 条件

不変条件（安全性）

- `slotMatchesReflected`：反映済みの設定があれば、slot は反映済みの接続先のセッションだけ。
- `noOrphanSession`：takt-mcp に残っているセッションは、slot か、置き場を持つ要求のものだけ。
- `requestUsesItsEndpoint`：置き場を持つ要求は、自分が解決した接続先のセッションを使う。
- `slotSessionIsLive`：slot に登録されたセッションは takt-mcp に残っている。
- `safety`：上の 4 つをすべて満たす。

進行の条件（到達できること。`--witnesses` で数えます）

- `staleRequestServed`：旧設定の要求が、反映のあとに（反映済みの接続先と違う接続先のまま）置き場を得て終わる（拒まずに処理する）。置き場を求めた時点の不一致を要求に記録して数えるので、設定を変える前に終わった要求は数えない。
- `slotShared`：2 つの要求が同じ slot のセッションを共有している。
- `slotMovedToNewEndpoint`：接続先の変更が反映され、新しい接続先の slot ができる。
- `oldSlotClosedOnReflect`：反映で旧接続先の slot を手放し、そのセッションを閉じる。反映が手放したセッションを記録して数えるので、旧設定の要求だけのセッションを閉じても数えない。

## 結果

quint 0.32.0 で実行しました。実行したのは `quint typecheck`、`quint run`、`quint test` だけです。

### 修正前の遷移：旧接続先の置き場が戻る反例あり

```sh
quint run sessionEndpoint.qnt --main=current --invariant=slotMatchesReflected \
  --max-steps=15 --max-samples=10000 --seed=0x364
```

8 手で `slotMatchesReflected` に違反します。接続先 1 で解決済みの要求が、接続先 2（または設定から外れる前の 2）の反映のあとに置き場を求め、接続先 1 の置き場を作って slot を置き換えます（筋書きテストの `staleRequestReregistersOldEndpointBeforeFixTest`）。

ほかの 3 つの不変条件は、修正前でも反例なしです（max-steps 25、50000 サンプル、seed 0x364）。修正前の問題は、セッションが残り続けることではなく、slot が新しい接続先を指さなくなることです。追い出された置き場は持ち主が消えると閉じます。

### 修正案の遷移：反例なし

```sh
quint run sessionEndpoint.qnt --main=fixed --invariant=safety \
  --witnesses staleRequestServed slotShared slotMovedToNewEndpoint oldSlotClosedOnReflect \
  --max-steps=25 --max-samples=50000 --seed=0x364
```

50000 サンプル、max-steps 25、seed 0x364 で `safety` の反例なし。反映（`reflect`）はいつでも起こせるので、trace はすべて 26 状態です。

| 条件 | 到達した trace |
| --- | --- |
| `staleRequestServed` | 18893 / 50000（37.79%） |
| `slotShared` | 24691 / 50000（49.38%） |
| `slotMovedToNewEndpoint` | 15553 / 50000（31.11%） |
| `oldSlotClosedOnReflect` | 15097 / 50000（30.19%） |

### 筋書きテスト

```sh
quint test sessionEndpoint_test.qnt --main=sessionEndpoint_test
```

10 件すべて成功。`newConfigRequestBeforeReflectionIsEphemeralTest` は、修正案の代わりに払うもの（反映より先に新しい設定で解決した要求は、次の反映までその要求だけのセッションを使う）を固定します。
