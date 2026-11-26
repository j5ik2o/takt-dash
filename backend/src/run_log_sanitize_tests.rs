//! run ログの文字列と run の要約の切り詰めに、省略記号が付くことを確かめる（設計:
//! docs/design/issue-230.md の P3・P4）。

use proptest::prelude::*;
use serde_json::{json, Value};

use super::{sanitize, MAX_SUMMARY_CHARS};
use crate::{model::ELLIPSIS, run_progress::task_summary};

fn long_text() -> impl Strategy<Value = String> {
  (prop_oneof![Just("あ"), Just("a"), Just("😀"), Just("…")], 0usize..700).prop_map(|(unit, count)| unit.repeat(count))
}

proptest! {
  // P4：上限以下の文字列は変えず、超える文字列は先頭の上限文字＋省略記号 1 つにする。
  #[test]
  fn sanitize_marks_only_cut_strings(a in long_text(), b in long_text()) {
    let mut record = json!({ "kind": "step", "data": { "text": a, "items": [b, 1] } });
    sanitize(&mut record);
    let sanitized_a = record["data"]["text"].as_str().map(str::to_owned);
    let sanitized_b = record["data"]["items"][0].as_str().map(str::to_owned);
    prop_assert_eq!(record["kind"].clone(), Value::from("step"));
    for (original, sanitized) in [(&a, sanitized_a), (&b, sanitized_b)] {
      let sanitized = sanitized.expect("文字列のままです");
      if original.chars().count() <= MAX_SUMMARY_CHARS {
        prop_assert_eq!(&sanitized, original);
      } else {
        let head: String = original.chars().take(MAX_SUMMARY_CHARS).collect();
        prop_assert_eq!(sanitized, format!("{head}{ELLIPSIS}"));
      }
    }
  }

  // P3：1 行目が 80 文字以下なら 1 行目そのもの、超えれば先頭 80 文字＋省略記号 1 つ。
  #[test]
  fn task_summary_marks_only_a_cut_first_line(first in long_text(), rest in "[a-z\n]{0,20}") {
    let task = format!("{first}\n{rest}");
    let summary = task_summary(&task);
    let line = task.trim().lines().next().unwrap_or_default().to_owned();
    if line.chars().count() <= 80 {
      prop_assert_eq!(summary, line);
    } else {
      let head: String = line.chars().take(80).collect();
      prop_assert_eq!(summary, format!("{head}{ELLIPSIS}"));
    }
  }
}

#[test]
fn prompt_fields_are_still_removed() {
  let mut record = json!({ "systemPrompt": "秘密", "content": "本文", "kind": "x" });
  sanitize(&mut record);
  assert_eq!(record, json!({ "kind": "x" }));
}
