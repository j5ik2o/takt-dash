//! run の集計（`run_progress`）が、本体ログを前回の続きから読んで takt_get_run と同じ規則で
//! step の記録と現在ステップの開始を求めることと、読めない run の失敗の分類を、一時ディレクトリの
//! run の保存場所だけで検証する。

mod common;

use std::path::PathBuf;

use common::*;
use serde_json::json;
use takt_dash_backend::{
  domain::{RunSlug, TaskName},
  model::{RunStatus, TaskEntry, TaskStatus},
  run_log::InitialRead,
  run_progress::{ProjectRunLogs, RunLogHandles, RunReadFailure},
};

const WORKFLOW: &str = "development-core";

struct Project {
  _root: tempfile::TempDir,
  cwd:   PathBuf,
}

impl Project {
  fn new() -> Project {
    let root = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let cwd = std::fs::canonicalize(root.path()).expect("実パスを解決できます").join("project");
    std::fs::create_dir_all(&cwd).expect("プロジェクトの cwd を作れます");
    Project { _root: root, cwd }
  }

  fn fixture(&self, run_slugs: &[&str]) -> RunFixture {
    let fixture = RunFixture::new(&self.cwd);
    let lines: Vec<RunTaskLine<'_>> =
      run_slugs.iter().map(|run_slug| RunTaskLine { run_slug: Some(run_slug), source_run_slug: None }).collect();
    fixture.write_tasks(&lines);
    fixture
  }

  fn logs(&self) -> ProjectRunLogs {
    ProjectRunLogs::new(self.cwd.clone())
  }
}

fn run_slug(value: &str) -> RunSlug {
  RunSlug::try_from(value).expect("テストの run slug は有効です")
}

fn task(started_at: Option<&str>) -> TaskEntry {
  TaskEntry {
    name:            TaskName::try_from("implement").expect("テストのタスク名は有効です"),
    goal_id:         None,
    status:          TaskStatus::Running,
    started_at:      started_at.map(time),
    completed_at:    None,
    run_slug:        Some(run_slug("r1")),
    source_run_slug: None,
    current_step:    None,
    merge_task:      false,
  }
}

fn step_names(logs: &mut ProjectRunLogs, slug: &str) -> Vec<String> {
  logs.read(&run_slug(slug)).expect("run を読めます").step_logs().into_iter().map(|log| log.step_name).collect()
}

#[test]
fn records_before_the_first_workflow_start_are_not_counted() {
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    step_complete_record("orphan", "coder", 1, "2026-11-13T09:01:00Z"),
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:02:00Z"),
  ]);

  assert_eq!(step_names(&mut project.logs(), "r1"), vec!["plan".to_owned()]);
}

#[test]
fn a_later_workflow_start_resets_the_steps_on_the_same_aggregate() {
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:02:00Z"),
  ]);
  let mut logs = project.logs();
  assert_eq!(step_names(&mut logs, "r1"), vec!["plan".to_owned()]);

  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("review", "reviewer", 1, "2026-11-13T09:10:00Z"),
  ]);

  assert_eq!(step_names(&mut logs, "r1"), vec!["review".to_owned()], "後の workflow_start で集計を初期化します");
}

#[test]
fn a_replaced_body_log_is_read_again_and_replaces_the_steps() {
  // 本体ログを別のファイルへ置き換える（inode が変わる）と、読み取り層は先頭から読み直し、
  // 先頭の workflow_start で集計が初期化されるため、前の steps に追記しない。
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:02:00Z"),
    step_complete_record("implement", "coder", 1, "2026-11-13T09:05:00Z"),
  ]);
  let mut logs = project.logs();
  assert_eq!(step_names(&mut logs, "r1").len(), 2);

  let body_log = fixture.body_log("r1");
  let replacement = body_log.with_extension("next");
  std::fs::write(
    &replacement,
    format!(
      "{}\n{}\n",
      workflow_start_record(WORKFLOW),
      step_complete_record("again", "coder", 1, "2026-11-13T09:30:00Z")
    ),
  )
  .expect("置き換えのファイルを書けます");
  std::fs::rename(&replacement, &body_log).expect("本体ログを置き換えられます");

  assert_eq!(step_names(&mut logs, "r1"), vec!["again".to_owned()]);
}

