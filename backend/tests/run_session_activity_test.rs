//! 読み取り層がセッションログ（`logs/phase1-execution/*.jsonl`）から活動の印（直近の道具名、道具の
//! 呼び出しの件数、更新時刻）を組み立てることを、一時ディレクトリだけで検証する。
//!
//! セッションログのレコードには時刻が無いため、順序と更新時刻はファイルの更新時刻で決める。
//! fixture は実物のセッションログの形（`kind`、`event.type`、`event.data.
//! tool`）に合わせた合成データで、 実在のプロンプト本文は含めない。

use std::{
  fs,
  io::Write,
  os::unix::fs::symlink,
  path::{Path, PathBuf},
  time::{Duration, UNIX_EPOCH},
};

use serde_json::json;
use takt_dash_backend::run_log::{RunLogReader, SessionActivity};

const EARLIER_MTIME: u64 = 1_700_000_000;
const LATER_MTIME: u64 = 1_700_000_200;
const MAX_SUMMARY_CHARS: usize = 512;

struct Fixture {
  _root:       tempfile::TempDir,
  project_cwd: PathBuf,
  run_dir:     PathBuf,
}

impl Fixture {
  fn new() -> Fixture {
    let root = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let project_cwd = fs::canonicalize(root.path()).expect("実パスを解決できます").join("project");
    let run_dir = project_cwd.join(".takt").join("runs").join("run-1");
    fs::create_dir_all(run_dir.join("logs").join("phase1-execution")).expect("phase1-execution を作れます");
    Fixture { _root: root, project_cwd, run_dir }
  }

  fn reader(&self) -> RunLogReader {
    RunLogReader::new(self.project_cwd.clone(), self.project_cwd.join("worktrees"))
  }

  fn session_log(&self, name: &str) -> PathBuf {
    self.run_dir.join("logs").join("phase1-execution").join(name)
  }
}

fn append(path: &Path, text: &str) {
  let mut file = fs::OpenOptions::new().append(true).create(true).open(path).expect("ファイルを開けます");
  file.write_all(text.as_bytes()).expect("ファイルへ追記できます");
}

fn set_modified(path: &Path, seconds: u64) {
  let file = fs::File::options().write(true).open(path).expect("ファイルを開けます");
  file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds)).expect("更新時刻を設定できます");
}

fn tool_use(tool: &str) -> String {
  format!(
    "{}\n",
    json!({ "kind": "provider_event", "event": { "type": "tool_use", "data": { "tool": tool, "input": { "command": "ls" } } } })
  )
}

fn text_event(text: &str) -> String {
  format!("{}\n", json!({ "kind": "provider_event", "event": { "type": "text", "data": { "text": text } } }))
}

fn activity(reader: &mut RunLogReader, run_dir: &Path) -> SessionActivity {
  reader.read_session_activity(run_dir)
}

#[test]
fn counts_only_tool_use_events_and_returns_the_last_tool() {
  // 文字の出力、道具の結果、`tool_use` を本文に含むだけの行は、道具の呼び出しとして数えない。
  let fixture = Fixture::new();
  let log = fixture.session_log("implement-1.jsonl");
  append(&log, &tool_use("Read"));
  append(&log, &text_event("tool_use という語を含む文章です"));
  append(
    &log,
    &format!(
      "{}\n",
      json!({ "kind": "provider_event", "event": { "type": "tool_result", "data": { "tool": "Read" } } })
    ),
  );
  append(&log, &format!("{}\n", json!({ "kind": "content_chunk", "content": "\"type\":\"tool_use\"" })));
  append(&log, &tool_use("Bash"));
  let mut reader = fixture.reader();

  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(read.tool_use_count, 2, "provider_event の tool_use だけを数えます");
  assert_eq!(read.latest_tool.as_deref(), Some("Bash"), "最後の道具の呼び出しの道具名です");
}

