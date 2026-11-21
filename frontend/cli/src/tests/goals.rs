//! ゴールの一覧と詳細の表示。

use tonic::Status;

use super::{
  fixtures::{formatted_time, sample_goal_detail, sample_goals},
  support::{
    reserve_closed_addr, run_cli, start_dummy_server, start_dummy_server_with_goals, start_goal_detail_error_server,
    start_goal_detail_server, start_goals_error_server, start_goals_server, write_args, write_config,
  },
};
use crate::{
  proto::v1::{self, DashboardState},
  run_with, DEFAULT_BACKEND_ADDR, EXIT_FAILURE, EXIT_OK, EXIT_USAGE,
};

// ----------------------------------------------------------------------------
// CLI-GOALS-LIST / CLI-GOAL-DETAIL: ゴールの一覧と詳細の表示（要件 1〜13）
// ----------------------------------------------------------------------------

/// 指定した文字列を含む行と、その前後 `radius` 行をまとめて返す。
///
/// 表示の項目が同じ行に並ぶか隣の行に分かれるかは実装の書式に委ね、項目の値と状態の
/// 対応を観測できる範囲だけを取り出す。
fn block_containing(text: &str, needle: &str, radius: usize) -> String {
  let lines: Vec<&str> = text.lines().collect();
  let index = match lines.iter().position(|line| line.contains(needle)) {
    | Some(index) => index,
    | None => panic!("「{needle}」を含む行がありません:\n{text}"),
  };
  let start = index.saturating_sub(radius);
  let end = (index + radius + 1).min(lines.len());
  lines[start..end].join("\n")
}

#[test]
fn goals_command_lists_all_servers_and_projects() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_goals_server(sample_goals()));

  let (code, stdout, stderr) = run_cli(&["goals".to_owned()], server.addr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "ゴール一覧の表示は成功する必要があります。stderr: {stderr}");
  assert_eq!(seen_calls, vec!["GetGoals".to_owned()], "goals は GetGoals だけを呼ぶ必要があります");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");

  let cli_line = block_containing(&stdout, "goal-cli", 0);
  for fragment in [
    "mini",
    "taktdash-cli",
    "goal-cli",
    "CLI に閲覧コマンドを足す",
    "作成済み",
    "稼働中",
    "完了3件",
    "失敗1件",
    "未着手1件",
    "実行中2件",
    "マージ待ち: あり",
    "#42",
    "https://example.test/pr/42",
  ] {
    assert!(cli_line.contains(fragment), "ゴールの行に「{fragment}」が必要です: {cli_line}");
  }

  let web_line = block_containing(&stdout, "goal-web", 0);
  for fragment in [
    "studio",
    "corporate-site",
    "goal-web",
    "ゴール詳細を表示する",
    "人のマージ待ち",
    "一時停止中",
    "完了5件",
    "失敗2件",
    "未着手0件",
    "実行中0件",
    "マージ待ち: なし",
    "プルリクエスト: なし",
  ] {
    assert!(web_line.contains(fragment), "ゴールの行に「{fragment}」が必要です: {web_line}");
  }

  for extra in ["上限超過", "PR 失敗", "保守作業のため"] {
    assert!(!stdout.contains(extra), "要件に無い項目「{extra}」を表示してはいけません: {stdout}");
  }
}