#[test]
fn execution_count_counts_top_level_starts_of_the_current_step_and_skips_parallel_children() {
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 2, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut parallel_child = step_start_record("implement", WORKFLOW, 2, "2026-11-13T09:20:00Z", &["implement"]);
  parallel_child["parallel"] = json!({ "role": "child" });
  let mut parallel_parent = step_start_record("implement", WORKFLOW, 2, "2026-11-13T09:10:00Z", &["implement"]);
  parallel_parent["parallel"] = json!({ "role": "parent" });
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    step_start_record("implement", WORKFLOW, 1, "2026-11-13T09:01:00Z", &["implement"]),
    step_start_record("implement", "other-workflow", 2, "2026-11-13T09:05:00Z", &["implement"]),
    parallel_parent,
    parallel_child,
  ]);

  let view = project.logs().read(&run_slug("r1")).expect("run を読めます");

  assert_eq!(view.current_step_execution_count, Some(2), "iteration 1 の開始と parallel の親を数えます");
  assert_eq!(
    view.current_step_started_at,
    Some(time("2026-11-13T09:10:00Z")),
    "現在の iteration の最上位の開始を採り、parallel の子と別 workflow は採りません"
  );
}

#[test]
fn nested_leaf_under_the_current_step_sets_the_start_when_no_top_level_start_matches() {
  // 現在ステップ develop の中で入れ子の workflow が動いているとき、最上位の一致が無ければ、
  // 外側の step が develop の最後の step_start を開始とする。
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("develop", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    step_start_record("write_tests", "development-core", 1, "2026-11-13T09:01:00Z", &["develop", "write_tests"]),
    step_start_record("write_tests", "development-core", 2, "2026-11-13T09:08:00Z", &["develop", "write_tests"]),
  ]);

  let view = project.logs().read(&run_slug("r1")).expect("run を読めます");

  assert_eq!(view.current_step_started_at, Some(time("2026-11-13T09:08:00Z")));
  assert_eq!(view.current_step_execution_count, Some(2), "入れ子の葉と同じ step と workflow の開始を数えます");
}

#[test]
fn execution_count_is_unset_without_any_step_start() {
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[workflow_start_record(WORKFLOW)]);

  let view = project.logs().read(&run_slug("r1")).expect("run を読めます");

  assert_eq!(view.current_step_execution_count, None);
  assert_eq!(view.current_step_started_at, None);
}

#[test]
fn run_entry_uses_the_meta_start_time_and_falls_back_to_the_task_start() {
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  let mut meta = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  fixture.write_meta("r1", &meta);
  let mut logs = project.logs();

  let with_meta =
    logs.read(&run_slug("r1")).expect("run を読めます").run_entry(&run_slug("r1"), &task(Some("2026-11-13T08:00:00Z")));
  assert_eq!(with_meta.started_at, Some(time("2026-11-13T09:00:00Z")), "meta.json の startTime を優先します");
  assert_eq!(with_meta.last_log_at, Some(time("2026-11-13T09:00:00Z")), "ログが無ければ開始時刻です");

  meta.as_object_mut().expect("meta はオブジェクトです").remove("startTime");
  fixture.write_meta("r1", &meta);
  let without_meta =
    logs.read(&run_slug("r1")).expect("run を読めます").run_entry(&run_slug("r1"), &task(Some("2026-11-13T08:00:00Z")));
  assert_eq!(without_meta.started_at, Some(time("2026-11-13T08:00:00Z")), "startTime が無ければタスクの開始時刻です");
  assert_eq!(without_meta.status, RunStatus::Running);
}

#[test]
fn read_failures_are_classified_by_their_cause() {
  let project = Project::new();
  let fixture = project.fixture(&["r-missing", "r-broken", "r-no-status", "r-unknown", "r-bad-time"]);
  fixture.write_meta_raw("r-broken", "{ this is not json");
  let mut no_status = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  no_status.as_object_mut().expect("meta はオブジェクトです").remove("status");
  fixture.write_meta("r-no-status", &no_status);
  let mut unknown = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  unknown["status"] = json!("queued");
  fixture.write_meta("r-unknown", &unknown);
  fixture.write_meta("r-bad-time", &running_meta("implement", 1, WORKFLOW, "not-a-time"));
  let mut logs = project.logs();

  assert_eq!(logs.read(&run_slug("r-not-listed")), Err(RunReadFailure::NotListed));
  assert_eq!(logs.read(&run_slug("r-missing")), Err(RunReadFailure::Missing));
  assert!(matches!(logs.read(&run_slug("r-broken")), Err(RunReadFailure::Unreadable(_))));
  assert!(matches!(logs.read(&run_slug("r-no-status")), Err(RunReadFailure::InvalidMeta(_))));
  assert!(matches!(logs.read(&run_slug("r-unknown")), Err(RunReadFailure::InvalidMeta(_))));
  assert!(matches!(logs.read(&run_slug("r-bad-time")), Err(RunReadFailure::InvalidMeta(_))));
}

