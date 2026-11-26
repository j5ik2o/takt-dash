//! manager の会話の保存先を直接読む経路（時系列・増分取得・プロジェクト識別・
//! 形式の分類・保存先の不在）を、一時 DB と一時 conversations ディレクトリで検証する。
//!
//! テストは実ホームを参照せず、`ConversationFixture` が作る一時パスだけを使う。

mod conversation_fixture;

use std::path::Path;

use chrono::{DateTime, Utc};
use conversation_fixture::{
  assistant_text, data_text, director_turn_data, event_turn_data, fenced_assistant_data,
  fenced_assistant_without_message_data, garbled_user_data, inquiry_answered_data, inquiry_answered_with_answer_data,
  invalid_fenced_assistant_data, legacy_inquiry_answered_data, plain_user_data, turn_data, wrapped_user_data,
  wrapped_user_without_speaker_data, ConversationFixture, BUSINESS_CONTEXT, CONVERSATION_SCHEMA, RESPONSE_FORMAT,
};
use takt_dash_backend::{
  conversation::{ConversationPage, ConversationQuery, ConversationReader, ConversationSpeaker},
  domain::ProjectName,
};

const FIRST_MESSAGE_AT: i64 = 1_794_613_221_331;

fn project_name(value: &str) -> ProjectName {
  ProjectName::try_from(value).expect("テストのプロジェクト名は有効です")
}

fn query(project: &str, cwd: &Path) -> ConversationQuery {
  ConversationQuery {
    project:       project_name(project),
    cwd:           cwd.to_path_buf(),
    project_roots: vec![cwd.to_path_buf()],
    session_id:    None,
    after_seq:     0,
    limit:         50,
  }
}

fn texts(page: &ConversationPage) -> Vec<String> {
  page.messages.iter().map(|message| message.text.clone()).collect()
}

fn seqs(page: &ConversationPage) -> Vec<i64> {
  page.messages.iter().map(|message| message.seq).collect()
}

fn skipped_count(page: &ConversationPage, fragment: &str) -> u32 {
  page.skipped.iter().filter(|summary| summary.reason.contains(fragment)).map(|summary| summary.count).sum()
}

fn total_skipped(page: &ConversationPage) -> u32 {
  page.skipped.iter().map(|summary| summary.count).sum()
}

fn notices(page: &ConversationPage) -> Vec<Option<String>> {
  page.messages.iter().map(|message| message.notice.clone()).collect()
}

fn user_data(text: &str, time: i64) -> String {
  format!(r#"{{"text":"{text}","time":{time}}}"#)
}

fn assistant_text_data(text: &str, time: i64) -> String {
  format!(r#"{{"content":[{{"type":"text","text":"{text}"}}],"time":{time}}}"#)
}

fn seed_single_project(fixture: &ConversationFixture, project_cwd: &Path) -> ConversationReader {
  fixture.write_manager("alpha", "ses_alpha", project_cwd, "opencode");
  fixture.insert_session("ses_alpha", project_cwd, 2_000);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &user_data("最初の発言", FIRST_MESSAGE_AT));
  fixture.reader()
}

#[test]
fn reads_messages_in_seq_order_with_speakers_and_times() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    7,
    "system",
    1_794_613_200_000,
    &user_data("設定を読み込みました", 1_794_613_200_000),
  );
  fixture.insert_message(
    "ses_alpha",
    4,
    "assistant",
    1_794_613_222_000,
    &assistant_text_data("実装を始めます", 1_794_613_222_000),
  );

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(seqs(&page), vec![1, 4, 7], "seq の昇順に並べる必要があります");
  assert_eq!(page.messages.iter().map(|message| message.speaker).collect::<Vec<_>>(), vec![
    ConversationSpeaker::User,
    ConversationSpeaker::Manager,
    ConversationSpeaker::System
  ]);
  assert_eq!(page.messages[0].occurred_at, DateTime::<Utc>::from_timestamp_millis(FIRST_MESSAGE_AT).unwrap());
  assert_eq!(
    page.messages[2].occurred_at,
    DateTime::<Utc>::from_timestamp_millis(1_794_613_200_000).unwrap(),
    "並び順は time_created ではなく seq で決める必要があります"
  );
  assert_eq!(page.session_id, "ses_alpha");
  assert_eq!(page.provider, "opencode");
  assert_eq!(page.unavailable_reason, None);
}

#[test]
fn incremental_read_returns_only_rows_appended_after_the_cursor() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message("ses_alpha", 4, "user", 1_794_613_222_000, &user_data("2番目", 1_794_613_222_000));

  let first = reader.read(&query("takt-dash", &project_cwd));
  assert_eq!(seqs(&first), vec![1, 4]);
  assert_eq!(first.next_after_seq, 4);
  assert!(!first.has_more);

  fixture.insert_message(
    "ses_alpha",
    9,
    "assistant",
    1_794_613_229_000,
    &assistant_text_data("追記", 1_794_613_229_000),
  );
  let second = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    None,
    after_seq:     first.next_after_seq,
    limit:         50,
  });

  assert_eq!(texts(&second), vec!["追記"], "同じ読み取りで追記された行だけを返す必要があります");
  assert_eq!(second.next_after_seq, 9);
}

#[test]
fn limit_bounds_examined_rows_and_reports_more() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message("ses_alpha", 4, "user", 1_794_613_222_000, &user_data("2番目", 1_794_613_222_000));
  fixture.insert_message("ses_alpha", 7, "user", 1_794_613_223_000, &user_data("3番目", 1_794_613_223_000));

  let first = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    None,
    after_seq:     0,
    limit:         2,
  });
  assert_eq!(seqs(&first), vec![1, 4]);
  assert_eq!(first.next_after_seq, 4);
  assert!(first.has_more);

  let second = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    None,
    after_seq:     first.next_after_seq,
    limit:         2,
  });
  assert_eq!(seqs(&second), vec![7]);
  assert_eq!(second.next_after_seq, 7);
  assert!(!second.has_more);
}

#[test]
fn skipped_rows_advance_the_cursor() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    4,
    "assistant",
    1_794_613_222_000,
    r#"{"content":[{"type":"tool","name":"bash","state":{"text":"内部の出力"}}],"time":1794613222000}"#,
  );
  fixture.insert_message("ses_alpha", 7, "user", 1_794_613_223_000, &user_data("3番目", 1_794_613_223_000));

  let first = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    None,
    after_seq:     0,
    limit:         1,
  });
  assert_eq!(seqs(&first), vec![1]);
  assert_eq!(first.next_after_seq, 1);
  assert!(first.has_more);

  let second = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    None,
    after_seq:     first.next_after_seq,
    limit:         1,
  });
  assert!(second.messages.is_empty());
  assert_eq!(second.next_after_seq, 4, "スキップした行でもカーソルを進める必要があります");
  assert!(second.has_more, "スキップした行の後ろに未読の行が残っています");

  let third = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    None,
    after_seq:     second.next_after_seq,
    limit:         1,
  });
  assert_eq!(seqs(&third), vec![7]);
  assert_eq!(third.next_after_seq, 7);
  assert!(!third.has_more);
}

