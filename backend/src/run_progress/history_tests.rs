use super::*;

#[test]
fn detail_history_read_ranges() {
  let root = tempfile::tempdir().unwrap();
  let cwd = std::fs::canonicalize(root.path()).unwrap();
  let run = cwd.join(".takt/runs/r1");
  std::fs::create_dir_all(run.join("logs")).unwrap();
  std::fs::write(cwd.join(".takt/tasks.yaml"), "tasks:\n  - run_slug: r1\n").unwrap();
  std::fs::write(run.join("meta.json"), r#"{"status":"running","workflow":"outer","currentStep":"gate"}"#).unwrap();
  let body = format!(
    "{}\n{}\n{}\n",
    serde_json::json!({"type":"workflow_start"}),
    serde_json::json!({"type":"message","content":"x".repeat(4096)}),
    serde_json::json!({"type":"step_complete","step":"gate","workflow":"outer","iteration":1,"matchedRuleIndex":0})
  );
  std::fs::write(run.join("logs/body.jsonl"), &body).unwrap();
  let mut progress = RunProgress::new(cwd.clone(), cwd.join("worktrees"), InitialRead::Tail(512));
  let slug = RunSlug::try_from("r1").unwrap();
  let first = progress.read(&slug).unwrap();
  let second = progress.read(&slug).unwrap();
  eprintln!(
    "display={},{}; supplement={:?}; body={}",
    first.body_log_bytes_read,
    second.body_log_bytes_read,
    progress.reader.evidence_reads.borrow(),
    body.len()
  );
  assert_eq!(first.evidence, second.evidence);
  assert_eq!(second.body_log_bytes_read, 0);
  assert_eq!(*progress.reader.evidence_reads.borrow(), vec![(0, body.len() as u64), (body.len() as u64, 0)]);
  let appended = format!(
    "{}\n",
    serde_json::json!({"type":"step_complete","step":"gate","workflow":"outer","iteration":2,"matchedRuleIndex":0,"timestamp":"2026-11-19T00:00:00Z"})
  );
  use std::io::Write;
  let path = run.join("logs/body.jsonl");
  std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(appended.as_bytes()).unwrap();
  let updates = progress.read_updates(&slug, None).unwrap();
  let third = progress.read(&slug).unwrap();
  assert!(updates.lines.iter().any(|line| line.text.contains("step_complete gate")));
  assert_eq!(third.evidence.events.len(), 2);
  assert_eq!(third.evidence.events[1].log_index, Some(1));
  assert_eq!(progress.reader.evidence_reads.borrow().last(), Some(&(body.len() as u64, appended.len() as u64)));
  assert!(progress
    .read_updates(&slug, Some(LogCursor { epoch: updates.epoch, sequence: updates.lines.last().unwrap().sequence }))
    .unwrap()
    .lines
    .is_empty());

  // 同じ inode の縮小と、別 inode の置換は、それぞれ古い証拠とリンクを捨てる。
  let replacement = format!("{}\n", serde_json::json!({"type":"workflow_start"}));
  std::fs::write(&path, &replacement).unwrap();
  assert!(progress.read(&slug).unwrap().evidence.events.is_empty());
  let next = path.with_extension("next");
  std::fs::write(&next, format!("{replacement}{appended}")).unwrap();
  std::fs::rename(&next, &path).unwrap();
  let replaced = progress.read(&slug).unwrap();
  let fresh = RunProgress::new(cwd.clone(), cwd.join("worktrees"), InitialRead::Tail(512)).read(&slug).unwrap();
  assert_eq!(replaced.evidence, fresh.evidence);
  assert_eq!(replaced.evidence.events.len(), 1);
  assert_eq!(replaced.evidence.events[0].log_index, Some(0));
  std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(appended.as_bytes()).unwrap();
  let positions = progress.reader.history_positions();
  progress.reader.history_failure = Some(path);
  let failed_supplement = progress.read(&slug).unwrap();
  assert_eq!(failed_supplement.status, RunStatus::Running);
  assert_eq!(failed_supplement.step_logs().len(), 2, "補完失敗でも取得済み詳細を返す");
  assert_eq!(progress.reader.history_positions(), positions);
  progress.reader.history_failure = None;
  assert_eq!(progress.read(&slug).unwrap().evidence.events.len(), 2);
  assert_eq!(progress.read(&slug).unwrap().evidence.events.len(), 2);
}

#[test]
fn history_io_failure_does_not_commit_partial_positions_and_recovers_once() {
  let root = tempfile::tempdir().unwrap();
  let dir = root.path();
  std::fs::create_dir(dir.join("logs")).unwrap();
  let first = dir.join("logs/a.jsonl");
  let second = dir.join("logs/b.jsonl");
  std::fs::write(&first, "{\"type\":\"workflow_start\"}\n").unwrap();
  std::fs::write(&second, "{\"type\":\"step_start\",\"step\":\"gate\"}\n").unwrap();
  let mut reader = RunLogReader::new(dir.into(), dir.join("worktrees"));
  assert_eq!(reader.read_execution_evidence(dir, &[]).unwrap().events.len(), 1);
  use std::io::Write;
  std::fs::OpenOptions::new()
    .append(true)
    .open(&first)
    .unwrap()
    .write_all(b"{\"type\":\"step_complete\",\"step\":\"gate\"}\n")
    .unwrap();
  let positions = reader.history_positions();
  reader.history_failure = Some(std::fs::canonicalize(&second).unwrap());
  let failed = reader.read_execution_evidence(dir, &[]);
  reader.history_failure = None;
  assert!(failed.is_err());
  assert_eq!(reader.history_positions(), positions);
  let recovered = reader.read_execution_evidence(dir, &[]).unwrap();
  assert_eq!(recovered.events.len(), 2);
  assert_eq!(reader.read_execution_evidence(dir, &[]).unwrap(), recovered);
}

#[test]
fn malformed_lines_and_unfinished_tails_preserve_evidence_in_record_order() {
  use std::io::Write;
  let root = tempfile::tempdir().unwrap();
  let cwd = std::fs::canonicalize(root.path()).unwrap();
  let run = cwd.join(".takt/runs/r1");
  std::fs::create_dir_all(run.join("logs")).unwrap();
  std::fs::write(cwd.join(".takt/tasks.yaml"), "tasks:\n  - run_slug: r1\n").unwrap();
  std::fs::write(run.join("meta.json"), r#"{"status":"running","workflow":"outer","currentStep":"gate"}"#).unwrap();
  let path = run.join("logs/body.jsonl");
  let start = "{\"type\":\"workflow_start\"}\n";
  let complete = r#"{"type":"step_complete","step":"gate","matchedRuleIndex":0}"#;
  std::fs::write(&path, format!("invalid\n{start}{complete}\n{complete}")).unwrap();
  let slug = RunSlug::try_from("r1").unwrap();
  for mode in [InitialRead::Start, InitialRead::Tail(512)] {
    let mut progress = RunProgress::new(cwd.clone(), cwd.join("worktrees"), mode);
    let first = progress.read(&slug).unwrap();
    assert!(first.evidence.history_complete, "開始前の不正行は新しい履歴を不完全にしない");
    assert_eq!(first.evidence.events.len(), 1);
    let mut output = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    output.write_all(b"\ninvalid\n").unwrap();
    let second = progress.read(&slug).unwrap();
    assert_eq!(second.evidence.events.len(), 2);
    assert!(!second.evidence.history_complete, "開始後の欠落を既知ゼロにしない");
    assert_eq!(progress.read(&slug).unwrap().evidence, second.evidence);
    output.write_all(start.as_bytes()).unwrap();
    let reset = progress.read(&slug).unwrap();
    assert!(reset.evidence.history_complete);
    assert!(reset.evidence.events.is_empty());
    std::fs::write(&path, format!("invalid\n{start}{complete}\n{complete}")).unwrap();
  }
}
