//! takt-mcp の応答の印から、切り詰めを決める規則の性質を確かめる（設計: docs/design/issue-230.md
//! の P1・P2）。

use proptest::prelude::*;
use serde_json::json;

use super::{goal::parse_goal_objective, parse::Shortened};

/// takt と同じく、文字の境界で `limit` バイト以下に切る。
fn truncate_utf8(text: &str, limit: usize) -> &str {
  let mut end = text.len().min(limit);
  while !text.is_char_boundary(end) {
    end -= 1;
  }
  &text[..end]
}

fn text() -> impl Strategy<Value = String> {
  proptest::collection::vec(
    prop_oneof![
      Just("あ"),
      Just("😀"),
      Just("https://example.test/path"),
      Just("\n"),
      Just("。"),
      Just("」"),
      Just("…"),
      Just("a"),
    ],
    0..80,
  )
  .prop_map(|parts| parts.concat())
}

fn page_paths() -> impl Strategy<Value = Vec<String>> {
  proptest::collection::vec(
    prop_oneof![
      Just("goals[].objective".to_owned()),
      Just("goals[].pullRequest.url".to_owned()),
      Just("goals.0.objective".to_owned()),
      Just("questions[].body".to_owned()),
      Just("objective".to_owned()),
    ],
    0..4,
  )
}

proptest! {
  // P1：要素の totalBytes が元の文のバイト数なら、印は「preview が元の文と違う」ことと一致し、
  // 文は preview のまま。
  #[test]
  fn objective_is_truncated_exactly_when_the_preview_differs(source in text(), limit in 0usize..300) {
    let preview = truncate_utf8(&source, limit);
    let item = json!({ "objective": preview, "omissions": { "objective": { "totalBytes": source.len() } } });
    let objective = parse_goal_objective(&item, &Shortened::default());
    prop_assert_eq!(objective.truncated, preview != source);
    prop_assert_eq!(objective.text, preview);
  }

  // P2：印は、要素の印（totalBytes）かページの印（goals[].objective、omittedDetails）のどちらかが
  // 立つときだけ立つ。番号の残った path や、ほかの配列・項目の path では立たない。
  #[test]
  fn objective_mark_comes_only_from_the_item_or_the_page(
    text in text(),
    extra_bytes in prop_oneof![Just(None), (0usize..3).prop_map(Some)],
    paths in page_paths(),
    omitted in 0u64..3,
  ) {
    let mut item = json!({ "objective": text });
    if let Some(extra) = extra_bytes {
      item["omissions"] = json!({ "objective": { "totalBytes": text.len() + extra } });
    }
    let page = json!({
      "goals": [item.clone()],
      "omissions": paths.iter().map(|path| json!({ "path": path })).collect::<Vec<_>>(),
      "omittedDetails": omitted,
    });
    let shortened = Shortened::from_value(&page, "goals");
    let objective = parse_goal_objective(&item, &shortened);
    let item_mark = extra_bytes.is_some_and(|extra| extra > 0);
    let page_mark = !text.is_empty() && (omitted > 0 || paths.iter().any(|path| path == "goals[].objective"));
    prop_assert_eq!(objective.truncated, item_mark || page_mark);
    prop_assert_eq!(objective.text, text);
  }

  // 文を短くした項目だけに印を付け、表示用の文は末尾に省略記号を 1 つ足す。空の文には付けない。
  #[test]
  fn display_marks_only_the_shortened_field(text in text(), paths in page_paths()) {
    let page = json!({ "questions": [], "omissions": paths.iter().map(|path| json!({ "path": path })).collect::<Vec<_>>() });
    let shortened = Shortened::from_value(&page, "questions");
    let cut = !text.is_empty() && paths.iter().any(|path| path == "questions[].body");
    let shown = shortened.display(text.clone(), "body");
    if cut {
      prop_assert_eq!(shown, format!("{text}…"));
    } else {
      prop_assert_eq!(shown, text);
    }
  }
}

#[test]
fn a_broken_total_smaller_than_the_preview_is_not_a_cut() {
  let item = json!({ "objective": "目標です", "omissions": { "objective": { "totalBytes": 3 } } });
  assert!(!parse_goal_objective(&item, &Shortened::default()).truncated);
  let item = json!({ "objective": "目標です", "omissions": { "objective": { "totalBytes": "多い" } } });
  assert!(!parse_goal_objective(&item, &Shortened::default()).truncated);
}
