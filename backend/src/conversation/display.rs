//! 保存形式の包みを解釈し、表示する本文を決める。

use serde_json::Value;

const ENCODING_ERROR_THRESHOLD: usize = 3;

pub(super) fn user_text(original: String) -> String {
  let extracted = current_input_section(&original).and_then(|(section, previous_input)| {
    let body = section.trim();
    if let Some((description, input)) = body.split_once('\n') {
      if description.contains("人の発言から始ま") {
        return current_human_text(input);
      }
    }
    if !previous_input {
      return None;
    }
    let json = if body.starts_with('{') { Some(body) } else { fenced_json(body) }?;
    let value: Value = serde_json::from_str(json).ok()?;
    if value.get("speaker").and_then(Value::as_str) != Some("user") {
      return None;
    }
    value.get("text").and_then(Value::as_str).map(str::to_owned)
  });
  // 包みを確定できない発言は、引用や読める部分を失わないよう原文で返す。
  extracted.unwrap_or(original)
}

fn current_human_text(input: &str) -> Option<String> {
  let mut values = serde_json::Deserializer::from_str(input).into_iter::<Value>();
  let value = values.next()?.ok()?;
  let suffix = input[values.byte_offset()..].trim();
  if !suffix.is_empty() && !suffix.starts_with("Business context (not approval):") {
    return None;
  }
  let speaker = value.get("speaker")?.as_object()?;
  speaker.get("namespace")?.as_str()?;
  speaker.get("id")?.as_str()?;
  value.get("text")?.as_str().map(str::to_owned)
}

pub(super) fn assistant_text(original: String) -> String {
  let extracted = fenced_json(&original)
    .and_then(|json| serde_json::from_str::<Value>(json).ok())
    .and_then(|value| value.get("message").and_then(Value::as_str).map(str::to_owned));
  // 本文全体が応答の包みであると確定できなければ、説明やコード例を保持する。
  extracted.unwrap_or(original)
}

pub(super) fn has_encoding_error(text: &str) -> bool {
  text.chars().filter(|character| *character == '\u{fffd}').take(ENCODING_ERROR_THRESHOLD).count()
    == ENCODING_ERROR_THRESHOLD
}

/// 表示する本文に添える注意書き。本文の置換文字がしきい値以上のときだけ、元記録の文字化けを示す。
///
/// 本文そのものは変更しません。
pub(super) fn notice(text: &str) -> Option<String> {
  let count = text.chars().filter(|character| *character == '\u{fffd}').count();
  (count >= ENCODING_ERROR_THRESHOLD).then(|| format!("元記録で文字化けしています（置換文字 {count} 個）"))
}

fn current_input_section(text: &str) -> Option<(&str, bool)> {
  let mut previous_input = false;
  let mut start = None;
  let mut offset = 0;
  let mut fence = None;
  for line in text.split_inclusive('\n') {
    let trimmed = line.trim();
    if let Some((marker, length, info)) = fence_line(trimmed) {
      match fence {
        | Some((opening_marker, opening_length)) => {
          if marker == opening_marker && length >= opening_length && info.is_empty() {
            fence = None;
          }
        },
        | None => fence = Some((marker, length)),
      }
    } else if fence.is_none() {
      if trimmed == "## 前のターンで受け取った人の発言" {
        previous_input = true;
        start = None;
      } else if trimmed == "## このターンの入力" {
        start = Some(offset + line.len());
      } else if start.is_some() && (trimmed.starts_with("# ") || trimmed.starts_with("## ")) {
        return start.map(|start| (&text[start..offset], previous_input));
      }
    }
    offset += line.len();
  }
  start.map(|start| (&text[start..], previous_input))
}

fn fenced_json(text: &str) -> Option<&str> {
  let (opening, rest) = text.trim().split_once('\n')?;
  let (marker, length, info) = fence_line(opening.trim())?;
  if info != "json" {
    return None;
  }
  let (body, closing) = rest.rsplit_once('\n')?;
  let (closing_marker, closing_length, closing_info) = fence_line(closing.trim())?;
  (closing_marker == marker && closing_length >= length && closing_info.is_empty()).then_some(body)
}

