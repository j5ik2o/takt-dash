//! GetConversation の gRPC 経路（実ソケットのテストサーバと生成クライアント）を検証する。
//!
//! 一時ホームの manager の記録と opencode の DB を読み、proto のメッセージ型へ写した
//! 応答、増分取得、要求の検証、保存先の不在の扱いを確認する。

mod common;
mod conversation_fixture;
mod grpc_harness;

use std::path::Path;

use common::{project, server_meta};
use conversation_fixture::{
  director_turn_data, event_turn_data, fenced_assistant_data, garbled_user_data, inquiry_answered_data,
  wrapped_user_data, ConversationFixture,
};
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

fn notices(response: &GetConversationResponse) -> Vec<Option<String>> {
  response.messages.iter().map(|message| message.notice.clone()).collect()
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

#[tokio::test]
async fn get_conversation_returns_presented_bodies_and_a_garbled_notice() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  let project_cwd = harness.project_dir();
  fixture.write_manager("alpha", "ses_alpha", project_cwd, "opencode");
  fixture.insert_session("ses_alpha", project_cwd, 2_000);
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &wrapped_user_data("前のターンの本文", "見出しを直してください。", FIRST_MESSAGE_AT),
  );
  fixture.insert_message(
    "ses_alpha",
    4,
    "assistant",
    1_794_613_222_000,
    &fenced_assistant_data("確認しました。", Some("README を追加する"), 1_794_613_222_000),
  );
  fixture.insert_message("ses_alpha", 7, "user", 1_794_613_223_000, &garbled_user_data(12, 1_794_613_223_000));
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client.get_conversation(request(None, 0, 50)).await.expect("会話を取得できます").into_inner();

  let bodies = texts(&response);
  assert_eq!(bodies[0], "見出しを直してください。", "包み付きの user の行は人の発言だけを返す必要があります");
  assert_eq!(bodies[1], "確認しました。", "fenced JSON の assistant の行は message だけを返す必要があります");
  assert_eq!(bodies[2].matches('\u{FFFD}').count(), 12, "本文の置換文字はそのまま残す必要があります");
  assert_eq!(notices(&response), vec![None, None, Some("元記録で文字化けしています（置換文字 12 個）".to_owned()),]);

  server.stop().await;
}

#[tokio::test]
async fn get_conversation_returns_the_folded_detail_of_non_speech_turns() {
  let harness = harness();
  let fixture = ConversationFixture::under(harness.home());
  let project_cwd = harness.project_dir();
  fixture.write_manager("alpha", "ses_alpha", project_cwd, "opencode");
  fixture.insert_session("ses_alpha", project_cwd, 2_000);
  let event = json!({ "kind": "wait_command_completed", "id": "35cb820a", "status": "succeeded" });
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &event_turn_data(None, &event, false, FIRST_MESSAGE_AT),
  );
  fixture.insert_message(
    "ses_alpha",
    2,
    "user",
    1_794_613_222_000,
    &inquiry_answered_data("どう進めますか。", "案A で進めてください。", true, 1_794_613_222_000),
  );
  fixture.insert_message(
    "ses_alpha",
    3,
    "user",
    1_794_613_223_000,
    &wrapped_user_data("前のターンの本文", "見出しを直してください。", 1_794_613_223_000),
  );
  fixture.insert_message(
    "ses_alpha",
    4,
    "user",
    1_794_613_224_000,
    &director_turn_data("director の依頼です。", false, 1_794_613_224_000),
  );
  let server = start_server(&harness).await;
  let mut client = client(server.address).await;

  let response = client.get_conversation(request(None, 0, 50)).await.expect("会話を取得できます").into_inner();

  let speakers: Vec<i32> = response.messages.iter().map(|message| message.speaker).collect();
  assert_eq!(speakers, vec![
    ConversationSpeaker::System as i32,
    ConversationSpeaker::User as i32,
    ConversationSpeaker::User as i32,
    ConversationSpeaker::Director as i32
  ]);
  assert_eq!(texts(&response), vec![
    "イベント: wait_command_completed",
    "案A で進めてください。",
    "見出しを直してください。",
    "director の依頼です。"
  ]);
  let event_detail = response.messages[0].detail.as_ref().expect("イベントの折りたたみがあります");
  assert_eq!(event_detail.question, None);
  assert_eq!(serde_json::from_str::<serde_json::Value>(&event_detail.source_json).expect("JSON です"), event);
  let answer_detail = response.messages[1].detail.as_ref().expect("回答の折りたたみがあります");
  assert_eq!(answer_detail.question.as_deref(), Some("どう進めますか。"));
  assert!(answer_detail.source_json.contains("inquiry_answered"), "元の JSON を返す必要があります");
  assert_eq!(response.messages[2].detail, None, "人の発言に折りたたみはありません");

  server.stop().await;
}