#[test]
fn an_unreadable_task_table_is_unreadable() {
  let project = Project::new();

  assert!(matches!(project.logs().read(&run_slug("r1")), Err(RunReadFailure::Unreadable(_))));
}

#[test]
fn retain_drops_the_aggregate_of_runs_that_are_no_longer_kept() {
  // 捨てた run を次に読むと先頭から読み直すため、既読範囲の書き換えが見える。残した run は
  // 前回の続きから読むため、書き換えは見えない。
  let project = Project::new();
  let fixture = project.fixture(&["kept", "dropped"]);
  for slug in ["kept", "dropped"] {
    fixture.write_meta(slug, &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
    fixture.append_records(slug, &[
      workflow_start_record(WORKFLOW),
      step_complete_record("plan", "architect", 1, "2026-11-13T09:02:00Z"),
    ]);
  }
  let mut logs = project.logs();
  step_names(&mut logs, "kept");
  step_names(&mut logs, "dropped");

  logs.retain(&[run_slug("kept")]);
  for slug in ["kept", "dropped"] {
    fixture.overwrite_body_log_in_place(slug, "\"plan\"", "\"PLAN\"");
  }

  assert_eq!(step_names(&mut logs, "kept"), vec!["plan".to_owned()]);
  assert_eq!(step_names(&mut logs, "dropped"), vec!["PLAN".to_owned()]);
}

#[test]
fn the_least_recently_read_aggregate_is_dropped_beyond_the_limit() {
  let project = Project::new();
  let slugs: Vec<String> = (0..33).map(|index| format!("r{index}")).collect();
  let slug_refs: Vec<&str> = slugs.iter().map(String::as_str).collect();
  let fixture = project.fixture(&slug_refs);
  for slug in &slug_refs {
    fixture.write_meta(slug, &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
    fixture.append_records(slug, &[
      workflow_start_record(WORKFLOW),
      step_complete_record("plan", "architect", 1, "2026-11-13T09:02:00Z"),
    ]);
  }
  let mut logs = project.logs();
  for slug in &slug_refs {
    step_names(&mut logs, slug);
  }
  fixture.overwrite_body_log_in_place("r0", "\"plan\"", "\"PLAN\"");
  fixture.overwrite_body_log_in_place("r32", "\"plan\"", "\"PLAN\"");

  assert_eq!(step_names(&mut logs, "r32"), vec!["plan".to_owned()], "最近読んだ run の集計は残ります");
  assert_eq!(step_names(&mut logs, "r0"), vec!["PLAN".to_owned()], "33 件目を読んだときに最も古い r0 を捨てます");
}

/// 計画が決めた、run 詳細の初回に読む本体ログの末尾の上限（1 MiB）。
const TAIL_BYTES: u64 = 1024 * 1024;
const LARGE_BODY_LOG_BYTES: u64 = 9 * 1024 * 1024;

/// 先頭付近に head-marker、9 MiB の集計対象外の record を挟んで末尾に tail-marker の step_complete
/// を 持つ実行中の run r1 を作る。
fn large_run(project: &Project) -> RunFixture {
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("head-marker", "architect", 1, "2026-11-13T09:01:00Z"),
  ]);
  append_filler_records(&fixture, "r1", LARGE_BODY_LOG_BYTES);
  fixture.append_records("r1", &[step_complete_record("tail-marker", "coder", 1, "2026-11-13T09:30:00Z")]);
  fixture
}

fn read_with(handles: &mut RunLogHandles<String>, project: &Project) -> takt_dash_backend::run_progress::RunLogView {
  let handle = handles.handle_for(&"takt-dash".to_owned(), &project.cwd);
  let mut logs = handle.lock().expect("集計を取得できます");
  logs.read(&run_slug("r1")).expect("run を読めます")
}

fn names(view: &takt_dash_backend::run_progress::RunLogView) -> Vec<String> {
  view.step_logs().into_iter().map(|log| log.step_name).collect()
}

