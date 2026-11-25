//! 書き込みの通し番号による古い読み取り結果の判定と、書き込みゲートによる確認と反映の
//! 直列化を固定する。

//! 書き込みの通し番号による古い読み取り結果の判定と、書き込みゲートによる
//! 確認と反映の直列化を固定する。

use super::{read_result_is_stale, McpClientRegistry};

#[test]
fn read_result_is_fresh_without_writes() {
  assert!(!read_result_is_stale(0, 0), "書き込みが無ければ結果は新しい");
  assert!(!read_result_is_stale(3, 3), "番号が同じなら結果は新しい");
}

#[test]
fn read_result_is_stale_after_a_write() {
  assert!(read_result_is_stale(0, 1), "読み取り開始後に書き込みがあれば結果は古い");
  assert!(read_result_is_stale(3, 4), "番号が1つでも進んでいれば結果は古い");
}

/// 書き込み側がゲートを保持している間は poller の反映の段が進まず、書き込みの
/// 完了後に読み取り結果が古いものとして捨てられることを、チャネルの合図だけで
/// 同期して観測します（時間の長さには依存しません）。
#[tokio::test]
async fn poller_apply_waits_for_the_write_gate_and_discards_the_stale_result() {
  let registry = McpClientRegistry::new();
  // 読み取り開始時点の番号。この読み取り結果は書き込み前の状態を映しています。
  let sequence_at_start = registry.write_sequence("local", "proj");

  // 書き込み側は、ゲートを保持したまま合図があるまで通し番号を進めません
  // （gRPC の書き込み経路が、書き込み RPC の発行前から記録までゲートを保持する形）。
  let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
  let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel::<()>();
  let write_registry = registry.clone();
  let write_task = tokio::spawn(async move {
    let gate = write_registry.write_gate("local", "proj");
    let _guard = gate.lock().await;
    let _ = held_tx.send(());
    let _ = proceed_rx.await;
    write_registry.record_write("local", "proj");
  });
  held_rx.await.expect("書き込み側がゲートを保持します");

  // poller 側は、反映の段の入口で合図してからゲートを取得し、取得後に古さを判定します。
  let (entering_tx, entering_rx) = tokio::sync::oneshot::channel::<()>();
  let (decided_tx, mut decided_rx) = tokio::sync::oneshot::channel::<bool>();
  let apply_registry = registry.clone();
  let apply_task = tokio::spawn(async move {
    let gate = apply_registry.write_gate("local", "proj");
    let _ = entering_tx.send(());
    let _guard = gate.lock().await;
    let now = apply_registry.write_sequence("local", "proj");
    let _ = decided_tx.send(read_result_is_stale(sequence_at_start, now));
  });
  entering_rx.await.expect("poller 側が反映の段に入ります");
  // 判定の送信はゲートの取得の先にしか無いため、書き込み側がゲートを保持している
  // 間は決して届きません。届いていないことはタイミングではなく構造で決まります。
  assert!(
    matches!(decided_rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)),
    "書き込みが完了するまで、反映の段はゲートを取得できません"
  );

  // 書き込みを完了させます。通し番号が進んでからゲートが解放されます。
  let _ = proceed_tx.send(());
  write_task.await.expect("書き込み側のタスクが終わります");
  let stale = decided_rx.await.expect("反映の段が判定を返します");
  apply_task.await.expect("poller 側のタスクが終わります");
  assert!(stale, "書き込み前の状態を映す結果は、古いものとして捨てられます");
  assert_eq!(registry.write_sequence("local", "proj"), 1, "書き込みの通し番号は1つ進んでいます");
}

/// poller の反映の段がゲートを保持している間は、書き込み側の record_write が
/// 進まないことを、チャネルの合図だけで同期して観測します。
#[tokio::test]
async fn record_write_waits_while_the_poller_apply_holds_the_gate() {
  let registry = McpClientRegistry::new();
  // poller 側が反映の段のゲートを保持します。
  let gate = registry.write_gate("local", "proj");
  let apply_guard = gate.lock().await;

  // 書き込み側は、入口で合図してからゲートを取得し、取得後に通し番号を進めます
  // （gRPC の書き込み経路と同じ順序）。
  let (entering_tx, entering_rx) = tokio::sync::oneshot::channel::<()>();
  let (recorded_tx, mut recorded_rx) = tokio::sync::oneshot::channel::<u64>();
  let write_registry = registry.clone();
  let write_task = tokio::spawn(async move {
    let gate = write_registry.write_gate("local", "proj");
    let _ = entering_tx.send(());
    let _guard = gate.lock().await;
    let sequence = write_registry.record_write("local", "proj");
    let _ = recorded_tx.send(sequence);
  });
  entering_rx.await.expect("書き込み側がゲートの取得へ入ります");
  // 記録の送信はゲートの取得の先にしか無いため、反映の段がゲートを保持している
  // 間は決して届きません。
  assert!(
    matches!(recorded_rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)),
    "反映が終わるまで、書き込みの記録は進みません"
  );
  assert_eq!(registry.write_sequence("local", "proj"), 0, "通し番号も進んでいません");

  // 反映が終わってゲートが解放されると、書き込みの記録が進みます。
  drop(apply_guard);
  let sequence = recorded_rx.await.expect("書き込みの記録が終わります");
  write_task.await.expect("書き込み側のタスクが終わります");
  assert_eq!(sequence, 1, "記録された通し番号は1です");
  assert_eq!(registry.write_sequence("local", "proj"), 1, "通し番号は1つ進んでいます");
}

/// 書き込みゲートはプロジェクト（とサーバー）ごとに独立しており、ある
/// プロジェクトの保持が別のプロジェクトを待たせないことを観測します。
#[tokio::test]
async fn write_gates_are_independent_per_project() {
  let registry = McpClientRegistry::new();
  let gate = registry.write_gate("local", "proj");
  let _guard = gate.lock().await;
  let other_project = registry.write_gate("local", "other");
  assert!(other_project.try_lock().is_ok(), "別プロジェクトのゲートは保持中のゲートに影響されません");
  let other_server = registry.write_gate("remote", "proj");
  assert!(other_server.try_lock().is_ok(), "別サーバーのゲートも独立です");
}
