//! `.takt` 配下の run の読み取り層（場所の解決・meta の読み取り・本体ログの追記読み・
//! ログ群の最新更新時刻）を、一時ディレクトリだけで検証する。
//!
//! fixture は実物の本体ログのキー構成（type、step、phase、phaseName、phaseExecutionId、
//! timestamp、workflow、stack）に合わせた合成データを使い、実在のプロンプト本文は含めない。
//! 実ホームや実在の `takt-worktrees` は参照しない。

use std::{
  fs,
  io::Write,
  os::unix::fs::symlink,
  path::{Path, PathBuf},
  time::{Duration, UNIX_EPOCH},
};

use serde_json::{json, Value};
use takt_dash_backend::{
  domain::{DomainError, RunSlug},
  run_log::{BodyLogRead, MetaRead, RunLocation, RunLogReader, RunMeta},
};

const MAX_SUMMARY_CHARS: usize = 512;
const PHASE1_DIRECTORY: &str = "phase1-execution";

const BODY_MTIME: u64 = 1_700_000_000;
const PHASE1_MTIME: u64 = 1_700_000_100;
const IGNORED_MTIME: u64 = 1_700_000_300;

struct TaskEntry<'a> {
  run_slug:        &'a str,
  source_run_slug: Option<&'a str>,
  worktree_path:   Option<&'a Path>,
}

struct Fixture {
  _root:          tempfile::TempDir,
  project_cwd:    PathBuf,
  worktrees_root: PathBuf,
}

impl Fixture {
  fn new() -> Fixture {
    let root = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let project_cwd = root.path().join("project");
    let worktrees_root = root.path().join("worktrees");
    fs::create_dir_all(project_cwd.join(".takt")).expect("プロジェクトの .takt を作れます");
    fs::create_dir_all(&worktrees_root).expect("worktree 置き場を作れます");
    Fixture {
      _root:          root,
      project_cwd:    canonical(&project_cwd),
      worktrees_root: canonical(&worktrees_root),
    }
  }

  fn reader(&self) -> RunLogReader {
    RunLogReader::new(self.project_cwd.clone(), self.worktrees_root.clone())
  }

  fn tasks_path(&self) -> PathBuf {
    self.project_cwd.join(".takt").join("tasks.yaml")
  }

  fn write_tasks(&self, entries: &[TaskEntry<'_>]) {
    let mut yaml = String::from("tasks:\n");
    for entry in entries {
      yaml.push_str(&format!("  - run_slug: {}\n", entry.run_slug));
      if let Some(source) = entry.source_run_slug {
        yaml.push_str(&format!("    source_run_slug: {source}\n"));
      }
      if let Some(path) = entry.worktree_path {
        yaml.push_str(&format!("    worktree_path: {}\n", path.display()));
      }
    }
    self.write_tasks_raw(&yaml);
  }

  fn write_tasks_raw(&self, content: &str) {
    fs::write(self.tasks_path(), content).expect("tasks.yaml を書けます");
  }

  fn temp_root(&self) -> PathBuf {
    self.project_cwd.parent().expect("プロジェクトの親があります").to_path_buf()
  }

  fn worktree(&self, name: &str) -> PathBuf {
    let path = self.worktrees_root.join(name);
    fs::create_dir_all(&path).expect("worktree を作れます");
    canonical(&path)
  }

  fn create_run(&self, base: &Path, slug: &str) -> PathBuf {
    let run_dir = base.join(".takt").join("runs").join(slug);
    fs::create_dir_all(run_dir.join("logs")).expect("run の logs ディレクトリを作れます");
    run_dir
  }

  fn write_meta(&self, run_dir: &Path) {
    fs::write(
      run_dir.join("meta.json"),
      r#"{"status":"in_progress","currentStep":"implement","currentIteration":1,"phase":1,"updatedAt":"2026-11-16T01:00:00Z"}"#,
    )
    .expect("meta.json を書けます");
  }
}

fn run_slug(value: &str) -> RunSlug {
  RunSlug::try_from(value).expect("テストの run slug は有効です")
}

fn canonical(path: &Path) -> PathBuf {
  fs::canonicalize(path).expect("実パスを解決できます")
}

fn body_log(run_dir: &Path) -> PathBuf {
  run_dir.join("logs").join("body.jsonl")
}

fn append_bytes(path: &Path, bytes: &[u8]) {
  let mut file = fs::OpenOptions::new().append(true).create(true).open(path).expect("本体ログを開けます");
  file.write_all(bytes).expect("本体ログへ追記できます");
}

fn append_record(path: &Path, record: &Value) {
  let mut line = serde_json::to_string(record).expect("レコードを JSON にできます");
  line.push('\n');
  append_bytes(path, line.as_bytes());
}

fn set_modified(path: &Path, seconds: u64) {
  let file = fs::File::options().write(true).open(path).expect("ファイルを開けます");
  file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds)).expect("更新時刻を設定できます");
}