#[test]
fn goals_command_marks_empty_list() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_goals_server(Vec::new()));

  let (code, stdout, stderr) = run_cli(&["goals".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "空の一覧でも成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("（なし）"), "空の一覧は「（なし）」で示す必要があります: {stdout}");
}

#[test]
fn goal_command_prints_detail_and_passes_goal_id() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_goal_detail_server(sample_goal_detail()));
  let requested_goal_id = "goal-cli-detail";

  let (code, stdout, stderr) =
    run_cli(&["goal".to_owned(), "--goal".to_owned(), requested_goal_id.to_owned()], server.addr);
  let requests = server.goal_detail_requests();
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "ゴール詳細の表示は成功する必要があります。stderr: {stderr}");
  assert_eq!(seen_calls, vec!["GetGoalDetail".to_owned()], "goal は GetGoalDetail だけを呼ぶ必要があります");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert_eq!(requests.len(), 1, "ゴール詳細の取得は1回だけ行う必要があります");
  assert_eq!(requests[0].goal_id, requested_goal_id, "要求の goal_id は --goal の値と一致する必要があります");

  for fragment in [
    "CLI にゴールの一覧と詳細の表示を足す。",
    "backend の変更",
    "goals コマンドで一覧を表示できる。",
    "goal コマンドで詳細を表示できる。",
    "proto を変更しない。",
    "takt/20261115T0040-goal-5984be7d",
    "統合ブランチ",
    "main",
    "人のマージ待ち",
    "稼働中",
  ] {
    assert!(stdout.contains(fragment), "ゴール詳細に「{fragment}」が必要です: {stdout}");
  }

  let log_block = block_containing(&stdout, "一覧の表示を実装しました。", 1);
  assert!(log_block.contains(&formatted_time(1_731_600_000)), "作業記録に recorded_at の時刻が必要です: {log_block}");

  let pending_block = block_containing(&stdout, "並び順はどれがよいですか。", 0);
  assert!(pending_block.contains("未回答"), "未回答の質問は「未回答」と示す必要があります: {pending_block}");

  let answered_block = block_containing(&stdout, "この方針で進めてよいですか。", 0);
  assert!(answered_block.contains("回答済み"), "回答済みの質問は「回答済み」と示す必要があります: {answered_block}");
  assert!(answered_block.contains("承認"), "承認の質問は種類を日本語で示す必要があります: {answered_block}");
  let answered_with_answer = block_containing(&stdout, "この方針で進めてよいですか。", 1);
  assert!(
    answered_with_answer.contains("はい"),
    "回答があれば回答本文を併記する必要があります: {answered_with_answer}"
  );

  let withdrawn_block = block_containing(&stdout, "ログを取得してください。", 0);
  assert!(withdrawn_block.contains("取り下げ"), "取り下げた質問は「取り下げ」と示す必要があります: {withdrawn_block}");
  assert!(withdrawn_block.contains("作業の依頼"), "作業の依頼は種類を日本語で示す必要があります: {withdrawn_block}");

  for raw in ["pending", "answered", "withdrawn", "operation_request"] {
    assert!(!stdout.contains(raw), "既知の質問の値を原値「{raw}」のまま表示してはいけません: {stdout}");
  }

  let listed_task = block_containing(&stdout, "一覧の実装", 0);
  assert!(listed_task.contains("完了"), "タスクの状態を日本語で示す必要があります: {listed_task}");
  assert!(listed_task.contains("run-1"), "タスクに run_slug を表示する必要があります: {listed_task}");

  let pending_task = block_containing(&stdout, "詳細の実装", 0);
  assert!(pending_task.contains("未着手"), "タスクの状態を日本語で示す必要があります: {pending_task}");
  assert!(pending_task.contains("なし"), "run_slug が無いタスクは「なし」と示す必要があります: {pending_task}");

  let failed_task = block_containing(&stdout, "失敗したタスク", 0);
  assert!(failed_task.contains("失敗"), "タスクの状態を日本語で示す必要があります: {failed_task}");
  assert!(failed_task.contains("run-2"), "タスクに run_slug を表示する必要があります: {failed_task}");
}

// ----------------------------------------------------------------------------
// CLI-GOAL-ARGS: ゴールコマンドの引数解釈と接続先の指定（要件 14, 16〜20）
// ----------------------------------------------------------------------------

#[test]
fn usage_lists_goals_and_goal_commands() {
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(&["--bogus".to_owned()], Ok(DEFAULT_BACKEND_ADDR.to_owned()), &mut stdout, &mut stderr);

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_USAGE);
  assert!(stdout.is_empty(), "使い方の誤りで標準出力へ出してはいけません: {stdout}");
  assert!(stderr.contains("goals"), "使い方に goals コマンドを載せる必要があります: {stderr}");
  assert!(stderr.contains("goal --goal"), "使い方に goal --goal コマンドを載せる必要があります: {stderr}");
}

#[test]
fn goal_commands_reject_bad_arguments_without_calling_backend() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server(DashboardState::default()));
  let cases = [
    write_args(&["goal"]),
    write_args(&["goal", "--goal"]),
    write_args(&["goals", "--goal", "goal-cli"]),
    write_args(&["goals", "extra"]),
    write_args(&["goal", "--goal", "goal-cli", "extra"]),
    write_args(&["goals", "--server", "mini"]),
  ];

  for args in cases {
    let (code, stdout, stderr) = run_cli(&args, server.addr);
    assert_eq!(code, EXIT_USAGE, "引数 {args:?} は使い方の誤りとして終わる必要があります: {stderr}");
    assert!(stderr.contains("使い方: takt-dash-cli"), "使い方を示す必要があります: {stderr}");
    assert!(stdout.is_empty(), "使い方の誤りで標準出力へ出してはいけません: {stdout}");
  }

  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());
  assert!(seen_calls.is_empty(), "引数の誤りで backend を呼んではいけません: {seen_calls:?}");
}

