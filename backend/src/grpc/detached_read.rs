//! stdio の画面読み取りを、要求の打ち切りから切り離して行う（issue #180、設計は
//! `docs/design/issue-59-180.md` の 2）。
//!
//! 要求の future の中で子プロセスの起動・初期化・道具の呼び出しを進めると、Web の deadline や
//! backend の総上限で要求が打ち切られたとき、初期化中の子プロセスも、読みかけの子プロセスも捨てる
//! ことになる。次の要求はまた起動と初期化から始め、初期化が期限より長い負荷では、どの要求も
//! 成功しない。そこで読み取りを別のタスクで行い、要求は結果を待つだけにする。
//!
//! 要求が諦めた後の扱い：
//! - 鍵を待っている間なら、鍵を取らずに終える（設計レビューの指摘 1）。
//! - 鍵を取った後なら、進行中の段（起動と初期化、または 1 回の道具の呼び出し）だけを終え、新しい
//!   呼び出しは始めない（[`AwaitedCaller`]）。子プロセスは使える状態で残り、誰も待たない呼び出しで
//!   レーンをふさがない。
//!
//! Quint の仕様は `specs/quint/run-detail-deadline/` にある（修正案は `DETACH = "call"`）。

use std::{
  future::Future,
  pin::Pin,
  sync::{Arc, Weak},
};

use serde_json::Value;
use tokio::sync::{oneshot, Mutex as AsyncMutex, OwnedMutexGuard};

use crate::mcp::{CallError, ToolCaller, ToolError, TransportError};

/// 要求がまだ結果を待っている間だけ道具を呼ぶ呼び出し先。
///
/// 呼び出しのたびに、先に接続の開始（stdio の起動と初期化）を済ませ、そのあとで要求が待っているかを
/// 見る。要求が諦めていれば、道具を送らずに [`TransportError::NotSent`] で返す。進行中の呼び出しは
/// 要求が諦めても最後まで進める（途中で捨てると、読みかけの子プロセスを捨てることになる）。
pub(super) struct AwaitedCaller<C: ToolCaller + Send + 'static> {
  client: OwnedMutexGuard<C>,
  waiter: Weak<()>,
}

impl<C: ToolCaller + Send + 'static> AwaitedCaller<C> {
  fn awaited(&self) -> bool {
    self.waiter.strong_count() > 0
  }
}

impl<C: ToolCaller + Send + 'static> ToolCaller for AwaitedCaller<C> {
  fn call_tool<'a>(
    &'a mut self,
    name: &'a str,
    arguments: Value,
  ) -> Pin<Box<dyn Future<Output = Result<String, CallError>> + Send + 'a>> {
    Box::pin(async move {
      self.client.ensure_started().await?;
      if !self.awaited() {
        return Err(CallError::Transport(TransportError::NotSent(
          "要求が結果を待つのをやめたので、道具を呼びません".to_owned(),
        )));
      }
      self.client.call_tool(name, arguments).await
    })
  }

  fn connection_key(&self) -> String {
    self.client.connection_key()
  }

  fn ensure_started<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'a>> {
    self.client.ensure_started()
  }
}

