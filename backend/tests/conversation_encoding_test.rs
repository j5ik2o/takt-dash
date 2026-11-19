//! 抽出後の本文で文字化けを判定する。

mod conversation_fixture;

use conversation_fixture::{current_manager_user_wrapper, manager_user_wrapper, ConversationFixture};
use serde_json::json;
use takt_dash_backend::{conversation::ConversationQuery, domain::ProjectName};

#[test]
fn encoding_marker_uses_the_display_body_and_preserves_replacement_characters() {
  let (_directory, fixture) = ConversationFixture::new();
  let cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("encoding", "ses_encoding", &cwd, "opencode");
  fixture.insert_session("ses_encoding", &cwd, 2_000);
  let cases = [
    (
      "user",
      json!({"text": format!("{}\n\nBusiness context (not approval): ���",
        current_manager_user_wrapper(&json!({"speaker":{"namespace":"slack","id":"fixture-user"},"text":"正常な入力"}).to_string()))}),
      "正常な入力".to_owned(),
      false,
    ),
    (
      "user",
      json!({"text": current_manager_user_wrapper(&json!({"speaker":{"namespace":"slack","id":"fixture-user"},"text":"読める部分���"}).to_string())}),
      "読める部分���".to_owned(),
      true,
    ),
    ("user", json!({"text": "正常な再送"}), "正常な再送".to_owned(), false),
    ("user", json!({"text": "読める部分�と�"}), "読める部分�と�".to_owned(), false),
    ("user", json!({"text": "読める部分�と�と�"}), "読める部分�と�と�".to_owned(), true),
    ("user", json!({"text": format!("読める部分{}", "�".repeat(87))}), format!("読める部分{}", "�".repeat(87)), true),
    (
      "user",
      json!({"text": manager_user_wrapper("{\"speaker\":\"user\",\"text\":\"正常な入力\"}").replace("内部メモ", "���")}),
      "正常な入力".to_owned(),
      false,
    ),
    (
      "assistant",
      json!({"content": [{"type": "text", "text": "```json\n{\"message\":\"正常な応答\",\"summary\":\"���\"}\n```"}]}),
      "正常な応答".to_owned(),
      false,
    ),
    (
      "assistant",
      json!({"content": [{"type": "text", "text": "```json\n{\"message\":\"読める応答���\",\"summary\":\"正常\"}\n```"}]}),
      "読める応答���".to_owned(),
      true,
    ),
  ];
  for (index, (kind, data, _, _)) in cases.iter().enumerate() {
    fixture.insert_message("ses_encoding", i64::try_from(index).unwrap() + 1, kind, 2_000, &data.to_string());
  }
  let page = fixture.reader().read(&ConversationQuery {
    project:       ProjectName::try_from("takt-dash").unwrap(),
    cwd:           cwd.clone(),
    project_roots: vec![cwd],
    session_id:    None,
    after_seq:     0,
    limit:         50,
  });
  assert_eq!(page.messages.len(), cases.len());
  for (message, (_, _, text, marked)) in page.messages.iter().zip(&cases) {
    assert_eq!(&message.text, text, "seq {} の置換文字を含む本文を保持する", message.seq);
    assert_eq!(message.has_encoding_error, *marked, "seq {} の本文だけから判定する", message.seq);
  }
}
