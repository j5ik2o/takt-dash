//! 書き込み RPC の重複を防ぐ台帳（issue #305）。
//!
//! 書き込み要求が持つ `idempotency_key` ごとに、書き込みの進行と結果を覚えます。takt 側で
//! 書き込みが済んでいるのに呼び出し側へ応答が届かなかったとき、同じ鍵の再試行や同時の重複は
//! takt へ送り直さず、先の書き込みの結果を受け取ります。
//!
//! 書き込みは呼び出しから切り離して実行します。呼び出し側が期限切れで待つのをやめても、
//! 書き込みは最後まで進み、結果が台帳に残ります。
//!
//! 台帳は backend のメモリにだけあります。backend を再起動するか [`RETENTION`] を過ぎると
//! 記録は消え、そのあとの同じ鍵の要求は新しい操作として扱われます。

use std::{
  collections::HashMap,
  future::Future,
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};

use tokio::sync::watch;
use tonic::{Response, Status};

use crate::domain::IdempotencyKey;

/// 決着した書き込みの記録を保つ時間。
const RETENTION: Duration = Duration::from_secs(60 * 60);

/// 書き込みの失敗。台帳が記録を残すかどうかを分けます。
#[derive(Debug)]
pub(super) enum WriteFailure {
  /// 書き込みが適用されなかったと分かっている失敗。記録を消し、同じ鍵でやり直せます。
  Failed(Status),
  /// 書き込みが適用されたかどうか分からない失敗。記録を残し、同じ鍵では送り直しません。
  Unknown(Status),
}

impl From<Status> for WriteFailure {
  fn from(status: Status) -> Self {
    WriteFailure::Failed(status)
  }
}

impl WriteFailure {
  fn into_status(self) -> Status {
    match self {
      | WriteFailure::Failed(status) | WriteFailure::Unknown(status) => status,
    }
  }
}

/// 決着した書き込みの結果。成功の応答は proto の符号化で持ち、RPC ごとの型へ戻します。
#[derive(Clone, Debug)]
enum Settled {
  Succeeded(Vec<u8>),
  Failed(Status),
  Unknown(Status),
}

struct Entry {
  /// RPC の名前と要求の内容。同じ鍵が別の操作に使われたことを見分けます。
  fingerprint: Vec<u8>,
  settled:     watch::Receiver<Option<Settled>>,
  settled_at:  Option<Instant>,
}

/// 鍵に対する要求の扱い。
enum Begin {
  /// この鍵の最初の要求。書き込みを実行し、結果を [`Ticket::settle`] へ渡します。
  Fresh(Ticket),
  /// 同じ鍵の書き込みが進行中。その結果を待ちます。
  InFlight(watch::Receiver<Option<Settled>>),
  /// 同じ鍵の書き込みが決着済み。
  Replay(Settled),
  /// 同じ鍵が別の内容の要求に使われた。
  Mismatch,
}

/// 進行中の書き込みの記録を決着させる権利。決着させずに捨てると、結果不明として残ります。
struct Ticket {
  ledger:  WriteLedger,
  key:     String,
  settled: watch::Sender<Option<Settled>>,
  done:    bool,
}

impl Ticket {
  fn settle(mut self, outcome: Settled) {
    self.record(outcome);
  }

  fn record(&mut self, outcome: Settled) {
    self.done = true;
    let mut entries = self.ledger.entries.lock().expect("書き込みの台帳のロックが壊れています");
    if matches!(outcome, Settled::Failed(_)) {
      entries.remove(&self.key);
    } else if let Some(entry) = entries.get_mut(&self.key) {
      entry.settled_at = Some(Instant::now());
    }
    self.settled.send_replace(Some(outcome));
  }
}

impl Drop for Ticket {
  fn drop(&mut self) {
    if !self.done {
      self.record(Settled::Unknown(Status::deadline_exceeded(
        "書き込みの処理が中断したため、操作の結果は不明です。状態を確かめてから、必要ならやり直してください。",
      )));
    }
  }
}

/// 鍵ごとの書き込みの記録。
#[derive(Clone, Default)]
pub(super) struct WriteLedger {
  entries: Arc<Mutex<HashMap<String, Entry>>>,
}

