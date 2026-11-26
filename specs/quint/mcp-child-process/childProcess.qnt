// -*- mode: Bluespec; -*-
// takt-mcp 子プロセスの起動・所有・往復・打ち切り・破棄と、
// 鍵なし書き込みの切り離し実行の状態機械（PR #379 / issue #279）。
//
// 対象コード:
//   - backend/src/mcp/client.rs（McpClient の start・warm_up・ensure_started・spawn・request・shutdown、
//     OnSpawnFailure、next_spawn_at、kill_on_drop）
//   - backend/src/grpc/write_ledger.rs（WriteLedger::run の鍵なし書き込みの tokio::spawn）
//
// 3 つの定数で PR #379 の修正前・途中版・修正後を切り替える:
//   - TAKE_DURING_REQUEST = false: 修正前（request が self.process.as_mut() で借りたまま往復する）
//   - TAKE_DURING_REQUEST = true : 修正後（request が self.process.take() で外して往復し、
//                                  打ち切りや通信失敗で kill_on_drop により破棄する）
//   - WARMUP_RETRY = false       : 修正前（warm_up が ensure_started を呼び、失敗で next_spawn_at を設定する）
//   - WARMUP_RETRY = true        : 修正後（warm_up が OnSpawnFailure::Retry を使い、抑制を始めない）
//   - DETACH_UNKEYED_WRITE = false: 修正前 / PR #379 初版（鍵なし書き込みを呼び出しタスクの中で待つ）
//   - DETACH_UNKEYED_WRITE = true : 修正後（鍵なし書き込みも tokio::spawn で切り離して最後まで実行する）
module childProcess {
  const TAKE_DURING_REQUEST: bool
  const WARMUP_RETRY: bool
  const DETACH_UNKEYED_WRITE: bool

  // Closed は kill_on_drop / child.kill() により終了指令を出してクライアントの管理から外れた状態を表す
  // （drop 後の OS 側プロセス回収 waitpid の非同期遅延はモデルの抽象化境界の外）。
  type ChildStatus =
    | Initializing
    | Alive
    | Closed

  type OpKind =
    | WarmUp
    | ReadCall
    | UnkeyedWrite
    | KeyedWrite

  type OpPhase =
    | WaitingLock
    | Starting
    | Spawning
    | ReadyToCall
    | Exchanging
    | Done
    | Failed
    | Aborted

  type ClientState = {
    slot: int,
    lockedBy: int,
    consecutiveFailures: int,
    nextSpawnAt: int,
  }

  type ChildProcess = {
    status: ChildStatus,
    dirty: bool,
    closeCount: int,
    usedWhileDirty: bool,
    cancelledWhileActive: bool,
  }

  type OpState = {
    kind: OpKind,
    phase: OpPhase,
    callerCancelled: bool,
    heldChild: int,
    sentToTakt: bool,
    writeRecorded: bool,
  }

  // 不変条件と進行条件の観測用の履歴（仕様のゴースト状態）。
  type AuditState = {
    warmupStartedBackoff: bool,
    callFailureMissedBackoff: bool,
    spawnedDuringBackoff: bool,
    callSpawnFailuresSinceSuccess: int,
    sawWarmUpFailWithoutBackoff: bool,
    sawBackoffReject: bool,
    sawRecoveryAfterBackoff: bool,
    sawExchangeFailure: bool,
    sawRecoveryAfterExchangeFailure: bool,
    sawCancelDuringSpawn: bool,
    sawCancelDuringExchange: bool,
    sawRebuildAfterExchangeCancel: bool,
    sawToolErrorReuse: bool,
    sawShutdownClose: bool,
    sawShutdownAfterClose: bool,
  }

  pure val NONE: int = -1
  pure val OP_IDS: Set[int] = 0.to(4)
  pure val ALL_OP_KINDS: Set[OpKind] = Set(WarmUp, ReadCall, UnkeyedWrite, KeyedWrite)
  pure val MAX_SHUTDOWNS: int = 2
  pure val MAX_TIME: int = 7

  var client: ClientState
  var children: int -> ChildProcess
  var nextChildId: int
  var ops: int -> OpState
  var now: int
  var shutdownCalls: int
  var audit: AuditState