#[test]
fn goals_and_goal_accept_addr_and_config_flags() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server_with_goals(
    None,
    DashboardState::default(),
    Vec::new(),
    Some(Ok(v1::GetGoalsResponse { goals: sample_goals() })),
    Some(Ok(sample_goal_detail())),
  ));
  let dir = tempfile::tempdir().expect("一時ディレクトリを作れませんでした");
  let config = write_config(&dir, &format!(r#"{{"listen":"{}"}}"#, server.addr));
  let addr = format!("{}", server.addr);
  let config_path = config.display().to_string();
  let run = |args: Vec<String>, env_addr: Result<String, std::env::VarError>| {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run_with(&args, env_addr, &mut stdout, &mut stderr);
    (
      code,
      String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります"),
      String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります"),
    )
  };

  let cases = vec![
    (write_args(&["goals", "--addr", addr.as_str()]), Ok("127.0.0.1:1".to_owned())),
    (write_args(&["goals", "--config", config_path.as_str()]), Err(std::env::VarError::NotPresent)),
    (write_args(&["goal", "--goal", "goal-cli", "--addr", addr.as_str()]), Ok("127.0.0.1:1".to_owned())),
    (
      write_args(&["goal", "--goal", "goal-cli", "--config", config_path.as_str()]),
      Err(std::env::VarError::NotPresent),
    ),
  ];
  for (args, env_addr) in cases {
    let (code, _stdout, stderr) = run(args.clone(), env_addr);
    assert_eq!(code, EXIT_OK, "接続先の指定 {args:?} を受理する必要があります: {stderr}");
    assert!(stderr.is_empty(), "接続先の指定 {args:?} で標準エラーへ出してはいけません: {stderr}");
  }

  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());
  assert_eq!(
    seen_calls,
    vec!["GetGoals".to_owned(), "GetGoals".to_owned(), "GetGoalDetail".to_owned(), "GetGoalDetail".to_owned()],
    "goals は GetGoals、goal は GetGoalDetail を呼ぶ必要があります"
  );
}

/// `grpc-timeout` の値（例: "5000000u"）をマイクロ秒へ直す。
fn grpc_timeout_micros(value: &str) -> u64 {
  let (digits, unit) = value.split_at(value.len() - 1);
  let amount: u64 = digits.parse().expect("grpc-timeout の数値を読めます");
  match unit {
    | "n" => amount / 1_000,
    | "u" => amount,
    | "m" => amount * 1_000,
    | "S" => amount * 1_000_000,
    | "M" => amount * 60 * 1_000_000,
    | "H" => amount * 60 * 60 * 1_000_000,
    | other => panic!("未知の grpc-timeout の単位です: {other}"),
  }
}

#[test]
fn goal_detail_deadline_is_longer_than_the_goal_list_deadline() {
  // GetGoalDetail は backend 側で複数の takt-mcp 呼び出しを直列に行うため、他の読み取りより
  // 長い期限を持たせます。生成クライアントが付ける `grpc-timeout` をダミーサーバーで観測し、
  // ゴール一覧の期限が5秒のままで、詳細の期限がそれより長いことを固定します。
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_dummy_server_with_goals(
    None,
    DashboardState::default(),
    Vec::new(),
    Some(Ok(v1::GetGoalsResponse { goals: sample_goals() })),
    Some(Ok(sample_goal_detail())),
  ));

  let (goals_code, _goals_stdout, goals_stderr) = run_cli(&write_args(&["goals"]), server.addr);
  assert_eq!(goals_code, EXIT_OK, "goals は成功します: {goals_stderr}");
  let (detail_code, _detail_stdout, detail_stderr) = run_cli(&write_args(&["goal", "--goal", "goal-cli"]), server.addr);
  assert_eq!(detail_code, EXIT_OK, "goal は成功します: {detail_stderr}");

  let timeouts = server.timeouts();
  runtime.block_on(server.shutdown());

  let goals_timeout = timeouts
    .iter()
    .find(|(name, _)| name == "GetGoals")
    .map(|(_, value)| grpc_timeout_micros(value))
    .expect("GetGoals の grpc-timeout を記録します");
  let detail_timeout = timeouts
    .iter()
    .find(|(name, _)| name == "GetGoalDetail")
    .map(|(_, value)| grpc_timeout_micros(value))
    .expect("GetGoalDetail の grpc-timeout を記録します");

  assert_eq!(goals_timeout, 5_000_000, "他の読み取りの期限は5秒のままです");
  assert!(detail_timeout > goals_timeout, "ゴール詳細の期限は他の読み取りより長い必要があります");
}

