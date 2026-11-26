//! backend の停止：定期読み取りの周期の打ち切りと、停止の上限。
//!
//! 上限の並びは、`specs/quint/backend-shutdown-deadline/` の仕様と同じです。

use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::watch;

use super::{CycleReport, Poller};
use crate::{config::DEFAULT_INTERVAL_SECONDS, mcp::SHUTDOWN_DEADLINE};

/// 停止の合図のあと、途中の定期読み取りの周期が終わるのを待つ上限。ふだんの周期はこの時間で
/// 終わり、その周期の出来事を記録してから止まります。周期は道具を何回か呼び、上限は 1 回ごと
/// なので、周期の全体は長く続きえます。それを超えたら周期を打ち切ります。
pub const CYCLE_GRACE: Duration = Duration::from_secs(2);

/// 停止の合図から、backend が終わるまでの上限。`main` が、gRPC サーバーの穏やかな停止と
/// 定期読み取りの停止をこの時間で打ち切ります。
pub const STOP_DEADLINE: Duration = Duration::from_secs(18);

/// 停止の全体の上限のあと、tokio のランタイムを捨てるときに、始まった `spawn_blocking` の終わりを
/// 待つ上限。`main` が `Runtime::shutdown_timeout` に渡します。
pub const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

/// launchd が SIGTERM のあと SIGKILL するまでの時間（plist の `ExitTimeOut` の既定）。本番の
/// backend は `~/Library/LaunchAgents/com.ideoplus.taktdash-backend.plist` で動き、plist に
/// `ExitTimeOut` がありません。plist に足すなら、この値も合わせて直します。
pub const LAUNCHD_EXIT_TIMEOUT: Duration = Duration::from_secs(20);

const _: () = assert!(
  CYCLE_GRACE.as_secs() + SHUTDOWN_DEADLINE.as_secs() < STOP_DEADLINE.as_secs()
    && STOP_DEADLINE.as_secs() + RUNTIME_SHUTDOWN_TIMEOUT.as_secs() < LAUNCHD_EXIT_TIMEOUT.as_secs(),
  "周期を待つ上限と閉じる段の上限は停止の全体の上限に収め、それを launchd の SIGKILL より前に終えます"
);

/// サイクルがプロジェクトごとに起動した読み取りタスク。
///
/// 停止のときにサイクルの future を捨てたら、タスクも中断します。[`tokio::task::JoinHandle`] を
/// 捨てるだけではタスクは走り続け、読み取りハンドルやクライアントの鍵を持ったままになります。
/// すると停止の段が鍵を取れず、応答するサーバーのセッションも閉じられません。
pub(super) struct ReadTask<T>(tokio::task::JoinHandle<T>);

impl<T: Send + 'static> ReadTask<T> {
  pub(super) fn spawn(task: impl std::future::Future<Output = T> + Send + 'static) -> Self {
    ReadTask(tokio::spawn(task))
  }
}

impl<T> std::future::Future for ReadTask<T> {
  type Output = Result<T, tokio::task::JoinError>;

  fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
    std::pin::Pin::new(&mut self.0).poll(cx)
  }
}

impl<T> Drop for ReadTask<T> {
  fn drop(&mut self) {
    // 終わったタスクへの中断は何もしません。
    self.0.abort();
  }
}

/// 読み取りサイクルを周期実行する。停止の合図が立つまで回ります。
///
/// 各サイクルの設定解決は 1 回で、read_cycle_once が返す間隔をそのまま次の待機に使います。
/// 設定読み込みに失敗したサイクルは既定の間隔で待ちます。待機中も停止要求を監視し、
/// 要求を受けたら間隔の満了を待たずにループを抜けて子プロセスを終了します。
///
/// サイクルの途中で停止要求を受けたら、サイクルが終わるのを [`CYCLE_GRACE`] まで待ち、
/// それでも終わらなければサイクルを打ち切ります。打ち切りは `await` の境目で起き、出来事の記録
/// （1 回ずつ同期で書く）の途中では起きません。
pub async fn run_until_stopped(poller: &mut Poller, mut stop: watch::Receiver<bool>) {
  while !*stop.borrow() {
    let started = Utc::now();
    let Some(cycle) = cycle_until_grace(poller, &mut stop).await else {
      tracing::warn!(
        grace_secs = CYCLE_GRACE.as_secs_f64(),
        "停止要求を受けたあと定期読み取りのサイクルが終わらないため、サイクルを打ち切ります。"
      );
      break;
    };
    let interval = match cycle {
      | Ok(report) => {
        tracing::debug!(
          projects_read = report.projects_read,
          projects_failed = report.projects_failed,
          events_recorded = report.events_recorded,
          "定期読み取りの1サイクルを終えました。"
        );
        report.interval
      },
      | Err(error) => {
        tracing::warn!(error = %error, "定期読み取りの1サイクルで設定を読み込めませんでした。");
        Duration::from_secs(DEFAULT_INTERVAL_SECONDS)
      },
    };
    tokio::select! {
      _ = sleep_until_next(started, interval) => {}
      _ = wait_for_stop(&mut stop) => break,
    }
  }
  poller.shutdown().await;
}