#[test]
fn a_tail_handle_reads_at_most_the_tail_bytes_of_a_large_body_log_on_the_first_read() {
  // 末尾読みの取っ手は、読み位置を持たない初回に本体ログの末尾 1 MiB 以内だけを読み、先頭付近の
  // head-marker を集計しない。途中から読み始めても集計を始め、tail-marker を集計する。
  let project = Project::new();
  let fixture = large_run(&project);
  let size = std::fs::metadata(fixture.body_log("r1")).expect("本体ログの大きさを読めます").len();
  assert!(size > LARGE_BODY_LOG_BYTES, "fixture の本体ログは 9 MiB を超えます: {size}");
  let mut handles = RunLogHandles::with_initial_read(InitialRead::Tail(TAIL_BYTES));

  let view = read_with(&mut handles, &project);

  assert!(view.body_log_bytes_read <= TAIL_BYTES, "初回に読んだのは末尾の上限以内です: {}", view.body_log_bytes_read);
  assert!(view.body_log_bytes_read > 0, "末尾の範囲は読みます");
  assert_eq!(names(&view), vec!["tail-marker".to_owned()]);
}

#[test]
fn a_tail_handle_reads_only_the_appended_bytes_on_the_next_read() {
  // 末尾読みの取っ手も、2回目は前回の続き（追記した分）だけを読み、集計に積み上げる。
  let project = Project::new();
  let fixture = large_run(&project);
  let mut handles = RunLogHandles::with_initial_read(InitialRead::Tail(TAIL_BYTES));
  read_with(&mut handles, &project);
  let appended = step_complete_record("appended", "coder", 2, "2026-11-13T09:40:00Z");
  let appended_bytes = appended.to_string().len() as u64 + 1;
  fixture.append_records("r1", &[appended]);

  let view = read_with(&mut handles, &project);

  assert_eq!(view.body_log_bytes_read, appended_bytes, "2回目に読むのは追記した1行だけです");
  assert_eq!(names(&view), vec!["tail-marker".to_owned(), "appended".to_owned()]);
}

#[test]
fn the_default_handle_still_reads_the_body_log_from_the_beginning() {
  // 定期読み取り（poller）が使う既定の取っ手は、従来どおり先頭から読み、head-marker も集計する。
  let project = Project::new();
  let fixture = large_run(&project);
  let size = std::fs::metadata(fixture.body_log("r1")).expect("本体ログの大きさを読めます").len();
  let mut handles = RunLogHandles::<String>::default();

  let view = read_with(&mut handles, &project);

  assert_eq!(view.body_log_bytes_read, size, "既定の取っ手は本体ログの全体を読みます");
  assert_eq!(names(&view), vec!["head-marker".to_owned(), "tail-marker".to_owned()]);
}

fn tail_names_after(handles: &mut RunLogHandles<String>, project: &Project) -> (Vec<String>, String) {
  let view = read_with(handles, project);
  (names(&view), view.log_tail())
}

#[test]
fn a_tail_handle_drops_the_old_steps_when_the_body_log_is_replaced_without_a_workflow_start() {
  // 末尾読みの取っ手で読んだあと、本体ログを workflow_start の無い別のファイル（上限より大きい）へ
  // 置き換える。読み直した集計は置き換え後の記録だけで、置き換え前の tail-marker を残さない。
  let project = Project::new();
  let fixture = large_run(&project);
  let mut handles = RunLogHandles::with_initial_read(InitialRead::Tail(TAIL_BYTES));
  let (before, _) = tail_names_after(&mut handles, &project);
  assert_eq!(before, vec!["tail-marker".to_owned()]);
  let body_log = fixture.body_log("r1");
  let replacement = body_log.with_extension("next");
  std::fs::rename(&body_log, &replacement).expect("本体ログを退避できます");
  append_filler_records(&fixture, "r1", 2 * TAIL_BYTES);
  fixture.append_records("r1", &[step_complete_record("after-replace", "coder", 2, "2026-11-13T10:00:00Z")]);
  std::fs::remove_file(&replacement).expect("退避した本体ログを消せます");

  let (after, log_tail) = tail_names_after(&mut handles, &project);

  assert_eq!(after, vec!["after-replace".to_owned()], "置き換え後の記録だけを集計します");
  assert!(
    !log_tail.lines().any(|line| line.split(' ').any(|part| part == "tail-marker")),
    "ログ末尾に置き換え前の記録を残しません: {log_tail:?}"
  );
}

