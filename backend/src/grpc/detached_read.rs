//! stdio の画面読み取りを、要求の打ち切りから切り離して行う（issue #180、設計は
//! `docs/design/issue-59-180.md` の 2）。
//!
//! 要求の future の中で子プロセスの起動・初期化・道具の呼び出しを進めると、Web の deadline や
//! backend の総上限で要求が打ち切られたとき、初期化中の子プロセスも、読みかけの子プロセスも捨てる
//! ことになる。次の要求はまた起動と初期化から始め、初期化が期限より長い負荷では、どの要求も
//! 成功しない。そこで読み取りを別のタスクで行い、要求は結果を待つだけにする。
//!
//! タスクは鍵を待つ間だけ、要求がまだ待っているかを見る。要求が先に諦めたら、鍵を取らずに終える。
//! 鍵を取った後は、要求が諦めても最後まで進める。Quint の仕様は
//! `specs/quint/run-detail-deadline/` にある。

use std::future::Future;

use tokio::sync::{oneshot, OwnedMutexGuard};

use crate::mcp::{ClientHandle, McpClient, ToolError, TransportError};

/// 鍵を取ってから `read` を最後まで進めるタスクを起こし、その結果を待つ。
///
/// 返す future を捨てても（要求の打ち切り）、鍵を取った後のタスクは止まらない。
/// 鍵を待つ間に捨てれば、 タスクは鍵を取らずに終える。
pub(super) async fn detached<T, F, Fut>(handle: ClientHandle, read: F) -> Result<T, ToolError>
where
  T: Send + 'static,
  F: FnOnce(OwnedMutexGuard<McpClient>) -> Fut + Send + 'static,
  Fut: Future<Output = Result<T, ToolError>> + Send + 'static, {
  let (mut sender, receiver) = oneshot::channel();
  tokio::spawn(async move {
    let client = tokio::select! {
      client = handle.lock_owned() => client,
      () = sender.closed() => return,
    };
    // 鍵と諦めが同時に揃ったときも、誰も待っていない読み取りは始めない。
    if sender.is_closed() {
      return;
    }
    let _ = sender.send(read(client).await);
  });
  receiver.await.map_err(|_| {
    ToolError::Transport(TransportError::Io("画面の読み取りのタスクが結果を返さずに終わりました".to_owned()))
  })?
}

#[cfg(test)]
mod tests {
  use std::{
    sync::{
      atomic::{AtomicUsize, Ordering},
      Arc,
    },
    time::Duration,
  };

  use proptest::prelude::*;
  use tokio::sync::Mutex as AsyncMutex;

  use super::*;
  use crate::mcp::McpClientConfig;

  fn handle() -> ClientHandle {
    // 子プロセスは起動しない（道具を呼ばない）ので、起動の設定は使われない。
    Arc::new(AsyncMutex::new(McpClient::new(McpClientConfig {
      command:         "never-started".to_owned(),
      args:            Vec::new(),
      env:             Default::default(),
      cwd:             std::env::temp_dir(),
      request_timeout: Duration::from_secs(15),
      tool_timeouts:   Default::default(),
    })))
  }

  /// 1 件の要求。`cancel` は要求を打ち切る時刻（ミリ秒）で、`None` は打ち切らない。
  #[derive(Debug, Clone)]
  struct Waiter {
    cancel: Option<u64>,
  }

  /// 要求ごとの記録。
  #[derive(Default)]
  struct Record {
    started:  AtomicUsize,
    finished: AtomicUsize,
  }

  /// 鍵の持ち主が `held` 秒後に鍵を放し、`waiters` が順に鍵を待つ。各読み取りは `work` 秒かかる。
  /// 打ち切りは x.5 秒に置き、鍵の受け渡し（整数秒）と同時にならないようにする。
  fn scenario() -> impl Strategy<Value = (u64, u64, Vec<Waiter>)> {
    (
      0u64..=60,
      0u64..=60,
      prop::collection::vec(prop::option::of((0u64..=180).prop_map(|half| half * 1000 + 500)), 1..=4),
    )
      .prop_map(|(held, work, cancels)| (held, work, cancels.into_iter().map(|cancel| Waiter { cancel }).collect()))
  }