#[test]
fn assistant_text_element_is_the_conversation_body() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    4,
    "assistant",
    1_794_613_222_000,
    r#"{"content":[{"type":"tool","name":"bash","state":{"text":"内部の出力"}},{"type":"text","text":"実装を始めます"}],"time":1794613222000}"#,
  );

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["最初の発言", "実装を始めます"]);
  assert_eq!(page.messages[1].speaker, ConversationSpeaker::Manager);
  assert!(!texts(&page).iter().any(|text| text == "内部の出力"), "tool の state.text を会話本文に混ぜてはいけません");
}

#[test]
fn tool_only_assistant_is_skipped_with_reason_and_count() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    4,
    "assistant",
    1_794_613_222_000,
    r#"{"content":[{"type":"tool","name":"bash","state":{"text":"内部の出力"}}],"time":1794613222000}"#,
  );

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["最初の発言"], "本文を持たない assistant の行は会話に含めません");
  assert_eq!(skipped_count(&page, "本文が無い"), 1);
}

#[test]
fn system_text_is_returned_as_system_speaker() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    4,
    "system",
    1_794_613_222_000,
    r#"{"text":"設定を読み込みました","time":1794613222000}"#,
  );

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["最初の発言", "設定を読み込みました"]);
  assert_eq!(page.messages[1].speaker, ConversationSpeaker::System);
}

#[test]
fn synthetic_text_is_skipped_with_unsupported_type_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    4,
    "synthetic",
    1_794_613_222_000,
    r#"{"text":"内部イベント","time":1794613222000}"#,
  );

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["最初の発言"], "synthetic の text を会話本文に混ぜてはいけません");
  assert_eq!(skipped_count(&page, "synthetic"), 1);
}

#[test]
fn unsupported_message_type_is_skipped_with_its_type_in_the_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    4,
    "agent-switched",
    1_794_613_222_000,
    r#"{"agent":"builder","previous":"manager","time":1794613222000}"#,
  );

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["最初の発言"]);
  assert_eq!(skipped_count(&page, "agent-switched"), 1);
}

#[test]
fn malformed_data_is_skipped_without_stopping_the_next_rows() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = seed_single_project(&fixture, &project_cwd);
  fixture.insert_message("ses_alpha", 4, "assistant", 1_794_613_222_000, "{not json");
  fixture.insert_message("ses_alpha", 7, "user", 1_794_613_223_000, &user_data("3番目", 1_794_613_223_000));

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["最初の発言", "3番目"], "解釈できない行を飛ばしても後続の行は返す必要があります");
  assert_eq!(total_skipped(&page), 1);
}