// ----------------------------------------------------------------------------
// CLI-GOAL-FAILURE: 接続不能と NOT_FOUND の扱い（要件 15, 21〜23）
// ----------------------------------------------------------------------------
#[test]
fn goal_commands_fail_when_backend_is_down() {
  let addr = reserve_closed_addr();

  for args in [write_args(&["goals"]), write_args(&["goal", "--goal", "goal-cli"])] {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let code = run_with(&args, Ok(format!("{addr}")), &mut stdout, &mut stderr);

    let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
    let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
    assert_eq!(code, EXIT_FAILURE, "接続できないときは取得の失敗として終わる必要があります: {args:?}");
    assert!(
      stderr.contains("backend に接続できません。"),
      "接続できない旨を標準エラーへ日本語で出す必要があります: {args:?}: {stderr}"
    );
    assert!(stdout.is_empty(), "接続できないときはゴールを標準出力へ出してはいけません: {args:?}: {stdout}");
  }
}

#[test]
fn goals_command_reports_backend_failure_without_connection_message() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_goals_error_server(Status::internal("一覧の射影に失敗しました。")));

  let (code, stdout, stderr) = run_cli(&["goals".to_owned()], server.addr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "backend が応答した失敗は取得の失敗として終わる必要があります");
  assert!(
    stderr.contains("backend からゴール一覧を取得できませんでした"),
    "backend の失敗をゴール一覧の取得失敗として表示する必要があります: {stderr}"
  );
  assert!(stderr.contains("一覧の射影に失敗しました。"), "backend の失敗理由を表示する必要があります: {stderr}");
  assert!(
    !stderr.contains("backend に接続できません。"),
    "backend が応答した失敗を接続不能として表示してはいけません: {stderr}"
  );
  assert!(stdout.is_empty(), "取得に失敗したときはゴール一覧を表示してはいけません: {stdout}");
  assert_eq!(seen_calls, vec!["GetGoals".to_owned()]);
}

#[test]
fn goal_command_reports_backend_failure_without_connection_message() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_goal_detail_error_server(Status::internal("詳細の射影に失敗しました。")));

  let (code, stdout, stderr) = run_cli(&["goal".to_owned(), "--goal".to_owned(), "goal-cli".to_owned()], server.addr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "backend が応答した失敗は取得の失敗として終わる必要があります");
  assert!(
    stderr.contains("backend からゴール") && stderr.contains("取得できませんでした"),
    "backend の失敗をゴールの取得失敗として表示する必要があります: {stderr}"
  );
  assert!(stderr.contains("詳細の射影に失敗しました。"), "backend の失敗理由を表示する必要があります: {stderr}");
  assert!(
    !stderr.contains("backend に接続できません。"),
    "backend が応答した失敗を接続不能として表示してはいけません: {stderr}"
  );
  assert!(stdout.is_empty(), "取得に失敗したときはゴール詳細を表示してはいけません: {stdout}");
  assert_eq!(seen_calls, vec!["GetGoalDetail".to_owned()]);
}

#[test]
fn goal_command_reports_not_found_without_connection_message() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_goal_detail_error_server(Status::not_found("ゴールがありません。")));

  let (code, stdout, stderr) =
    run_cli(&["goal".to_owned(), "--goal".to_owned(), "goal-missing".to_owned()], server.addr);
  let seen_calls = server.seen_calls();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "NOT_FOUND は取得の失敗として終わる必要があります");
  assert!(stderr.contains("ゴールが見つかりません"), "NOT_FOUND を日本語で示す必要があります: {stderr}");
  assert!(!stderr.contains("backend に接続できません。"), "NOT_FOUND を接続不能として表示してはいけません: {stderr}");
  assert!(stdout.is_empty(), "見つからないときはゴール詳細を表示してはいけません: {stdout}");
  assert_eq!(seen_calls, vec!["GetGoalDetail".to_owned()]);
}
