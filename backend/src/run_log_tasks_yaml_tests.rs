//! `parse_task_entries` が、`tools/project-status/task_context.py` の `parse_tasks_yaml`
//! と同じ項目を 読むことを、同じファイルを両方に読ませて確かめる。
//!
//! Python は 1 本の子プロセスで立て、標準入出力で 1 件ずつ問い合わせる。`python3` が無ければ
//! テストは失敗する。

use std::{
  cell::RefCell,
  fs,
  io::{BufRead, BufReader, Write},
  path::{Path, PathBuf},
  process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use proptest::{prelude::*, test_runner::TestRunner};
use serde_json::{json, Value};

use super::{parse_task_entries, TaskEntry};

const ORACLE: &str = r#"
import json, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import task_context

def text_or_none(entry, key):
    value = entry.get(key, "")
    return value if value else None

for request in sys.stdin:
    path = json.loads(request)["path"]
    has_tasks = any(line.strip() == "tasks:" for line in Path(path).read_text(encoding="utf-8").splitlines())
    entries = [
        {key: text_or_none(entry, key) for key in ("run_slug", "source_run_slug", "worktree_path")}
        for entry in task_context.parse_tasks_yaml(path)
    ]
    print(json.dumps({"has_tasks": has_tasks, "entries": entries}), flush=True)
"#;

/// Python 側の reader が読んだ結果。
#[derive(Debug, PartialEq)]
struct OracleRead {
  has_tasks: bool,
  entries:   Vec<Value>,
}

struct Oracle {
  child:  Child,
  stdin:  ChildStdin,
  stdout: BufReader<ChildStdout>,
  _dir:   tempfile::TempDir,
  path:   PathBuf,
}

impl Oracle {
  fn start() -> Oracle {
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/project-status");
    let mut child = Command::new("python3")
      .arg("-I")
      .arg("-c")
      .arg(ORACLE)
      .arg(&tools)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .spawn()
      .expect("python3 を起動できます（このテストは Python 側の reader と比べるため python3 が要ります）");
    let stdin = child.stdin.take().expect("python3 の標準入力があります");
    let stdout = BufReader::new(child.stdout.take().expect("python3 の標準出力があります"));
    let dir = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let path = dir.path().join("tasks.yaml");
    Oracle { child, stdin, stdout, _dir: dir, path }
  }

  /// `text` をファイルに書き、Rust と Python の両方に読ませる。
  fn read_both(&mut self, text: &str) -> (Result<Vec<TaskEntry>, String>, OracleRead) {
    fs::write(&self.path, text).expect("tasks.yaml を書けます");
    let rust = parse_task_entries(&fs::read_to_string(&self.path).expect("書いた tasks.yaml を読めます"));
    let request = json!({ "path": self.path });
    writeln!(self.stdin, "{request}").expect("python3 へ問い合わせを書けます");
    let mut line = String::new();
    self.stdout.read_line(&mut line).expect("python3 の答えを読めます");
    let answer: Value =
      serde_json::from_str(&line).unwrap_or_else(|_| panic!("python3 の答えが JSON ではありません: {line:?}"));
    let python = OracleRead {
      has_tasks: answer["has_tasks"].as_bool().expect("has_tasks は真偽値です"),
      entries:   answer["entries"].as_array().expect("entries は配列です").clone(),
    };
    (rust, python)
  }

  /// I1・I2: `tasks:` の行の有無で `Ok`／`Err` が決まり、`Ok` なら項目が Python と一致する。
  fn check(&mut self, text: &str) -> Result<(), TestCaseError> {
    let (rust, python) = self.read_both(text);
    match rust {
      | Ok(entries) => {
        prop_assert!(python.has_tasks, "tasks: の行が無いのに Ok を返しました: {text:?}");
        let rust_entries: Vec<Value> = entries.iter().map(entry_json).collect();
        prop_assert_eq!(rust_entries, python.entries, "Python と項目が違います: {:?}", text);
      },
      | Err(_) => prop_assert!(!python.has_tasks, "tasks: の行があるのに Err を返しました: {text:?}"),
    }
    Ok(())
  }
}

impl Drop for Oracle {
  fn drop(&mut self) {
    let _ = self.child.kill();
    let _ = self.child.wait();
  }
}

fn entry_json(entry: &TaskEntry) -> Value {
  json!({
    "run_slug": entry.run_slug,
    "source_run_slug": entry.source_run_slug,
    "worktree_path": entry.worktree_path.as_ref().map(|path| path.to_str().expect("UTF-8 のパスです")),
  })
}

fn run_property(cases: u32, strategy: impl Strategy<Value = String>) {
  let oracle = RefCell::new(Oracle::start());
  let mut runner = TestRunner::new(ProptestConfig { cases, ..ProptestConfig::default() });
  runner.run(&strategy, |text| oracle.borrow_mut().check(&text)).unwrap_or_else(|error| panic!("{error}"));
}

/// 構造を作る文字と、Python と Rust で扱いが分かれうる Unicode に寄せた文字。
const SOUP: &[char] = &[
  '-', '-', ':', ':', ' ', ' ', ' ', '\n', '\n', '\r', '\t', '\'', '"', '#', '>', '|', '~', '+', 'a', 'r', 'u', 'n',
  '_', 's', 'l', 'g', '/', '.', 'k', 'é', 'パ', 'ス', '🚀', '\u{3000}', '\u{a0}', '\u{feff}', '\u{0b}', '\u{0c}',
  '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}', '\u{85}', '\u{2028}', '\u{2029}', '\u{200b}',
];

fn soup(max: usize) -> impl Strategy<Value = String> {
  prop::collection::vec(prop::sample::select(SOUP), 0..max).prop_map(|chars| chars.into_iter().collect())
}

fn key() -> impl Strategy<Value = String> {
  prop_oneof![
    4 => prop::sample::select(vec!["run_slug", "source_run_slug", "worktree_path"]).prop_map(str::to_owned),
    1 => prop::sample::select(vec![
      "name", "retry_note", "completion", "'run_slug'", "\"worktree_path\"", "'source_run_slug\"", "run_slug ",
      "run slug", "tasks", "", "- run_slug",
    ])
    .prop_map(str::to_owned),
    1 => soup(6),
  ]
}

fn value_text() -> impl Strategy<Value = String> {
  prop_oneof![
    3 => "[a-z0-9][a-z0-9-]{0,10}",
    2 => "/[a-zA-Z0-9_/.-]{0,12}",
    1 => prop::sample::select(vec!["/作業/パス", "ラン-1", "🚀run", "a: b", "C:/x", "x # y", "- run_slug: z", "''", "it's"])
      .prop_map(str::to_owned),
    1 => soup(10),
  ]
}

fn value() -> impl Strategy<Value = String> {
  prop_oneof![
    3 => value_text(),
    2 => value_text().prop_map(|text| format!("\"{text}\"")),
    2 => value_text().prop_map(|text| format!("'{text}'")),
    1 => value_text().prop_map(|text| format!("\"{text}")),
    1 => value_text().prop_map(|text| format!("{text}'")),
    1 => prop::sample::select(vec!["", " ", "\"", "'", "\"\"", "''", "~", "null", "Null", "NULL", "\"null\""])
      .prop_map(str::to_owned),
    2 => prop::sample::select(vec![">", ">-", ">+", "|", "|-", "|+", ">2", "|foo"]).prop_map(str::to_owned),
  ]
}

fn field() -> impl Strategy<Value = String> {
  (key(), " {0,2}", value()).prop_map(|(key, space, value)| format!("{key}:{space}{value}"))
}

/// 1 行の中身（字下げの後）。
fn line_body() -> impl Strategy<Value = String> {
  prop_oneof![
    4 => (" {1,3}", field()).prop_map(|(space, field)| format!("-{space}{field}")),
    6 => field(),
    1 => soup(8).prop_map(|text| format!("# {text}")),
    1 => Just(String::new()),
    1 => Just("-".to_owned()),
    1 => Just("tasks:".to_owned()),
    1 => value_text(),
    1 => soup(12),
  ]
}

/// P1: `tasks:` の見出しの後に、字下げと形を変えた行を並べた文書。
fn structured_document() -> impl Strategy<Value = String> {
  let header = prop::sample::select(vec!["tasks:", "tasks:  ", "  tasks:", "tasks:\t", "\u{3000}tasks:"]);
  let preamble = prop::collection::vec(prop::sample::select(vec!["", "# 一覧", "version: 1"]), 0..2);
  let line = (0usize..=8, line_body()).prop_map(|(indent, body)| format!("{}{body}", " ".repeat(indent)));
  let newline = prop::sample::select(vec!["\n", "\r\n", "\r"]);
  (preamble, header, prop::collection::vec(line, 0..24), newline, any::<bool>()).prop_map(
    |(preamble, header, lines, newline, trailing_newline)| {
      let mut document: Vec<String> = preamble.into_iter().map(str::to_owned).collect();
      document.push(header.to_owned());
      document.extend(lines);
      let mut text = document.join(newline);
      if trailing_newline {
        text.push_str(newline);
      }
      text
    },
  )
}

#[test]
fn structured_documents_read_the_same_entries_as_python() {
  run_property(512, structured_document());
}

#[test]
fn arbitrary_text_after_the_tasks_header_reads_the_same_entries_as_python() {
  run_property(512, prop_oneof![soup(200), any::<String>()].prop_map(|text| format!("tasks:\n{text}")));
}

#[test]
fn arbitrary_text_is_err_exactly_when_python_sees_no_tasks_line() {
  run_property(512, prop_oneof![
    soup(120),
    any::<String>(),
    structured_document().prop_map(|text| text.replace("tasks", "task"))
  ]);
}

fn entry(run_slug: Option<&str>, source_run_slug: Option<&str>, worktree_path: Option<&str>) -> TaskEntry {
  TaskEntry {
    run_slug:        run_slug.map(str::to_owned),
    source_run_slug: source_run_slug.map(str::to_owned),
    worktree_path:   worktree_path.map(PathBuf::from),
  }
}

/// 代表ケース: 決めた結果を読み、Python も同じ結果を読む。
#[test]
fn representative_cases_match_python_and_the_expected_entries() {
  let cases: Vec<(&str, Vec<TaskEntry>)> = vec![
    ("tasks:\n  - run_slug: \"run-a\"\n    worktree_path: '/abs/path'\n", vec![entry(
      Some("run-a"),
      None,
      Some("/abs/path"),
    )]),
    ("tasks:\n- run_slug: run-a\n  source_run_slug: run-s\n- run_slug: run-b\n", vec![
      entry(Some("run-a"), Some("run-s"), None),
      entry(Some("run-b"), None, None),
    ]),
    ("tasks:\n      - run_slug: run-a\n        worktree_path: /wt\n", vec![entry(Some("run-a"), None, Some("/wt"))]),
    ("tasks:\n  - retry_note: >-\n      run_slug: bogus\n    run_slug: run-a\n", vec![entry(
      Some("run-a"),
      None,
      None,
    )]),
    ("tasks:\n  - run_slug: >-\n      run-a\n    worktree_path: /wt\n", vec![entry(None, None, Some("/wt"))]),
    ("tasks:\n  - run_slug: ~\n    source_run_slug: null\n    worktree_path: \"\"\n", vec![entry(None, None, None)]),
    ("tasks:\r\n  - run_slug: 'ラン'\r\n    worktree_path: /作業/パス\r\n", vec![entry(
      Some("ラン"),
      None,
      Some("/作業/パス"),
    )]),
    ("tasks:\n  - 'run_slug': run-a\n    \"worktree_path\" : /wt\n", vec![entry(Some("run-a"), None, Some("/wt"))]),
    ("tasks:\n  - name: a\n  - run_slug: run-b\n", vec![entry(None, None, None), entry(Some("run-b"), None, None)]),
    ("tasks:\n", vec![]),
    ("tasks:\n  -\n    run_slug: run-a\n  -\n    run_slug: run-b\n    worktree_path: /wt\n", vec![
      entry(Some("run-a"), None, None),
      entry(Some("run-b"), None, Some("/wt")),
    ]),
  ];
  let mut oracle = Oracle::start();
  for (text, expected) in cases {
    let (rust, python) = oracle.read_both(text);
    let rust = rust.unwrap_or_else(|error| panic!("{text:?} を読めません: {error}"));
    let rust_json: Vec<Value> = rust.iter().map(entry_json).collect();
    let expected_json: Vec<Value> = expected.iter().map(entry_json).collect();
    assert_eq!(rust_json, expected_json, "期待した項目と違います: {text:?}");
    assert_eq!(python.entries, expected_json, "Python が期待した項目と違います: {text:?}");
  }
}

#[test]
fn text_without_tasks_line_is_err() {
  let mut oracle = Oracle::start();
  for text in ["", "\n", "- run_slug: run-a\n", "tasks: []\n", "tasks:x\n"] {
    let (rust, python) = oracle.read_both(text);
    assert_eq!(rust.is_err(), !python.has_tasks, "Err の判定が Python の tasks: の有無と違います: {text:?}");
  }
  assert!(parse_task_entries("- run_slug: run-a\n").is_err(), "tasks: の行が無ければ Err にします");
}
