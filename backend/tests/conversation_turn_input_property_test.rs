//! 「## このターンの入力」の包みから本文と折りたたみの中身を取り出す規則を、
//! 生成した入力で確かめる（設計: docs/design/issue-377.md の「PBT の性質」）。
//!
//! 包みの組み立ては `conversation_fixture` の実データと同じ並び（見出し、空行、説明文、JSON の行、
//! Business context、応答形式の指示）を使い、説明文・イベント・質問・回答・
//! 前のターンの本文を生成する。

mod conversation_fixture;

use std::path::Path;

use conversation_fixture::{
  data_text, director_turn_data, event_turn_data, inquiry_answered_data, plain_user_data, turn_data, wrapped_user_data,
  ConversationFixture, BUSINESS_CONTEXT, RESPONSE_FORMAT,
};
use proptest::{
  collection::{btree_map, vec},
  prelude::*,
};
use serde_json::{json, Map, Value};
use takt_dash_backend::{
  conversation::{ConversationMessage, ConversationQuery, ConversationSpeaker},
  domain::ProjectName,
};

const AT: i64 = 1_794_613_221_331;
const MARKER: &str = "## このターンの入力";
const INSTRUCTION: &str =
  "To advance the assigned goals, compare the change conveyed by this event with the current state.";

fn read_single(data: &str) -> ConversationMessage {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("alpha", "ses_alpha", &project_cwd, "opencode");
  fixture.insert_session("ses_alpha", &project_cwd, 2_000);
  fixture.insert_message("ses_alpha", 1, "user", AT, data);
  let page = fixture.reader().read(&query(&project_cwd));
  assert_eq!(page.messages.len(), 1, "user の行は1件として返る: {:?}", page.skipped);
  page.messages.into_iter().next().expect("1件ある")
}

fn query(cwd: &Path) -> ConversationQuery {
  ConversationQuery {
    project:       ProjectName::try_from("takt-dash").expect("有効なプロジェクト名"),
    cwd:           cwd.to_path_buf(),
    project_roots: vec![cwd.to_path_buf()],
    session_id:    None,
    after_seq:     0,
    limit:         50,
  }
}

/// 包みの前に、前のターンの本文（見出しを含みうる）を置く。
fn with_previous_turn(data: &str, previous: &str) -> String {
  let text = format!("\n## 前のターンで受け取った人の発言\n\n{previous}\n{}", data_text(data));
  serde_json::to_string(&json!({ "time": { "created": AT }, "text": text })).expect("data を組み立てられる")
}

/// 包みの構造に関わる行（JSON の形の行、見出し、Business context、応答形式の指示で始まる行）。
fn is_structural(line: &str) -> bool {
  let trimmed = line.trim();
  (trimmed.starts_with('{') && trimmed.ends_with('}'))
    || line.starts_with(MARKER)
    || line.trim_start().starts_with("Business context (not approval):")
    || line.trim_start().starts_with("次の JSON schema に一致する")
}

fn description_line() -> impl Strategy<Value = String> {
  "[^\n]{0,40}"
}

fn description_lines() -> impl Strategy<Value = Vec<String>> {
  vec(description_line(), 0..4)
    .prop_filter("包みの構造に関わる行を含まない", |lines| lines.iter().all(|line| !is_structural(line)))
}

fn expected_description(lines: &[String]) -> Option<String> {
  let kept: Vec<&str> = lines.iter().map(|line| line.trim()).filter(|line| !line.is_empty()).collect();
  (!kept.is_empty()).then(|| kept.join("\n"))
}

/// 文字列・数・真偽・null・配列・入れ子のオブジェクトを含む任意の JSON の値。
fn json_value() -> impl Strategy<Value = Value> {
  let leaf = prop_oneof![
    Just(Value::Null),
    any::<bool>().prop_map(Value::Bool),
    any::<i64>().prop_map(Value::from),
    any::<String>().prop_map(Value::String),
  ];
  leaf.prop_recursive(3, 24, 4, |inner| {
    prop_oneof![
      vec(inner.clone(), 0..4).prop_map(Value::Array),
      btree_map(any::<String>(), inner, 0..4).prop_map(|map| Value::Object(map.into_iter().collect())),
    ]
  })
}