#[test]
fn latest_tool_comes_from_the_most_recently_modified_file_not_the_last_file_name() {
  // ファイル名の順で最後の z-plan ではなく、更新時刻が新しい a-implement の最後の道具を採る。
  let fixture = Fixture::new();
  let older = fixture.session_log("z-plan.jsonl");
  let newer = fixture.session_log("a-implement.jsonl");
  append(&older, &tool_use("Read"));
  append(&newer, &tool_use("Edit"));
  set_modified(&older, EARLIER_MTIME);
  set_modified(&newer, LATER_MTIME);
  let mut reader = fixture.reader();

  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(read.latest_tool.as_deref(), Some("Edit"));
  assert_eq!(read.tool_use_count, 2, "件数は全ファイルの合計です");
  assert_eq!(
    read.updated_at.map(|time| time.timestamp()),
    Some(LATER_MTIME as i64),
    "更新時刻はファイルの更新時刻の最大値です"
  );
}

#[test]
fn a_newer_file_without_tool_use_does_not_hide_the_latest_tool() {
  // 道具の呼び出しを持たないファイルの方が新しくても、
  // 直近の道具は道具の呼び出しを持つファイルから採る。 更新時刻は道具の有無に関係なく、
  // 最新のファイルの時刻です。
  let fixture = Fixture::new();
  let with_tool = fixture.session_log("plan-1.jsonl");
  let text_only = fixture.session_log("implement-1.jsonl");
  append(&with_tool, &tool_use("Grep"));
  append(&text_only, &text_event("考えています"));
  set_modified(&with_tool, EARLIER_MTIME);
  set_modified(&text_only, LATER_MTIME);
  let mut reader = fixture.reader();

  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(read.latest_tool.as_deref(), Some("Grep"));
  assert_eq!(read.tool_use_count, 1);
  assert_eq!(read.updated_at.map(|time| time.timestamp()), Some(LATER_MTIME as i64));
}

#[test]
fn does_not_count_a_partial_line_until_it_is_completed() {
  let fixture = Fixture::new();
  let log = fixture.session_log("implement-1.jsonl");
  append(&log, &tool_use("Read"));
  let partial = tool_use("Edit");
  let (head, tail) = partial.split_at(partial.len() / 2);
  append(&log, head);
  let mut reader = fixture.reader();

  let before = activity(&mut reader, &fixture.run_dir);
  append(&log, tail);
  let after = activity(&mut reader, &fixture.run_dir);

  assert_eq!(before.tool_use_count, 1, "改行の無い書きかけの行は数えません");
  assert_eq!(before.latest_tool.as_deref(), Some("Read"));
  assert_eq!(after.tool_use_count, 2, "完成した行を1回だけ数えます");
  assert_eq!(after.latest_tool.as_deref(), Some("Edit"));
}

#[test]
fn repeated_reads_do_not_count_the_same_lines_twice() {
  let fixture = Fixture::new();
  let log = fixture.session_log("implement-1.jsonl");
  append(&log, &tool_use("Read"));
  append(&log, &tool_use("Grep"));
  let mut reader = fixture.reader();

  let first = activity(&mut reader, &fixture.run_dir);
  let second = activity(&mut reader, &fixture.run_dir);
  append(&log, &tool_use("Bash"));
  let third = activity(&mut reader, &fixture.run_dir);

  assert_eq!(first.tool_use_count, 2);
  assert_eq!(second.tool_use_count, 2, "追記が無ければ件数は変わりません");
  assert_eq!(third.tool_use_count, 3, "追記した1件だけを足します");
  assert_eq!(third.latest_tool.as_deref(), Some("Bash"));
}

#[test]
fn skips_corrupt_lines_and_keeps_counting_the_following_lines() {
  let fixture = Fixture::new();
  let log = fixture.session_log("implement-1.jsonl");
  append(&log, "{\"kind\":\"provider_event\",\"event\":{\"type\":\"tool_use\"\n");
  append(&log, "\u{0}not json tool_use\n");
  append(
    &log,
    &format!("{}\n", json!({ "kind": "provider_event", "event": { "type": "tool_use", "data": { "tool": 42 } } })),
  );
  append(&log, &tool_use("Write"));
  let mut reader = fixture.reader();

  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(read.tool_use_count, 1, "壊れた行と道具名が文字列でない行は数えません");
  assert_eq!(read.latest_tool.as_deref(), Some("Write"));
}