fn step_record(marker: &str) -> Value {
  json!({
    "type": "step_start",
    "step": "plan",
    "phase": 1,
    "phaseName": "execute",
    "phaseExecutionId": "plan:1:1:1",
    "timestamp": "2026-11-16T01:55:48Z",
    "workflow": "development-core",
    "stack": [{
      "workflow": "development-core",
      "step": "plan",
      "kind": "agent",
      "occurrence": 1
    }],
    "marker": marker
  })
}

fn expect_found(location: RunLocation) -> PathBuf {
  match location {
    | RunLocation::Found(path) => path,
    | other => panic!("Found を期待しましたが {other:?} でした"),
  }
}

fn expect_missing(location: RunLocation) {
  match location {
    | RunLocation::Missing => {},
    | other => panic!("Missing を期待しましたが {other:?} でした"),
  }
}

fn expect_not_listed(location: RunLocation) {
  match location {
    | RunLocation::NotListed => {},
    | other => panic!("NotListed を期待しましたが {other:?} でした"),
  }
}

fn expect_rejected(location: RunLocation) {
  match location {
    | RunLocation::Rejected(_) => {},
    | other => panic!("Rejected を期待しましたが {other:?} でした"),
  }
}

fn expect_unreadable(location: RunLocation) -> String {
  match location {
    | RunLocation::Unreadable(reason) => reason,
    | other => panic!("Unreadable を期待しましたが {other:?} でした"),
  }
}

fn expect_meta_found(read: MetaRead) -> RunMeta {
  match read {
    | MetaRead::Found(meta) => meta,
    | other => panic!("Found を期待しましたが {other:?} でした"),
  }
}

fn expect_meta_missing(read: MetaRead) {
  match read {
    | MetaRead::Missing => {},
    | other => panic!("Missing を期待しましたが {other:?} でした"),
  }
}

fn expect_meta_unreadable(read: MetaRead) {
  match read {
    | MetaRead::Unreadable(_) => {},
    | other => panic!("Unreadable を期待しましたが {other:?} でした"),
  }
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
  match value {
    | Value::String(text) => out.push(text.clone()),
    | Value::Array(items) => {
      for item in items {
        collect_strings(item, out);
      }
    },
    | Value::Object(map) => {
      for item in map.values() {
        collect_strings(item, out);
      }
    },
    | _ => {},
  }
}

#[test]
fn resolves_run_slug_to_worktree_run_directory() {
  let fixture = Fixture::new();
  let worktree = fixture.worktree("wt-a");
  let run_dir = fixture.create_run(&worktree, "run-a");
  fixture.write_meta(&run_dir);
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-a",
    source_run_slug: None,
    worktree_path:   Some(worktree.as_path()),
  }]);

  let found = expect_found(fixture.reader().resolve(&run_slug("run-a")));

  assert_eq!(found, canonical(&run_dir), "worktree 側の .takt/runs/<run_slug> を返す必要があります");
}

