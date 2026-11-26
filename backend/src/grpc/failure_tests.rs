//! 失敗の文面の性質（設計 docs/design/issue-306.md の P1〜P6）。

use proptest::prelude::*;
use tonic::Code;

use super::*;
use crate::mcp::{response_excerpt, ToolCallError, RESPONSE_EXCERPT_CHARS};

/// 本文の生成。空、改行、制御文字、多バイト文字、上限を超える長さを含みます。
fn text() -> impl Strategy<Value = String> {
  prop_oneof![
    any::<String>(),
    prop::collection::vec(any::<char>(), 0..600).prop_map(String::from_iter),
    prop::collection::vec(prop::sample::select(vec!['a', 'あ', '\n', '\r', '\t', ' ', '{', '}', '（', '🙂']), 0..600)
      .prop_map(String::from_iter),
  ]
}

/// `ToolError` の枝。本文を持つ枝は `text` を、秒数を持つ枝は `seconds` を使います。
#[derive(Debug, Clone, Copy)]
enum Kind {
  Spawn,
  Io,
  NotSent,
  Timeout,
  Backoff,
  InvalidJson,
  Http,
  Protocol,
  Rpc,
  ProcessExited,
  Tool,
  Parse,
}

const KINDS: [Kind; 12] = [
  Kind::Spawn,
  Kind::Io,
  Kind::NotSent,
  Kind::Timeout,
  Kind::Backoff,
  Kind::InvalidJson,
  Kind::Http,
  Kind::Protocol,
  Kind::Rpc,
  Kind::ProcessExited,
  Kind::Tool,
  Kind::Parse,
];

fn kind() -> impl Strategy<Value = Kind> {
  prop::sample::select(KINDS.to_vec())
}

/// 本文 `text`、秒数 `seconds`、道具名とコード `label` から失敗を作る。
fn error_of(kind: Kind, text: &str, seconds: u64, label: &str) -> ToolError {
  let text = text.to_owned();
  match kind {
    | Kind::Spawn => ToolError::Transport(TransportError::Spawn(text)),
    | Kind::Io => ToolError::Transport(TransportError::Io(text)),
    | Kind::NotSent => ToolError::Transport(TransportError::NotSent(text)),
    | Kind::Timeout => ToolError::Transport(TransportError::Timeout(seconds)),
    | Kind::Backoff => ToolError::Transport(TransportError::Backoff(seconds)),
    | Kind::InvalidJson => ToolError::Transport(TransportError::InvalidJson(text)),
    | Kind::Http => ToolError::Transport(TransportError::Http(text)),
    | Kind::Protocol => ToolError::Transport(TransportError::Protocol(text)),
    | Kind::Rpc => ToolError::Transport(TransportError::Rpc { code: label.len() as i64 - 32_600, message: text }),
    | Kind::ProcessExited => ToolError::Transport(TransportError::ProcessExited),
    | Kind::Tool => ToolError::Tool(ToolCallError { tool: label.to_owned(), message: text }),
    | Kind::Parse => ToolError::Parse(text),
  }
}

/// takt の理由を文面に続ける枝かどうか。
fn carries_takt_reason(kind: Kind) -> bool {
  matches!(kind, Kind::Tool | Kind::Rpc)
}

/// 変更の前の `tool_error_status` のコード。
fn expected_tool_code(kind: Kind) -> Code {
  match kind {
    | Kind::Parse => Code::Internal,
    | _ => Code::Unavailable,
  }
}

/// 変更の前の `write_failure` の振り分けとコード。`true` は Unknown（結果不明）。
fn expected_write(kind: Kind) -> (bool, Code) {
  match kind {
    | Kind::Timeout | Kind::Io | Kind::ProcessExited | Kind::InvalidJson | Kind::Protocol | Kind::Parse => {
      (true, Code::DeadlineExceeded)
    },
    | Kind::Tool | Kind::Rpc | Kind::Http | Kind::Spawn | Kind::Backoff | Kind::NotSent => (false, Code::Unavailable),
  }
}

fn split(failure: WriteFailure) -> (bool, Status) {
  match failure {
    | WriteFailure::Unknown(status) => (true, status),
    | WriteFailure::Failed(status) => (false, status),
  }
}

fn assert_displayable(message: &str) {
  assert!(!message.chars().any(char::is_control), "文面に改行や制御文字を入れません: {message:?}");
}

