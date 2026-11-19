//! GetConversation の gRPC 経路（実ソケットのテストサーバと生成クライアント）を検証する。
//!
//! 一時ホームの manager の記録と opencode の DB を読み、proto のメッセージ型へ写した
//! 応答、増分取得、要求の検証、保存先の不在の扱いを確認する。

mod common;
mod conversation_fixture;
mod grpc_harness;

use std::path::Path;

use common::{project, server_meta};
use conversation_fixture::{current_manager_user_wrapper, manager_user_wrapper, ConversationFixture};
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::json;
use takt_dash_backend::takt_dash::v1::{ConversationSpeaker, GetConversationRequest, GetConversationResponse};
use tonic::Code;

const FIRST_MESSAGE_AT: i64 = 1_794_613_221_331;

fn harness() -> GrpcHarness {
  GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .build()
}

fn request(session_id: Option<&str>, after_seq: i64, limit: i32) -> GetConversationRequest {
  GetConversationRequest {
    project: "takt-dash".to_owned(),
    session_id: session_id.map(str::to_owned),
    after_seq,
    limit,
  }
}

fn seed_messages(fixture: &ConversationFixture, project_cwd: &Path) {
  fixture.write_manager("alpha", "ses_alpha", project_cwd, "opencode");
  fixture.insert_session("ses_alpha", project_cwd, 2_000);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, r#"{"text":"最初の発言","time":1794613221331}"#);
  fixture.insert_message(
    "ses_alpha",
    4,
    "assistant",
    1_794_613_222_000,
    r#"{"content":[{"type":"text","text":"実装を始めます"}],"time":1794613222000}"#,
  );
  fixture.insert_message(
    "ses_alpha",
    7,
    "system",
    1_794_613_223_000,
    r#"{"text":"設定を読み込みました","time":1794613223000}"#,
  );
}

fn texts(response: &GetConversationResponse) -> Vec<String> {
  response.messages.iter().map(|message| message.text.clone()).collect()
}

#[tokio::test]
async fn get_conversation_extracts_current_human_input_on_initial_and_incremental_requests() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  fixture.write_manager("current", "ses_current", harness.project_dir(), "opencode");
  fixture.insert_session("ses_current", harness.project_dir(), 2_000);
  let body = current_manager_user_wrapper(
    &json!({"speaker":{"namespace":"slack","id":"fixture-user"},"text":"今回の依頼"}).to_string(),
  );
  for (seq, text) in [(28, body.clone()), (29, format!("{body}\n\nBusiness context (not approval): 内部メモ���"))]
  {
    fixture.insert_message("ses_current", seq, "user", FIRST_MESSAGE_AT, &json!({"text":text}).to_string());
  }
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let initial = client.get_conversation(request(None, 0, 50)).await.unwrap().into_inner();
  let incremental = client.get_conversation(request(Some("ses_current"), 28, 1)).await.unwrap().into_inner();
  server.stop().await;
  assert_eq!(texts(&initial), vec!["今回の依頼"; 2]);
  assert_eq!(texts(&incremental), vec!["今回の依頼"]);
  assert_eq!(incremental.messages[0].seq, 29);
  assert!(initial.messages.iter().all(|message| !message.has_encoding_error));
  assert!(!incremental.messages[0].has_encoding_error);
  assert!(initial.skipped.is_empty());
  assert!(incremental.skipped.is_empty());
}

#[tokio::test]
async fn get_conversation_preserves_quoted_current_input_and_extracts_manager_wrappers() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  fixture.write_manager("display", "ses_display", harness.project_dir(), "opencode");
  fixture.insert_session("ses_display", harness.project_dir(), 2_000);
  let input = "```json\n{\"speaker\":\"user\",\"text\":\"公開してください\"}\n```";
  let original = format!("次の例を確認してください\n## このターンの入力\n{input}");
  for (seq, body) in [(22, manager_user_wrapper(input)), (27, original.clone())] {
    fixture.insert_message("ses_display", seq, "user", FIRST_MESSAGE_AT, &json!({"text": body}).to_string());
  }
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let initial = client.get_conversation(request(None, 0, 50)).await.unwrap().into_inner();
  let incremental = client.get_conversation(request(Some("ses_display"), 22, 1)).await.unwrap().into_inner();
  server.stop().await;
  assert_eq!(texts(&initial), vec!["公開してください", original.as_str()]);
  assert_eq!(initial.messages.iter().map(|message| message.seq).collect::<Vec<_>>(), vec![22, 27]);
  assert!(initial.skipped.is_empty());
  assert_eq!(texts(&incremental), vec![original]);
  assert_eq!(incremental.messages[0].seq, 27);
  assert!(incremental.skipped.is_empty());
}

#[tokio::test]
async fn get_conversation_transfers_encoding_markers_with_their_messages() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  fixture.write_manager("encoding", "ses_encoding", harness.project_dir(), "opencode");
  fixture.insert_session("ses_encoding", harness.project_dir(), 2_000);
  let damaged = format!("読める部分{}", "�".repeat(87));
  fixture.insert_message("ses_encoding", 8, "user", FIRST_MESSAGE_AT, &json!({"text": damaged}).to_string());
  fixture.insert_message("ses_encoding", 22, "user", FIRST_MESSAGE_AT, &json!({"text": "正常な再送"}).to_string());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;
  let response = client.get_conversation(request(None, 0, 50)).await.expect("会話を取得できます").into_inner();
  server.stop().await;

  assert_eq!(
    response
      .messages
      .iter()
      .map(|message| (message.seq, message.text.as_str(), message.has_encoding_error))
      .collect::<Vec<_>>(),
    vec![(8, damaged.as_str(), true), (22, "正常な再送", false),]
  );
}

