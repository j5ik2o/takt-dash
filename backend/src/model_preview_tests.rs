//! 省略記号の付け方の性質を、生成した入力で確かめる（設計: docs/design/issue-230.md の P3・P5）。

use proptest::prelude::*;

use super::{truncate_chars, with_ellipsis, PreviewText, ELLIPSIS};

/// 日本語・絵文字・URL・改行・省略記号を混ぜた文字列。
fn text() -> impl Strategy<Value = String> {
  proptest::collection::vec(
    prop_oneof![
      Just("あ".to_owned()),
      Just("😀".to_owned()),
      Just("https://example.test/a?b=c".to_owned()),
      Just("\n".to_owned()),
      Just(" ".to_owned()),
      Just("。".to_owned()),
      Just(ELLIPSIS.to_string()),
      "[a-z]{1,4}",
    ],
    0..40,
  )
  .prop_map(|parts| parts.concat())
}

proptest! {
  // P5：切ってあるときだけ、元の文の末尾に省略記号を 1 つ足す。
  #[test]
  fn display_adds_one_ellipsis_only_when_truncated(text in text(), truncated in any::<bool>()) {
    let preview = PreviewText { text: text.clone(), truncated };
    let shown = preview.display();
    if truncated {
      prop_assert_eq!(shown, format!("{text}{ELLIPSIS}"));
    } else {
      prop_assert_eq!(shown, text);
    }
  }

  // P3 の核：上限以下の文は変えない。超えた文は先頭の上限文字＋省略記号 1 つ。
  #[test]
  fn truncate_chars_marks_only_a_cut(text in text(), max in 0usize..60) {
    let cut = truncate_chars(&text, max);
    let count = text.chars().count();
    if count <= max {
      prop_assert_eq!(cut, text);
    } else {
      let head: String = text.chars().take(max).collect();
      prop_assert_eq!(cut.chars().count(), max + 1);
      prop_assert_eq!(cut, with_ellipsis(&head, true));
    }
  }
}

#[test]
fn complete_text_is_never_marked() {
  assert_eq!(PreviewText::from("文").display(), "文");
  assert_eq!(PreviewText::complete(String::new()).display(), "");
}