#[test]
fn a_replaced_file_is_counted_again_from_the_start_without_double_counting() {
  // inode が変わったファイルは、そのファイルの件数を数え直す。前の件数に足す実装は 3
  // を返して落ちる。
  let fixture = Fixture::new();
  let log = fixture.session_log("implement-1.jsonl");
  append(&log, &tool_use("Read"));
  append(&log, &tool_use("Grep"));
  let mut reader = fixture.reader();
  assert_eq!(activity(&mut reader, &fixture.run_dir).tool_use_count, 2);

  let replacement = fixture.run_dir.join("replacement.tmp");
  append(&replacement, &tool_use("Edit"));
  fs::rename(&replacement, &log).expect("セッションログを差し替えられます");
  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(read.tool_use_count, 1, "置き換わったファイルは先頭から数え直します");
  assert_eq!(read.latest_tool.as_deref(), Some("Edit"));
}

#[test]
fn a_removed_file_is_no_longer_counted() {
  let fixture = Fixture::new();
  let kept = fixture.session_log("implement-1.jsonl");
  let removed = fixture.session_log("plan-1.jsonl");
  append(&kept, &tool_use("Read"));
  append(&removed, &tool_use("Grep"));
  let mut reader = fixture.reader();
  assert_eq!(activity(&mut reader, &fixture.run_dir).tool_use_count, 2);

  fs::remove_file(&removed).expect("セッションログを消せます");
  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(read.tool_use_count, 1, "件数は現存するファイルの合計です");
  assert_eq!(read.latest_tool.as_deref(), Some("Read"));
}

#[test]
fn caps_a_long_tool_name_to_the_summary_limit() {
  let fixture = Fixture::new();
  let log = fixture.session_log("implement-1.jsonl");
  append(&log, &tool_use(&"T".repeat(MAX_SUMMARY_CHARS + 100)));
  let mut reader = fixture.reader();

  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(
    read.latest_tool,
    Some("T".repeat(MAX_SUMMARY_CHARS) + "…"),
    "道具名は上限で切り詰め、切ったことが分かるよう末尾に省略記号を付けます"
  );
}

#[test]
fn returns_an_empty_activity_without_session_logs() {
  let fixture = Fixture::new();
  fs::remove_dir(fixture.run_dir.join("logs").join("phase1-execution")).expect("phase1-execution を消せます");
  let mut reader = fixture.reader();

  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(read, SessionActivity { latest_tool: None, tool_use_count: 0, updated_at: None });
}

#[test]
fn ignores_a_phase1_directory_symlinked_outside_the_run() {
  let fixture = Fixture::new();
  let phase1 = fixture.run_dir.join("logs").join("phase1-execution");
  fs::remove_dir(&phase1).expect("phase1-execution を消せます");
  let outside = fixture.project_cwd.parent().expect("プロジェクトの親があります").join("outside-phase1");
  fs::create_dir_all(&outside).expect("run の外のディレクトリを作れます");
  append(&outside.join("implement-1.jsonl"), &tool_use("Bash"));
  symlink(&outside, &phase1).expect("phase1-execution のリンクを作れます");
  let mut reader = fixture.reader();

  let read = activity(&mut reader, &fixture.run_dir);

  assert_eq!(
    read,
    SessionActivity { latest_tool: None, tool_use_count: 0, updated_at: None },
    "run の外のファイルは読みません"
  );
}

#[test]
fn reading_session_activity_does_not_move_the_body_log_position() {
  // セッションログの読み取りは本体ログの読み位置に触れない。本体ログを先に読んだ分を、
  // 次の本体ログの 読み取りで重複させず、追記分だけを返す。
  let fixture = Fixture::new();
  let body = fixture.run_dir.join("logs").join("body.jsonl");
  append(&body, &format!("{}\n", json!({ "type": "step_start", "step": "plan", "marker": "first" })));
  append(&fixture.session_log("implement-1.jsonl"), &tool_use("Read"));
  let mut reader = fixture.reader();

  assert_eq!(reader.read_body_log(&fixture.run_dir).records.len(), 1);
  activity(&mut reader, &fixture.run_dir);
  append(&body, &format!("{}\n", json!({ "type": "step_start", "step": "plan", "marker": "second" })));
  let next = reader.read_body_log(&fixture.run_dir);

  assert_eq!(next.records.len(), 1, "本体ログは追記分だけを返します");
  assert_eq!(next.records[0]["marker"], "second");
}