#[test]
fn resolves_resumed_run_from_source_run_slug() {
  let fixture = Fixture::new();
  let worktree = fixture.worktree("wt-r");
  let source_run_dir = fixture.create_run(&worktree, "run-source");
  fixture.write_meta(&source_run_dir);
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-current",
    source_run_slug: Some("run-source"),
    worktree_path:   Some(worktree.as_path()),
  }]);

  let found = expect_found(fixture.reader().resolve(&run_slug("run-source")));

  assert_eq!(found, canonical(&source_run_dir), "再開した run は source_run_slug から解決する必要があります");
}

#[test]
fn resolves_run_without_worktree_path_to_project_runs() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-merge");
  fixture.write_meta(&run_dir);
  fixture.write_tasks(&[TaskEntry { run_slug: "run-merge", source_run_slug: None, worktree_path: None }]);

  let found = expect_found(fixture.reader().resolve(&run_slug("run-merge")));

  assert_eq!(found, canonical(&run_dir), "worktree_path が無い run はプロジェクトの .takt/runs から解決します");
}

#[test]
fn does_not_guess_run_directory_from_identifier() {
  let fixture = Fixture::new();
  fixture.create_run(&fixture.project_cwd, "ghost");
  fixture.write_tasks(&[TaskEntry { run_slug: "run-known", source_run_slug: None, worktree_path: None }]);

  expect_not_listed(fixture.reader().resolve(&run_slug("ghost")));
}

#[test]
fn prefers_run_slug_match_over_source_run_slug() {
  let fixture = Fixture::new();
  let exact_worktree = fixture.worktree("wt-exact");
  let resumed_worktree = fixture.worktree("wt-resumed");
  let exact_run_dir = fixture.create_run(&exact_worktree, "alpha");
  fixture.write_meta(&exact_run_dir);
  fixture.create_run(&resumed_worktree, "alpha");
  fixture.write_tasks(&[
    TaskEntry { run_slug: "alpha", source_run_slug: None, worktree_path: Some(exact_worktree.as_path()) },
    TaskEntry {
      run_slug:        "beta",
      source_run_slug: Some("alpha"),
      worktree_path:   Some(resumed_worktree.as_path()),
    },
  ]);

  let found = expect_found(fixture.reader().resolve(&run_slug("alpha")));

  assert_eq!(found, canonical(&exact_run_dir), "run_slug の一致は source_run_slug の一致より優先します");
}

#[test]
fn ignores_run_slug_inside_block_scalar() {
  let fixture = Fixture::new();
  let holder = fixture.worktree("wt-holder");
  let real = fixture.worktree("wt-b");
  fixture.create_run(&holder, "bogus");
  fixture.create_run(&real, "run-b");
  let yaml = format!(
    "tasks:\n  - name: holder\n    retry_note: >-\n      run_slug: bogus\n    worktree_path: {}\n  - run_slug: run-b\n    worktree_path: {}\n",
    holder.display(),
    real.display()
  );
  fixture.write_tasks_raw(&yaml);

  expect_not_listed(fixture.reader().resolve(&run_slug("bogus")));
}

#[test]
fn rejects_mapping_outside_allowed_roots() {
  let fixture = Fixture::new();
  let outside = fixture.temp_root().join("outside");
  fs::create_dir_all(&outside).expect("許可ルート外のディレクトリを作れます");
  fixture.create_run(&outside, "run-evil");
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-evil",
    source_run_slug: None,
    worktree_path:   Some(outside.as_path()),
  }]);

  expect_rejected(fixture.reader().resolve(&run_slug("run-evil")));
}

