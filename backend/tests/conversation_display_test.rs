//! 表示用の本文抽出と、保存済みの会話の保持を実 DB で検証する。

mod conversation_fixture;

use conversation_fixture::{current_manager_user_wrapper, manager_user_wrapper, ConversationFixture};
use serde_json::json;
use takt_dash_backend::{conversation::ConversationQuery, domain::ProjectName};

fn read_bodies(kind: &str, bodies: &[String]) -> Vec<String> {
  let (_directory, fixture) = ConversationFixture::new();
  let cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("display", "ses_display", &cwd, "opencode");
  fixture.insert_session("ses_display", &cwd, 2_000);
  for (index, body) in bodies.iter().enumerate() {
    let data =
      if kind == "assistant" { json!({"content": [{"type": "text", "text": body}]}) } else { json!({"text": body}) };
    fixture.insert_message("ses_display", i64::try_from(index).unwrap() + 1, kind, 2_000, &data.to_string());
  }
  let page = fixture.reader().read(&ConversationQuery {
    project:       ProjectName::try_from("takt-dash").unwrap(),
    cwd:           cwd.clone(),
    project_roots: vec![cwd],
    session_id:    None,
    after_seq:     0,
    limit:         50,
  });
  assert_eq!(page.unavailable_reason, None);
  assert!(page.skipped.is_empty(), "有効な記録の本文を抽出できなくても発言を捨てない");
  page.messages.into_iter().map(|message| message.text).collect()
}

fn user_wrapper(text: &str) -> String {
  manager_user_wrapper(&json!({"speaker": "user", "text": text, "context": {"text": "入れ子の内部メモ"}}).to_string())
}

#[test]
fn current_user_wrapper_extracts_human_text_with_or_without_business_context() {
  let text = "今回の依頼\n引用: {\"text\":\"本文の引用\"}";
  let input = json!({"speaker": {"namespace": "slack", "id": "fixture-user"}, "text": text,
    "context": {"text": "入れ子の内部メモ"}})
  .to_string();
  let wrapper = current_manager_user_wrapper(&input);
  let bodies = [
    wrapper.clone(),
    format!("{wrapper}\n\nBusiness context (not approval): 内部メモ"),
    format!("## 前のターンで受け取った人の発言\n前回の依頼\n{wrapper}"),
  ];
  assert_eq!(read_bodies("user", &bodies), vec![text; 3]);
}

#[test]
fn current_user_wrapper_preserves_events_quotes_and_unrecognizable_input() {
  let input = json!({"speaker": {"namespace": "slack", "id": "fixture-user"}, "text": "今回"}).to_string();
  let originals = [
    format!("通常の説明\n```json\n{input}\n```"),
    format!("## このターンの入力\nイベントから始まったターンです。\n{input}"),
    format!("## このターンの入力\n{input}"),
    format!("```markdown\n{}\n```", current_manager_user_wrapper(&input)),
    current_manager_user_wrapper("{broken"),
    current_manager_user_wrapper("{\"speaker\":\"user\",\"text\":\"判別不能\"}"),
    current_manager_user_wrapper("{\"speaker\":{},\"text\":\"判別不能\"}"),
    current_manager_user_wrapper("{\"speaker\":{\"namespace\":\"slack\",\"id\":42},\"text\":\"判別不能\"}"),
    current_manager_user_wrapper("{\"speaker\":{\"namespace\":\"slack\",\"id\":\"fixture-user\"},\"text\":42}"),
    format!("{}\n説明が続きます。", current_manager_user_wrapper(&input)),
    format!("{}\n{{\"text\":\"別の値\"}}", current_manager_user_wrapper(&input)),
  ];
  assert_eq!(read_bodies("user", &originals), originals);
}

#[test]
fn user_wrapper_returns_only_the_current_human_text() {
  let text = "公開してください。\n引用: {\"text\":\"この引用も本文です\"}";
  assert_eq!(read_bodies("user", &[user_wrapper(text)]), vec![text]);
}

#[test]
fn user_without_wrapper_preserves_prose_and_quoted_json() {
  let original = "次の例を確認してください\n```json\n{\"speaker\":\"user\",\"text\":\"公開してください\"}\n```\n";
  assert_eq!(read_bodies("user", &[original.to_owned()]), vec![original]);
}

#[test]
fn user_without_wrapper_preserves_current_input_heading_and_quoted_json() {
  let original =
    "次の例を確認してください\n## このターンの入力\n```json\n{\"speaker\":\"user\",\"text\":\"公開してください\"}\n```";
  let input = "```json\n{\"speaker\":\"user\",\"text\":\"公開してください\"}\n```";
  assert_eq!(read_bodies("user", &[manager_user_wrapper(input), original.to_owned()]), vec![
    "公開してください",
    original
  ]);
}

