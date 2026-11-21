//! gh を使ったプルリクエストの状態確認。
//!
//! 読み取りサイクルの中で、対象のゴールのプルリクエストが閉じているか（`CLOSED`/`MERGED`）を
//! 確認します。gh はシェルを介さず引数配列で起動し、プロジェクトの cwd を作業ディレクトリに、
//! 期限で打ち切ります。非ゼロ終了・タイムアウト・非 JSON・未知の字面・出力なしは「確認できない」
//! として扱い、呼び出し側は表示を維持します（誤って隠さない）。
//!
//! テストでは PATH 上の偽の `gh` を差し替え、各状態と各失敗を決定的に再現します。実 gh の
//! 認証は gh 自身の設定に委ね、backend は認証情報を持ちません。

use std::{path::Path, process::Stdio, time::Duration};

use serde_json::Value;
use tokio::{process::Command, time::timeout};

use crate::domain::{PullRequestNumber, PullRequestState};

/// gh の呼び出しの期限。ネットワーク越しの応答に余裕を持たせます。
const GH_TIMEOUT: Duration = Duration::from_secs(10);

/// プルリクエストの状態確認の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestProbe {
  /// 状態を確認できた。`OPEN`・`CLOSED`・`MERGED` のいずれか。
  Confirmed(PullRequestState),
  /// 状態を確認できなかった（失敗・時間切れ・解釈できない出力）。表示を維持する。
  Unknown,
}

/// 指定した gh の実行ファイルで、プルリクエストの状態を確認する。
///
/// `program` は gh の実行ファイルのパスです（テストは偽の gh を指します）。`cwd` は
/// プロジェクトの cwd で、gh はこのディレクトリからリポジトリを解決します。gh が実行できない・
/// 非ゼロで終わる・期限を超える・JSON でない・状態の字面が既知でない場合は、すべて
/// [`PullRequestProbe::Unknown`] にします。この関数は状態を確認するだけで、終了の副作用は
/// 持ちません。
pub async fn probe_pull_request_with(program: &Path, cwd: &Path, pull_request: PullRequestNumber) -> PullRequestProbe {
  let number = pull_request.get().to_string();
  let output = timeout(
    GH_TIMEOUT,
    Command::new(program)
      .args(["pr", "view", &number, "--json", "state"])
      .current_dir(cwd)
      .stdin(Stdio::null())
      .stdout(Stdio::piped())
      .stderr(Stdio::null())
      .kill_on_drop(true)
      .output(),
  )
  .await;
  let output = match output {
    | Ok(Ok(output)) => output,
    | Ok(Err(_)) | Err(_) => return PullRequestProbe::Unknown,
  };
  if !output.status.success() {
    return PullRequestProbe::Unknown;
  }
  parse_state(&output.stdout)
}

/// gh の JSON 出力から状態を取り出す。`{"state":"OPEN"}` のような形だけを受け付けます。
///
/// 型・キーの欠落・空・未知の字面は「確認できない」にします。
fn parse_state(stdout: &[u8]) -> PullRequestProbe {
  let Ok(value) = serde_json::from_slice::<Value>(stdout) else {
    return PullRequestProbe::Unknown;
  };
  let Some(state) = value.get("state").and_then(Value::as_str) else {
    return PullRequestProbe::Unknown;
  };
  match PullRequestState::try_from(state) {
    | Ok(state) => PullRequestProbe::Confirmed(state),
    | Err(_) => PullRequestProbe::Unknown,
  }
}

#[cfg(test)]
mod tests {
  use super::{parse_state, PullRequestProbe};
  use crate::domain::PullRequestState;

  #[test]
  fn parses_known_states() {
    assert_eq!(parse_state(br#"{"state":"OPEN"}"#), PullRequestProbe::Confirmed(PullRequestState::Open));
    assert_eq!(parse_state(br#"{"state":"CLOSED"}"#), PullRequestProbe::Confirmed(PullRequestState::Closed));
    assert_eq!(parse_state(br#"{"state":"MERGED"}"#), PullRequestProbe::Confirmed(PullRequestState::Merged));
  }

  #[test]
  fn unknown_state_and_malformed_output_stay_unknown() {
    assert_eq!(parse_state(br#"{"state":"open"}"#), PullRequestProbe::Unknown);
    assert_eq!(parse_state(br#"{"state":"DRAFT"}"#), PullRequestProbe::Unknown);
    assert_eq!(parse_state(br#"{"state":""}"#), PullRequestProbe::Unknown);
    assert_eq!(parse_state(br#"{}"#), PullRequestProbe::Unknown);
    assert_eq!(parse_state(b"not a json"), PullRequestProbe::Unknown);
    assert_eq!(parse_state(b""), PullRequestProbe::Unknown);
    assert_eq!(parse_state(br#"{"state":42}"#), PullRequestProbe::Unknown);
  }
}