fn fence_line(line: &str) -> Option<(u8, usize, &str)> {
  let marker = *line.as_bytes().first()?;
  if marker != b'`' && marker != b'~' {
    return None;
  }
  let length = line.bytes().take_while(|byte| *byte == marker).count();
  (length >= 3).then(|| (marker, length, line[length..].trim()))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn user_input_is_validated_and_unrecognized_wrappers_are_preserved() {
    let wrapper = "## 前のターンで受け取った人の発言\n前回\n## このターンの入力\n```json\n{\"speaker\":\"user\",\"text\":\"公開してください\"}\n```\n## Business context\n{\"text\":\"内部\"}";
    assert_eq!(user_text(wrapper.to_owned()), "公開してください");
    for text in ["通常の発言", "## このターンの入力\n{broken", "## このターンの入力\n{\"speaker\":\"user\"}"]
    {
      assert_eq!(user_text(text.to_owned()), text);
    }
  }

  #[test]
  fn assistant_extracts_only_a_string_message_from_a_complete_json_fence() {
    for fence in ["```", "~~~~"] {
      let body = format!("{fence}json\n{{\"message\":\"応答\\n続き\",\"summary\":\"内部\"}}\n{fence}");
      assert_eq!(assistant_text(body), "応答\n続き");
    }
    for body in
      ["```json\n{broken\n```", "```json\n{\"message\":null}\n```", "説明\n```json\n{\"message\":\"例\"}\n```"]
    {
      assert_eq!(assistant_text(body.to_owned()), body);
    }
  }

  #[test]
  fn replacement_character_threshold_counts_characters_without_changing_the_body() {
    assert!(!has_encoding_error("日本語�と�"));
    assert!(has_encoding_error("日本語�と�と�"));
    assert!(has_encoding_error(&"�".repeat(87)));
    assert!(!has_encoding_error("日本語の通常文"));
  }

  #[test]
  fn current_section_excludes_other_sections_and_ignores_quoted_headings() {
    let text = "## 前のターンで受け取った人の発言\n```markdown\n## このターンの入力\n引用例\n```\n## このターンの入力\n今回\n## Business context\n内部";
    assert_eq!(current_input_section(text), Some(("今回\n", true)));
    assert_eq!(current_input_section("```markdown\n## このターンの入力\n引用例\n```"), None);
    assert_eq!(current_input_section("## このターンの入力\n今回"), Some(("今回", false)));
    assert_eq!(
      current_input_section("```markdown\n## 前のターンで受け取った人の発言\n```\n## このターンの入力\n今回"),
      Some(("今回", false))
    );
  }

  #[test]
  fn current_human_input_requires_a_speaker_and_only_allows_business_context_after_json() {
    let input = r#"{"speaker":{"namespace":"slack","id":"fixture-user"},"text":"本文\n続き"}"#;
    assert_eq!(current_human_text(input), Some("本文\n続き".to_owned()));
    assert_eq!(
      current_human_text(&format!("{input}\n\nBusiness context (not approval): ���")),
      Some("本文\n続き".to_owned())
    );
    for input in [
      "{broken",
      r#"{"speaker":{},"text":"本文"}"#,
      r#"{"speaker":{"namespace":"slack","id":"fixture-user"},"text":42}"#,
    ] {
      assert_eq!(current_human_text(input), None);
    }
    assert_eq!(current_human_text(&format!("{input}\n説明")), None);
    assert_eq!(current_human_text(&format!("{input}\n{{}}")), None);
  }

  #[test]
  fn json_fences_require_matching_delimiters_and_cover_the_entire_body() {
    assert_eq!(fenced_json(" \n````json\n{}\n`````\n"), Some("{}"));
    assert_eq!(fenced_json("~~~json\n{}\n~~~"), Some("{}"));
    for body in ["```json\n{}", "````json\n{}\n```", "```json\n{}\n~~~", "```text\n{}\n```", "```json\n{}\n```\n説明"]
    {
      assert_eq!(fenced_json(body), None, "{body}");
    }
    let response = "```json\n{\"message\":\"引用 ``` と ~~~\"}\n```";
    assert_eq!(assistant_text(response.to_owned()), "引用 ``` と ~~~");
  }

  #[test]
  fn fence_line_identifies_markers_lengths_and_info() {
    assert_eq!(fence_line("````json"), Some((b'`', 4, "json")));
    assert_eq!(fence_line("~~~"), Some((b'~', 3, "")));
    assert_eq!(fence_line("``"), None);
    assert_eq!(fence_line(""), None);
    assert_eq!(fence_line("本文"), None);
  }
}