fn event_payload() -> impl Strategy<Value = (String, Value)> {
  (
    any::<String>().prop_filter("種類は空でない", |kind| !kind.is_empty()),
    btree_map(any::<String>(), json_value(), 0..4),
  )
    .prop_map(|(kind, fields)| {
      let mut event: Map<String, Value> = fields.into_iter().collect();
      event.insert("kind".to_owned(), Value::String(kind.clone()));
      (kind, Value::Object(event))
    })
}

/// 回答の `text` の候補。無い、空、空白だけ、任意の文字列。
fn answer_text() -> impl Strategy<Value = Option<String>> {
  prop_oneof![Just(None), Just(Some(String::new())), Just(Some(" \n\t".to_owned())), any::<String>().prop_map(Some)]
}

/// 回答の `report` の候補。無い、空、completed、unable、任意の文字列、文字列でない値。
fn answer_report() -> impl Strategy<Value = Option<Value>> {
  prop_oneof![
    Just(None),
    Just(Some(json!(""))),
    Just(Some(json!("completed"))),
    Just(Some(json!("unable"))),
    any::<String>().prop_map(|report| Some(json!(report))),
    any::<i64>().prop_map(|report| Some(json!(report))),
  ]
}

/// 設計の「回答の本文」の表。
fn expected_answer_body(text: Option<&str>, report: Option<&Value>) -> String {
  let text = text.filter(|text| !text.trim().is_empty());
  let report = report.and_then(Value::as_str).filter(|report| !report.is_empty());
  let word = |report: &str| match report {
    | "completed" => "実施した".to_owned(),
    | "unable" => "対応できない".to_owned(),
    | other => other.to_owned(),
  };
  match (text, report) {
    | (Some(text), None) => text.to_owned(),
    | (Some(text), Some(report)) => format!("{text}\n（回答: {}）", word(report)),
    | (None, Some(report)) if report == "completed" || report == "unable" => word(report),
    | (None, Some(report)) => format!("回答: {report}"),
    | (None, None) => "回答（本文なし）".to_owned(),
  }
}

/// どの形にも当たらない JSON の行。キーはどれも `x_` で始め、判定に使うキーを持たない。
fn unmatched_input() -> impl Strategy<Value = Value> {
  btree_map("x_[a-zA-Z]{0,8}", json_value(), 0..4).prop_map(|map| Value::Object(map.into_iter().collect()))
}

const GENERATED_BUSINESS_CONTEXT: &str = "Business context (not approval): {\"pending\":null}";
const GENERATED_RESPONSE_FORMAT: &str = "次の JSON schema に一致する fenced JSON block をちょうど1つ返してください:";

/// 現在の包み（行頭の最後の見出しより後）の応答形式の指示と Business context が、本文にも
/// 折りたたみにも出ていないことを確かめる。説明文に見出しの行があると、それより前は前のターンの
/// 本文として扱われ、元のまま残る。
fn assert_no_wrapper_leak(message: &ConversationMessage, lines: &[String]) {
  let split = lines.iter().rposition(|line| line == MARKER).map_or(0, |index| index + 1);
  let (previous, current) = lines.split_at(split);
  let mut forbidden = vec![BUSINESS_CONTEXT, RESPONSE_FORMAT];
  for generated in [GENERATED_BUSINESS_CONTEXT, GENERATED_RESPONSE_FORMAT] {
    if current.iter().any(|line| line == generated) && !previous.iter().any(|line| line == generated) {
      forbidden.push(generated);
    }
  }
  let detail = message.detail.as_ref().map(|detail| detail.source_json.as_str()).unwrap_or("");
  let question = message.detail.as_ref().and_then(|detail| detail.question.as_deref()).unwrap_or("");
  for shown in [message.text.as_str(), detail, question] {
    for text in &forbidden {
      assert!(!shown.contains(text), "{text:?} を出さない: {shown:?}");
    }
  }
}