#[test]
fn project_is_identified_by_assignment_and_session_directory() {
  let (_directory, fixture) = ConversationFixture::new();
  let first_cwd = fixture.project_dir("takt-dash");
  let second_cwd = fixture.project_dir("corporate-site");
  fixture.write_manager("first", "ses_first", &first_cwd, "opencode");
  fixture.insert_session("ses_first", &first_cwd, 1_000);
  fixture.insert_message("ses_first", 1, "user", FIRST_MESSAGE_AT, &user_data("最初のプロジェクト", FIRST_MESSAGE_AT));
  fixture.write_manager("second", "ses_second", &second_cwd, "opencode");
  fixture.insert_session("ses_second", &second_cwd, 2_000);
  fixture.insert_message(
    "ses_second",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &user_data("2番目のプロジェクト", FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let first = reader.read(&query("takt-dash", &first_cwd));
  let second = reader.read(&query("corporate-site", &second_cwd));

  assert_eq!(texts(&first), vec!["最初のプロジェクト"]);
  assert_eq!(texts(&second), vec!["2番目のプロジェクト"]);
}

#[test]
fn project_paths_sharing_a_prefix_are_not_matched() {
  let (_directory, fixture) = ConversationFixture::new();
  let short_cwd = fixture.project_dir("takt");
  let long_cwd = fixture.project_dir("takt-worktrees");
  fixture.write_manager("short", "ses_short", &short_cwd, "opencode");
  fixture.insert_session("ses_short", &short_cwd, 1_000);
  fixture.insert_message("ses_short", 1, "user", FIRST_MESSAGE_AT, &user_data("短い名前", FIRST_MESSAGE_AT));
  fixture.write_manager("long", "ses_long", &long_cwd, "opencode");
  fixture.insert_session("ses_long", &long_cwd, 2_000);
  fixture.insert_message("ses_long", 1, "user", FIRST_MESSAGE_AT, &user_data("長い名前", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt", &short_cwd));

  assert_eq!(texts(&page), vec!["短い名前"], "takt と takt-worktrees は別のプロジェクトです");
}

#[test]
fn worktree_directory_belongs_to_the_repository_project() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let worktree = project_cwd.join("worktrees").join("20261115-run");
  std::fs::create_dir_all(&worktree).expect("worktree のディレクトリを作れます");
  fixture.write_manager("worktree", "ses_worktree", &project_cwd, "opencode");
  fixture.insert_session("ses_worktree", &worktree, 1_000);
  fixture.insert_message("ses_worktree", 1, "user", FIRST_MESSAGE_AT, &user_data("worktree の会話", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["worktree の会話"]);
}

#[test]
fn external_worktree_directory_belongs_to_the_assigned_project() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let external = fixture.home.join("external-worktrees").join("20261115-run");
  std::fs::create_dir_all(&external).expect("外部 worktree のディレクトリを作れます");
  let external = std::fs::canonicalize(&external).expect("外部 worktree の実パスを解決できます");
  fixture.write_manager("external", "ses_external", &project_cwd, "opencode");
  fixture.insert_session("ses_external", &external, 1_000);
  fixture.insert_message(
    "ses_external",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &user_data("外部 worktree の会話", FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["外部 worktree の会話"]);
  assert_eq!(page.unavailable_reason, None);
}

#[test]
fn dotted_session_directory_is_normalized_to_its_real_path() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  std::fs::create_dir_all(project_cwd.join("nested")).expect("入れ子のディレクトリを作れます");
  let dotted = project_cwd.join("nested").join("..");
  fixture.write_manager("dotted", "ses_dotted", &project_cwd, "opencode");
  fixture.insert_session("ses_dotted", &dotted, 1_000);
  fixture.insert_message("ses_dotted", 1, "user", FIRST_MESSAGE_AT, &user_data("実パスの会話", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["実パスの会話"]);
}

#[test]
fn session_from_another_project_is_not_returned() {
  let (_directory, fixture) = ConversationFixture::new();
  let first_cwd = fixture.project_dir("takt-dash");
  let second_cwd = fixture.project_dir("corporate-site");
  fixture.write_manager("first", "ses_first", &first_cwd, "opencode");
  fixture.insert_session("ses_first", &first_cwd, 1_000);
  fixture.insert_message("ses_first", 1, "user", FIRST_MESSAGE_AT, &user_data("最初のプロジェクト", FIRST_MESSAGE_AT));
  fixture.write_manager("second", "ses_second", &second_cwd, "opencode");
  fixture.insert_session("ses_second", &second_cwd, 2_000);
  fixture.insert_message(
    "ses_second",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &user_data("2番目のプロジェクト", FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           first_cwd.clone(),
    project_roots: vec![first_cwd, second_cwd],
    session_id:    Some("ses_second".to_owned()),
    after_seq:     0,
    limit:         50,
  });

  assert!(page.messages.is_empty(), "別プロジェクトのセッションの本文を返してはいけません");
  assert!(page.unavailable_reason.is_some());
}

#[test]
fn session_directory_owned_by_another_configured_project_is_not_returned() {
  let (_directory, fixture) = ConversationFixture::new();
  let first_cwd = fixture.project_dir("takt-dash");
  let second_cwd = fixture.project_dir("corporate-site");
  fixture.write_manager("first", "ses_first", &first_cwd, "opencode");
  fixture.insert_session("ses_first", &second_cwd, 1_000);
  fixture.insert_message(
    "ses_first",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &user_data("別プロジェクトの実体", FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           first_cwd.clone(),
    project_roots: vec![first_cwd, second_cwd],
    session_id:    None,
    after_seq:     0,
    limit:         50,
  });

  assert!(page.messages.is_empty(), "別の設定プロジェクトに属するセッションの本文を返してはいけません");
  assert_eq!(page.unavailable_reason.as_deref(), Some("manager の記録のセッションが別のプロジェクトに属しています"));
}

#[test]
fn latest_manager_session_is_selected_when_session_id_is_omitted() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("old", "ses_old", &project_cwd, "opencode");
  fixture.set_manager_modified("old", 1_700_000_000);
  fixture.insert_session("ses_old", &project_cwd, 1_000);
  fixture.insert_message("ses_old", 1, "user", FIRST_MESSAGE_AT, &user_data("古いセッション", FIRST_MESSAGE_AT));
  fixture.write_manager("new", "ses_new", &project_cwd, "opencode");
  fixture.set_manager_modified("new", 1_700_000_000);
  fixture.insert_session("ses_new", &project_cwd, 2_000);
  fixture.insert_message("ses_new", 1, "user", FIRST_MESSAGE_AT, &user_data("新しいセッション", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let latest = reader.read(&query("takt-dash", &project_cwd));
  let explicit = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    Some("ses_old".to_owned()),
    after_seq:     0,
    limit:         50,
  });

  assert_eq!(
    latest.session_id, "ses_new",
    "更新時刻が同じでも、列挙順に依存せず session_id の昇順で最新を確定する必要があります"
  );
  assert_eq!(texts(&latest), vec!["新しいセッション"]);
  assert_eq!(texts(&explicit), vec!["古いセッション"]);
  assert_eq!(explicit.session_id, "ses_old");
}

#[test]
fn same_modification_time_and_provider_picks_latest_by_session_id_regardless_of_enumeration_order() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("first", "ses_a", &project_cwd, "opencode");
  fixture.set_manager_modified("first", 1_700_000_000);
  fixture.insert_session("ses_a", &project_cwd, 1_000);
  fixture.insert_message("ses_a", 1, "user", FIRST_MESSAGE_AT, &user_data("A のセッション", FIRST_MESSAGE_AT));
  fixture.write_manager("second", "ses_b", &project_cwd, "opencode");
  fixture.set_manager_modified("second", 1_700_000_000);
  fixture.insert_session("ses_b", &project_cwd, 2_000);
  fixture.insert_message("ses_b", 1, "user", FIRST_MESSAGE_AT, &user_data("B のセッション", FIRST_MESSAGE_AT));
  fixture.write_manager("third", "ses_c", &project_cwd, "opencode");
  fixture.set_manager_modified("third", 1_700_000_000);
  fixture.insert_session("ses_c", &project_cwd, 3_000);
  fixture.insert_message("ses_c", 1, "user", FIRST_MESSAGE_AT, &user_data("C のセッション", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let mut observed = std::collections::BTreeSet::new();
  for _ in 0..5 {
    let page = reader.read(&query("takt-dash", &project_cwd));
    assert_eq!(page.unavailable_reason, None);
    observed.insert(page.session_id);
  }

  assert_eq!(
    observed.into_iter().collect::<Vec<_>>(),
    vec!["ses_a".to_owned()],
    "同じ更新時刻・同じ provider の群では、列挙順に依存せず session_id の昇順の先頭を選ぶ必要があります"
  );
}

#[test]
fn old_opencode_record_does_not_become_latest_when_a_newer_unsupported_provider_record_exists() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("old", "ses_old", &project_cwd, "opencode");
  fixture.set_manager_modified("old", 1_700_000_000);
  fixture.insert_session("ses_old", &project_cwd, 1_000);
  fixture.insert_message("ses_old", 1, "user", FIRST_MESSAGE_AT, &user_data("古い opencode の会話", FIRST_MESSAGE_AT));
  fixture.write_manager("new", "ses_new", &project_cwd, "claude-sdk");
  fixture.set_manager_modified("new", 1_700_000_600);
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(
    page.messages.is_empty(),
    "最新が未対応プロバイダのとき、古い opencode の本文を最新として返してはいけません: {:?}",
    texts(&page)
  );
  assert!(
    !texts(&page).iter().any(|text| text == "古い opencode の会話"),
    "古い会話の本文が応答に混ざってはいけません"
  );
  assert_ne!(page.session_id, "ses_old", "古い opencode のセッションを最新として選んではいけません");
  let reason = page.unavailable_reason.expect("最新が未対応プロバイダである理由を返す必要があります");
  assert!(reason.contains("claude-sdk"), "理由には最新の記録の provider 名を含める必要があります: {reason}");
}

#[test]
fn explicit_session_id_still_selects_its_record_when_a_newer_unsupported_provider_record_exists() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("old", "ses_old", &project_cwd, "opencode");
  fixture.set_manager_modified("old", 1_700_000_000);
  fixture.insert_session("ses_old", &project_cwd, 1_000);
  fixture.insert_message("ses_old", 1, "user", FIRST_MESSAGE_AT, &user_data("古い opencode の会話", FIRST_MESSAGE_AT));
  fixture.write_manager("new", "ses_new", &project_cwd, "claude-sdk");
  fixture.set_manager_modified("new", 1_700_000_600);
  let reader = fixture.reader();

  let page = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    Some("ses_old".to_owned()),
    after_seq:     0,
    limit:         50,
  });

  assert_eq!(texts(&page), vec!["古い opencode の会話"], "明示指定した記録の本文を返す必要があります");
  assert_eq!(page.session_id, "ses_old");
  assert_eq!(page.unavailable_reason, None);
}