proptest! {
  #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

  /// P1・P3：takt の理由を続けない枝の文面は本文によらず、コードは変更の前と同じ。
  #[test]
  fn tool_error_message_does_not_depend_on_the_body(kind in kind(), a in text(), b in text(), seconds in any::<u64>()) {
    prop_assume!(!carries_takt_reason(kind));
    let first = tool_error_status(error_of(kind, &a, seconds, "tool"));
    let second = tool_error_status(error_of(kind, &b, seconds, "tool"));
    prop_assert_eq!(first.message(), second.message());
    prop_assert_eq!(first.code(), expected_tool_code(kind));
    assert_displayable(first.message());
  }

  /// P1a：道具のエラーと JSON-RPC のエラーの文面は、固定の文と takt の理由だけで決まり、
  /// 道具名と JSON-RPC のコードによらない。
  #[test]
  fn refused_message_is_the_fixed_text_and_the_takt_reason(
    reason_kind in prop::sample::select(vec![Kind::Tool, Kind::Rpc]),
    body in text(),
    label_a in text(),
    label_b in text(),
  ) {
    let first = tool_error_status(error_of(reason_kind, &body, 0, &label_a));
    let second = tool_error_status(error_of(reason_kind, &body, 0, &label_b));
    prop_assert_eq!(first.message(), second.message());
    prop_assert_eq!(first.message(), with_takt_reason(refused_text(), &body));
    prop_assert_eq!(first.code(), Code::Unavailable);
    assert_displayable(first.message());
  }

  /// P2・P3：書き込みの失敗の文面も本文によらず、Unknown／Failed の別とコードは変更の前と同じ。
  #[test]
  fn write_failure_message_does_not_depend_on_the_body(kind in kind(), a in text(), b in text(), seconds in any::<u64>()) {
    let (first_unknown, first) = split(write_failure(error_of(kind, &a, seconds, "tool")));
    let (second_unknown, second) = split(write_failure(error_of(kind, &b, seconds, "tool")));
    prop_assert_eq!(first_unknown, second_unknown);
    prop_assert_eq!((first_unknown, first.code()), expected_write(kind));
    if carries_takt_reason(kind) {
      prop_assert_eq!(first.message(), with_takt_reason(refused_text(), &a));
    } else {
      prop_assert_eq!(first.message(), second.message());
    }
    assert_displayable(first.message());
  }

  /// P4：登録案の食い違いの文面は、固定の文と takt の理由だけで決まり、コードは FAILED_PRECONDITION。
  #[test]
  fn stale_registration_message_is_the_fixed_text_and_the_takt_reason(before in text(), after in text()) {
    let message = format!("{before}stale_summary{after}");
    let (unknown, status) = split(super::super::writes::registration_write_failure(ToolError::Tool(ToolCallError {
      tool: "takt_approve_goal_registration".to_owned(),
      message: message.clone(),
    })));
    prop_assert!(!unknown);
    prop_assert_eq!(status.code(), Code::FailedPrecondition);
    prop_assert_eq!(
      status.message(),
      with_takt_reason(
        "ゴール登録案は、作り直されたか、すでに承認・却下されています。画面を読み込み直してから、もう一度操作してください。"
          .to_owned(),
        &message,
      )
    );
    assert_displayable(status.message());
  }

  /// P5：設定の読み込みの失敗の文面は、パスと本文によらず、コードは UNAVAILABLE。
  #[test]
  fn config_message_does_not_depend_on_the_path(path_a in text(), path_b in text(), body in text(), which in 0usize..3) {
    let error_of = |path: &str| match which {
      | 0 => ConfigError::Read { path: path.to_owned(), source: std::io::Error::other(body.clone()) },
      | 1 => ConfigError::Parse {
        path: path.to_owned(),
        source: serde_json::from_str::<serde_json::Value>("{").expect_err("閉じていない JSON は不正です"),
      },
      | _ => ConfigError::Invalid(format!("{path}{body}")),
    };
    let first = config_load_status(error_of(&path_a));
    let second = config_load_status(error_of(&path_b));
    prop_assert_eq!(first.message(), second.message());
    prop_assert_eq!(first.code(), Code::Unavailable);
  }

  /// P1a：takt の理由は改行と制御文字を含まず、上限と「…」を超えない。整えた本文が上限以下なら、
  /// 切らずにそのまま。
  #[test]
  fn takt_reason_is_one_bounded_line(body in text()) {
    let reason = takt_reason(&body);
    assert_displayable(&reason);
    prop_assert!(reason.chars().count() <= TAKT_REASON_MAX_CHARS + 1);
    prop_assert_eq!(reason.trim(), reason.as_str());
    let words: Vec<&str> = body.split(|c: char| c.is_control() || c.is_whitespace()).filter(|w| !w.is_empty()).collect();
    let flattened = words.join(" ");
    if flattened.chars().count() <= TAKT_REASON_MAX_CHARS {
      prop_assert_eq!(reason, flattened);
    } else {
      prop_assert!(reason.ends_with('…'));
      let kept = reason.trim_end_matches('…');
      prop_assert!(flattened.starts_with(kept));
    }
  }

  /// P6：応答の本文の抜き出しは、上限以下ならそのまま、超えれば先頭の上限分と省略の印だけ。
  #[test]
  fn response_excerpt_keeps_only_the_head(body in text()) {
    let excerpt = response_excerpt(&body);
    let length = body.chars().count();
    if length <= RESPONSE_EXCERPT_CHARS {
      prop_assert_eq!(excerpt, body);
    } else {
      let head: String = body.chars().take(RESPONSE_EXCERPT_CHARS).collect();
      prop_assert_eq!(excerpt, format!("{head}…（ほか {} 文字）", length - RESPONSE_EXCERPT_CHARS));
    }
  }
}

#[test]
fn an_empty_takt_reason_adds_nothing() {
  assert_eq!(with_takt_reason("固定の文。".to_owned(), " \n\t "), "固定の文。");
}

#[test]
fn backoff_and_timeout_show_only_the_seconds() {
  let backoff = tool_error_status(ToolError::Transport(TransportError::Backoff(7)));
  assert!(backoff.message().contains("7 秒ほどおいて"), "{}", backoff.message());
  let timeout = tool_error_status(ToolError::Transport(TransportError::Timeout(15)));
  assert!(timeout.message().contains("15 秒以内"), "{}", timeout.message());
}