/// 鍵を取ってから `read` を進めるタスクを起こし、その結果を待つ。
///
/// 返す future を捨てても（要求の打ち切り）、タスクは止まらない。鍵を待つ間に捨てれば、タスクは
/// 鍵を取らずに終える。鍵を取った後に捨てれば、`read` の中の進行中の段だけが終わり、新しい道具の
/// 呼び出しは [`AwaitedCaller`] が断る。
pub(super) async fn detached<C, T, F, Fut>(handle: Arc<AsyncMutex<C>>, read: F) -> Result<T, ToolError>
where
  C: ToolCaller + Send + 'static,
  T: Send + 'static,
  F: FnOnce(AwaitedCaller<C>) -> Fut + Send + 'static,
  Fut: Future<Output = Result<T, ToolError>> + Send + 'static, {
  // 要求が待っている印。この future が捨てられると印も消え、タスクは要求が諦めたと分かる。
  let waiting = Arc::new(());
  let waiter = Arc::downgrade(&waiting);
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
    let _ = sender.send(read(AwaitedCaller { client, waiter }).await);
  });
  let result = receiver.await;
  drop(waiting);
  result.map_err(|_| {
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
  use serde_json::json;
  use tokio::sync::Mutex as AsyncMutex;

  use super::*;

  /// 呼び出しのたびに `work` 秒かかる偽の呼び出し先。呼び出し先に届いた呼び出しを数える。
  struct FakeCaller {
    work:     Duration,
    received: Arc<AtomicUsize>,
  }

  impl ToolCaller for FakeCaller {
    fn call_tool<'a>(
      &'a mut self,
      _name: &'a str,
      _arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, CallError>> + Send + 'a>> {
      Box::pin(async move {
        self.received.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.work).await;
        Ok(String::new())
      })
    }

    fn connection_key(&self) -> String {
      "fake".to_owned()
    }
  }

  /// 鍵の持ち主が `held` 秒後に鍵を放し、要求（打ち切る時刻、ミリ秒。`None` は打ち切らない）が順に
  /// 鍵を待つ。各要求の読み取りは道具を `calls` 回呼び、1 回に `work` 秒かかる。打ち切りは x.5
  /// 秒に置き、 鍵の受け渡しと呼び出しの切れ目（整数秒）と同時にならないようにする。
  fn scenario() -> impl Strategy<Value = (u64, u64, usize, Vec<Option<u64>>)> {
    (
      0u64..=60,
      0u64..=20,
      1usize..=4,
      prop::collection::vec(prop::option::of((0u64..=180).prop_map(|half| half * 1000 + 500)), 1..=4),
    )
  }

  /// 鍵は先着順に渡る。要求ごとに、始める呼び出しの回数を求める。鍵を取る前に打ち切った要求は 0
  /// 回。
  /// 鍵を取った後は、打ち切りより前に始まる呼び出しだけを数える（進行中の呼び出しは最後まで進む）。
  fn expected_calls(held: u64, work: u64, calls: usize, cancels: &[Option<u64>]) -> Vec<usize> {
    let mut free_at = held * 1000;
    cancels
      .iter()
      .map(|cancel| {
        let count = match cancel {
          | Some(cancel) if *cancel < free_at => 0,
          | Some(cancel) => (0..calls).take_while(|index| free_at + *index as u64 * work * 1000 < *cancel).count(),
          | None => calls,
        };
        free_at += count as u64 * work * 1000;
        count
      })
      .collect()
  }

  proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// 設計の PBT 2：鍵を取る前に打ち切った要求は道具を呼ばない。鍵を取った後に打ち切った要求は、
    /// 打ち切りより前に始めた呼び出しだけを最後まで進め、新しい呼び出しは始めない。打ち切らない要求は
    /// 結果を受け取る。鍵を待つタスクの数は、待っている要求の数以下。
    #[test]
    fn detached_reads_stop_calling_once_the_request_leaves((held, work, calls, cancels) in scenario()) {
      let runtime = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
      runtime.block_on(async move {
        let started: Vec<Arc<AtomicUsize>> = cancels.iter().map(|_| Arc::new(AtomicUsize::new(0))).collect();
        let finished: Vec<Arc<AtomicUsize>> = cancels.iter().map(|_| Arc::new(AtomicUsize::new(0))).collect();
        let acquired: Vec<Arc<AtomicUsize>> = cancels.iter().map(|_| Arc::new(AtomicUsize::new(0))).collect();
        let received = Arc::new(AtomicUsize::new(0));
        let handle =
          Arc::new(AsyncMutex::new(FakeCaller { work: Duration::from_secs(work), received: received.clone() }));
        let holder = handle.clone().lock_owned().await;
        tokio::spawn(async move {
          tokio::time::sleep(Duration::from_secs(held)).await;
          drop(holder);
        });
        let running = Arc::new(AtomicUsize::new(0));
        let mut results = Vec::new();
        for (index, cancel) in cancels.iter().enumerate() {
          let (started, finished, running) = (started[index].clone(), finished[index].clone(), running.clone());
          let (acquired, received) = (acquired[index].clone(), received.clone());
          let read = detached(handle.clone(), move |mut client| async move {
            acquired.fetch_add(1, Ordering::SeqCst);
            running.fetch_add(1, Ordering::SeqCst);
            let mut done = 0;
            let outcome = loop {
              if done == calls {
                break Ok(done);
              }
              let before = received.load(Ordering::SeqCst);
              let call = client.call_tool("takt_get_run", json!({})).await;
              if received.load(Ordering::SeqCst) > before {
                started.fetch_add(1, Ordering::SeqCst);
              }
              match call {
                | Ok(_) => {
                  finished.fetch_add(1, Ordering::SeqCst);
                  done += 1;
                },
                | Err(error) => break Err(ToolError::from(error)),
              }
            };
            running.fetch_sub(1, Ordering::SeqCst);
            drop(client);
            outcome
          });
          let cancel = *cancel;
          results.push(tokio::spawn(async move {
            match cancel {
              | Some(cancel) => tokio::time::timeout(Duration::from_millis(cancel), read).await.ok(),
              | None => Some(read.await),
            }
          }));
          // 要求は並べた順に鍵を待ち始める。
          tokio::task::yield_now().await;
        }
        let end = held * 1000 + work * 1000 * (calls * cancels.len()) as u64 + 200_000;
        let mut now = 0;
        while now < end {
          tokio::time::sleep(Duration::from_millis(250)).await;
          now += 250;
          for _ in 0..8 {
            tokio::task::yield_now().await;
          }
          // 鍵を待つタスク：取っ手を持つ数から、テスト自身の 1 つと、読み取り中のタスクと、鍵の持ち主の
          // タスク（鍵を放すまで）を除く。
          let holder_alive = usize::from(now < held * 1000);
          let waiting_tasks = Arc::strong_count(&handle) - 1 - running.load(Ordering::SeqCst) - holder_alive;
          let waiting_requests = cancels
            .iter()
            .enumerate()
            .filter(|(index, cancel)| {
              cancel.is_none_or(|cancel| cancel > now) && acquired[*index].load(Ordering::SeqCst) == 0
            })
            .count();
          assert!(
            waiting_tasks <= waiting_requests,
            "時刻 {now} ms：鍵を待つタスク {waiting_tasks} は、待っている要求 {waiting_requests} 以下です"
          );
        }
        let expected = expected_calls(held, work, calls, &cancels);
        for (index, cancel) in cancels.iter().enumerate() {
          let started = started[index].load(Ordering::SeqCst);
          let finished = finished[index].load(Ordering::SeqCst);
          assert_eq!(started, expected[index], "要求 {index} が呼び出し先に送った呼び出しの回数");
          assert_eq!(finished, started, "要求 {index}：送った呼び出しは、打ち切られても最後まで進みます");
          let result = results.remove(0).await.unwrap();
          if cancel.is_none() {
            assert_eq!(result.unwrap().unwrap(), calls, "打ち切らない要求は結果を受け取ります");
          }
        }
        assert_eq!(received.load(Ordering::SeqCst), expected.iter().sum::<usize>(), "呼び出し先に届いた呼び出しの数");
        assert_eq!(Arc::strong_count(&handle), 1, "終わった後に、鍵を待つタスクは残りません");
      });
    }
  }
}