  // --- 純粋関数（判定・状態遷移ロジック） ---

  pure def isWriteKind(kind: OpKind): bool =
    kind == UnkeyedWrite or kind == KeyedWrite

  pure def isLockHoldingPhase(phase: OpPhase): bool =
    phase == Starting or phase == Spawning or phase == ReadyToCall or phase == Exchanging

  pure def isActivePhase(phase: OpPhase): bool =
    phase == WaitingLock or isLockHoldingPhase(phase)

  pure def isDetached(kind: OpKind, detachUnkeyed: bool): bool =
    kind == KeyedWrite or (kind == UnkeyedWrite and detachUnkeyed)

  pure def shouldBackoffOnFailure(kind: OpKind, warmupRetry: bool): bool =
    if (kind == WarmUp) not(warmupRetry) else true

  pure def backoffDelay(failures: int): int =
    if (failures <= 1) 1 else if (failures == 2) 2 else 4

  pure def isBackoffActive(c: ClientState, t: int): bool =
    c.nextSpawnAt != NONE and t < c.nextSpawnAt

  pure def closeChild(
    cs: int -> ChildProcess,
    cid: int,
    fromCancel: bool
  ): int -> ChildProcess = {
    val ch = cs.get(cid)
    cs.set(cid, {
      ...ch,
      status: Closed,
      closeCount: ch.closeCount + 1,
      cancelledWhileActive: ch.cancelledWhileActive or fromCancel,
    })
  }

  pure def markChildCancelledWithoutClose(
    cs: int -> ChildProcess,
    cid: int
  ): int -> ChildProcess = {
    val ch = cs.get(cid)
    cs.set(cid, {
      ...ch,
      cancelledWhileActive: true,
    })
  }

  pure def applySpawnFailureClient(
    c: ClientState,
    kind: OpKind,
    t: int,
    warmupRetry: bool
  ): ClientState =
    if (shouldBackoffOnFailure(kind, warmupRetry)) {
      val nextFailures = c.consecutiveFailures + 1
      {
        ...c,
        lockedBy: NONE,
        consecutiveFailures: nextFailures,
        nextSpawnAt: t + backoffDelay(nextFailures),
      }
    } else {
      { ...c, lockedBy: NONE }
    }

  pure def applySpawnFailureAudit(
    a: AuditState,
    cBefore: ClientState,
    cAfter: ClientState,
    kind: OpKind,
    t: int
  ): AuditState = {
    val wasActive = isBackoffActive(cBefore, t)
    val nowActive = isBackoffActive(cAfter, t)
    val warmupBroke =
      kind == WarmUp and (
        (not(wasActive) and nowActive)
        or cAfter.consecutiveFailures != cBefore.consecutiveFailures
        or cAfter.nextSpawnAt != cBefore.nextSpawnAt
      )
    val callMissed =
      kind != WarmUp and (
        not(nowActive)
        or cAfter.consecutiveFailures != cBefore.consecutiveFailures + 1
      )
    val warmupOkNoBackoff =
      kind == WarmUp and not(nowActive) and cAfter.nextSpawnAt == cBefore.nextSpawnAt
    {
      ...a,
      warmupStartedBackoff: a.warmupStartedBackoff or warmupBroke,
      callFailureMissedBackoff: a.callFailureMissedBackoff or callMissed,
      callSpawnFailuresSinceSuccess:
        if (kind == WarmUp) a.callSpawnFailuresSinceSuccess
        else a.callSpawnFailuresSinceSuccess + 1,
      sawWarmUpFailWithoutBackoff: a.sawWarmUpFailWithoutBackoff or warmupOkNoBackoff,
    }
  }

  // ガード群

  pure def canAcquireLock(c: ClientState, op: OpState): bool =
    op.phase == WaitingLock and c.lockedBy == NONE

  pure def canStartAlreadyRunning(c: ClientState, op: OpState, id: int): bool =
    op.phase == Starting and c.lockedBy == id and c.slot != NONE

  pure def canStartBlockedByBackoff(c: ClientState, op: OpState, id: int, t: int): bool =
    op.phase == Starting and c.lockedBy == id and c.slot == NONE and isBackoffActive(c, t)

