use std::sync::atomic::{AtomicUsize, Ordering};

use tonic::Code;

use super::*;

const REQUEST: &str = "要求";

/// 書き込みの回数を数え、`outcome` を返す書き込みを実行する。
async fn run_counted(
  ledger: &WriteLedger,
  key: &str,
  writes: &Arc<AtomicUsize>,
  outcome: Result<String, WriteFailure>,
) -> Result<String, Status> {
  let writes = writes.clone();
  ledger
    .run("Rpc", Some(key), &REQUEST.to_owned(), || {
      Ok(async move {
        writes.fetch_add(1, Ordering::SeqCst);
        outcome
      })
    })
    .await
    .map(Response::into_inner)
}

#[tokio::test]
async fn an_unknown_outcome_is_kept_and_the_write_is_not_resent() {
  let ledger = WriteLedger::default();
  let writes = Arc::new(AtomicUsize::new(0));
  let unknown = || Err(WriteFailure::Unknown(Status::deadline_exceeded("takt が応答しません")));

  let first = run_counted(&ledger, "k", &writes, unknown()).await.expect_err("結果不明です");
  assert_eq!(first.message(), "takt が応答しません", "最初の呼び出しは起きた失敗をそのまま受け取ります");

  let retried = run_counted(&ledger, "k", &writes, Ok("成功".to_owned())).await.expect_err("送り直しません");
  assert_eq!(retried.code(), Code::DeadlineExceeded);
  assert!(retried.message().contains("再送していません"), "{}", retried.message());
  assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_write_that_stops_without_an_outcome_is_recorded_as_unknown() {
  let ledger = WriteLedger::default();

  let status = ledger
    .run::<_, String, _>("Rpc", Some("k"), &REQUEST.to_owned(), || {
      Ok(async { panic!("書き込みの途中で止まります") })
    })
    .await
    .expect_err("結果を受け取れません");
  assert_eq!(status.code(), Code::DeadlineExceeded);

  let writes = Arc::new(AtomicUsize::new(0));
  run_counted(&ledger, "k", &writes, Ok("成功".to_owned())).await.expect_err("途中で止まった書き込みは送り直しません");
  assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_write_keeps_running_after_its_caller_stops_waiting() {
  let ledger = WriteLedger::default();
  let (release, released) = tokio::sync::oneshot::channel::<()>();
  let caller = {
    let ledger = ledger.clone();
    tokio::spawn(async move {
      ledger
        .run::<_, String, _>("Rpc", Some("k"), &REQUEST.to_owned(), || {
          Ok(async move {
            released.await.expect("書き込みを進める合図が届きます");
            Ok("適用済み".to_owned())
          })
        })
        .await
    })
  };
  tokio::task::yield_now().await;
  caller.abort();
  assert!(caller.await.expect_err("呼び出しは打ち切られます").is_cancelled());

  release.send(()).expect("書き込みは打ち切られずに合図を待っています");
  let writes = Arc::new(AtomicUsize::new(0));
  let retried = run_counted(&ledger, "k", &writes, Ok("二重の書き込み".to_owned())).await;

  assert_eq!(retried.expect("再試行は先の書き込みの結果を受け取ります"), "適用済み");
  assert_eq!(writes.load(Ordering::SeqCst), 0, "再試行は書き込みません");
}

#[tokio::test]
async fn a_failed_validation_is_not_recorded() {
  let ledger = WriteLedger::default();

  let status = ledger
    .run::<_, String, std::future::Ready<Result<String, WriteFailure>>>("Rpc", Some("k"), &REQUEST.to_owned(), || {
      Err(Status::invalid_argument("入力が正しくありません"))
    })
    .await
    .expect_err("検証で失敗します");
  assert_eq!(status.code(), Code::InvalidArgument);

  let writes = Arc::new(AtomicUsize::new(0));
  run_counted(&ledger, "k", &writes, Ok("成功".to_owned())).await.expect("同じ鍵でやり直せます");
  assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_settled_record_expires_but_a_write_in_flight_does_not() {
  let ledger = WriteLedger::default();
  let writes = Arc::new(AtomicUsize::new(0));
  run_counted(&ledger, "settled", &writes, Ok("成功".to_owned())).await.expect("成功します");
  let Begin::Fresh(_in_flight) = ledger.begin("in-flight", vec![1], Instant::now()) else {
    panic!("最初の要求は新しい書き込みです");
  };
  let fingerprint = |key: &str| ledger.entries.lock().unwrap().get(key).map(|entry| entry.fingerprint.clone());
  let settled = fingerprint("settled").expect("決着した記録があります");

  let before_expiry = Instant::now() + RETENTION - Duration::from_secs(60);
  assert!(matches!(ledger.begin("settled", settled.clone(), before_expiry), Begin::Replay(_)));

  let after_expiry = Instant::now() + RETENTION + Duration::from_secs(60);
  assert!(matches!(ledger.begin("settled", settled, after_expiry), Begin::Fresh(_)), "期限を過ぎた記録は消えます");
  assert!(
    matches!(ledger.begin("in-flight", vec![1], after_expiry), Begin::InFlight(_)),
    "進行中の書き込みは時間が経っても消しません"
  );
}
