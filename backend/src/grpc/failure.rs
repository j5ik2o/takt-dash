//! takt-mcp と設定の失敗を、画面に出せる gRPC の Status へ変換する境界（issue #306）。
//!
//! Status の文面は、失敗の種類ごとに決めた文だけにします。設定のパス、輸送のエラーの本文、
//! takt-mcp の応答の本文は文面に入れず、backend のログに残します。takt が断った理由（道具の
//! エラーと JSON-RPC のエラーの本文）だけは、利用者が次の操作を決める材料なので、改行を空白に
//! して上限で切った「takt の理由」として文面に続けます。gRPC の状態コードは種類ごとに固定です。

use tonic::Status;

use super::write_ledger::WriteFailure;
use crate::{
  config::ConfigError,
  mcp::{excerpt, ToolError, TransportError},
};

/// 画面へ出す「takt の理由」の文字数の上限。
pub(super) const TAKT_REASON_MAX_CHARS: usize = 200;

/// ログへ残す元のエラーの文字数の上限。takt-mcp の本文が長くてもログを埋めないための上限です。
const LOG_DETAIL_MAX_CHARS: usize = 2000;

/// 詳しい理由の置き場の案内。本番の backend は tracing を標準エラーに書き、launchd の
/// `StandardErrorPath` が配置先の `data/backend-launchd.err.log` を指します。
const SEE_LOG: &str = "詳しい理由は backend のログ（配置先の data/backend-launchd.err.log）にあります。";

const CHECK: &str = "状態を確かめてから、必要ならやり直してください。";

/// takt が断った失敗の文。
fn refused_text() -> String {
  format!(
    "takt が操作を受け付けませんでした。画面を読み込み直して状態を確かめ、必要ならもう一度操作してください。{SEE_LOG}"
  )
}

/// takt の理由を画面に出せる形に整える。改行と制御文字を空白 1 つにまとめ、前後の空白を落とし、
/// 上限を超えたら切って「…」を付けます。
pub(super) fn takt_reason(message: &str) -> String {
  let mut flattened = String::with_capacity(message.len().min(TAKT_REASON_MAX_CHARS * 4));
  let mut pending_space = false;
  for character in message.chars() {
    if character.is_control() || character.is_whitespace() {
      pending_space = true;
      continue;
    }
    if pending_space && !flattened.is_empty() {
      flattened.push(' ');
    }
    pending_space = false;
    flattened.push(character);
  }
  match flattened.char_indices().nth(TAKT_REASON_MAX_CHARS) {
    | Some((cut, _)) => format!("{}…", flattened[..cut].trim_end()),
    | None => flattened,
  }
}

/// 固定の文のあとに「takt の理由: …」を続ける。理由が空なら固定の文だけです。
pub(super) fn with_takt_reason(text: String, message: &str) -> String {
  let reason = takt_reason(message);
  if reason.is_empty() {
    text
  } else {
    format!("{text}takt の理由: {reason}")
  }
}

/// 元のエラーを上限つきでログへ残す。
pub(super) fn log_detail(context: &str, error: &dyn std::fmt::Display) {
  tracing::warn!(error = %excerpt(&error.to_string(), LOG_DETAIL_MAX_CHARS), "{context}");
}