  /// 鍵は先着順に渡る。各要求が鍵を取る時刻（ミリ秒）と、読み取りを始めるかを求める。
  fn expected_starts(held: u64, work: u64, waiters: &[Waiter]) -> Vec<bool> {
    let mut free_at = held * 1000;
    waiters
      .iter()
      .map(|waiter| {
        let starts = waiter.cancel.is_none_or(|cancel| cancel > free_at);
        if starts {
          free_at += work * 1000;
        }
        starts
      })
      .collect()
  }

  proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 設計の PBT 2：鍵を取る前に打ち切った要求の読み取りは始まらない。始まった読み取りは、打ち切られても
    /// 最後まで進む。打ち切らない要求は結果を受け取る。鍵を待つタスクの数は、待っている要求の数以下。
    #[test]
    fn detached_reads_finish_once_started_and_never_start_for_abandoned_waits((held, work, waiters) in scenario()) {
      let runtime = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
      runtime.block_on(async move {
        let handle = handle();
        let holder = handle.clone().lock_owned().await;
        tokio::spawn(async move {
          tokio::time::sleep(Duration::from_secs(held)).await;
          drop(holder);
        });
        let records: Vec<Arc<Record>> = waiters.iter().map(|_| Arc::new(Record::default())).collect();
        let running = Arc::new(AtomicUsize::new(0));
        let mut results = Vec::new();
        for (index, waiter) in waiters.iter().enumerate() {
          let record = records[index].clone();
          let running = running.clone();
          let read = detached(handle.clone(), move |client| async move {
            let _client = client;
            record.started.fetch_add(1, Ordering::SeqCst);
            running.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(work)).await;
            running.fetch_sub(1, Ordering::SeqCst);
            record.finished.fetch_add(1, Ordering::SeqCst);
            Ok(index)
          });
          let cancel = waiter.cancel;
          results.push(tokio::spawn(async move {
            match cancel {
              | Some(cancel) => tokio::time::timeout(Duration::from_millis(cancel), read).await.ok(),
              | None => Some(read.await),
            }
          }));
          // 要求は並べた順に鍵を待ち始める。
          tokio::task::yield_now().await;
        }
        let end = held * 1000 + work * 1000 * waiters.len() as u64 + 200_000;
        let mut now = 0;
        while now < end {
          tokio::time::sleep(Duration::from_millis(250)).await;
          now += 250;
          for _ in 0..8 {
            tokio::task::yield_now().await;
          }
          // 鍵を待つタスク：取っ手を持つ数から、テスト自身の 1 つと、読み取り中のタスクを除く。
          // 鍵の持ち主のタスクも取っ手を持つ（鍵を放すまで）。
          let holder_alive = usize::from(now < held * 1000);
          let waiting_tasks =
            Arc::strong_count(&handle) - 1 - running.load(Ordering::SeqCst) - holder_alive;
          let waiting_requests = waiters
            .iter()
            .enumerate()
            .filter(|(index, waiter)| {
              waiter.cancel.is_none_or(|cancel| cancel > now) && records[*index].started.load(Ordering::SeqCst) == 0
            })
            .count();
          assert!(
            waiting_tasks <= waiting_requests,
            "時刻 {now} ms：鍵を待つタスク {waiting_tasks} は、待っている要求 {waiting_requests} 以下です"
          );
        }
        let expected = expected_starts(held, work, &waiters);
        for (index, waiter) in waiters.iter().enumerate() {
          let started = records[index].started.load(Ordering::SeqCst);
          let finished = records[index].finished.load(Ordering::SeqCst);
          assert_eq!(started, usize::from(expected[index]), "要求 {index} の読み取りを始めた回数");
          assert_eq!(finished, started, "要求 {index}：始めた読み取りは、打ち切られても最後まで進みます");
          let result = results.remove(0).await.unwrap();
          if waiter.cancel.is_none() {
            assert_eq!(result.unwrap().unwrap(), index, "打ち切らない要求は結果を受け取ります");
          }
        }
        assert_eq!(Arc::strong_count(&handle), 1, "終わった後に、鍵を待つタスクは残りません");
      });
    }
  }
}