#[test]
fn records_with_the_same_modification_time_and_different_providers_are_not_resolved_to_the_old_conversation() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("opencode", "ses_opencode", &project_cwd, "opencode");
  fixture.set_manager_modified("opencode", 1_700_000_000);
  fixture.insert_session("ses_opencode", &project_cwd, 1_000);
  fixture.insert_message(
    "ses_opencode",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &user_data("同時刻の opencode", FIRST_MESSAGE_AT),
  );
  fixture.write_manager("claude", "ses_claude", &project_cwd, "claude-sdk");
  fixture.set_manager_modified("claude", 1_700_000_000);
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(
    page.messages.is_empty(),
    "同じ更新時刻で provider が異なるとき、opencode の本文を最新として返してはいけません: {:?}",
    texts(&page)
  );
  assert_ne!(page.session_id, "ses_opencode", "確定できないときに opencode のセッションを選んではいけません");
  assert!(page.unavailable_reason.is_some(), "最新を確定できない理由を返す必要があります");
}

#[test]
fn uncheckpointed_wal_append_is_visible_to_the_reader() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let writer = fixture.writer();
  assert_eq!(fixture.journal_mode().to_lowercase(), "wal", "テストの DB は WAL モードで作る必要があります");
  fixture.write_manager("alpha", "ses_alpha", &project_cwd, "opencode");
  writer
    .execute("INSERT INTO session_v2 (id, directory, time_updated) VALUES (?1, ?2, ?3)", rusqlite::params![
      "ses_alpha",
      project_cwd.display().to_string(),
      2_000
    ])
    .expect("セッションを記録できます");
  writer
    .execute(
      "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
      rusqlite::params![
        "msg-1",
        "ses_alpha",
        "user",
        1_i64,
        FIRST_MESSAGE_AT,
        user_data("最初の発言", FIRST_MESSAGE_AT)
      ],
    )
    .expect("最初のメッセージを記録できます");
  let reader = fixture.reader();

  let first = reader.read(&query("takt-dash", &project_cwd));
  assert_eq!(seqs(&first), vec![1]);

  writer
    .execute(
      "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
      rusqlite::params![
        "msg-2",
        "ses_alpha",
        "assistant",
        2_i64,
        1_794_613_229_000_i64,
        assistant_text_data("未チェックポイントの追記", 1_794_613_229_000)
      ],
    )
    .expect("追記できます");

  let second = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    None,
    after_seq:     first.next_after_seq,
    limit:         50,
  });

  assert_eq!(
    texts(&second),
    vec!["未チェックポイントの追記"],
    "書き込み接続を保持したまま追記しても、チェックポイント前の行を読み取り側が取得する必要があります"
  );
  assert_eq!(second.next_after_seq, 2);
  drop(writer);
}

#[test]
fn missing_conversations_directory_returns_an_empty_page_with_a_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  let reader = fixture.reader();
  std::fs::remove_dir_all(&fixture.conversations).expect("conversations ディレクトリを消せます");

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(page.messages.is_empty());
  assert!(page.unavailable_reason.is_some());
}

#[test]
fn missing_database_returns_an_empty_page_with_a_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("alpha", "ses_alpha", &project_cwd, "opencode");
  let reader = fixture.reader();
  std::fs::remove_file(&fixture.database).expect("DB を消せます");

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(page.messages.is_empty());
  assert!(page.unavailable_reason.is_some());
}

#[test]
fn schema_mismatch_in_session_message_returns_an_empty_page_with_a_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.replace_schema(
    "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT NOT NULL, time_updated INTEGER NOT NULL);
     CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL);",
  );
  fixture.write_manager("alpha", "ses_alpha", &project_cwd, "opencode");
  fixture.insert_session("ses_alpha", &project_cwd, 1_000);
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(page.messages.is_empty());
  assert_eq!(page.unavailable_reason.as_deref(), Some("会話のデータベースのスキーマを読めません"));
}

#[test]
fn schema_mismatch_in_session_directory_returns_an_empty_page_with_a_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.replace_schema(
    "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, time_updated INTEGER NOT NULL);
     CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);",
  );
  fixture.write_manager("alpha", "ses_alpha", &project_cwd, "opencode");
  fixture.insert_session_without_directory("ses_alpha", 1_000);
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(page.messages.is_empty());
  assert_eq!(page.unavailable_reason.as_deref(), Some("会話のデータベースのスキーマを読めません"));
}