/// 1 サイクルを実行する。停止要求を受けたら、サイクルの終わりを [`CYCLE_GRACE`] まで待ち、
/// 待ち切れなければ `None` を返します（サイクルの future は捨てます）。
async fn cycle_until_grace(
  poller: &mut Poller,
  stop: &mut watch::Receiver<bool>,
) -> Option<Result<CycleReport, crate::config::ConfigError>> {
  let mut cycle = std::pin::pin!(poller.read_cycle_once());
  let finished = tokio::select! {
    result = &mut cycle => Some(result),
    _ = wait_for_stop(stop) => None,
  };
  match finished {
    | Some(result) => Some(result),
    | None => tokio::time::timeout(CYCLE_GRACE, cycle).await.ok(),
  }
}

/// `run` を実行し、停止の合図から `deadline` が過ぎたら打ち切って `None` を返す。
///
/// `main` が、gRPC サーバーと定期読み取りの停止の全体を [`STOP_DEADLINE`] で打ち切るために
/// 使います。合図を送る側が合図を送らずに消えたときは、打ち切りません。
pub async fn run_until_stop_deadline<F: std::future::Future>(
  run: F,
  mut stop: watch::Receiver<bool>,
  deadline: Duration,
) -> Option<F::Output> {
  let deadline_after_stop = async move {
    while !*stop.borrow() {
      if stop.changed().await.is_err() {
        std::future::pending::<()>().await;
      }
    }
    tokio::time::sleep(deadline).await;
  };
  // 同時に終わったときは、終わった `run` の結果を返します。
  tokio::select! {
    biased;
    output = run => Some(output),
    _ = deadline_after_stop => None,
  }
}

async fn wait_for_stop(stop: &mut watch::Receiver<bool>) {
  while !*stop.borrow() {
    if stop.changed().await.is_err() {
      return;
    }
  }
}

async fn sleep_until_next(started: DateTime<Utc>, interval: Duration) {
  let elapsed = Utc::now().signed_duration_since(started);
  let remaining = interval.as_millis().saturating_sub(elapsed.num_milliseconds().max(0) as u128);
  if remaining > 0 {
    tokio::time::sleep(Duration::from_millis(remaining as u64)).await;
  }
}

#[cfg(test)]
mod stop_deadline_tests {
  //! 停止の全体の上限（`run_until_stop_deadline`）を、止めた時計で確かめる。

  use std::time::Duration;

  use proptest::prelude::*;

  use super::run_until_stop_deadline;

  proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// `run_until_stop_deadline` は、`run` が終わる時刻と、合図から `deadline` が過ぎる時刻の
    /// 早いほうで返る。合図が無ければ `run` の終わりまで待つ。
    #[test]
    fn returns_at_the_earlier_of_the_run_end_and_the_stop_deadline(
      run_secs in 0u64..=60,
      signal_secs in proptest::option::of(0u64..=60),
      deadline_secs in 1u64..=30,
    ) {
      let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("テストの tokio ランタイムを作れます");
      runtime.block_on(async {
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        if let Some(signal_secs) = signal_secs {
          tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(signal_secs)).await;
            let _ = stop_tx.send(true);
            std::future::pending::<()>().await;
          });
        } else {
          // 合図を送らずに送り手が消えても、打ち切りません。
          drop(stop_tx);
        }
        let started = tokio::time::Instant::now();
        let run = async {
          tokio::time::sleep(Duration::from_secs(run_secs)).await;
          "finished"
        };
        let output = run_until_stop_deadline(run, stop_rx, Duration::from_secs(deadline_secs)).await;
        let elapsed = started.elapsed();

        let cut_at = signal_secs.map(|signal| signal + deadline_secs);
        match cut_at {
          | Some(cut_at) if cut_at < run_secs => {
            prop_assert_eq!(output, None);
            prop_assert_eq!(elapsed, Duration::from_secs(cut_at));
          },
          | _ => {
            prop_assert_eq!(output, Some("finished"));
            prop_assert_eq!(elapsed, Duration::from_secs(run_secs));
          },
        }
        Ok(())
      })?;
    }
  }
}