#[test]
fn identifier_rejected_by_run_slug_never_reaches_resolution() {
  let fixture = Fixture::new();
  let worktree = fixture.worktree("wt-slug");
  let escape_dir = worktree.join(".takt").join("escape");
  fs::create_dir_all(&escape_dir).expect("解決先の候補を作れます");
  fixture.write_meta(&escape_dir);
  let run_dir = fixture.create_run(&worktree, "run-ok");
  fixture.write_meta(&run_dir);
  fixture.write_tasks(&[
    TaskEntry { run_slug: "../escape", source_run_slug: None, worktree_path: Some(worktree.as_path()) },
    TaskEntry { run_slug: "run-ok", source_run_slug: None, worktree_path: Some(worktree.as_path()) },
  ]);
  let reader = fixture.reader();

  for value in ["../escape", "runs/1", "runs\\1", ".", ".."] {
    assert_eq!(
      RunSlug::try_from(value),
      Err(DomainError::Syntax("run slug")),
      "{value} は RunSlug として拒否される必要があります"
    );
    let location = RunSlug::try_from(value).ok().map(|slug| reader.resolve(&slug));
    assert!(location.is_none(), "{value} で場所の解決が進んではいけません: {location:?}");
  }

  let found = expect_found(reader.resolve(&run_slug("run-ok")));
  assert_eq!(found, canonical(&run_dir), "有効な識別子は同じ対応表から解決できる必要があります");
}

#[test]
fn reports_unreadable_tasks_yaml_distinctly_from_missing_run() {
  let fixture = Fixture::new();
  fs::write(fixture.tasks_path(), [0xff_u8, 0xfe]).expect("読めない tasks.yaml を書けます");

  let reason = expect_unreadable(fixture.reader().resolve(&run_slug("run-any")));
  assert!(!reason.is_empty(), "読めなかった理由を返す必要があります");

  fixture.write_tasks(&[TaskEntry { run_slug: "run-lonely", source_run_slug: None, worktree_path: None }]);
  expect_missing(fixture.reader().resolve(&run_slug("run-lonely")));
}

#[test]
fn reports_cleaned_worktree_as_missing() {
  let fixture = Fixture::new();
  let cleaned = fixture.worktrees_root.join("wt-gone");
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-gone",
    source_run_slug: None,
    worktree_path:   Some(cleaned.as_path()),
  }]);

  expect_missing(fixture.reader().resolve(&run_slug("run-gone")));
}

#[test]
fn resolves_worktree_stub_to_project_run_data() {
  let fixture = Fixture::new();
  let worktree = fixture.worktree("wt-stub");
  let stub_dir = worktree.join(".takt").join("runs").join("run-stub");
  fs::create_dir_all(stub_dir.join("ci-evidence")).expect("worktree 側の stub を作れます");
  let project_run_dir = fixture.create_run(&fixture.project_cwd, "run-stub");
  fixture.write_meta(&project_run_dir);
  append_record(&body_log(&project_run_dir), &step_record("project"));
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-stub",
    source_run_slug: None,
    worktree_path:   Some(worktree.as_path()),
  }]);

  let found = expect_found(fixture.reader().resolve(&run_slug("run-stub")));

  assert_eq!(
    found,
    canonical(&project_run_dir),
    "worktree 側が stub ならプロジェクト側の run データを返す必要があります"
  );
}

#[test]
fn resolves_project_run_data_when_worktree_has_no_runs_directory() {
  let fixture = Fixture::new();
  let worktree = fixture.worktree("wt-empty");
  let project_run_dir = fixture.create_run(&fixture.project_cwd, "run-empty");
  append_record(&body_log(&project_run_dir), &step_record("project"));
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-empty",
    source_run_slug: None,
    worktree_path:   Some(worktree.as_path()),
  }]);

  let found = expect_found(fixture.reader().resolve(&run_slug("run-empty")));

  assert_eq!(
    found,
    canonical(&project_run_dir),
    "worktree 側に .takt/runs が無くてもプロジェクト側の本体ログから解決する必要があります"
  );
}

#[test]
fn prefers_worktree_run_data_over_project_run_data() {
  let fixture = Fixture::new();
  let worktree = fixture.worktree("wt-both");
  let worktree_run_dir = fixture.create_run(&worktree, "run-both");
  fixture.write_meta(&worktree_run_dir);
  let project_run_dir = fixture.create_run(&fixture.project_cwd, "run-both");
  fixture.write_meta(&project_run_dir);
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-both",
    source_run_slug: None,
    worktree_path:   Some(worktree.as_path()),
  }]);

  let found = expect_found(fixture.reader().resolve(&run_slug("run-both")));

  assert_eq!(found, canonical(&worktree_run_dir), "両側に run データがある場合は worktree 側を採用する必要があります");
}