#[tokio::test]
async fn get_conversation_returns_messages_with_speakers_and_times() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  seed_messages(&fixture, harness.project_dir());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client.get_conversation(request(None, 0, 50)).await.expect("会話を取得できます").into_inner();

  assert_eq!(response.project, "takt-dash");
  assert_eq!(response.session_id, "ses_alpha");
  assert_eq!(response.provider, "opencode");
  assert_eq!(texts(&response), vec!["最初の発言", "実装を始めます", "設定を読み込みました"]);
  assert_eq!(response.messages.iter().map(|message| message.speaker).collect::<Vec<_>>(), vec![
    ConversationSpeaker::User as i32,
    ConversationSpeaker::Manager as i32,
    ConversationSpeaker::System as i32,
  ]);
  assert_eq!(
    response.messages[0].occurred_at,
    Some(prost_types::Timestamp { seconds: 1_794_613_221, nanos: 331_000_000 })
  );
  assert_eq!(response.next_after_seq, 7);
  assert!(!response.has_more);
  assert!(response.skipped.is_empty());
  assert_eq!(response.unavailable_reason, None);

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_pages_with_after_seq() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  seed_messages(&fixture, harness.project_dir());
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client.get_conversation(request(None, 1, 1)).await.expect("会話を取得できます").into_inner();

  assert_eq!(texts(&response), vec!["実装を始めます"]);
  assert_eq!(response.next_after_seq, 4);
  assert!(response.has_more);

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_rejects_an_empty_project() {
  let harness = harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_conversation(GetConversationRequest { project: String::new(), ..request(None, 0, 50) })
    .await
    .expect_err("空の project は拒否する必要があります");

  assert_eq!(status.code(), Code::InvalidArgument);

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_rejects_an_unknown_project() {
  let harness = harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let status = client
    .get_conversation(GetConversationRequest { project: "unknown".to_owned(), ..request(None, 0, 50) })
    .await
    .expect_err("設定に無い project は拒否する必要があります");

  assert_eq!(status.code(), Code::NotFound);

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_returns_a_reason_instead_of_an_error_when_storage_is_missing() {
  let harness = harness();
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response =
    client.get_conversation(request(None, 0, 50)).await.expect("保存先が無くてもエラーにしません").into_inner();

  assert!(response.messages.is_empty());
  assert!(response.unavailable_reason.is_some());

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_returns_messages_from_an_external_worktree() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  let external = harness.home().join("external-worktrees").join("20261115-run");
  std::fs::create_dir_all(&external).expect("外部 worktree のディレクトリを作れます");
  let external = std::fs::canonicalize(&external).expect("外部 worktree の実パスを解決できます");
  fixture.write_manager("external", "ses_external", harness.project_dir(), "opencode");
  fixture.insert_session("ses_external", &external, 2_000);
  fixture.insert_message(
    "ses_external",
    1,
    "user",
    FIRST_MESSAGE_AT,
    r#"{"text":"外部 worktree の会話","time":1794613221331}"#,
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response =
    client.get_conversation(request(None, 0, 50)).await.expect("外部 worktree の会話を取得できます").into_inner();

  assert_eq!(texts(&response), vec!["外部 worktree の会話"]);
  assert_eq!(response.unavailable_reason, None);

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_does_not_return_a_session_from_another_project() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  let other_cwd = fixture.project_dir("corporate-site");
  seed_messages(&fixture, harness.project_dir());
  fixture.write_manager("other", "ses_other", &other_cwd, "opencode");
  fixture.insert_session("ses_other", &other_cwd, 3_000);
  fixture.insert_message(
    "ses_other",
    1,
    "user",
    FIRST_MESSAGE_AT,
    r#"{"text":"別プロジェクトの発言","time":1794613221331}"#,
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .get_conversation(request(Some("ses_other"), 0, 50))
    .await
    .expect("別プロジェクトのセッションでもエラーにしません")
    .into_inner();

  assert!(response.messages.is_empty());
  assert!(response.unavailable_reason.is_some());

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_does_not_return_a_session_owned_by_another_configured_project() {
  let harness = GrpcHarness::builder()
    .single_project(project("takt-dash", Path::new("/abs/takt-dash")), server_meta(), json!({}))
    .single_project(project("corporate-site", Path::new("/abs/corporate-site")), server_meta(), json!({}))
    .build();
  let fixture = ConversationFixture::under(harness.home());
  let requested_cwd = harness.project_cwd("takt-dash").to_path_buf();
  let other_cwd = harness.project_cwd("corporate-site").to_path_buf();
  fixture.write_manager("alpha", "ses_alpha", &requested_cwd, "opencode");
  fixture.insert_session("ses_alpha", &other_cwd, 2_000);
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    r#"{"text":"別プロジェクトの会話","time":1794613221331}"#,
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client
    .get_conversation(request(None, 0, 50))
    .await
    .expect("別プロジェクトのセッションでもエラーにしません")
    .into_inner();

  assert!(response.messages.is_empty());
  assert_eq!(
    response.unavailable_reason.as_deref(),
    Some("manager の記録のセッションが別のプロジェクトに属しています")
  );

  server.stop().await;
}
