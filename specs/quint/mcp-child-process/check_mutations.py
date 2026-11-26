#!/usr/bin/env python3
"""childProcess.qnt の守りを 1 つずつ外した写しを作り、対応する不変条件が違反を出すかを確かめる。

使い方（このディレクトリで）: python3 check_mutations.py
元の childProcess.qnt は書き換えません。写しは一時ディレクトリに作ります。
"""

import pathlib
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
SPEC = (HERE / "childProcess.qnt").read_text()
RUN_ARGS = ["--main=current", "--max-steps=35", "--max-samples=10000", "--seed=0x379"]

# (名前, 外す守り, 置き換え前, 置き換え後, 何番目の出現か, 違反を期待する不変条件)
MUTATIONS = [
  (
    "spawn-cancel-no-kill",
    "initialize 中の打ち切りで spawn() のローカル process を終了しない（kill_on_drop なし, client.rs:311）",
    "else if (op.phase == Spawning or TAKE_DURING_REQUEST) closeChild(children, op.heldChild, true)",
    "else if (op.phase != Spawning and TAKE_DURING_REQUEST) closeChild(children, op.heldChild, true)",
    0,
    "noChildRemainsAfterCancel",
  ),
  (
    "init-failure-no-kill",
    "initialize の失敗で process を終了しない（client.rs:329-331 の ? で drop されない）",
    "children' = closeChild(children, cid, false),\n      client' = nextClient,",
    "children' = children,\n      client' = nextClient,",
    0,
    "noOrphanAliveChild",
  ),
  (
    "exchange-failure-keeps-child",
    "往復の失敗で process を self.process に戻す（client.rs:350-353 で捨てない）",
    "children' = closeChild(children, cid, false),\n      client' = { ...client, slot: NONE, lockedBy: NONE },",
    "children' = children,\n      client' = { ...client, slot: cid, lockedBy: NONE },",
    0,
    "noDirtyChildReused",
  ),
  (
    "exchange-failure-closed-but-kept",
    "往復の失敗で子プロセスを終了させたのに、self.process に戻す",
    "children' = closeChild(children, cid, false),\n      client' = { ...client, slot: NONE, lockedBy: NONE },",
    "children' = closeChild(children, cid, false),\n      client' = { ...client, slot: cid, lockedBy: NONE },",
    0,
    "closedChildNotReferenced",
  ),
  (
    "exchange-success-not-returned",
    "往復の成功で process を self.process に戻さない（client.rs:347 なし）",
    "client' = { ...client, slot: cid, lockedBy: NONE },\n      ops' = ops.set(id, {\n        ...op,\n        phase: Done,",
    "client' = { ...client, slot: NONE, lockedBy: NONE },\n      ops' = ops.set(id, {\n        ...op,\n        phase: Done,",
    0,
    "noOrphanAliveChild",
  ),
  (
    "exchange-success-stays-dirty",
    "応答を読み終えずに戻す（読みかけのまま self.process へ戻す）",
    "children' = children.set(cid, { ...ch, dirty: false }),\n      client' = { ...client, slot: cid, lockedBy: NONE },",
    "children' = children,\n      client' = { ...client, slot: cid, lockedBy: NONE },",
    0,
    "idleSlotIsClean",
  ),
  (
    "shutdown-without-take",
    "shutdown が take() せず、process を self.process に残したまま kill する（client.rs:249）",
    "client' = { ...client, slot: NONE },\n      audit' = { ...audit, sawShutdownClose: true },",
    "client' = client,\n      audit' = { ...audit, sawShutdownClose: true },",
    0,
    "noDoubleClose",
  ),
  (
    "shutdown-without-lock",
    "shutdown がクライアントのロックを取らない（registry.rs:402 の client.lock().await なし）",
    "client.lockedBy == NONE,\n    shutdownCalls < MAX_SHUTDOWNS,",
    "true,\n    shutdownCalls < MAX_SHUTDOWNS,",
    0,
    "lockHolderCanProgress",
  ),
  (
    "spawn-while-running",
    "start が既存の process を確かめずに起動する（client.rs:269-271 なし）",
    "op.phase == Starting and c.lockedBy == id and c.slot == NONE and not(isBackoffActive(c, t))",
    "op.phase == Starting and c.lockedBy == id and not(isBackoffActive(c, t))",
    0,
    "atMostOneLiveChild",
  ),
  (
    "spawn-ignores-backoff",
    "start が next_spawn_at を確かめずに起動する（client.rs:272-277 なし）",
    "op.phase == Starting and c.lockedBy == id and c.slot == NONE and not(isBackoffActive(c, t))",
    "op.phase == Starting and c.lockedBy == id and c.slot == NONE",
    0,
    "noSpawnDuringBackoff",
  ),
  (
    "warm-up-backs-off",
    "温めの起動失敗でも抑制を始める（warm_up が OnSpawnFailure::BackOff を使う, client.rs:226）",
    "if (kind == WarmUp) not(warmupRetry) else true",
    "if (kind == WarmUp) true else true",
    0,
    "warmUpFailureDoesNotStartBackoff",
  ),
  (
    "read-call-retries",
    "読み取りの経路の起動失敗で抑制を始めない（ensure_started が Retry を使う, client.rs:265）",
    "if (kind == WarmUp) not(warmupRetry) else true",
    "if (kind == WarmUp) not(warmupRetry) else kind != ReadCall",
    0,
    "callFailureStartsBackoff",
  ),
  (
    "success-keeps-failure-count",
    "起動の成功で連続失敗の回数と next_spawn_at を戻さない（client.rs:280-281 なし）",
    "client' = { slot: cid, lockedBy: id, consecutiveFailures: 0, nextSpawnAt: NONE },",
    "client' = { ...client, slot: cid, lockedBy: id },",
    0,
    "callFailureStartsBackoff",
  ),
  (
    "request-borrows-child",
    "request が take() せず self.process を借りたまま往復する（client.rs:342）",
    "client' = { ...client, slot: if (TAKE_DURING_REQUEST) NONE else cid },",
    "client' = { ...client, slot: cid },",
    0,
    "noChildRemainsAfterCancel",
  ),
  (
    "unkeyed-write-not-spawned",
    "鍵なしの書き込みを tokio::spawn せず呼び出しのタスクで待つ（write_ledger.rs:141-146）",
    "kind == KeyedWrite or (kind == UnkeyedWrite and detachUnkeyed)",
    "kind == KeyedWrite",
    0,
    "unkeyedWriteSurvivesCallerCancel",
  ),
  (
    "keyed-write-not-spawned",
    "鍵ありの書き込みを tokio::spawn せず呼び出しのタスクで待つ（write_ledger.rs:173-181）",
    "kind == KeyedWrite or (kind == UnkeyedWrite and detachUnkeyed)",
    "(kind == UnkeyedWrite and detachUnkeyed)",
    0,
    "keyedWriteSurvivesCallerCancel",
  ),
  (
    "detached-write-loses-child",
    "切り離した書き込みの打ち切りで、往復中の process を手放す",
    "ops' = ops.set(id, { ...op, callerCancelled: true }),",
    "ops' = ops.set(id, { ...op, callerCancelled: true, heldChild: NONE }),",
    0,
    "sentWriteCanFinish",
  ),
  (
    "tool-error-records-write",
    "道具エラー（isError）でも record_write() を呼ぶ（writes.rs:110-122 の ? なし）",
    "writeRecorded: false,\n      }),\n      audit' = { ...audit, sawToolErrorReuse: true },",
    "writeRecorded: isWriteKind(op.kind),\n      }),\n      audit' = { ...audit, sawToolErrorReuse: true },",
    0,
    "writeRecordedOnlyOnSuccess",
  ),
  (
    "cancel-keeps-lock",
    "打ち切りでクライアントのロックを手放さない（MutexGuard が drop されない）",
    "lockedBy: if (client.lockedBy == id) NONE else client.lockedBy,",
    "lockedBy: client.lockedBy,",
    0,
    "lockOwnershipConsistent",
  ),
]