proptest! {
  #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

  /// 出来事から始まったターンは、話者がシステムで、本文は説明文（空なら「イベント: 種類」）、
  /// 折りたたみはイベントの JSON と同じ値で、instruction を含まない。
  /// 前のターンの本文に見出しがあっても変わらない。
  #[test]
  fn event_turn_shows_the_description_and_folds_the_event(
    (kind, event) in event_payload(),
    lines in description_lines(),
    business_context in any::<bool>(),
    previous in any::<String>(),
  ) {
    let data = with_previous_turn(&event_turn_data(Some(&lines.join("\n")), &event, business_context, AT), &previous);

    let message = read_single(&data);

    prop_assert_eq!(message.speaker, ConversationSpeaker::System);
    let expected = expected_description(&lines).unwrap_or_else(|| format!("イベント: {kind}"));
    prop_assert_eq!(&message.text, &expected);
    let detail = message.detail.as_ref().expect("折りたたみがある");
    prop_assert_eq!(detail.question.as_deref(), None);
    prop_assert_eq!(serde_json::from_str::<Value>(&detail.source_json).expect("JSON"), event);
    prop_assert!(!detail.source_json.contains(INSTRUCTION), "instruction を折りたたみに入れない");
  }

  /// 質問への回答（kind のある形と、kind の無い古い形）は、話者がユーザーで、本文は
  /// (text, report) の組み合わせで設計の表のとおり。折りたたみは質問の文（空なら無し）と元の JSON。
  #[test]
  fn inquiry_answer_body_follows_the_text_and_report(
    legacy in any::<bool>(),
    text in answer_text(),
    report in answer_report(),
    question in any::<String>(),
    business_context in any::<bool>(),
    previous in any::<String>(),
  ) {
    let mut answer = json!({ "source": "takt-mcp", "answeredAt": "2026-11-20T08:00:00.000Z" });
    if let Some(text) = &text {
      answer["text"] = json!(text);
    }
    if let Some(report) = &report {
      answer["report"] = report.clone();
    }
    let mut input = json!({ "goalId": null, "questionId": "751b5b5f", "question": question, "answer": answer });
    if !legacy {
      input["kind"] = json!("inquiry_answered");
    }
    let description = "このターンは保存した質問への人の回答から始まりました。";
    let data = with_previous_turn(&turn_data(Some(description), &input, business_context, true, AT), &previous);

    let message = read_single(&data);

    prop_assert_eq!(message.speaker, ConversationSpeaker::User);
    prop_assert_eq!(&message.text, &expected_answer_body(text.as_deref(), report.as_ref()));
    let detail = message.detail.as_ref().expect("折りたたみがある");
    prop_assert_eq!(detail.question.as_deref(), (!question.is_empty()).then_some(question.as_str()));
    prop_assert_eq!(serde_json::from_str::<Value>(&detail.source_json).expect("JSON"), input);
  }

  /// director の依頼は、話者がディレクターで、本文は text そのもの、折りたたみは元の JSON。
  #[test]
  fn director_turn_shows_the_request_and_folds_the_source(
    text in any::<String>().prop_filter("空白だけではない", |text| !text.trim().is_empty()),
    business_context in any::<bool>(),
    previous in any::<String>(),
  ) {
    let message = read_single(&with_previous_turn(&director_turn_data(&text, business_context, AT), &previous));

    prop_assert_eq!(message.speaker, ConversationSpeaker::Director);
    prop_assert_eq!(&message.text, &text);
    let source: Value = serde_json::from_str(&message.detail.as_ref().expect("折りたたみ").source_json).expect("JSON");
    prop_assert_eq!(&source["text"], text.as_str());
    prop_assert_eq!(&source["origin"], "director");
  }

  /// 人が話しかけたターンは、これまでどおり人の発言だけを返し、折りたたみを持たない。
  #[test]
  fn human_speech_turn_is_unchanged(
    human in any::<String>().prop_filter("発言は空でない", |human| !human.is_empty()),
    previous in any::<String>(),
  ) {
    let message = read_single(&wrapped_user_data(&previous, &human, AT));

    prop_assert_eq!(message.speaker, ConversationSpeaker::User);
    prop_assert_eq!(&message.text, &human);
    prop_assert_eq!(message.detail, None);
  }

  /// 行頭に見出しの無い本文は、そのまま返り、折りたたみを持たない。
  #[test]
  fn text_without_a_marker_line_is_returned_unchanged(
    text in any::<String>().prop_filter("空でない", |text| !text.is_empty()),
  ) {
    prop_assume!(!text.starts_with(MARKER) && !text.contains(&format!("\n{MARKER}")));

    let message = read_single(&plain_user_data(&text, AT));

    prop_assert_eq!(message.speaker, ConversationSpeaker::User);
    prop_assert_eq!(&message.text, &text);
    prop_assert_eq!(message.detail, None);
  }

  /// どの形にも当たらない包みは、見つけた応答形式の指示と Business context だけを除いた
  /// 元の本文を返す。どちらも無ければ元の本文のまま。前のターンの本文と説明文は落とさない。
  #[test]
  fn unmatched_wrapper_drops_only_the_found_instructions(
    input in unmatched_input(),
    lines in description_lines(),
    business_context in any::<bool>(),
    response_format in any::<bool>(),
    previous in any::<String>(),
  ) {
    let data = with_previous_turn(&turn_data(Some(&lines.join("\n")), &input, business_context, response_format, AT), &previous);
    let raw = data_text(&data);
    let mut expected = raw.clone();
    if response_format {
      expected.truncate(expected.rfind(RESPONSE_FORMAT).expect("指示がある"));
    }
    if business_context {
      let at = expected.rfind(&format!("{BUSINESS_CONTEXT}\n")).expect("Business context がある");
      expected.replace_range(at..at + BUSINESS_CONTEXT.len() + 1, "");
    }
    if business_context || response_format {
      expected = expected.trim_end().to_owned();
    }

    let message = read_single(&data);

    prop_assert_eq!(message.speaker, ConversationSpeaker::User);
    prop_assert_eq!(&message.text, &expected);
    prop_assert_eq!(message.detail, None);
  }

  /// 出来事の性質で外した説明文（JSON の形の行、見出しだけの行、`{"speaker":…}` の行）を
  /// 含めても、応答形式の指示と Business context は本文にも折りたたみにも出ない。
  #[test]
  fn wrapper_never_leaks_for_any_description(
    (_kind, event) in event_payload(),
    lines in vec(prop_oneof![
      description_line(),
      Just(MARKER.to_owned()),
      Just("{}".to_owned()),
      Just(r#"{"speaker":{"id":"x"},"text":"説明文に紛れた発言"}"#.to_owned()),
      Just(r#"{"x_unknown":1}"#.to_owned()),
      Just(GENERATED_BUSINESS_CONTEXT.to_owned()),
      Just(GENERATED_RESPONSE_FORMAT.to_owned()),
      "\\{[^\n]{0,20}\\}",
    ], 0..4),
    business_context in any::<bool>(),
  ) {
    let message = read_single(&event_turn_data(Some(&lines.join("\n")), &event, business_context, AT));

    assert_no_wrapper_leak(&message, &lines);
  }

  /// 1つの JSON の行が複数の形に当たるときは、人の発言 → 出来事 → 回答 → director の順で決まる。
  #[test]
  fn overlapping_shapes_follow_the_precedence(
    case in 0usize..3,
    text in any::<String>().prop_filter("空白だけではない", |text| !text.trim().is_empty()),
    kind in any::<String>().prop_filter("空でない", |kind| !kind.is_empty()),
  ) {
    let (input, speaker, expected) = match case {
      | 0 => (
        json!({ "speaker": { "id": "operator" }, "text": text, "event": { "kind": kind } }),
        ConversationSpeaker::User,
        text.clone(),
      ),
      | 1 => (
        json!({ "event": { "kind": kind }, "kind": "inquiry_answered", "answer": { "text": text } }),
        ConversationSpeaker::System,
        "説明文".to_owned(),
      ),
      | _ => (
        json!({ "kind": "inquiry_answered", "answer": { "text": text }, "origin": "director", "text": "依頼" }),
        ConversationSpeaker::User,
        expected_answer_body(Some(&text), None),
      ),
    };

    let message = read_single(&turn_data(Some("説明文"), &input, true, true, AT));

    prop_assert_eq!(message.speaker, speaker);
    prop_assert_eq!(&message.text, &expected);
  }

  /// 文字化けの印は、表示する本文（回答、出来事の説明文）の置換文字の数で決まる。
  /// 折りたたみの中身（質問、イベントの JSON）の置換文字は数えない。
  #[test]
  fn garbled_notice_follows_the_displayed_text(
    replacements in 0usize..20,
    hidden_replacements in 0usize..20,
    event_turn in any::<bool>(),
  ) {
    let shown = "\u{FFFD}".repeat(replacements);
    let hidden = "\u{FFFD}".repeat(hidden_replacements);
    let data = if event_turn {
      event_turn_data(Some(&format!("説明文{shown}")), &json!({ "kind": "task_failed", "reason": hidden }), false, AT)
    } else {
      inquiry_answered_data(&format!("質問{hidden}"), &format!("回答{shown}"), false, AT)
    };

    let message = read_single(&data);

    prop_assert_eq!(message.notice.is_some(), replacements >= 10);
  }
}