#[test]
fn latest_selection_does_not_require_the_session_time_updated_column() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.replace_schema(
    "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT NOT NULL);
     CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL, data TEXT NOT NULL);",
  );
  fixture.write_manager("old", "ses_old", &project_cwd, "opencode");
  fixture.set_manager_modified("old", 1_700_000_000);
  fixture.write_manager("new", "ses_new", &project_cwd, "opencode");
  fixture.set_manager_modified("new", 1_700_000_600);
  {
    let connection = rusqlite::Connection::open(&fixture.database).expect("会話の DB を開けます");
    for session_id in ["ses_old", "ses_new"] {
      connection
        .execute("INSERT INTO session_v2 (id, directory) VALUES (?1, ?2)", rusqlite::params![
          session_id,
          project_cwd.display().to_string()
        ])
        .expect("セッションを記録できます");
    }
  }
  fixture.insert_message("ses_old", 1, "user", FIRST_MESSAGE_AT, &user_data("古い会話", FIRST_MESSAGE_AT));
  fixture.insert_message("ses_new", 1, "user", FIRST_MESSAGE_AT, &user_data("新しい会話", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(
    texts(&page),
    vec!["新しい会話"],
    "最新の判定は session_v2.time_updated 列に依存せず、manager の記録の更新時刻で行う必要があります"
  );
  assert_eq!(page.session_id, "ses_new");
  assert_eq!(page.unavailable_reason, None);
}

#[test]
fn unsupported_provider_returns_an_empty_page_with_a_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("claude", "ses_claude", &project_cwd, "claude-sdk");
  fixture.insert_session("ses_claude", &project_cwd, 1_000);
  fixture.insert_message("ses_claude", 1, "user", FIRST_MESSAGE_AT, &user_data("未対応の会話", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(page.messages.is_empty());
  assert!(page.unavailable_reason.is_some());
}

#[test]
fn unsupported_provider_reason_is_not_hidden_by_a_missing_database() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("claude", "ses_claude", &project_cwd, "claude-sdk");
  let reader = fixture.reader();
  std::fs::remove_file(&fixture.database).expect("DB を消せます");

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(page.messages.is_empty(), "未対応 provider の会話を返してはいけません");
  let reason = page.unavailable_reason.expect("未対応 provider である理由を返す必要があります");
  assert!(reason.contains("claude-sdk"), "理由には選択した記録の provider 名を含める必要があります: {reason}");
  assert!(reason.contains("対応していません"), "理由は provider が未対応であることを示す必要があります: {reason}");
  assert!(
    !reason.contains("データベース"),
    "opencode.db が無いことで未対応 provider の理由が隠れてはいけません: {reason}"
  );
}

#[test]
fn explicit_unsupported_provider_session_reason_is_not_hidden_by_a_missing_database() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.write_manager("old", "ses_old", &project_cwd, "opencode");
  fixture.set_manager_modified("old", 1_700_000_000);
  fixture.insert_session("ses_old", &project_cwd, 1_000);
  fixture.write_manager("new", "ses_new", &project_cwd, "claude-sdk");
  fixture.set_manager_modified("new", 1_700_000_600);
  let reader = fixture.reader();
  std::fs::remove_file(&fixture.database).expect("DB を消せます");

  let page = reader.read(&ConversationQuery {
    project:       project_name("takt-dash"),
    cwd:           project_cwd.clone(),
    project_roots: vec![project_cwd.clone()],
    session_id:    Some("ses_new".to_owned()),
    after_seq:     0,
    limit:         50,
  });

  assert!(page.messages.is_empty(), "未対応 provider の会話を返してはいけません");
  let reason = page.unavailable_reason.expect("明示指定した記録が未対応 provider である理由を返す必要があります");
  assert!(reason.contains("claude-sdk"), "理由には選択した記録の provider 名を含める必要があります: {reason}");
  assert!(reason.contains("対応していません"), "理由は provider が未対応であることを示す必要があります: {reason}");
  assert!(
    !reason.contains("データベース"),
    "opencode.db が無いことで未対応 provider の理由が隠れてはいけません: {reason}"
  );
}

#[test]
fn missing_manager_record_returns_an_empty_page_with_a_reason() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.insert_session("ses_alpha", &project_cwd, 1_000);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &user_data("記録なし", FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert!(page.messages.is_empty());
  assert!(page.unavailable_reason.is_some());
}

#[test]
fn reader_works_with_only_the_allowed_tables() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  fixture.replace_schema(CONVERSATION_SCHEMA);
  let reader = seed_single_project(&fixture, &project_cwd);

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["最初の発言"], "credential や account のテーブルが無くても読める必要があります");
}

fn seed_session(fixture: &ConversationFixture, project_cwd: &Path) {
  fixture.write_manager("alpha", "ses_alpha", project_cwd, "opencode");
  fixture.insert_session("ses_alpha", project_cwd, 2_000);
}

#[test]
fn wrapped_user_input_is_unwrapped_to_the_human_text() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let human_text = "見出しを直してください。";
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &wrapped_user_data("前のターンの本文", human_text, FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec![human_text], "包み付きの user の行は人の発言だけを本文にする必要があります");
  assert_eq!(notices(&page), vec![None]);
  let body = &page.messages[0].text;
  for hidden in ["このターンの入力", "Business context", "次の JSON schema", "前のターンの本文", "\"speaker\""]
  {
    assert!(!body.contains(hidden), "本文に包みの内容を含めてはいけません: {hidden}");
  }
}

#[test]
fn wrapped_user_input_ignores_a_marker_inside_the_previous_text() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let previous = "## このターンの入力\n{\"speaker\":{\"id\":\"operator\"},\"text\":\"前の依頼\"}";
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &wrapped_user_data(previous, "今回の依頼", FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(
    texts(&page),
    vec!["今回の依頼"],
    "前の本文に同じ見出しと JSON が含まれていても、現在の包みの発言を取り出す必要があります"
  );
}

#[test]
fn wrapped_user_input_without_a_speaker_is_left_as_is() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let data = wrapped_user_without_speaker_data("内部の下書き", FIRST_MESSAGE_AT);
  let raw = data_text(&data);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  let without_business_context = raw.replace("Business context (not approval): {}\n", "").trim_end().to_owned();
  assert_eq!(
    texts(&page),
    vec![without_business_context],
    "どの形にも当たらない包みは、Business context の行だけを除いた元の本文を返す必要があります"
  );
  assert_eq!(page.messages[0].speaker, ConversationSpeaker::User);
  assert_eq!(page.messages[0].detail, None);
}

struct EventTurnCase {
  kind:             &'static str,
  description:      Option<&'static str>,
  event:            serde_json::Value,
  business_context: bool,
  expected_text:    &'static str,
}