#[test]
fn reports_directory_without_run_data_as_missing() {
  let fixture = Fixture::new();
  let worktree = fixture.worktree("wt-no-data");
  let stub_dir = worktree.join(".takt").join("runs").join("run-no-data");
  fs::create_dir_all(stub_dir.join("ci-evidence")).expect("worktree 側の stub を作れます");
  fixture.create_run(&fixture.project_cwd, "run-no-data");
  fixture.write_tasks(&[TaskEntry {
    run_slug:        "run-no-data",
    source_run_slug: None,
    worktree_path:   Some(worktree.as_path()),
  }]);

  expect_missing(fixture.reader().resolve(&run_slug("run-no-data")));
}

#[test]
fn reads_meta_fields() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-meta");
  fs::write(
    run_dir.join("meta.json"),
    r#"{"status":"in_progress","currentStep":"implement","currentIteration":4,"phase":2,"updatedAt":"2026-11-16T01:00:00Z"}"#,
  )
  .expect("meta.json を書けます");

  let meta = expect_meta_found(fixture.reader().read_meta(&run_dir));

  assert_eq!(meta.status.as_deref(), Some("in_progress"));
  assert_eq!(meta.current_step.as_deref(), Some("implement"));
  assert_eq!(meta.current_iteration, Some(4));
  assert_eq!(meta.phase, Some(2));
  assert_eq!(meta.updated_at.as_deref(), Some("2026-11-16T01:00:00Z"));
}

#[test]
fn reads_meta_without_phase_as_none() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-meta-nophase");
  fs::write(
    run_dir.join("meta.json"),
    r#"{"status":"completed","currentStep":"done","currentIteration":10,"updatedAt":"2026-11-16T01:00:00Z"}"#,
  )
  .expect("meta.json を書けます");

  let meta = expect_meta_found(fixture.reader().read_meta(&run_dir));

  assert_eq!(meta.phase, None, "phase が無い meta.json を 0 で埋めてはいけません");
}

#[test]
fn reports_missing_meta_as_missing() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-no-meta");

  expect_meta_missing(fixture.reader().read_meta(&run_dir));
}

#[test]
fn reports_broken_meta_as_unreadable() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-broken-meta");
  fs::write(run_dir.join("meta.json"), r#"{"status":"#).expect("壊れた meta.json を書けます");

  expect_meta_unreadable(fixture.reader().read_meta(&run_dir));
}

#[test]
fn consumes_only_complete_lines_and_keeps_tail() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-append");
  let log = body_log(&run_dir);
  append_record(&log, &step_record("first"));
  let mut reader = fixture.reader();

  let first = reader.read_body_log(&run_dir);
  assert_eq!(first.records.len(), 1, "改行で終わる完全な行だけを消費します");
  assert_eq!(first.records[0]["marker"], "first");
  assert_eq!(first.skipped, 0);

  append_bytes(&log, br#"{"type":"step_start","step":"plan","marker":"second""#);
  let partial = reader.read_body_log(&run_dir);
  assert_eq!(partial.records.len(), 0, "改行の無い書きかけ行は消費しません");
  assert_eq!(partial.skipped, 0, "書きかけ行を破損行として数えてはいけません");

  append_bytes(&log, b"}\n");
  let completed = reader.read_body_log(&run_dir);
  assert_eq!(completed.records.len(), 1, "追記で完成した行だけを返します");
  assert_eq!(completed.records[0]["marker"], "second");
  assert_eq!(completed.skipped, 0);
}