#[test]
fn a_tail_handle_drops_the_old_steps_when_the_body_log_shrinks() {
  // 同じファイル（inode は同じ）を、読んだ位置より小さく書き直す。読み直した集計は書き直した後の
  // 記録だけです。
  let project = Project::new();
  let fixture = large_run(&project);
  let mut handles = RunLogHandles::with_initial_read(InitialRead::Tail(TAIL_BYTES));
  tail_names_after(&mut handles, &project);
  let body_log = fixture.body_log("r1");
  let inode_before = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&body_log).expect("大きさを読めます"));
  std::fs::OpenOptions::new().write(true).truncate(true).open(&body_log).expect("本体ログを空にできます");
  append_filler_records(&fixture, "r1", 2 * TAIL_BYTES);
  fixture.append_records("r1", &[step_complete_record("after-shrink", "coder", 2, "2026-11-13T10:00:00Z")]);
  let inode_after = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&body_log).expect("大きさを読めます"));
  assert_eq!(inode_before, inode_after, "同じファイルを縮めた状態です");

  let (after, log_tail) = tail_names_after(&mut handles, &project);

  assert_eq!(after, vec!["after-shrink".to_owned()], "縮めた後の記録だけを集計します");
  assert!(
    !log_tail.lines().any(|line| line.split(' ').any(|part| part == "tail-marker")),
    "ログ末尾に縮める前の記録を残しません: {log_tail:?}"
  );
}

/// 末尾読みの取っ手で、小さな本体ログ（session.jsonl）を持つ実行中の run r1 を作る。
fn small_run(project: &Project) -> RunFixture {
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    step_complete_record("plan", "architect", 1, "2026-11-13T09:02:00Z"),
  ]);
  fixture
}

#[test]
fn a_tail_handle_keeps_the_steps_when_a_new_body_log_file_appears() {
  // 末尾読みの取っ手で session.jsonl を読み終えた後、logs/ に新しい本体ログが現れる。同じ取っ手で
  // 読み直した集計は、新しく作った取っ手が読む結果と一致し、session.jsonl の plan とその行を
  // 失わない。新しいファイルの記録だけを残す実装は plan を失って落ちる。
  let project = Project::new();
  let fixture = small_run(&project);
  let mut handles = RunLogHandles::with_initial_read(InitialRead::Tail(TAIL_BYTES));
  let (before, _) = tail_names_after(&mut handles, &project);
  assert_eq!(before, vec!["plan".to_owned()]);
  std::fs::write(fixture.run_dir("r1").join("logs").join("usage-events.jsonl"), format!("{}\n", filler_record(0, 16)))
    .expect("新しい本体ログを書けます");

  let view = read_with(&mut handles, &project);
  let fresh = read_with(&mut RunLogHandles::with_initial_read(InitialRead::Tail(TAIL_BYTES)), &project);

  assert_eq!(names(&view), vec!["plan".to_owned()], "新しいファイルが現れても既読の step を残します");
  assert!(
    view.log_tail().lines().any(|line| line.split(' ').any(|part| part == "plan")),
    "ログ末尾に session.jsonl の行を残します: {:?}",
    view.log_tail()
  );
  assert_eq!(view.step_logs(), fresh.step_logs(), "集計は新しく作った取っ手の結果と一致します");
  assert_eq!(view.log_tail(), fresh.log_tail(), "ログ末尾は新しく作った取っ手の結果と一致します");
}

#[test]
fn a_tail_handle_does_not_read_again_without_any_new_body_log_file() {
  // 変化の無い本体ログを同じ取っ手でもう一度読んでも、読み直さず、集計も変わらない。
  let project = Project::new();
  small_run(&project);
  let mut handles = RunLogHandles::with_initial_read(InitialRead::Tail(TAIL_BYTES));
  let first = read_with(&mut handles, &project);

  let second = read_with(&mut handles, &project);

  assert_eq!(second.body_log_bytes_read, 0, "変化が無ければ本体ログを読みません");
  assert_eq!(second.step_logs(), first.step_logs());
  assert_eq!(second.log_tail(), first.log_tail());
}

