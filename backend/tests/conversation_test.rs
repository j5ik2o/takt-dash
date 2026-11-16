//! manager の会話の保存先を直接読む経路（時系列・増分取得・プロジェクト識別・
//! 形式の分類・保存先の不在）を、一時 DB と一時 conversations ディレクトリで検証する。
//!
//! テストは実ホームを参照せず、`ConversationFixture` が作る一時パスだけを使う。

mod conversation_fixture;

use std::path::Path;

use chrono::{DateTime, Utc};
use conversation_fixture::{ConversationFixture, CONVERSATION_SCHEMA};
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