  pure def canAttemptSpawn(c: ClientState, op: OpState, id: int, t: int): bool =
    op.phase == Starting and c.lockedBy == id and c.slot == NONE and not(isBackoffActive(c, t))

  pure def canFinishSpawn(c: ClientState, op: OpState, id: int): bool =
    op.phase == Spawning and c.lockedBy == id and op.heldChild != NONE

  pure def canBeginRequest(c: ClientState, op: OpState, id: int): bool =
    op.phase == ReadyToCall and c.lockedBy == id and c.slot != NONE

  pure def canFinishExchange(c: ClientState, op: OpState, id: int): bool =
    op.phase == Exchanging and c.lockedBy == id and op.heldChild != NONE

  pure def canCancelCaller(op: OpState): bool =
    not(op.callerCancelled) and isActivePhase(op.phase)

  // ロックを持つ操作に、次に取れる遷移が残っているか（取り残されて止まらないか）。
  pure def lockHolderHasNextStep(c: ClientState, op: OpState, id: int, t: int): bool =
    match op.phase {
      | Starting =>
        canStartAlreadyRunning(c, op, id)
        or canStartBlockedByBackoff(c, op, id, t)
        or canAttemptSpawn(c, op, id, t)
      | Spawning => canFinishSpawn(c, op, id)
      | ReadyToCall => canBeginRequest(c, op, id)
      | Exchanging => canFinishExchange(c, op, id)
      | _ => true
    }

  // --- 初期状態 ---