fn event_turn_cases() -> Vec<EventTurnCase> {
  vec![
    EventTurnCase {
      kind:             "patrol",
      description:      Some("このターンは daemon が届けた定期的な見回りです。対象ゴールの状態、run の判断と根拠を確かめ、完成へ向けた手配を判断します。"),
      event:            serde_json::json!({ "kind": "patrol", "repositoryDirectory": "/tmp/takt-dash", "goalIds": ["1ae45d6e", "9e0a6490"] }),
      business_context: true,
      expected_text:    "このターンは daemon が届けた定期的な見回りです。対象ゴールの状態、run の判断と根拠を確かめ、完成へ向けた手配を判断します。",
    },
    EventTurnCase {
      kind:             "goal_resumed",
      description:      Some("このターンはイベントから始まりました。種類: goal_resumed（manager がゴールを再開した）。origin は manager です。manager の操作で実行が可能になった状態として受け取り、保存された合意、各タスクの停止理由、回答への依存を確かめて進めます。"),
      event:            serde_json::json!({ "kind": "goal_resumed", "repositoryDirectory": "/tmp/takt-dash", "goalId": "3e11a84e", "origin": "manager" }),
      business_context: false,
      expected_text:    "このターンはイベントから始まりました。種類: goal_resumed（manager がゴールを再開した）。origin は manager です。manager の操作で実行が可能になった状態として受け取り、保存された合意、各タスクの停止理由、回答への依存を確かめて進めます。",
    },
    EventTurnCase {
      kind:             "goal_pr_failed",
      description:      Some("このターンはイベントから始まりました。種類: goal_pr_failed（ゴールの PR の準備に失敗した）。原因と現在の状態を確かめます。"),
      event:            serde_json::json!({ "kind": "goal_pr_failed", "goalId": "6ddf5683", "expectedSha": "b652850e", "reason": "Goal PR creation failed: Command failed: gh pr create --title …\n## 目的\n" }),
      business_context: false,
      expected_text:    "このターンはイベントから始まりました。種類: goal_pr_failed（ゴールの PR の準備に失敗した）。原因と現在の状態を確かめます。",
    },
    EventTurnCase {
      kind:             "goal_registered",
      description:      Some("このターンはイベントから始まりました。種類: goal_registered（ゴールが登録された）。保存された合意から作業を手配します。"),
      event:            serde_json::json!({ "kind": "goal_registered", "repositoryDirectory": "/tmp/takt-dash", "goalId": "67df7701" }),
      business_context: true,
      expected_text:    "このターンはイベントから始まりました。種類: goal_registered（ゴールが登録された）。保存された合意から作業を手配します。",
    },
    EventTurnCase {
      kind:             "task_completed",
      description:      Some("このターンはイベントから始まりました。種類: task_completed（タスクが完了した）。結果と成果を確かめ、次の作業を判断します。"),
      event:            serde_json::json!({ "kind": "task_completed", "task": { "taskName": "issue-200" }, "result": { "success": true } }),
      business_context: false,
      expected_text:    "このターンはイベントから始まりました。種類: task_completed（タスクが完了した）。結果と成果を確かめ、次の作業を判断します。",
    },
    EventTurnCase {
      kind:             "task_failed",
      description:      Some("このターンはイベントから始まりました。種類: task_failed（タスクの失敗・中断の通知）。次の対応を決めるため、通知された終了理由と現在の状態を照合し、そのタスクの停止理由と未決定事項に沿って判断します。"),
      event:            serde_json::json!({ "kind": "task_failed", "task": { "taskName": "merge-pr-655" }, "result": { "success": false, "failureReason": "Command failed" } }),
      business_context: false,
      expected_text:    "このターンはイベントから始まりました。種類: task_failed（タスクの失敗・中断の通知）。次の対応を決めるため、通知された終了理由と現在の状態を照合し、そのタスクの停止理由と未決定事項に沿って判断します。",
    },
    EventTurnCase {
      kind:             "wait_command_completed",
      description:      None,
      event:            serde_json::json!({ "kind": "wait_command_completed", "id": "35cb820a", "description": "PR #654 の再実行の完了を待つ", "status": "succeeded", "exitCode": 0 }),
      business_context: false,
      expected_text:    "イベント: wait_command_completed",
    },
    EventTurnCase {
      kind:             "watch_observation",
      description:      Some("このターンは見張りが届けたイベントから始まりました。種類: watch_observation（実行や PR などの状態変化を観測した）。観測の種類と現在の状態を照合します。"),
      event:            serde_json::json!({ "kind": "watch_observation", "observation": { "kind": "pr_changed", "observedAt": "2026-11-20T09:02:31.057Z" } }),
      business_context: false,
      expected_text:    "このターンは見張りが届けたイベントから始まりました。種類: watch_observation（実行や PR などの状態変化を観測した）。観測の種類と現在の状態を照合します。",
    },
  ]
}

#[test]
fn event_turns_show_the_description_as_the_system_and_fold_the_event_json() {
  for case in event_turn_cases() {
    let (_directory, fixture) = ConversationFixture::new();
    let project_cwd = fixture.project_dir("takt-dash");
    seed_session(&fixture, &project_cwd);
    let data = event_turn_data(case.description, &case.event, case.business_context, FIRST_MESSAGE_AT);
    fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);

    let page = fixture.reader().read(&query("takt-dash", &project_cwd));

    let message = &page.messages[0];
    assert_eq!(
      message.speaker,
      ConversationSpeaker::System,
      "{}: 出来事から始まったターンの話者はシステムです",
      case.kind
    );
    assert_eq!(message.text, case.expected_text, "{}: 本文は包みの説明文（無ければ種類）だけです", case.kind);
    let detail = message.detail.as_ref().unwrap_or_else(|| panic!("{}: 折りたたみの中身があります", case.kind));
    assert_eq!(detail.question, None, "{}: イベントに質問の文はありません", case.kind);
    let folded: serde_json::Value = serde_json::from_str(&detail.source_json).expect("折りたたみの中身は JSON です");
    assert_eq!(folded, case.event, "{}: 折りたたみにはイベントの JSON を置きます", case.kind);
    assert!(
      !detail.source_json.contains("To advance the assigned goals"),
      "{}: instruction は折りたたみに入れません",
      case.kind
    );
    for shown in [message.text.as_str(), detail.source_json.as_str()] {
      assert!(!shown.contains("JSON schema"), "{}: 応答形式の指示は出しません", case.kind);
      assert!(!shown.contains("Business context"), "{}: Business context は出しません", case.kind);
    }
  }
}

#[test]
fn inquiry_answer_turns_show_only_the_answer_and_fold_the_question_and_source_json() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let question = "PR #654 のマージ用タスクが停止しました。どう進めますか。";
  let answer = "窓口の回答です。案A で進めてください。\n\n1. 作業ツリーは残っています。";
  let data = inquiry_answered_data(question, answer, true, FIRST_MESSAGE_AT);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);

  let page = fixture.reader().read(&query("takt-dash", &project_cwd));

  let message = &page.messages[0];
  assert_eq!(message.speaker, ConversationSpeaker::User, "質問への回答は人が書いた文なので話者はユーザーです");
  assert_eq!(message.text, answer, "ふだんは回答の本文だけを見せます");
  let detail = message.detail.as_ref().expect("折りたたみの中身があります");
  assert_eq!(detail.question.as_deref(), Some(question), "折りたたみに質問の文を置きます");
  let folded: serde_json::Value = serde_json::from_str(&detail.source_json).expect("折りたたみの中身は JSON です");
  assert_eq!(folded["kind"], "inquiry_answered", "折りたたみに元の JSON を置きます");
  assert_eq!(folded["answer"]["text"], answer, "元の JSON は回答を含みます");
  assert!(!detail.source_json.contains("Business context"), "Business context は折りたたみにも出しません");
  assert!(!detail.source_json.contains("JSON schema"), "応答形式の指示は折りたたみにも出しません");
}