#[test]
fn run_entry_summary_is_the_first_line_of_the_meta_task() {
  // 一覧の run の要約は、meta.json の task を takt のタスク要約と同じ規則（前後の空白を除き、
  // 1行目の先頭 80 文字）で縮めたもの。空のままにする実装も、全文を入れる実装も落ちる。
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  let mut meta = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  meta["task"] = json!("\n  認証処理を実装する\n詳細: トークンの検証を加える\n");
  fixture.write_meta("r1", &meta);
  let mut logs = project.logs();

  let entry = logs.read(&run_slug("r1")).expect("run を読めます").run_entry(&run_slug("r1"), &task(None));

  assert_eq!(entry.summary, "認証処理を実装する");
}

#[test]
fn run_entry_summary_is_cut_at_eighty_characters() {
  // 1行目が 80 文字を超えるときは、先頭 80 文字（バイトではなく文字）で切る。
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  let mut meta = running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z");
  let long_line = "あ".repeat(79) + "いう";
  meta["task"] = json!(long_line);
  fixture.write_meta("r1", &meta);
  let mut logs = project.logs();

  let entry = logs.read(&run_slug("r1")).expect("run を読めます").run_entry(&run_slug("r1"), &task(None));

  assert_eq!(entry.summary, "あ".repeat(79) + "い");
}

#[test]
fn run_entry_summary_is_empty_without_a_meta_task() {
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut logs = project.logs();

  let entry = logs.read(&run_slug("r1")).expect("run を読めます").run_entry(&run_slug("r1"), &task(None));

  assert_eq!(entry.summary, "");
}

#[test]
fn run_entry_step_model_is_the_model_of_the_matching_step_start() {
  // 各ステップの model は、その step_complete より前で最も新しい同名・同 iteration の step_start の
  // model。最後の step_start の model を全ステップへ入れる実装は plan が sonnet になって落ちる。
  // model を持たない step_start のステップは空です。
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("review", 1, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut plan_start = step_start_record("plan", WORKFLOW, 1, "2026-11-13T09:01:00Z", &["plan"]);
  plan_start["model"] = json!("opus");
  let mut implement_start = step_start_record("implement", WORKFLOW, 1, "2026-11-13T09:03:00Z", &["implement"]);
  implement_start["model"] = json!("sonnet");
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    plan_start,
    step_complete_record("plan", "architect", 1, "2026-11-13T09:02:00Z"),
    implement_start,
    step_complete_record("implement", "coder", 1, "2026-11-13T09:04:00Z"),
    step_start_record("review", WORKFLOW, 1, "2026-11-13T09:05:00Z", &["review"]),
    step_complete_record("review", "reviewer", 1, "2026-11-13T09:06:00Z"),
  ]);
  let mut logs = project.logs();

  let entry = logs.read(&run_slug("r1")).expect("run を読めます").run_entry(&run_slug("r1"), &task(None));

  let models: Vec<(String, String)> =
    entry.steps.iter().map(|step| (step.step_name.clone(), step.model.clone())).collect();
  assert_eq!(models, vec![
    ("plan".to_owned(), "opus".to_owned()),
    ("implement".to_owned(), "sonnet".to_owned()),
    ("review".to_owned(), String::new()),
  ]);
}

#[test]
fn run_entry_step_model_uses_the_start_of_the_same_iteration() {
  // 同じ step が iteration を変えて2回動いたとき、各 step_complete の model はその iteration の
  // step_start のもの。run の最後の step_start を採る実装は iteration 1 が haiku になって落ちる。
  let project = Project::new();
  let fixture = project.fixture(&["r1"]);
  fixture.write_meta("r1", &running_meta("implement", 2, WORKFLOW, "2026-11-13T09:00:00Z"));
  let mut first_start = step_start_record("implement", WORKFLOW, 1, "2026-11-13T09:01:00Z", &["implement"]);
  first_start["model"] = json!("opus");
  let mut second_start = step_start_record("implement", WORKFLOW, 2, "2026-11-13T09:03:00Z", &["implement"]);
  second_start["model"] = json!("haiku");
  fixture.append_records("r1", &[
    workflow_start_record(WORKFLOW),
    first_start,
    step_complete_record("implement", "coder", 1, "2026-11-13T09:02:00Z"),
    second_start,
    step_complete_record("implement", "coder", 2, "2026-11-13T09:04:00Z"),
  ]);
  let mut logs = project.logs();

  let entry = logs.read(&run_slug("r1")).expect("run を読めます").run_entry(&run_slug("r1"), &task(None));

  let models: Vec<String> = entry.steps.iter().map(|step| step.model.clone()).collect();
  assert_eq!(models, vec!["opus".to_owned(), "haiku".to_owned()]);
}
