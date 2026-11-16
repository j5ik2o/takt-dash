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
  run_progress::{ProjectRunLogs, RunReadFailure},
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