#[test]
fn user_with_unextractable_current_input_preserves_the_whole_wrapper() {
  let originals =
    ["{broken", "{\"speaker\":\"assistant\",\"text\":\"人の入力ではない\"}", "{\"speaker\":\"user\",\"text\":42}"]
      .map(manager_user_wrapper);
  assert_eq!(read_bodies("user", &originals), originals);
  assert_eq!(read_bodies("user", &[manager_user_wrapper("{\"speaker\":\"user\",\"text\":\"42\"}")]), vec!["42"]);
}

#[test]
fn assistant_fenced_json_returns_message_instead_of_summary() {
  let text = "公開しました。\n次の確認をお願いします。";
  let body = format!("```json\n{}\n```", json!({"message": text, "summary": "内部要約"}));
  assert_eq!(read_bodies("assistant", &[body]), vec![text]);
}

#[test]
fn assistant_preserves_prose_examples_and_unclosed_fences() {
  let originals =
    ["応答例です\n```json\n{\"message\":\"公開しました\"}\n```", "```json\n{\"message\":\"公開しました\"}"]
      .map(str::to_owned);
  assert_eq!(read_bodies("assistant", &originals), originals);
}

#[test]
fn assistant_preserves_invalid_json_and_non_string_messages() {
  let originals = [
    "```json\n{broken\n```",
    "```json\n{\"summary\":\"要約だけ\"}\n```",
    "```json\n{\"message\":42}\n```",
    "```typescript\n{\"message\":\"コード例\"}\n```",
    "```json\n{\"message\":\"例\"}\n```\n説明が続きます。",
  ]
  .map(str::to_owned);
  assert_eq!(read_bodies("assistant", &originals), originals);
}

#[test]
fn assistant_extracts_after_joining_text_elements_and_ignores_tools() {
  let (_directory, fixture) = ConversationFixture::new();
  let cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("display", "ses_display", &cwd, "opencode");
  fixture.insert_session("ses_display", &cwd, 2_000);
  fixture.insert_message(
    "ses_display",
    23,
    "assistant",
    2_000,
    &json!({"content": [
      {"type": "text", "text": "```json"},
      {"type": "tool", "text": "内部ツール出力"},
      {"type": "text", "text": "{\"message\":\"公開しました\",\"summary\":\"内部要約\"}"},
      {"type": "text", "text": "```"}
    ]})
    .to_string(),
  );
  let page = fixture.reader().read(&ConversationQuery {
    project:       ProjectName::try_from("takt-dash").unwrap(),
    cwd:           cwd.clone(),
    project_roots: vec![cwd],
    session_id:    None,
    after_seq:     0,
    limit:         50,
  });
  assert_eq!(page.messages.iter().map(|message| message.text.as_str()).collect::<Vec<_>>(), vec!["公開しました"]);
}

#[test]
fn reading_display_messages_does_not_rewrite_stored_data() {
  let (_directory, fixture) = ConversationFixture::new();
  let cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("display", "ses_display", &cwd, "opencode");
  fixture.insert_session("ses_display", &cwd, 2_000);
  let originals = [
    json!({"text": user_wrapper(&format!("読める部分{}", "�".repeat(87)))}).to_string(),
    json!({"content": [{"type": "text", "text": "```json\n{\"message\":\"公開しました\",\"summary\":\"内部要約\"}\n```"}]}).to_string(),
    json!({"text": current_manager_user_wrapper(&json!({"speaker":{"namespace":"slack","id":"fixture-user"},"text":"現在の入力���"}).to_string())}).to_string(),
  ];
  for (index, data) in originals.iter().enumerate() {
    fixture.insert_message(
      "ses_display",
      i64::try_from(index).unwrap() + 1,
      if index == 1 { "assistant" } else { "user" },
      2_000,
      data,
    );
  }
  let page = fixture.reader().read(&ConversationQuery {
    project:       ProjectName::try_from("takt-dash").unwrap(),
    cwd:           cwd.clone(),
    project_roots: vec![cwd],
    session_id:    None,
    after_seq:     0,
    limit:         50,
  });
  assert_eq!(page.messages.len(), 3);
  assert_eq!(
    page.messages.iter().map(|message| message.notice.as_deref()).collect::<Vec<_>>(),
    vec![
      Some("元記録で文字化けしています（置換文字 87 個）"),
      None,
      Some("元記録で文字化けしています（置換文字 3 個）")
    ],
    "表示する本文の置換文字から注意書きを決め、保存データは変えない"
  );
  let connection = rusqlite::Connection::open(&fixture.database).unwrap();
  let mut statement = connection.prepare("SELECT data FROM session_message ORDER BY seq").unwrap();
  let stored = statement.query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
  assert_eq!(stored, originals);
}
