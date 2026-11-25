//! 「## このターンの入力」の包みから本文と折りたたみの中身を取り出す規則を、
//! 生成した入力で確かめる。
//!
//! 包みの組み立ては `conversation_fixture` の実データと同じ並び（見出し、空行、説明文、JSON の行、
//! Business context、応答形式の指示）を使い、説明文・イベント・質問・回答・
//! 前のターンの本文を生成する。

mod conversation_fixture;

use std::path::Path;

use conversation_fixture::{
  data_text, event_turn_data, inquiry_answered_data, inquiry_answered_with_answer_data, plain_user_data,
  wrapped_user_data, ConversationFixture,
};
use proptest::{collection::vec, prelude::*};
use serde_json::{Map, Value};
use takt_dash_backend::{
  conversation::{ConversationMessage, ConversationQuery, ConversationSpeaker},
  domain::ProjectName,
};

const AT: i64 = 1_794_613_221_331;
const MARKER: &str = "## このターンの入力";

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
  serde_json::to_string(&serde_json::json!({ "time": { "created": AT }, "text": text })).expect("data を組み立てられる")
}

fn is_json_shaped(line: &str) -> bool {
  let trimmed = line.trim();
  trimmed.starts_with('{') && trimmed.ends_with('}')
}

fn description_line() -> impl Strategy<Value = String> {
  "[^\n]{0,40}"
}

fn event_payload() -> impl Strategy<Value = (String, Value)> {
  (
    any::<String>().prop_filter("種類は空でない", |kind| !kind.is_empty()),
    vec((any::<String>(), any::<String>()), 0..4),
  )
    .prop_map(|(kind, fields)| {
      let mut event = Map::new();
      for (key, value) in fields {
        event.insert(key, Value::String(value));
      }
      event.insert("kind".to_owned(), Value::String(kind.clone()));
      (kind, Value::Object(event))
    })
}

fn assert_no_wrapper_leak(message: &ConversationMessage) {
  let detail = message.detail.as_ref().map(|detail| detail.source_json.as_str()).unwrap_or("");
  let question = message.detail.as_ref().and_then(|detail| detail.question.as_deref()).unwrap_or("");
  for shown in [message.text.as_str(), detail, question] {
    assert!(!shown.contains("fenced JSON block をちょうど1つ"), "応答形式の指示を出さない: {shown:?}");
    assert!(!shown.contains("Business context (not approval)"), "Business context を出さない: {shown:?}");
    assert!(!shown.contains(MARKER), "見出しを出さない: {shown:?}");
  }
}