#[test]
fn event_turns_without_a_usable_kind_are_unmatched_wrappers() {
  for event in
    [serde_json::json!({ "goalId": "67df7701" }), serde_json::json!({ "kind": "" }), serde_json::json!({ "kind": 3 })]
  {
    let (_directory, fixture) = ConversationFixture::new();
    let project_cwd = fixture.project_dir("takt-dash");
    seed_session(&fixture, &project_cwd);
    let data = event_turn_data(Some("このターンはイベントから始まりました。"), &event, false, FIRST_MESSAGE_AT);
    fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);

    let page = fixture.reader().read(&query("takt-dash", &project_cwd));

    let message = &page.messages[0];
    assert_eq!(message.speaker, ConversationSpeaker::User, "{event}: 種類が読めなければ話者は変えません");
    let raw = data_text(&data);
    let expected = raw[..raw.find("次の JSON schema").expect("指示がある")].trim_end();
    assert_eq!(message.text, expected, "{event}: 種類が読めなければ、指示を除いた元の本文を返します");
    assert_eq!(message.detail, None, "{event}: 種類が読めなければ折りたたみはありません");
  }
}

#[test]
fn operation_request_answers_without_text_show_the_chosen_report() {
  let cases = [
    (serde_json::json!({ "report": "completed", "source": "takt-dash" }), "実施した"),
    (serde_json::json!({ "report": "unable", "source": "takt-dash" }), "対応できない"),
    (serde_json::json!({ "report": "deferred" }), "回答: deferred"),
    (serde_json::json!({ "text": "", "report": "completed" }), "実施した"),
    (serde_json::json!({ "text": " \n\t", "report": "unable" }), "対応できない"),
    (
      serde_json::json!({ "text": "人が書いた回答です。", "report": "completed" }),
      "人が書いた回答です。\n（回答: 実施した）",
    ),
    (
      serde_json::json!({ "text": "人が書いた回答です。", "report": "unable" }),
      "人が書いた回答です。\n（回答: 対応できない）",
    ),
    (
      serde_json::json!({ "text": "人が書いた回答です。", "report": "deferred" }),
      "人が書いた回答です。\n（回答: deferred）",
    ),
    (serde_json::json!({}), "回答（本文なし）"),
    (serde_json::json!({ "text": "   ", "report": "" }), "回答（本文なし）"),
    (serde_json::json!({ "report": 1 }), "回答（本文なし）"),
    (serde_json::Value::Null, "回答（本文なし）"),
    (serde_json::json!("文字列の回答"), "回答（本文なし）"),
  ];
  for (answer, expected) in cases {
    let (_directory, fixture) = ConversationFixture::new();
    let project_cwd = fixture.project_dir("takt-dash");
    seed_session(&fixture, &project_cwd);
    let data =
      inquiry_answered_with_answer_data("PR を手でマージしてください。", answer.clone(), false, FIRST_MESSAGE_AT);
    fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);

    let page = fixture.reader().read(&query("takt-dash", &project_cwd));

    let message = &page.messages[0];
    assert_eq!(message.speaker, ConversationSpeaker::User, "{answer}: 回答の話者はユーザーです");
    assert_eq!(message.text, expected, "{answer}: text があれば text、無ければ選んだ報告を本文にします");
    let detail = message.detail.as_ref().expect("折りたたみがあります");
    assert_eq!(detail.question.as_deref(), Some("PR を手でマージしてください。"));
    let folded: serde_json::Value = serde_json::from_str(&detail.source_json).expect("JSON です");
    assert_eq!(folded["answer"], answer, "{answer}: 折りたたみに元の JSON を置きます");
  }
}

#[test]
fn legacy_inquiry_answers_without_a_kind_are_read_as_answers() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let answer = serde_json::json!({ "text": "完成まで進めてから repo-b を担当解除する（推奨）", "source": "tui" });
  let data = legacy_inquiry_answered_data("担当から外すためにどちらを選びますか。", answer.clone(), FIRST_MESSAGE_AT);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);

  let page = fixture.reader().read(&query("takt-dash", &project_cwd));

  let message = &page.messages[0];
  assert_eq!(message.speaker, ConversationSpeaker::User);
  assert_eq!(
    message.text, "完成まで進めてから repo-b を担当解除する（推奨）",
    "kind の無い古い回答も回答だけを見せます"
  );
  let detail = message.detail.as_ref().expect("折りたたみがあります");
  assert_eq!(detail.question.as_deref(), Some("担当から外すためにどちらを選びますか。"));
  let folded: serde_json::Value = serde_json::from_str(&detail.source_json).expect("JSON です");
  assert_eq!(folded["answer"], answer);
}

#[test]
fn director_turns_show_the_request_as_the_director_and_fold_the_source_json() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let request = "director からの確認です。PR 準備が3回失敗しています。原因と次の方針を作業記録に残してください。";
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &director_turn_data(request, true, FIRST_MESSAGE_AT),
  );

  let page = fixture.reader().read(&query("takt-dash", &project_cwd));

  let message = &page.messages[0];
  assert_eq!(message.speaker, ConversationSpeaker::Director, "director の依頼の話者はディレクターです");
  assert_eq!(message.text, request, "本文は依頼の text だけです");
  let detail = message.detail.as_ref().expect("折りたたみがあります");
  assert_eq!(detail.question, None);
  let folded: serde_json::Value = serde_json::from_str(&detail.source_json).expect("JSON です");
  assert_eq!(folded["origin"], "director", "折りたたみに元の JSON を置きます");
  assert_eq!(folded["text"], request);
  assert!(!detail.source_json.contains("Business context") && !message.text.contains("Business context"));
}

#[test]
fn director_shaped_lines_that_are_not_usable_are_unmatched_wrappers() {
  for input in [
    serde_json::json!({ "text": "依頼", "origin": "manager" }),
    serde_json::json!({ "text": "  ", "origin": "director" }),
    serde_json::json!({ "origin": "director" }),
  ] {
    let (_directory, fixture) = ConversationFixture::new();
    let project_cwd = fixture.project_dir("takt-dash");
    seed_session(&fixture, &project_cwd);
    let data = turn_data(Some("説明文"), &input, true, true, FIRST_MESSAGE_AT);
    fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);

    let page = fixture.reader().read(&query("takt-dash", &project_cwd));

    let message = &page.messages[0];
    assert_eq!(message.speaker, ConversationSpeaker::User, "{input}");
    assert_eq!(message.detail, None, "{input}");
    assert!(message.text.contains(&serde_json::to_string(&input).expect("JSON")), "{input}: JSON の行は残します");
    assert!(!message.text.contains("Business context") && !message.text.contains("JSON schema"), "{input}");
  }
}

