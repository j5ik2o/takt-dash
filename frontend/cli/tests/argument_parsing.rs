//! CLI の引数解析の観測可能な挙動（終了コード、stdout、stderr
//! の先頭行）を、ビルド済みの実行ファイルから確かめます。
//! 複数の誤りがあるときにどの誤りが先に報告されるかも固定します。

use std::process::{Command, Output};

/// 使い方の誤りで異常終了するときの終了コード。
const EXIT_USAGE: i32 = 2;
/// 接続・取得・引数のエラーで異常終了するときの終了コード。
const EXIT_FAILURE: i32 = 1;
/// 使い方の説明の先頭行の書き出し。
const USAGE_PREFIX: &str = "使い方: takt-dash-cli";
/// 接続を拒否する待ち受けアドレス。解析が通ったことを、
/// 使い方の誤りではない失敗で確かめるために使います。
const REFUSED_ADDR: &str = "127.0.0.1:1";

fn run_cli(args: &[&str]) -> Output {
  Command::new(env!("CARGO_BIN_EXE_takt-dash-cli"))
    .args(args)
    .env_remove("TAKTDASH_BACKEND_ADDR")
    .output()
    .expect("CLI を起動できること")
}

fn assert_usage_error(args: &[&str], expected_message: &str) {
  let output = run_cli(args);
  let stderr = String::from_utf8(output.stderr).expect("stderr が UTF-8 であること");
  assert_eq!(output.status.code(), Some(EXIT_USAGE), "args={args:?} stderr={stderr}");
  assert!(output.stdout.is_empty(), "args={args:?} では stdout へ何も出さないこと");
  let mut lines = stderr.lines();
  assert_eq!(lines.next(), Some(expected_message), "args={args:?} stderr={stderr}");
  assert!(lines.next().is_some_and(|line| line.starts_with(USAGE_PREFIX)), "args={args:?} stderr={stderr}");
}

fn assert_help(args: &[&str]) {
  let output = run_cli(args);
  let stdout = String::from_utf8(output.stdout).expect("stdout が UTF-8 であること");
  assert_eq!(output.status.code(), Some(0), "args={args:?} stdout={stdout}");
  assert!(stdout.starts_with(USAGE_PREFIX), "args={args:?} stdout={stdout}");
  assert!(output.stderr.is_empty(), "args={args:?} では stderr へ何も出さないこと");
}

fn assert_parsed_then_connection_failed(args: &[&str]) {
  let output = run_cli(args);
  let stderr = String::from_utf8(output.stderr).expect("stderr が UTF-8 であること");
  assert_eq!(output.status.code(), Some(EXIT_FAILURE), "args={args:?} stderr={stderr}");
  assert!(!stderr.contains(USAGE_PREFIX), "解析が通った args={args:?} では使い方を出さないこと: {stderr}");
}

#[test]
fn unknown_flag_before_help_is_reported_instead_of_help() {
  assert_usage_error(&["--bogus", "--help"], "未知の引数です: --bogus");
}

#[test]
fn help_wins_over_errors_that_appear_after_it() {
  assert_help(&["--help", "--bogus"]);
  assert_help(&["-h", "unknown-command"]);
  assert_help(&["merge", "--server", "s", "--help", "--pr", "0"]);
}

#[test]
fn session_is_rejected_for_overview_whether_given_before_or_after_the_command() {
  assert_usage_error(&["--session", "x", "overview"], "概要の表示に --session は使えません。");
  assert_usage_error(&["overview", "--session", "x"], "概要の表示に --session は使えません。");
  assert_usage_error(&["--session", "x"], "概要の表示に --session は使えません。");
}

#[test]
fn session_after_overview_is_rejected_before_its_value_is_checked() {
  assert_usage_error(&["overview", "--session"], "概要の表示に --session は使えません。");
}

#[test]
fn unknown_command_is_reported_with_its_name() {
  assert_usage_error(&["bogus"], "未知のコマンドです: bogus");
}

#[test]
fn command_specific_flag_before_the_command_is_an_unknown_argument() {
  assert_usage_error(&["--server", "s", "answer"], "未知の引数です: --server");
}

#[test]
fn positional_arguments_are_rejected_for_commands_that_take_none() {
  assert_usage_error(&["decisions", "foo"], "decisions に位置引数は取れません: foo");
  assert_usage_error(&["overview", "foo"], "overview に位置引数は取れません: foo");
}

#[test]
fn conversation_takes_exactly_one_positional_project() {
  assert_usage_error(&["conversation"], "会話の表示にはプロジェクト名が必要です。");
  assert_usage_error(&["conversation", "a", "b"], "会話の表示に位置引数は1つだけ取れます: b");
}

#[test]
fn flag_without_value_is_reported() {
  assert_usage_error(&["overview", "--addr"], "--addr には値が必要です。");
  assert_usage_error(&["--config", ""], "--config には値が必要です。");
}

#[test]
fn registration_decisions_require_a_positive_revision() {
  for command in ["approve-registration", "reject-registration"] {
    assert_usage_error(&[command, "--server", "s", "--project", "p"], "--revision には正の整数が必要です。");
    assert_usage_error(
      &[command, "--server", "s", "--project", "p", "--revision", "0"],
      "--revision には正の整数を指定してください: 0",
    );
    assert_usage_error(
      &[command, "--server", "s", "--project", "p", "--revision", "x"],
      "--revision には正の整数を指定してください: x",
    );
  }
}

#[test]
fn registration_decisions_with_valid_arguments_are_parsed() {
  for command in ["approve-registration", "reject-registration"] {
    assert_parsed_then_connection_failed(&[
      command,
      "--server",
      "s",
      "--project",
      "p",
      "--revision",
      "3",
      "--addr",
      REFUSED_ADDR,
    ]);
  }
}

#[test]
fn merge_reports_missing_goal_before_missing_pr_and_until() {
  assert_usage_error(&["merge", "--server", "s", "--project", "p"], "merge には --goal が必要です。");
  assert_usage_error(&["merge", "--server", "s", "--project", "p", "--goal", "g"], "merge には --pr が必要です。");
  assert_usage_error(
    &["merge", "--server", "s", "--project", "p", "--goal", "g", "--pr", "1"],
    "merge には --until が必要です。",
  );
}

#[test]
fn merge_reports_missing_server_before_missing_project() {
  assert_usage_error(&["merge"], "merge には --server が必要です。");
  assert_usage_error(&["merge", "--server", "s"], "merge には --project が必要です。");
}

#[test]
fn answer_reports_missing_question_before_conflicting_answer_kinds() {
  assert_usage_error(
    &["answer", "--server", "s", "--project", "p", "--text", "a", "--report", "completed"],
    "answer には --question が必要です。",
  );
  assert_usage_error(
    &["answer", "--server", "s", "--project", "p", "--question", "q", "--text", "a", "--report", "completed"],
    "--text と --report は同時に指定できません。どちらか一方だけを指定してください。",
  );
}

#[test]
fn goal_requires_goal_flag() {
  assert_usage_error(&["goal"], "goal には --goal が必要です。");
}

#[test]
fn goals_and_goal_reject_server_flag() {
  assert_usage_error(&["goals", "--server", "s"], "未知の引数です: --server");
  assert_usage_error(&["goal", "--server", "s"], "未知の引数です: --server");
}