#[test]
fn skips_corrupt_and_invalid_utf8_lines_and_counts_them() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-corrupt");
  let log = body_log(&run_dir);
  append_record(&log, &step_record("first"));
  append_bytes(&log, b"\xff\xfe{\"type\":\"step_start\"}\n");
  append_bytes(&log, b"not json\n");
  append_bytes(&log, b"[1,2]\n");
  append_record(&log, &step_record("last"));
  let mut reader = fixture.reader();

  let read = reader.read_body_log(&run_dir);

  assert_eq!(read.records.len(), 2, "壊れた行を飛ばして後続の正しい行を返します");
  assert_eq!(read.records[0]["marker"], "first");
  assert_eq!(read.records[1]["marker"], "last");
  assert_eq!(read.skipped, 3, "UTF-8 不正・JSON 不正・オブジェクトでない行を数えます");

  let again = reader.read_body_log(&run_dir);
  assert_eq!(again.records.len(), 0);
  assert_eq!(again.skipped, 0, "破損行も消費済みとして位置を進める必要があります");
}

#[test]
fn resets_offset_when_file_shrinks() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-shrink");
  let log = body_log(&run_dir);
  let long_padding = "p".repeat(300);
  append_record(&log, &json!({ "type": "step_start", "step": "plan", "marker": "old-1", "padding": long_padding }));
  append_record(&log, &json!({ "type": "step_start", "step": "plan", "marker": "old-2", "padding": long_padding }));
  let mut reader = fixture.reader();
  assert_eq!(reader.read_body_log(&run_dir).records.len(), 2);

  fs::write(&log, b"{\"type\":\"step_start\",\"step\":\"plan\",\"marker\":\"new\"}\n")
    .expect("縮小した本体ログを書けます");
  let read = reader.read_body_log(&run_dir);

  assert_eq!(read.records.len(), 1, "縮小したファイルは先頭から読み直します");
  assert_eq!(read.records[0]["marker"], "new");
}

#[test]
fn resets_offset_when_inode_changes() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-inode");
  let log = body_log(&run_dir);
  append_record(&log, &json!({ "type": "step_start", "step": "plan", "marker": "old" }));
  let mut reader = fixture.reader();
  assert_eq!(reader.read_body_log(&run_dir).records.len(), 1);

  let replacement = run_dir.join("replacement.tmp");
  let padding = "q".repeat(300);
  append_record(&replacement, &json!({ "type": "step_start", "step": "plan", "marker": "new", "padding": padding }));
  fs::rename(&replacement, &log).expect("本体ログを差し替えられます");
  let read = reader.read_body_log(&run_dir);

  assert_eq!(read.records.len(), 1, "inode が変わったファイルは先頭から読み直します");
  assert_eq!(read.records[0]["marker"], "new");
}

#[test]
fn new_reader_starts_from_beginning_without_persisted_offset() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-restart");
  let log = body_log(&run_dir);
  append_record(&log, &step_record("first"));
  append_record(&log, &step_record("second"));
  let mut first_reader = fixture.reader();
  assert_eq!(first_reader.read_body_log(&run_dir).records.len(), 2);

  let mut second_reader = fixture.reader();
  let read = second_reader.read_body_log(&run_dir);

  assert_eq!(read.records.len(), 2, "読み取り位置をプロセス外へ永続化してはいけません");
}