impl WriteLedger {
  /// 書き込みを実行する。
  ///
  /// `key` が無い要求は、`prepare` の返す書き込みをそのまま実行します。`key` がある要求は
  /// 台帳を引き、最初の要求だけが `prepare` を呼びます。`prepare` は入力を検証して書き込みを
  /// 組み立て、検証の失敗は台帳に残しません。
  pub(super) async fn run<Request, Reply, Write>(
    &self,
    rpc: &'static str,
    key: Option<IdempotencyKey>,
    request: &Request,
    prepare: impl FnOnce() -> Result<Write, Status>,
  ) -> Result<Response<Reply>, Status>
  where
    Request: prost::Message,
    Reply: prost::Message + Default + Send + 'static,
    Write: Future<Output = Result<Reply, WriteFailure>> + Send + 'static, {
    let Some(key) = key else {
      return prepare()?.await.map(Response::new).map_err(WriteFailure::into_status);
    };
    let mut fingerprint = rpc.as_bytes().to_vec();
    fingerprint.push(0);
    fingerprint.extend(request.encode_to_vec());
    let settled = match self.begin(key.as_str(), fingerprint, Instant::now()) {
      | Begin::Mismatch => {
        return Err(Status::invalid_argument(
          "同じ idempotency_key が別の内容の操作に使われています。操作ごとに新しい鍵を指定してください。",
        ));
      },
      | Begin::Replay(Settled::Unknown(_)) => {
        return Err(Status::deadline_exceeded(
          "この操作は前回の結果が不明のため再送していません。状態を確かめ、まだ必要なら数分おいてから\
           もう一度操作してください。",
        ));
      },
      | Begin::Replay(settled) => settled,
      | Begin::InFlight(receiver) => wait(receiver).await,
      | Begin::Fresh(ticket) => {
        let write = match prepare() {
          | Ok(write) => write,
          | Err(status) => {
            ticket.settle(Settled::Failed(status.clone()));
            return Err(status);
          },
        };
        let receiver = ticket.settled.subscribe();
        tokio::spawn(async move {
          let outcome = match write.await {
            | Ok(reply) => Settled::Succeeded(reply.encode_to_vec()),
            | Err(WriteFailure::Failed(status)) => Settled::Failed(status),
            | Err(WriteFailure::Unknown(status)) => Settled::Unknown(status),
          };
          ticket.settle(outcome);
        });
        wait(receiver).await
      },
    };
    match settled {
      | Settled::Succeeded(bytes) => Reply::decode(bytes.as_slice())
        .map(Response::new)
        .map_err(|error| Status::internal(format!("記録した書き込みの応答を復元できませんでした: {error}"))),
      | Settled::Failed(status) | Settled::Unknown(status) => Err(status),
    }
  }

  fn begin(&self, key: &str, fingerprint: Vec<u8>, now: Instant) -> Begin {
    let mut entries = self.entries.lock().expect("書き込みの台帳のロックが壊れています");
    entries.retain(|_, entry| entry.settled_at.is_none_or(|at| now.saturating_duration_since(at) < RETENTION));
    match entries.get(key) {
      | Some(entry) if entry.fingerprint != fingerprint => Begin::Mismatch,
      | Some(entry) => match entry.settled.borrow().clone() {
        | Some(settled) => Begin::Replay(settled),
        | None => Begin::InFlight(entry.settled.clone()),
      },
      | None => {
        let (sender, receiver) = watch::channel(None);
        entries.insert(key.to_owned(), Entry { fingerprint, settled: receiver, settled_at: None });
        Begin::Fresh(Ticket { ledger: self.clone(), key: key.to_owned(), settled: sender, done: false })
      },
    }
  }
}

/// 進行中の書き込みの決着を待つ。
async fn wait(mut receiver: watch::Receiver<Option<Settled>>) -> Settled {
  match receiver.wait_for(Option::is_some).await {
    | Ok(settled) => settled.clone().expect("決着した値を待ちました"),
    // Ticket は捨てられるときにも決着を送るため、送信側だけが消えることはありません。
    | Err(_) => Settled::Unknown(Status::deadline_exceeded("書き込みの結果を受け取れず、操作の結果は不明です。")),
  }
}

#[cfg(test)]
#[path = "write_ledger_tests.rs"]
mod tests;