  action init: bool = all {
    client' = {
      slot: NONE,
      lockedBy: NONE,
      consecutiveFailures: 0,
      nextSpawnAt: NONE,
    },
    children' = Map(),
    nextChildId' = 0,
    ops' = Map(),
    now' = 0,
    shutdownCalls' = 0,
    audit' = {
      warmupStartedBackoff: false,
      callFailureMissedBackoff: false,
      spawnedDuringBackoff: false,
      callSpawnFailuresSinceSuccess: 0,
      sawWarmUpFailWithoutBackoff: false,
      sawBackoffReject: false,
      sawRecoveryAfterBackoff: false,
      sawExchangeFailure: false,
      sawRecoveryAfterExchangeFailure: false,
      sawCancelDuringSpawn: false,
      sawCancelDuringExchange: false,
      sawRebuildAfterExchangeCancel: false,
      sawToolErrorReuse: false,
      sawShutdownClose: false,
      sawShutdownAfterClose: false,
    },
  }

  // --- アクション ---

  // 新しい操作（温め、読み取り、鍵なし書き込み、鍵あり書き込み）が到着する。
  action arrive(id: int, kind: OpKind): bool = all {
    OP_IDS.contains(id),
    not(ops.keys().contains(id)),
    ops' = ops.put(id, {
      kind: kind,
      phase: WaitingLock,
      callerCancelled: false,
      heldChild: NONE,
      sentToTakt: false,
      writeRecorded: false,
    }),
    client' = client,
    children' = children,
    nextChildId' = nextChildId,
    now' = now,
    shutdownCalls' = shutdownCalls,
    audit' = audit,
  }

  // McpClient の排他ロック（AsyncMutex<McpClient>）を取得し、start へ入る。
  action acquireLock(id: int): bool = all {
    ops.keys().contains(id),
    canAcquireLock(client, ops.get(id)),
    client' = { ...client, lockedBy: id },
    ops' = ops.set(id, { ...ops.get(id), phase: Starting }),
    children' = children,
    nextChildId' = nextChildId,
    now' = now,
    shutdownCalls' = shutdownCalls,
    audit' = audit,
  }

  // start: 既に子プロセスが起動済みならそのまま使う（client.rs:269-271）。
  action startAlreadyRunning(id: int): bool = all {
    ops.keys().contains(id),
    canStartAlreadyRunning(client, ops.get(id), id),
    val op = ops.get(id)
    all {
      if (op.kind == WarmUp) all {
        client' = { ...client, lockedBy: NONE },
        ops' = ops.set(id, { ...op, phase: Done }),
      } else all {
        client' = client,
        ops' = ops.set(id, { ...op, phase: ReadyToCall }),
      },
      children' = children,
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
      audit' = audit,
    },
  }

  // start: 再起動の抑制期間中（now < next_spawn_at）は起動せず Backoff を返す（client.rs:272-277）。
  action startBlockedByBackoff(id: int): bool = all {
    ops.keys().contains(id),
    canStartBlockedByBackoff(client, ops.get(id), id, now),
    client' = { ...client, lockedBy: NONE },
    ops' = ops.set(id, { ...ops.get(id), phase: Failed }),
    audit' = { ...audit, sawBackoffReject: true },
    children' = children,
    nextChildId' = nextChildId,
    now' = now,
    shutdownCalls' = shutdownCalls,
  }

  // start -> spawn: command.spawn() 自体が失敗する（子プロセスは作られない、client.rs:286-293, 312）。
  action failSpawnImmediate(id: int): bool = all {
    ops.keys().contains(id),
    canAttemptSpawn(client, ops.get(id), id, now),
    val op = ops.get(id)
    val nextClient = applySpawnFailureClient(client, op.kind, now, WARMUP_RETRY)
    all {
      client' = nextClient,
      ops' = ops.set(id, { ...op, phase: Failed }),
      audit' = {
        ...applySpawnFailureAudit(audit, client, nextClient, op.kind, now),
        spawnedDuringBackoff: audit.spawnedDuringBackoff or isBackoffActive(client, now),
      },
      children' = children,
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // start -> spawn: OS 子プロセスを起動し、initialize の往復へ入る（client.rs:311-329）。
  // この時点では self.process には入れず、spawn() のローカル変数（heldChild）が所有する。
  action beginSpawn(id: int): bool = all {
    ops.keys().contains(id),
    canAttemptSpawn(client, ops.get(id), id, now),
    val op = ops.get(id)
    val cid = nextChildId
    all {
      children' = children.put(cid, {
        status: Initializing,
        dirty: false,
        closeCount: 0,
        usedWhileDirty: false,
        cancelledWhileActive: false,
      }),
      nextChildId' = nextChildId + 1,
      ops' = ops.set(id, { ...op, phase: Spawning, heldChild: cid }),
      audit' = {
        ...audit,
        spawnedDuringBackoff: audit.spawnedDuringBackoff or isBackoffActive(client, now),
      },
      client' = client,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // spawn 成功: initialize と notifications/initialized が完了し、self.process に格納する（client.rs:279-285, 329-332）。
  action completeSpawn(id: int): bool = all {
    ops.keys().contains(id),
    canFinishSpawn(client, ops.get(id), id),
    val op = ops.get(id)
    val cid = op.heldChild
    val ch = children.get(cid)
    val hadBackoff = client.nextSpawnAt != NONE
    all {
      children' = children.set(cid, { ...ch, status: Alive }),
      if (op.kind == WarmUp) all {
        client' = { slot: cid, lockedBy: NONE, consecutiveFailures: 0, nextSpawnAt: NONE },
        ops' = ops.set(id, { ...op, phase: Done, heldChild: NONE }),
      } else all {
        client' = { slot: cid, lockedBy: id, consecutiveFailures: 0, nextSpawnAt: NONE },
        ops' = ops.set(id, { ...op, phase: ReadyToCall, heldChild: NONE }),
      },
      audit' = {
        ...audit,
        callSpawnFailuresSinceSuccess: 0,
        sawRecoveryAfterBackoff: audit.sawRecoveryAfterBackoff or hadBackoff,
      },
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // spawn 失敗: initialize / initialized の通信失敗で、ローカルの process が kill_on_drop により破棄される（client.rs:286-293, 311, 329-331）。
  action failSpawnInit(id: int): bool = all {
    ops.keys().contains(id),
    canFinishSpawn(client, ops.get(id), id),
    val op = ops.get(id)
    val cid = op.heldChild
    val nextClient = applySpawnFailureClient(client, op.kind, now, WARMUP_RETRY)
    all {
      children' = closeChild(children, cid, false),
      client' = nextClient,
      ops' = ops.set(id, { ...op, phase: Failed, heldChild: NONE }),
      audit' = applySpawnFailureAudit(audit, client, nextClient, op.kind, now),
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // request 開始: 要求を takt-mcp へ送信する（client.rs:340-345）。
  // TAKE_DURING_REQUEST = true なら self.process.take() で外して持ち、false なら slot に残したまま借りる。
  action beginRequest(id: int): bool = all {
    ops.keys().contains(id),
    canBeginRequest(client, ops.get(id), id),
    val op = ops.get(id)
    val cid = client.slot
    val ch = children.get(cid)
    all {
      client' = { ...client, slot: if (TAKE_DURING_REQUEST) NONE else cid },
      children' = children.set(cid, {
        ...ch,
        usedWhileDirty: ch.usedWhileDirty or ch.dirty,
        dirty: true,
      }),
      ops' = ops.set(id, { ...op, phase: Exchanging, heldChild: cid, sentToTakt: true }),
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
      audit' = audit,
    },
  }

  // request 正常完了: 応答の行を読み終えて道具も成功し、子プロセスを self.process に戻す（client.rs:238-239, 346-349）。
  // 書き込みなら writes.rs（L121, L161, L182, L206）で lane.record_write() が呼ばれる。
  action completeExchange(id: int): bool = all {
    ops.keys().contains(id),
    canFinishExchange(client, ops.get(id), id),
    val op = ops.get(id)
    val cid = op.heldChild
    val ch = children.get(cid)
    all {
      children' = children.set(cid, { ...ch, dirty: false }),
      client' = { ...client, slot: cid, lockedBy: NONE },
      ops' = ops.set(id, {
        ...op,
        phase: Done,
        heldChild: NONE,
        writeRecorded: isWriteKind(op.kind),
      }),
      audit' = {
        ...audit,
        sawRecoveryAfterExchangeFailure:
          audit.sawRecoveryAfterExchangeFailure or audit.sawExchangeFailure,
        sawRebuildAfterExchangeCancel:
          audit.sawRebuildAfterExchangeCancel
          or (
            audit.sawCancelDuringExchange
            and children.keys().exists(c =>
              c != cid
              and children.get(c).cancelledWhileActive
              and children.get(c).status == Closed
            )
          ),
      },
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // request の道具エラー（isError: true）: 輸送は成功したため子プロセスは破棄せず self.process に戻すが、
  // CallError::Tool として失敗し、書き込みの record_write() は呼ばない（client.rs:238-239, 420-430; writes.rs:110-122）。
  action completeExchangeToolError(id: int): bool = all {
    ops.keys().contains(id),
    canFinishExchange(client, ops.get(id), id),
    val op = ops.get(id)
    val cid = op.heldChild
    val ch = children.get(cid)
    all {
      children' = children.set(cid, { ...ch, dirty: false }),
      client' = { ...client, slot: cid, lockedBy: NONE },
      ops' = ops.set(id, {
        ...op,
        phase: Failed,
        heldChild: NONE,
        writeRecorded: false,
      }),
      audit' = { ...audit, sawToolErrorReuse: true },
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // request 失敗: Timeout / Io / InvalidJson / Rpc / ProcessExited で子プロセスを破棄する（client.rs:350-353）。
  action failExchange(id: int): bool = all {
    ops.keys().contains(id),
    canFinishExchange(client, ops.get(id), id),
    val op = ops.get(id)
    val cid = op.heldChild
    all {
      children' = closeChild(children, cid, false),
      client' = { ...client, slot: NONE, lockedBy: NONE },
      ops' = ops.set(id, { ...op, phase: Failed, heldChild: NONE }),
      audit' = { ...audit, sawExchangeFailure: true },
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // 呼び出し側が打ち切る（gRPC 期限切れ・切断など）。
  // - tokio::spawn で切り離された書き込み（KeyedWrite、および DETACH_UNKEYED_WRITE = true の UnkeyedWrite）は、
  //   JoinHandle / watch receiver が落ちるだけで、裏の書き込みタスクは打ち切られず最後まで進む（write_ledger.rs:141-146, 173-181）。
  // - 切り離されていない操作は future が drop される:
  //   - Spawning 中は spawn() 内のローカル変数が kill_on_drop で終了する（client.rs:311）。
  //   - Exchanging 中は、TAKE_DURING_REQUEST = true なら外した process が kill_on_drop で終了し（client.rs:342）、
  //     false なら self.process に読みかけのまま残る。
  action cancelCaller(id: int): bool = all {
    ops.keys().contains(id),
    canCancelCaller(ops.get(id)),
    val op = ops.get(id)
    if (isDetached(op.kind, DETACH_UNKEYED_WRITE)) all {
      ops' = ops.set(id, { ...op, callerCancelled: true }),
      client' = client,
      children' = children,
      audit' = audit,
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    } else all {
      ops' = ops.set(id, { ...op, callerCancelled: true, phase: Aborted, heldChild: NONE }),
      client' = {
        ...client,
        lockedBy: if (client.lockedBy == id) NONE else client.lockedBy,
      },
      children' =
        if (op.heldChild == NONE) children
        else if (op.phase == Spawning or TAKE_DURING_REQUEST) closeChild(children, op.heldChild, true)
        else markChildCancelledWithoutClose(children, op.heldChild),
      audit' = {
        ...audit,
        sawCancelDuringSpawn: audit.sawCancelDuringSpawn or (op.phase == Spawning),
        sawCancelDuringExchange: audit.sawCancelDuringExchange or (op.phase == Exchanging),
      },
      nextChildId' = nextChildId,
      now' = now,
      shutdownCalls' = shutdownCalls,
    },
  }

  // McpClient::shutdown（client.rs:248-252）。
  // self.process.take() で取り出せたときだけ子プロセスを終了させ、空なら何もしない（二度閉じない）。
  action shutdownClient: bool = all {
    client.lockedBy == NONE,
    shutdownCalls < MAX_SHUTDOWNS,
    shutdownCalls' = shutdownCalls + 1,
    if (client.slot != NONE) all {
      children' = closeChild(children, client.slot, false),
      client' = { ...client, slot: NONE },
      audit' = { ...audit, sawShutdownClose: true },
    } else all {
      children' = children,
      client' = client,
      audit' = {
        ...audit,
        sawShutdownAfterClose:
          audit.sawShutdownAfterClose
          or children.keys().exists(cid => children.get(cid).status == Closed),
      },
    },
    ops' = ops,
    nextChildId' = nextChildId,
    now' = now,
  }

  // 論理時刻を進める（next_spawn_at の満了を観測するため）。
  action advanceTime: bool = all {
    now < MAX_TIME,
    now' = now + 1,
    client' = client,
    children' = children,
    nextChildId' = nextChildId,
    ops' = ops,
    shutdownCalls' = shutdownCalls,
    audit' = audit,
  }

  action step: bool = any {
    nondet id = OP_IDS.oneOf()
    any {
      nondet kind = ALL_OP_KINDS.oneOf()
      arrive(id, kind),
      acquireLock(id),
      startAlreadyRunning(id),
      startBlockedByBackoff(id),
      failSpawnImmediate(id),
      beginSpawn(id),
      completeSpawn(id),
      failSpawnInit(id),
      beginRequest(id),
      completeExchange(id),
      completeExchangeToolError(id),
      failExchange(id),
      cancelCaller(id),
    },
    shutdownClient,
    advanceTime,
  }

  // --- 不変条件（安全性） ---

  // 1. 打ち切った後に、子プロセスが残らない（起動中・往復中に取り消されたら、その子プロセスは破棄される）。
  val noChildRemainsAfterCancel: bool =
    children.keys().forall(cid => {
      val ch = children.get(cid)
      ch.cancelledWhileActive implies (ch.status == Closed and client.slot != cid)
    })

  // 1b. 生きている（Closed でない）子プロセスは、必ず slot か実行中の操作に所有されており、取り残されない。
  val noOrphanAliveChild: bool =
    children.keys().forall(cid =>
      children.get(cid).status != Closed implies (
        client.slot == cid
        or ops.keys().exists(oid => ops.get(oid).heldChild == cid)
      )
    )

  // 1c. クライアントが管理する未終了（Closed でない）の子プロセスは同時に高々 1 つ
  //     （kill_on_drop による終了指令後の OS 側プロセス回収 waitpid の完了時刻までは扱わない）。
  val atMostOneLiveChild: bool =
    children.keys().filter(cid => children.get(cid).status != Closed).size() <= 1

  // 2. 閉じた接続（子プロセス）を二度閉じない。
  val noDoubleClose: bool =
    children.keys().forall(cid => {
      val ch = children.get(cid)
      ch.closeCount <= 1 and ((ch.status == Closed) iff (ch.closeCount == 1))
    })

  // 2b. 閉じた子プロセスは slot にも操作の所有にも残らない。
  val closedChildNotReferenced: bool =
    children.keys().forall(cid =>
      children.get(cid).status == Closed implies (
        client.slot != cid
        and ops.keys().forall(oid => ops.get(oid).heldChild != cid)
      )
    )

  // 3. 読みかけの応答を持った子プロセスが、次の要求に使われない。
  val noDirtyChildReused: bool =
    children.keys().forall(cid => not(children.get(cid).usedWhileDirty))

  // 3b. アイドル状態で slot にある子プロセスは生きていて読みかけを持たない。
  val idleSlotIsClean: bool =
    (client.lockedBy == NONE and client.slot != NONE) implies (
      children.keys().contains(client.slot)
      and children.get(client.slot).status == Alive
      and not(children.get(client.slot).dirty)
    )

  // 4a. 温めの起動失敗は、再起動の抑制（next_spawn_at）を始めない。
  val warmUpFailureDoesNotStartBackoff: bool =
    not(audit.warmupStartedBackoff)
    and (audit.callSpawnFailuresSinceSuccess == 0 implies client.nextSpawnAt == NONE)

  // 4b. 呼び出しの経路の起動失敗は、再起動の抑制を始める。
  val callFailureStartsBackoff: bool =
    not(audit.callFailureMissedBackoff)
    and (audit.callSpawnFailuresSinceSuccess > 0 implies client.nextSpawnAt != NONE)
    and (client.consecutiveFailures == audit.callSpawnFailuresSinceSuccess)

  // 4c. 抑制期間中は起動を試みない。
  val noSpawnDuringBackoff: bool =
    not(audit.spawnedDuringBackoff)

  // 5. キーなしの書き込みは、呼び出し側が打ち切っても、takt へ送ったものが最後まで進む（打ち切られない）。
  val unkeyedWriteSurvivesCallerCancel: bool =
    ops.keys().forall(oid => {
      val op = ops.get(oid)
      (op.kind == UnkeyedWrite and op.callerCancelled) implies op.phase != Aborted
    })

  // 5b. キーありの書き込みも、呼び出し側の打ち切りで中断されない。
  val keyedWriteSurvivesCallerCancel: bool =
    ops.keys().forall(oid => {
      val op = ops.get(oid)
      (op.kind == KeyedWrite and op.callerCancelled) implies op.phase != Aborted
    })

  // 5c. 書き込みの記録（record_write）は、成功完了した書き込み（Done）に限られる（isError や失敗・中断では記録しない）。
  val writeRecordedOnlyOnSuccess: bool =
    ops.keys().forall(oid => {
      val op = ops.get(oid)
      op.writeRecorded implies (op.phase == Done and op.sentToTakt and isWriteKind(op.kind))
    })

  // 6. ロックと所有の整合性（lockedBy とロック保持フェーズの双方向一致を含む）。
  val lockOwnershipConsistent: bool = all {
    ops.keys().filter(oid => isLockHoldingPhase(ops.get(oid).phase)).size() <= 1,
    client.lockedBy != NONE implies (
      ops.keys().contains(client.lockedBy)
      and isLockHoldingPhase(ops.get(client.lockedBy).phase)
    ),
    ops.keys().forall(oid =>
      isLockHoldingPhase(ops.get(oid).phase) implies client.lockedBy == oid
    ),
    ops.keys().forall(oid =>
      ops.get(oid).heldChild != NONE implies client.lockedBy == oid
    ),
  }

  // 5d. takt へ送った書き込みは、決着するまで往復を終えられる状態にある
  //     （打ち切りで子プロセスやロックを失って止まらない。5・5b とあわせて「最後まで進む」を表す）。
  val sentWriteCanFinish: bool =
    ops.keys().forall(oid => {
      val op = ops.get(oid)
      (isWriteKind(op.kind) and op.sentToTakt and isActivePhase(op.phase)) implies (
        op.phase == Exchanging and canFinishExchange(client, op, oid)
      )
    })

  // 6b. ロックを持つ操作には、次に取れる遷移がいつも残っている（ロックを持ったまま止まらない）。
  val lockHolderCanProgress: bool =
    ops.keys().forall(oid => {
      val op = ops.get(oid)
      isLockHoldingPhase(op.phase) implies lockHolderHasNextStep(client, op, oid, now)
    })

  val safety: bool = all {
    noChildRemainsAfterCancel,
    noOrphanAliveChild,
    atMostOneLiveChild,
    noDoubleClose,
    closedChildNotReferenced,
    noDirtyChildReused,
    idleSlotIsClean,
    warmUpFailureDoesNotStartBackoff,
    callFailureStartsBackoff,
    noSpawnDuringBackoff,
    unkeyedWriteSurvivesCallerCancel,
    keyedWriteSurvivesCallerCancel,
    writeRecordedOnlyOnSuccess,
    sentWriteCanFinish,
    lockOwnershipConsistent,
    lockHolderCanProgress,
  }

  // --- 進行の条件（到達できること。--witnesses で数える） ---

  val warmUpSucceeded: bool =
    ops.keys().exists(oid => ops.get(oid).kind == WarmUp and ops.get(oid).phase == Done)

  val warmUpFailedWithoutBackoff: bool =
    audit.sawWarmUpFailWithoutBackoff

  val callBackedOffAfterFailure: bool =
    audit.sawBackoffReject

  val recoveredAfterBackoff: bool =
    audit.sawRecoveryAfterBackoff

  val cancelledReadDisposedAndRebuilt: bool =
    audit.sawRebuildAfterExchangeCancel
    and children.keys().exists(cid =>
      children.get(cid).cancelledWhileActive and children.get(cid).status == Closed
    )

  val cancelledDuringSpawnDisposed: bool =
    audit.sawCancelDuringSpawn
    and children.keys().exists(cid =>
      children.get(cid).cancelledWhileActive and children.get(cid).status == Closed
    )

  val exchangeFailedAndRecovered: bool =
    audit.sawRecoveryAfterExchangeFailure

  val toolErrorReusesCleanChild: bool =
    audit.sawToolErrorReuse

  val unkeyedWriteCompletesAfterCallerCancel: bool =
    ops.keys().exists(oid => {
      val op = ops.get(oid)
      op.kind == UnkeyedWrite and op.callerCancelled and op.phase == Done and op.writeRecorded
    })

  val keyedWriteCompletesAfterCallerCancel: bool =
    ops.keys().exists(oid => {
      val op = ops.get(oid)
      op.kind == KeyedWrite and op.callerCancelled and op.phase == Done and op.writeRecorded
    })

  val shutdownClosedLiveChild: bool =
    audit.sawShutdownClose

  val shutdownSkippedWhenAlreadyClosed: bool =
    audit.sawShutdownAfterClose
}

// 現在の main（PR #379 適用後）の遷移。
module current {
  import childProcess(
    TAKE_DURING_REQUEST = true,
    WARMUP_RETRY = true,
    DETACH_UNKEYED_WRITE = true
  ).*
}

// PR #379 以前の遷移。
module beforePr379 {
  import childProcess(
    TAKE_DURING_REQUEST = false,
    WARMUP_RETRY = false,
    DETACH_UNKEYED_WRITE = false
  ).*
}

// PR #379 の最初のコミット（request の take と warm_up の Retry を入れたが、
// 鍵なし書き込みをまだ tokio::spawn していない版）。
module withoutUnkeyedDetach {
  import childProcess(
    TAKE_DURING_REQUEST = true,
    WARMUP_RETRY = true,
    DETACH_UNKEYED_WRITE = false
  ).*
}