def mutate(name: str, old: str, new: str, index: int) -> str:
  count = SPEC.count(old)
  if count <= index:
    sys.exit(f"{name}: 置き換え前の文字列が {index + 1} 個目まで見つかりません（{count} 個）")
  start = -1
  for _ in range(index + 1):
    start = SPEC.index(old, start + 1)
  return SPEC[:start] + new + SPEC[start + len(old):]


def run(spec_dir: pathlib.Path, invariant: str) -> tuple[bool, str]:
  result = subprocess.run(
    ["quint", "run", "childProcess.qnt", *RUN_ARGS, f"--invariant={invariant}"],
    cwd=spec_dir,
    capture_output=True,
    text=True,
  )
  output = result.stdout + result.stderr
  violated = result.returncode != 0 and "violation" in output.lower()
  if result.returncode != 0 and not violated:
    sys.exit(f"quint run が違反以外の理由で失敗しました:\n{output}")
  return violated, output


def main() -> int:
  failures = 0
  print("| 変異 | 外した守り | 期待する違反 | 結果 |")
  print("| --- | --- | --- | --- |")
  with tempfile.TemporaryDirectory() as tmp:
    spec_dir = pathlib.Path(tmp)
    spec_file = spec_dir / "childProcess.qnt"
    spec_file.write_text(SPEC)
    violated, _ = run(spec_dir, "safety")
    if violated:
      sys.exit("変異を入れる前の childProcess.qnt が safety に違反しています")
    for name, guard, old, new, index, invariant in MUTATIONS:
      spec_file.write_text(mutate(name, old, new, index))
      violated, _ = run(spec_dir, invariant)
      failures += 0 if violated else 1
      print(f"| `{name}` | {guard} | `{invariant}` | {'違反あり' if violated else '**違反なし（守りが効いていない）**'} |")
  return 1 if failures else 0


if __name__ == "__main__":
  sys.exit(main())