#[test]
fn drops_prompt_fields_and_caps_large_records() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-large");
  let log = body_log(&run_dir);
  let secret = "PROMPT_BODY_SECRET_9f2c";
  let record = json!({
    "type": "phase_start",
    "step": "plan",
    "phase": 1,
    "phaseName": "execute",
    "phaseExecutionId": "plan:1:1:1",
    "timestamp": "2026-11-16T01:55:48Z",
    "workflow": "development-core",
    "stack": [{
      "workflow": "development-core",
      "step": "plan",
      "kind": "agent",
      "occurrence": 1
    }],
    "systemPrompt": format!("{secret}{}", "x".repeat(70_000)),
    "userInstruction": format!("{secret}{}", "y".repeat(70_000)),
    "instruction": format!("{secret}{}", "z".repeat(70_000)),
    "content": format!("{secret}{}", "w".repeat(70_000)),
    "nested": { "content": "nested prompt body", "keep": "nested keep" }
  });
  let mut line = serde_json::to_string(&record).expect("大きなレコードを JSON にできます");
  line.push('\n');
  assert!(line.len() > 64 * 1024, "64KiB を超える行の fixture を作る必要があります");
  append_bytes(&log, line.as_bytes());
  let mut reader = fixture.reader();

  let read = reader.read_body_log(&run_dir);

  assert_eq!(read.records.len(), 1);
  let sanitized = &read.records[0];
  assert!(sanitized.get("systemPrompt").is_none(), "systemPrompt を結果へ含めてはいけません");
  assert!(sanitized.get("userInstruction").is_none(), "userInstruction を結果へ含めてはいけません");
  assert!(sanitized.get("instruction").is_none(), "instruction を結果へ含めてはいけません");
  assert!(sanitized.get("content").is_none(), "content を結果へ含めてはいけません");
  let nested = sanitized.get("nested").expect("入れ子のオブジェクトは残します");
  assert!(nested.get("content").is_none(), "入れ子の content も結果へ含めてはいけません");
  assert_eq!(nested.get("keep").and_then(Value::as_str), Some("nested keep"));
  assert_eq!(sanitized.get("type").and_then(Value::as_str), Some("phase_start"), "プロンプト以外の項目は残します");
  let serialized = serde_json::to_string(sanitized).expect("結果を JSON にできます");
  assert!(!serialized.contains(secret), "プロンプト本文を結果へ含めてはいけません");

  let mut strings = Vec::new();
  collect_strings(sanitized, &mut strings);
  assert!(
    strings.iter().all(|text| text.chars().count() <= MAX_SUMMARY_CHARS),
    "残した文字列は長さの上限以内に要約する必要があります"
  );
}

#[test]
fn keeps_values_that_mention_prompt_keys() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-note");
  let log = body_log(&run_dir);
  let note = "systemPrompt は捨てる対象です";
  append_record(
    &log,
    &json!({
      "type": "step_start",
      "step": "plan",
      "phase": 1,
      "phaseName": "execute",
      "phaseExecutionId": "plan:1:1:1",
      "timestamp": "2026-11-16T01:55:48Z",
      "workflow": "development-core",
      "stack": [],
      "note": note
    }),
  );
  let mut reader = fixture.reader();

  let read = reader.read_body_log(&run_dir);

  assert_eq!(read.records.len(), 1);
  assert_eq!(read.records[0].get("note").and_then(Value::as_str), Some(note), "値に現れた語だけでは項目を消しません");
}

#[test]
fn reports_missing_body_log_without_error() {
  let fixture = Fixture::new();
  let run_dir = fixture.project_cwd.join(".takt").join("runs").join("run-no-logs");
  fs::create_dir_all(&run_dir).expect("logs を持たない run ディレクトリを作れます");
  let mut reader = fixture.reader();

  let read: BodyLogRead = reader.read_body_log(&run_dir);

  assert!(read.missing, "logs が無い run は残っていない結果として返します");
  assert!(read.records.is_empty());
  assert_eq!(read.skipped, 0);
}

#[test]
fn latest_log_update_uses_file_mtime_and_ignores_record_timestamp() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-latest");
  let log = body_log(&run_dir);
  append_record(
    &log,
    &json!({
      "type": "step_start",
      "step": "plan",
      "phase": 1,
      "phaseName": "execute",
      "phaseExecutionId": "plan:1:1:1",
      "timestamp": "2099-01-01T00:00:00Z",
      "workflow": "development-core",
      "stack": []
    }),
  );
  let phase1_dir = run_dir.join("logs").join("phase1-execution");
  fs::create_dir_all(&phase1_dir).expect("phase1-execution ディレクトリを作れます");
  let phase1_log = phase1_dir.join("plan-1.jsonl");
  fs::write(&phase1_log, b"{\"kind\":\"content_chunk\"}\n").expect("phase1 のログを書けます");
  let ignored = run_dir.join("logs").join("ignore.txt");
  fs::write(&ignored, b"not a body log\n").expect("対象外のファイルを書けます");

  set_modified(&log, BODY_MTIME);
  set_modified(&phase1_log, PHASE1_MTIME);
  set_modified(&ignored, IGNORED_MTIME);

  let latest = fixture.reader().latest_log_update(&run_dir).expect("ログ群の最新更新時刻を返します");

  assert_eq!(latest.timestamp(), PHASE1_MTIME as i64, "本体ログと phase1 のファイルの更新時刻の最大値を返します");
  assert_ne!(latest.timestamp(), 4_070_908_800, "レコードの timestamp を最新判定に使ってはいけません");
}