/// takt-mcp の道具エラーを gRPC のステータスへ変換する境界。
pub(super) fn tool_error_status(error: ToolError) -> Status {
  log_detail("takt-mcp の道具の呼び出しに失敗しました。", &error);
  match error {
    | ToolError::Parse(_) => Status::internal(format!(
      "takt-mcp の応答を読み取れませんでした。takt と takt-dash の版の組み合わせを確かめてください。{SEE_LOG}"
    )),
    | ToolError::Tool(error) if error.refused => Status::unavailable(with_takt_reason(refused_text(), &error.message)),
    | ToolError::Tool(_) => Status::unavailable(format!(
      "takt-mcp の応答に本文がありませんでした。takt と takt-dash の版の組み合わせを確かめてください。{SEE_LOG}"
    )),
    | ToolError::Transport(TransportError::Rpc { message, .. }) => {
      Status::unavailable(with_takt_reason(refused_text(), &message))
    },
    | ToolError::Transport(TransportError::Backoff(seconds)) => Status::unavailable(format!(
      "takt-mcp を起動し直すまで待っています。{seconds} 秒ほどおいてから、もう一度操作してください。"
    )),
    | ToolError::Transport(TransportError::Timeout(seconds)) => Status::unavailable(format!(
      "takt-mcp から {seconds} 秒以内に応答がありませんでした。しばらくおいてから、もう一度操作してください。"
    )),
    | ToolError::Transport(
      TransportError::Spawn(_)
      | TransportError::Io(_)
      | TransportError::NotSent(_)
      | TransportError::InvalidJson(_)
      | TransportError::Http(_)
      | TransportError::Protocol(_)
      | TransportError::ProcessExited,
    ) => Status::unavailable(format!(
      "takt-mcp とやり取りできませんでした。takt-mcp が動いているかと、takt-dash の接続先の設定を確かめてから、\
       もう一度操作してください。{SEE_LOG}"
    )),
  }
}

/// 書き込み道具のエラーを、書き込みの失敗へ変換する境界。
///
/// 接続の確立は道具を送る前に試みていますが、確立済みのセッションを使い回す HTTP 接続先では、
/// 道具を送る段で初めて接続の失敗が分かることがあります。接続を張れなかった失敗と、消えた
/// セッションを作り直せなかった失敗（[`TransportError::NotSent`]）は、道具が takt へ届いて
/// いないので、書き込みは適用されていません。
/// takt が拒否を返した失敗（道具のエラー、JSON-RPC のエラー、HTTP のエラーステータス）も、
/// 書き込みが適用されていないと分かります。応答を受け取れなかった失敗と応答を読めなかった失敗は、
/// 適用されたかどうか分かりません。接続の失敗と区別して DEADLINE_EXCEEDED で返し、結果が不明で
/// あることを利用者へ伝えます。
pub(super) fn write_failure(error: ToolError) -> WriteFailure {
  match error {
    | ToolError::Transport(TransportError::Timeout(seconds)) => {
      log_detail("takt-mcp の書き込みの応答が時間内に届きませんでした。", &error);
      WriteFailure::Unknown(Status::deadline_exceeded(format!(
        "takt-mcp から {seconds} 秒以内に応答がなく、操作の結果は不明です。{CHECK}"
      )))
    },
    | ToolError::Transport(
      TransportError::Io(_)
      | TransportError::ProcessExited
      | TransportError::InvalidJson(_)
      | TransportError::Protocol(_),
    ) => {
      log_detail("takt-mcp の書き込みの応答を受け取れませんでした。", &error);
      WriteFailure::Unknown(Status::deadline_exceeded(format!(
        "takt-mcp の応答を受け取れず、操作の結果は不明です。{CHECK}"
      )))
    },
    | ToolError::Parse(_) => {
      log_detail("takt-mcp の書き込みの応答を解析できませんでした。", &error);
      WriteFailure::Unknown(Status::deadline_exceeded(format!(
        "takt-mcp の応答を読み取れず、操作の結果は不明です。{CHECK}"
      )))
    },
    | error @ (ToolError::Tool(_)
    | ToolError::Transport(
      TransportError::Rpc { .. }
      | TransportError::Http(_)
      | TransportError::Spawn(_)
      | TransportError::Backoff(_)
      | TransportError::NotSent(_),
    )) => WriteFailure::Failed(tool_error_status(error)),
  }
}

/// 設定の読み込みの失敗を gRPC のステータスへ変換する境界。設定のパスはログにだけ残します。
pub(super) fn config_load_status(error: ConfigError) -> Status {
  log_detail("設定を読み込めませんでした。", &error);
  Status::unavailable(format!(
    "takt-dash の設定ファイルを読み込めませんでした。設定ファイルを直してから、もう一度操作してください。{SEE_LOG}"
  ))
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;
