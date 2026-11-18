//! ドメインの値オブジェクトが、size → lexical content → syntax → semantics の順で
//! 不正な値の生成を拒否し、正しい値を受け入れることを検証する。
//!
//! 型の定義は `backend/src/domain.rs` に置く。この工程では本番コードを追加しないため、
//! `domain` モジュールが入るまではこのテストターゲットだけがコンパイルできない。

use takt_dash_backend::{
  domain::{
    DailyDate, DomainError, GoalId, InquiryId, InstructionId, MergeUntil, ProjectName, PullRequestNumber,
    RegistrationRevision, ReportFilename, RunSlug, ServerId, SilentMinutes, StaleMinutes, TaskCount, TaskName,
  },
  model::RunStatus,
};

fn assert_size<T: std::fmt::Debug>(result: Result<T, DomainError>, label: &str) {
  assert!(matches!(result, Err(DomainError::Size(_))), "{label} は size 段で拒否します: {result:?}");
}

fn assert_lexical<T: std::fmt::Debug>(result: Result<T, DomainError>, label: &str) {
  assert!(matches!(result, Err(DomainError::Lexical(_))), "{label} は lexical content 段で拒否します: {result:?}");
}

fn assert_syntax<T: std::fmt::Debug>(result: Result<T, DomainError>, label: &str) {
  assert!(matches!(result, Err(DomainError::Syntax(_))), "{label} は syntax 段で拒否します: {result:?}");
}

fn assert_semantics<T: std::fmt::Debug>(result: Result<T, DomainError>, label: &str) {
  assert!(matches!(result, Err(DomainError::Semantics(_))), "{label} は semantics 段で拒否します: {result:?}");
}

#[test]
fn server_id_validates_size_and_lexical_content_and_keeps_the_value() {
  assert_size(ServerId::try_from(""), "空のサーバー ID");
  assert_lexical(ServerId::try_from("   "), "空白だけのサーバー ID");
  for value in ["mac\nmini", "mac\u{7f}mini", "mac\u{0}mini"] {
    assert_lexical(ServerId::try_from(value), &format!("制御文字を含むサーバー ID {value:?}"));
  }

  let server_id = ServerId::try_from("mac-mini").expect("有効なサーバー ID を受け入れます");
  assert_eq!(server_id.as_str(), "mac-mini");
}

#[test]
fn project_name_validates_size_and_lexical_content_and_keeps_the_value() {
  assert_size(ProjectName::try_from(""), "空のプロジェクト名");
  assert_lexical(ProjectName::try_from("   "), "空白だけのプロジェクト名");
  for value in ["takt\ndash", "takt\u{7f}dash", "takt\u{0}dash"] {
    assert_lexical(ProjectName::try_from(value), &format!("制御文字を含むプロジェクト名 {value:?}"));
  }

  let name = ProjectName::try_from("takt-dash").expect("有効なプロジェクト名を受け入れます");
  assert_eq!(name.as_str(), "takt-dash");
}

#[test]
fn goal_id_validates_size_and_lexical_content_and_keeps_the_value() {
  assert_size(GoalId::try_from(""), "空のゴール ID");
  assert_lexical(GoalId::try_from(" "), "空白だけのゴール ID");
  assert_lexical(GoalId::try_from("g1\n"), "制御文字を含むゴール ID");

  let goal_id = GoalId::try_from("3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11").expect("有効なゴール ID を受け入れます");
  assert_eq!(goal_id.as_str(), "3f9c1a54-2b41-4d1e-9d2e-2c1a5b8f7e11");
}

#[test]
fn task_name_validates_size_and_lexical_content_and_keeps_the_value() {
  assert_size(TaskName::try_from(""), "空のタスク名");
  assert_lexical(TaskName::try_from("\t"), "空白だけのタスク名");
  assert_lexical(TaskName::try_from("実装\u{7f}"), "制御文字を含むタスク名");

  let task_name = TaskName::try_from("実装").expect("有効なタスク名を受け入れます");
  assert_eq!(task_name.as_str(), "実装");
}