#[test]
fn latest_log_update_returns_none_without_logs() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-empty");

  assert!(fixture.reader().latest_log_update(&run_dir).is_none());
}

#[test]
fn ignores_meta_symlink_outside_run_directory() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-meta-link");
  let outside = fixture.temp_root().join("outside-meta");
  fs::create_dir_all(&outside).expect("run の外のディレクトリを作れます");
  fixture.write_meta(&outside);
  symlink(outside.join("meta.json"), run_dir.join("meta.json")).expect("meta.json のリンクを作れます");

  expect_meta_missing(fixture.reader().read_meta(&run_dir));
}

#[test]
fn ignores_logs_symlink_outside_run_directory() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-logs-link");
  let outside = fixture.temp_root().join("outside-logs");
  fs::create_dir_all(outside.join(PHASE1_DIRECTORY)).expect("run の外のログ置き場を作れます");
  let outside_log = outside.join("body.jsonl");
  append_record(&outside_log, &step_record("outside"));
  append_bytes(&outside_log, b"not json\n");
  let outside_phase1 = outside.join(PHASE1_DIRECTORY).join("plan-1.jsonl");
  fs::write(&outside_phase1, b"{\"kind\":\"content_chunk\"}\n").expect("run の外の phase1 のログを書けます");
  set_modified(&outside_log, BODY_MTIME);
  set_modified(&outside_phase1, PHASE1_MTIME);
  fs::remove_dir(run_dir.join("logs")).expect("run の logs ディレクトリを消せます");
  symlink(&outside, run_dir.join("logs")).expect("logs のリンクを作れます");
  let mut reader = fixture.reader();

  let read = reader.read_body_log(&run_dir);

  assert!(read.missing, "run の外を指す logs は残っていない結果として返します");
  assert!(read.records.is_empty(), "run の外の本体ログを読んではいけません");
  assert_eq!(read.skipped, 0, "run の外の本体ログの行を数えてはいけません");
  assert!(reader.latest_log_update(&run_dir).is_none(), "run の外のファイルの更新時刻を使ってはいけません");
}

#[test]
fn ignores_phase1_symlink_outside_run_directory() {
  let fixture = Fixture::new();
  let run_dir = fixture.create_run(&fixture.project_cwd, "run-phase1-link");
  let log = body_log(&run_dir);
  append_record(&log, &step_record("inside"));
  set_modified(&log, BODY_MTIME);
  let outside = fixture.temp_root().join("outside-phase1");
  fs::create_dir_all(&outside).expect("run の外のディレクトリを作れます");
  let outside_log = outside.join("plan-1.jsonl");
  fs::write(&outside_log, b"{\"kind\":\"content_chunk\"}\n").expect("run の外の phase1 のログを書けます");
  set_modified(&outside_log, IGNORED_MTIME);
  symlink(&outside, run_dir.join("logs").join(PHASE1_DIRECTORY)).expect("phase1-execution のリンクを作れます");

  let latest = fixture.reader().latest_log_update(&run_dir).expect("run の中の本体ログの更新時刻を返します");

  assert_eq!(
    latest.timestamp(),
    BODY_MTIME as i64,
    "run の外を指す phase1-execution の更新時刻を使わず、run の中の本体ログの更新時刻を返します"
  );
}