#[test]
fn unmatched_wrappers_drop_only_the_found_instructions() {
  let input = serde_json::json!({ "content": "# テスト作成レポート", "reference": "test-report.md", "scope": "full" });
  let cases = [(true, true), (true, false), (false, true), (false, false)];
  for (business_context, response_format) in cases {
    let (_directory, fixture) = ConversationFixture::new();
    let project_cwd = fixture.project_dir("takt-dash");
    seed_session(&fixture, &project_cwd);
    let data = turn_data(Some("説明文です。"), &input, business_context, response_format, FIRST_MESSAGE_AT);
    let raw = data_text(&data);
    fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &data);

    let page = fixture.reader().read(&query("takt-dash", &project_cwd));

    let text = &page.messages[0].text;
    let label = format!("business_context={business_context} response_format={response_format}");
    if !business_context && !response_format {
      assert_eq!(text, &raw, "{label}: どちらも見つからなければ元の本文のままです");
      continue;
    }
    let expected = format!(
      "\n## このターンの入力\n\n\n\n\n\n\n\n説明文です。\n\n\n\n\n{}",
      serde_json::to_string(&input).expect("JSON")
    );
    assert_eq!(text, &expected, "{label}: 見つけた指示と Business context だけを除きます");
    assert!(!text.contains(BUSINESS_CONTEXT) && !text.contains(RESPONSE_FORMAT), "{label}");
  }
}

#[test]
fn human_speech_and_unwrapped_turns_have_no_folded_detail() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &wrapped_user_data("前のターンの本文", "人の発言です", FIRST_MESSAGE_AT),
  );
  fixture.insert_message(
    "ses_alpha",
    2,
    "user",
    FIRST_MESSAGE_AT + 1,
    &plain_user_data("包みのない発言", FIRST_MESSAGE_AT + 1),
  );

  let page = fixture.reader().read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["人の発言です".to_owned(), "包みのない発言".to_owned()]);
  assert!(page.messages.iter().all(|message| message.speaker == ConversationSpeaker::User), "話者はユーザーのままです");
  assert!(
    page.messages.iter().all(|message| message.detail.is_none()),
    "人の発言と包みの無い行に折りたたみはありません"
  );
}

#[test]
fn plain_user_text_is_returned_unchanged() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &plain_user_data("包みのない発言", FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["包みのない発言"]);
  assert_eq!(notices(&page), vec![None]);
}

#[test]
fn fenced_assistant_message_is_unwrapped_to_the_message() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    1,
    "assistant",
    FIRST_MESSAGE_AT,
    &fenced_assistant_data("確認しました。", Some("README を追加する"), FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(
    texts(&page),
    vec!["確認しました。"],
    "fenced JSON の assistant の行は message だけを本文にする必要があります"
  );
  let body = &page.messages[0].text;
  assert!(!body.contains("README を追加する"), "summary を本文に含めてはいけません");
  assert!(!body.contains("```"), "fence を本文に含めてはいけません");
  assert!(!body.contains("\"message\""), "JSON を本文に含めてはいけません");
}

#[test]
fn fenced_assistant_message_with_null_summary_is_unwrapped() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    1,
    "assistant",
    FIRST_MESSAGE_AT,
    &fenced_assistant_data("受領しました。", None, FIRST_MESSAGE_AT),
  );
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["受領しました。"], "summary が null でも message だけを本文にする必要があります");
}

#[test]
fn fenced_assistant_without_a_message_is_left_as_is() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let data = fenced_assistant_without_message_data("確認中", FIRST_MESSAGE_AT);
  let raw = assistant_text(&data);
  fixture.insert_message("ses_alpha", 1, "assistant", FIRST_MESSAGE_AT, &data);
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec![raw], "message を持たない fenced JSON は元の本文を返す必要があります");
  assert!(!page.messages[0].text.is_empty(), "解釈できない本文を空にしてはいけません");
}

#[test]
fn invalid_fenced_assistant_json_is_left_as_is() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  let data = invalid_fenced_assistant_data(FIRST_MESSAGE_AT);
  let raw = assistant_text(&data);
  fixture.insert_message("ses_alpha", 1, "assistant", FIRST_MESSAGE_AT, &data);
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec![raw], "解釈できない fenced JSON は元の本文を返す必要があります");
}

#[test]
fn multiple_assistant_text_elements_are_still_joined() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    1,
    "assistant",
    FIRST_MESSAGE_AT,
    r#"{"content":[{"type":"text","text":"前置き"},{"type":"text","text":"本文"}],"time":{"created":1794613221331}}"#,
  );
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["前置き\n本文"], "複数の text 要素は従来どおり改行で結合する必要があります");
}

#[test]
fn garbled_user_text_gets_a_notice_without_changing_the_body() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &garbled_user_data(87, FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  let body = &page.messages[0].text;
  assert_eq!(body.matches('\u{FFFD}').count(), 87, "本文の置換文字はそのまま残す必要があります");
  assert_eq!(
    notices(&page),
    vec![Some("元記録で文字化けしています（置換文字 87 個）".to_owned())],
    "置換文字を多く含む発言には文字化けの印を付ける必要があります"
  );
  assert!(page.skipped.is_empty(), "文字化けした行をスキップ扱いにしてはいけません");
}

#[test]
fn two_digit_replacements_are_marked_as_garbled() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &garbled_user_data(10, FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(
    notices(&page),
    vec![Some("元記録で文字化けしています（置換文字 10 個）".to_owned())],
    "運用の目安の 10 個に達した置換文字には印を付ける必要があります（issue #111 の観測: 文字化けした seq 8 は 87 個、正常な seq 22 は 0 個。読みやすさの境界を実証した値ではない）"
  );
}

#[test]
fn single_digit_replacements_are_not_marked_as_garbled() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message("ses_alpha", 1, "user", FIRST_MESSAGE_AT, &garbled_user_data(9, FIRST_MESSAGE_AT));
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(
    notices(&page),
    vec![None],
    "短い断片の破損（一桁）では本文が読めるため印を付けない必要があります（しきい値 10 の下側境界）"
  );
  assert_eq!(page.messages[0].text.matches('\u{FFFD}').count(), 9);
}

#[test]
fn reading_the_conversation_does_not_modify_the_source_records() {
  let (_directory, fixture) = ConversationFixture::new();
  let project_cwd = fixture.project_dir("takt-dash");
  seed_session(&fixture, &project_cwd);
  fixture.insert_message(
    "ses_alpha",
    1,
    "user",
    FIRST_MESSAGE_AT,
    &wrapped_user_data("前", "元記録を変えない", FIRST_MESSAGE_AT),
  );
  let manager_path = fixture.conversations.join("manager-alpha.json");
  let database_before = std::fs::read(&fixture.database).expect("DB のバイト列を読めます");
  let manager_before = std::fs::read(&manager_path).expect("manager の記録のバイト列を読めます");
  let reader = fixture.reader();

  let page = reader.read(&query("takt-dash", &project_cwd));

  assert_eq!(texts(&page), vec!["元記録を変えない"]);
  assert_eq!(std::fs::read(&fixture.database).expect("DB のバイト列を読めます"), database_before);
  assert_eq!(std::fs::read(&manager_path).expect("manager の記録のバイト列を読めます"), manager_before);
}