#[test]
fn inquiry_id_validates_size_and_lexical_content_and_keeps_the_value() {
  assert_size(InquiryId::try_from(""), "空の inquiry ID");
  assert_lexical(InquiryId::try_from("  "), "空白だけの inquiry ID");
  assert_lexical(InquiryId::try_from("q1\t"), "制御文字を含む inquiry ID");

  let inquiry_id =
    InquiryId::try_from("8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e").expect("有効な inquiry ID を受け入れます");
  assert_eq!(inquiry_id.as_str(), "8f14e45f-ceea-4f6a-9a3c-0f1a2b3c4d5e");
}

#[test]
fn run_slug_validates_size_lexical_content_and_syntax_and_keeps_the_value() {
  assert_size(RunSlug::try_from(""), "空の run slug");
  assert_lexical(RunSlug::try_from("  "), "空白だけの run slug");
  assert_lexical(RunSlug::try_from("run\n1"), "制御文字を含む run slug");
  for value in ["runs/1", "runs\\1", ".", ".."] {
    assert_syntax(RunSlug::try_from(value), &format!("run ディレクトリ名にならない {value:?}"));
  }

  let run_slug = RunSlug::try_from("20261116-000509-issue-3").expect("有効な run slug を受け入れます");
  assert_eq!(run_slug.as_str(), "20261116-000509-issue-3");
}

#[test]
fn pull_request_number_rejects_zero_and_negative_values() {
  assert_size(PullRequestNumber::try_from(0_i64), "0 のプルリクエスト番号");
  assert_size(PullRequestNumber::try_from(-1_i64), "負のプルリクエスト番号");

  assert_eq!(PullRequestNumber::try_from(1_i64).expect("1 は有効です").get(), 1);
  assert_eq!(PullRequestNumber::try_from(i64::MAX).expect("i64 の上限は有効です").get(), i64::MAX);
}

#[test]
fn registration_revision_rejects_zero_and_negative_values() {
  assert_size(RegistrationRevision::try_from(0_i32), "0 の revision");
  assert_size(RegistrationRevision::try_from(-1_i32), "負の revision");

  assert_eq!(RegistrationRevision::try_from(1_i32).expect("1 は有効です").get(), 1);
  assert_eq!(RegistrationRevision::try_from(i32::MAX).expect("i32 の上限は有効です").get(), i32::MAX);
}

#[test]
fn stale_minutes_accepts_zero_and_rejects_values_beyond_int32() {
  assert_eq!(StaleMinutes::try_from(0_u32).expect("0 分は有効です").get(), 0);
  assert_eq!(StaleMinutes::try_from(i32::MAX as u32).expect("i32 の上限は有効です").get(), i32::MAX);

  assert_size(StaleMinutes::try_from(i32::MAX as u32 + 1), "i32 の上限を超える滞留の閾値");
}

#[test]
fn silent_minutes_accepts_zero_and_rejects_values_beyond_int32() {
  assert_eq!(SilentMinutes::try_from(0_u32).expect("0 分は有効です").get(), 0);
  assert_eq!(SilentMinutes::try_from(i32::MAX as u32).expect("i32 の上限は有効です").get(), i32::MAX);

  assert_size(SilentMinutes::try_from(i32::MAX as u32 + 1), "i32 の上限を超える無音の閾値");
}

#[test]
fn task_count_rejects_negative_values_and_rejects_values_beyond_int32() {
  assert_size(TaskCount::try_from(-1_i64), "負のタスク件数");
  assert_size(TaskCount::try_from(i32::MAX as i64 + 1), "i32 の上限を超えるタスク件数");

  assert_eq!(TaskCount::try_from(0_i64).expect("0 件は有効です").get(), 0);
  assert_eq!(TaskCount::try_from(i32::MAX as i64).expect("i32 の上限は有効です").get(), i32::MAX);
}