proptest! {
  #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

  /// 出来事から始まったターンは、話者がシステムで、本文は説明文（空なら「イベント: 種類」）、
  /// 折りたたみはイベントの JSON と同じ値になる。前のターンの本文に見出しがあっても変わらない。
  #[test]
  fn event_turn_shows_the_description_and_folds_the_event(
    (kind, event) in event_payload(),
    lines in vec(description_line(), 0..4),
    business_context in any::<bool>(),
    previous in any::<String>(),
  ) {
    prop_assume!(lines.iter().all(|line| !is_json_shaped(line) && !line.starts_with(MARKER)));
    let description: Vec<&str> = lines.iter().map(|line| line.trim()).filter(|line| !line.is_empty()).collect();
    let data = with_previous_turn(
      &event_turn_data(Some(&lines.join("\n")), &event, business_context, AT),
      &previous,
    );

    let message = read_single(&data);

    prop_assert_eq!(message.speaker, ConversationSpeaker::System);
    let expected = if description.is_empty() { format!("イベント: {kind}") } else { description.join("\n") };
    prop_assert_eq!(&message.text, &expected);
    let detail = message.detail.as_ref().expect("折りたたみがある");
    prop_assert_eq!(detail.question.as_deref(), None);
    prop_assert_eq!(serde_json::from_str::<Value>(&detail.source_json).expect("JSON"), event);
  }

  /// 質問への回答は、話者がユーザーで、本文は回答の本文そのもの、折りたたみは質問の文
  /// （空なら無し）と元の JSON になる。
  #[test]
  fn inquiry_answer_shows_the_answer_and_folds_the_question(
    question in any::<String>(),
    answer in any::<String>().prop_filter("回答は空でない", |answer| !answer.is_empty()),
    business_context in any::<bool>(),
    previous in any::<String>(),
  ) {
    let data = with_previous_turn(&inquiry_answered_data(&question, &answer, business_context, AT), &previous);

    let message = read_single(&data);

    prop_assert_eq!(message.speaker, ConversationSpeaker::User);
    prop_assert_eq!(&message.text, &answer);
    let detail = message.detail.as_ref().expect("折りたたみがある");
    prop_assert_eq!(detail.question.as_deref(), (!question.is_empty()).then_some(question.as_str()));
    let source: Value = serde_json::from_str(&detail.source_json).expect("JSON");
    prop_assert_eq!(&source["kind"], "inquiry_answered");
    prop_assert_eq!(&source["answer"]["text"], answer.as_str());
    prop_assert_eq!(&source["question"], question.as_str());
  }

  /// text の無い回答は、報告をボタンと同じ言葉（知らない値なら「回答: 値」）にして本文にする。
  /// text があれば、報告があっても text を本文にする。
  #[test]
  fn inquiry_answer_falls_back_to_the_report(
    text in proptest::option::of(any::<String>()),
    report in prop_oneof![Just("completed".to_owned()), Just("unable".to_owned()), any::<String>()]
      .prop_filter("報告は空でない", |report| !report.is_empty()),
  ) {
    let mut answer = serde_json::json!({ "report": report });
    if let Some(text) = &text {
      answer["text"] = Value::String(text.clone());
    }
    let message = read_single(&inquiry_answered_with_answer_data("質問", answer.clone(), false, AT));

    let expected = match (text.as_deref().filter(|text| !text.is_empty()), report.as_str()) {
      | (Some(text), _) => text.to_owned(),
      | (None, "completed") => "実施した".to_owned(),
      | (None, "unable") => "対応できない".to_owned(),
      | (None, other) => format!("回答: {other}"),
    };
    prop_assert_eq!(message.speaker, ConversationSpeaker::User);
    prop_assert_eq!(&message.text, &expected);
    let source: Value = serde_json::from_str(&message.detail.as_ref().expect("折りたたみ").source_json).expect("JSON");
    prop_assert_eq!(&source["answer"], &answer);
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

  /// 上の性質で外した説明文（JSON の形の行、見出しで始まる行）を含めても、
  /// 応答形式の指示・Business context・見出しは本文にも折りたたみにも出ない。
  #[test]
  fn wrapper_never_leaks_for_any_description(
    (_kind, event) in event_payload(),
    lines in vec(prop_oneof![
      description_line(),
      Just(MARKER.to_owned()),
      Just("{}".to_owned()),
      Just(r#"{"speaker":{"id":"x"},"text":"説明文に紛れた発言"}"#.to_owned()),
      "\\{[^\n]{0,20}\\}",
    ], 0..4),
    business_context in any::<bool>(),
  ) {
    let message = read_single(&event_turn_data(Some(&lines.join("\n")), &event, business_context, AT));

    assert_no_wrapper_leak(&message);
  }

  /// 文字化けの印は、表示する本文の置換文字の数で決まる。
  #[test]
  fn garbled_notice_follows_the_displayed_text(
    replacements in 0usize..20,
    question_replacements in 0usize..20,
  ) {
    let answer = format!("回答{}", "\u{FFFD}".repeat(replacements));
    let question = format!("質問{}", "\u{FFFD}".repeat(question_replacements));

    let message = read_single(&inquiry_answered_data(&question, &answer, false, AT));

    prop_assert_eq!(message.notice.is_some(), replacements >= 10);
  }
}