#[test]
fn daily_date_validates_each_stage_and_keeps_the_value() {
  assert_size(DailyDate::try_from(""), "空の日付");
  assert_size(DailyDate::try_from("2026-1-01"), "10 文字でない日付");
  assert_lexical(DailyDate::try_from("2026/01/01"), "数字と - 以外を含む日付");
  assert_lexical(DailyDate::try_from("2026-01-0a"), "数字以外を含む日付");
  assert_syntax(DailyDate::try_from("20260-1-01"), "YYYY-MM-DD にならない日付");
  assert_semantics(DailyDate::try_from("2026-13-01"), "存在しない月の日付");
  assert_semantics(DailyDate::try_from("2026-02-30"), "存在しない日の日付");

  let date = DailyDate::try_from("2024-02-29").expect("うるう日は有効です");
  assert_eq!(date.as_str(), "2024-02-29");
  assert!(DailyDate::try_from("2026-02-29").is_err(), "平年の 2 月 29 日は拒否します");
}

#[test]
fn merge_until_accepts_only_prepare_and_merge() {
  assert_size(MergeUntil::try_from(""), "空の until");
  assert_lexical(MergeUntil::try_from("   "), "空白だけの until");
  assert_lexical(MergeUntil::try_from("Prepare"), "大文字を含む until");
  assert_syntax(MergeUntil::try_from("unknown"), "既知の 2 値でない until");

  assert_eq!(MergeUntil::try_from("prepare").expect("prepare は有効です").as_str(), "prepare");
  assert_eq!(MergeUntil::try_from("merge").expect("merge は有効です").as_str(), "merge");
}

#[test]
fn run_status_accepts_only_the_known_values() {
  for (text, expected) in [
    ("running", RunStatus::Running),
    ("completed", RunStatus::Completed),
    ("aborted", RunStatus::Aborted),
    ("failed", RunStatus::Failed),
  ] {
    assert_eq!(RunStatus::try_from(text).expect("既知の run の状態を受け入れます"), expected);
  }

  assert!(RunStatus::try_from("").is_err(), "空の run の状態は拒否します");
  assert!(RunStatus::try_from("Running").is_err(), "大文字を含む run の状態は拒否します");
  assert!(RunStatus::try_from("queued").is_err(), "未知の run の状態は拒否します");
}

#[test]
fn report_filename_accepts_a_nested_markdown_path_and_keeps_the_value() {
  for value in ["plan.md", "sub/review.md", ".hidden/notes.md"] {
    let filename = ReportFilename::try_from(value).expect("reports/ の下の .md の相対パスを受け入れます");
    assert_eq!(filename.as_str(), value);
  }
}

#[test]
fn report_filename_rejects_names_that_leave_the_reports_directory_or_are_not_markdown() {
  assert_size(ReportFilename::try_from(""), "空のレポートの名前");
  assert_size(ReportFilename::try_from(format!("{}.md", "a".repeat(1024)).as_str()), "長すぎるレポートの名前");
  assert_lexical(ReportFilename::try_from("   "), "空白だけのレポートの名前");
  assert_lexical(ReportFilename::try_from("plan\n.md"), "制御文字を含むレポートの名前");
  for value in ["/etc/hosts.md", "../meta.md", "sub/../../meta.md", "./plan.md", "sub//plan.md", "sub\\plan.md", "sub/"]
  {
    assert_syntax(ReportFilename::try_from(value), &format!("区切りが不正なレポートの名前 {value:?}"));
  }
  for value in ["plan.txt", "meta.json", "plan.md.bak"] {
    assert_semantics(ReportFilename::try_from(value), &format!(".md で終わらないレポートの名前 {value:?}"));
  }
}

#[test]
fn instruction_id_rejects_zero_negative_and_out_of_range_values() {
  assert_size(InstructionId::try_from(0_i32), "0 の指示書の ID");
  assert_size(InstructionId::try_from(-1_i32), "負の指示書の ID");
  assert_size(InstructionId::try_from(i64::from(i32::MAX) + 1), "i32 を超える指示書の ID");
  assert_eq!(InstructionId::try_from(2_i32).expect("正の ID を受け入れます").get(), 2);
  assert_eq!(InstructionId::try_from(3_i64).expect("正の ID を受け入れます").get(), 3);
}
