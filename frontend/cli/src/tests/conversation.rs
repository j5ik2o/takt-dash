//! 会話の取得と表示。

use super::{
  fixtures::{
    conversation_message, conversation_message_with_notice, conversation_page, formatted_time, sample_overview,
  },
  support::{reserve_closed_addr, run_cli, start_conversation_server},
};
use crate::{
  proto::v1::{self, ConversationSkipped},
  render_conversation, run_with, EXIT_FAILURE, EXIT_OK,
};

// ----------------------------------------------------------------------------
// CLI-CONVERSATION: 会話の取得と表示（AC38B-3/AC38B-4）
// ----------------------------------------------------------------------------

#[test]
fn conversation_rendering_marks_empty_conversation() {
  let rendered =
    render_conversation(&conversation_page("corporate-site", "session-1", Vec::new(), 0, false)).expect("有効です");

  assert!(rendered.contains("会話: corporate-site"), "{rendered}");
  assert!(
    rendered.contains("（会話はありません）"),
    "空の会話は空であることが分かる表示にする必要があります: {rendered}"
  );
}

#[test]
fn conversation_rendering_labels_the_director() {
  let rendered = render_conversation(&conversation_page(
    "corporate-site",
    "session-1",
    vec![conversation_message(1, 1_731_600_000, v1::ConversationSpeaker::Director, "director の依頼です。")],
    1,
    false,
  ))
  .expect("有効です");

  assert!(rendered.contains("ディレクター"), "director の依頼の話者はディレクターと表示する必要があります: {rendered}");
  assert!(rendered.contains("director の依頼です。"), "{rendered}");
}

#[test]
fn conversation_command_prints_messages_in_seq_order_and_follows_has_more() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let first_secs = 1_731_600_000;
  let second_secs = 1_731_600_060;
  let third_secs = 1_731_600_120;
  let pages = vec![
    {
      let mut first_page = conversation_page(
        "corporate-site",
        "session-1",
        vec![
          conversation_message(1, first_secs, v1::ConversationSpeaker::User, "最初の発言"),
          conversation_message(2, second_secs, v1::ConversationSpeaker::Manager, "次の発言"),
        ],
        4,
        true,
      );
      first_page.skipped = vec![ConversationSkipped { reason: "解釈できない行".to_owned(), count: 1 }];
      first_page
    },
    conversation_page(
      "corporate-site",
      "session-other",
      vec![conversation_message(4, third_secs, v1::ConversationSpeaker::System, "最後の発言")],
      4,
      false,
    ),
  ];
  let server = runtime.block_on(start_conversation_server(sample_overview(), pages));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  let requests = server.conversation_requests();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "会話の表示は成功する必要があります。stderr: {stderr}");
  assert!(stderr.is_empty(), "成功時に標準エラーへ出してはいけません: {stderr}");
  assert!(stdout.contains("会話: corporate-site"), "対象プロジェクト: {stdout}");
  assert!(
    stdout.contains("セッション: session-1"),
    "最初の応答の session_id を表示し、2ページ目の値で置き換えない: {stdout}"
  );
  assert!(stdout.contains("プロバイダ: opencode"), "プロバイダ: {stdout}");
  assert!(stdout.contains("ユーザー"), "ユーザーの発言: {stdout}");
  assert!(stdout.contains("manager"), "manager の発言: {stdout}");
  assert!(stdout.contains("システム"), "システムの発言: {stdout}");
  assert!(stdout.contains(&formatted_time(first_secs)), "発言の時刻: {stdout}");

  let first = stdout.find("最初の発言").expect("最初の発言を表示する必要があります");
  let second = stdout.find("次の発言").expect("次の発言を表示する必要があります");
  let third = stdout.find("最後の発言").expect("最後の発言を表示する必要があります");
  assert!(first < second && second < third, "会話は時系列で並ぶ必要があります: {stdout}");

  assert_eq!(requests.len(), 2, "has_more の間は続きを取得する必要があります");
  assert_eq!(requests[0].project, "corporate-site");
  assert!(requests[0].session_id.is_none(), "初回は session_id を省略する");
  assert_eq!(requests[0].after_seq, 0);
  assert_eq!(requests[0].limit, 200);
  assert_eq!(requests[1].session_id.as_deref(), Some("session-1"), "以降は最初の session_id を固定する");
  assert_eq!(requests[1].after_seq, 4, "続きは末尾メッセージの seq ではなく next_after_seq から取る");
}

#[test]
fn conversation_command_passes_session_id_to_backend() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_conversation_server(sample_overview(), vec![conversation_page(
    "corporate-site",
    "session-9",
    Vec::new(),
    0,
    false,
  )]));

  let (code, _stdout, stderr) = run_cli(
    &["conversation".to_owned(), "corporate-site".to_owned(), "--session".to_owned(), "session-9".to_owned()],
    server.addr,
  );
  let requests = server.conversation_requests();
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert_eq!(requests.len(), 1);
  assert_eq!(requests[0].session_id.as_deref(), Some("session-9"));
}

#[test]
fn conversation_command_shows_unavailable_reason() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let mut page = conversation_page("corporate-site", "session-1", Vec::new(), 0, false);
  page.unavailable_reason = Some("会話の保存先がありません。".to_owned());
  let server = runtime.block_on(start_conversation_server(sample_overview(), vec![page]));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "unavailable_reason は取得の成功として扱う必要があります。stderr: {stderr}");
  assert!(
    stdout.contains("会話を表示できません: 会話の保存先がありません。"),
    "会話が無い理由を表示する必要があります: {stdout}"
  );
}

#[test]
fn conversation_command_shows_skipped_rows() {
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let mut page = conversation_page("corporate-site", "session-1", Vec::new(), 0, false);
  page.skipped = vec![ConversationSkipped { reason: "解釈できない行".to_owned(), count: 2 }];
  let server = runtime.block_on(start_conversation_server(sample_overview(), vec![page]));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_OK, "stderr: {stderr}");
  assert!(
    stdout.contains("スキップした行: 解釈できない行 2件"),
    "スキップした行の理由と件数を表示する必要があります: {stdout}"
  );
}

#[test]
fn conversation_rendering_shows_notice_for_garbled_message() {
  let secs = 1_731_600_000;
  let page = conversation_page(
    "corporate-site",
    "session-1",
    vec![
      conversation_message(1, secs, v1::ConversationSpeaker::User, "正常な発言"),
      conversation_message_with_notice(
        2,
        secs + 60,
        v1::ConversationSpeaker::User,
        "文字化けした発言\u{FFFD}\u{FFFD}",
        "元記録で文字化けしています（置換文字 12 個）",
      ),
      conversation_message(3, secs + 120, v1::ConversationSpeaker::User, "後続の発言"),
    ],
    3,
    false,
  );

  let rendered = render_conversation(&page).expect("有効です");

  assert!(
    rendered.contains("元記録で文字化けしています（置換文字 12 個）"),
    "注意書き（文字化けの印）を表示する必要があります: {rendered}"
  );
  assert!(
    rendered.contains("文字化けした発言\u{FFFD}\u{FFFD}"),
    "注意書きを付けても本文そのものは変えない: {rendered}"
  );
  assert_eq!(
    rendered.matches("元記録で文字化けしています").count(),
    1,
    "注意書きの無い発言には印を出さない: {rendered}"
  );

  let normal_start = rendered.find("正常な発言").expect("正常な発言を表示する必要があります");
  let garbled_start =
    rendered.find("文字化けした発言\u{FFFD}\u{FFFD}").expect("文字化けした発言の本文を表示する必要があります");
  let notice_start = rendered.find("元記録で文字化けしています").expect("注意書きを表示する必要があります");
  let follow_start = rendered.find("後続の発言").expect("後続の発言を表示する必要があります");

  assert!(normal_start < garbled_start, "正常な発言のあとに文字化けした発言を並べる必要があります: {rendered}");
  assert!(garbled_start < notice_start, "注意書きは文字化けした本文の直後に続ける必要があります: {rendered}");
  assert!(notice_start < follow_start, "注意書きは後続の発言より前に出す必要があります: {rendered}");

  let lines: Vec<&str> = rendered.lines().collect();
  let garbled_line = lines
    .iter()
    .position(|line| line.contains("文字化けした発言\u{FFFD}\u{FFFD}"))
    .expect("文字化けした本文の行が必要です");
  assert_eq!(
    lines.get(garbled_line + 1).copied(),
    Some("  注意: 元記録で文字化けしています（置換文字 12 個）"),
    "注意書きの行は文字化けした本文の行の直後に置く必要があります: {rendered}"
  );
}

#[test]
fn conversation_command_fails_when_backend_is_down() {
  let addr = reserve_closed_addr();
  let mut stdout = Vec::new();
  let mut stderr = Vec::new();

  let code = run_with(
    &["conversation".to_owned(), "corporate-site".to_owned()],
    Ok(format!("{addr}")),
    &mut stdout,
    &mut stderr,
  );

  let stdout = String::from_utf8(stdout).expect("標準出力は UTF-8 である必要があります");
  let stderr = String::from_utf8(stderr).expect("標準エラーは UTF-8 である必要があります");
  assert_eq!(code, EXIT_FAILURE, "接続できないときは取得の失敗として終わる必要があります");
  assert!(stderr.contains("backend に接続できません。"), "接続できない旨を標準エラーへ出す必要があります: {stderr}");
  assert!(!stdout.contains("会話:"), "接続できないときは会話を表示してはいけません: {stdout}");
}

#[test]
fn conversation_reports_backend_failure_without_treating_it_as_connection_failure() {
  // 会話のページを持たないダミーサーバーは GetConversation に NotFound を返す。
  let runtime =
    tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("テスト用の実行環境を作れませんでした");
  let server = runtime.block_on(start_conversation_server(sample_overview(), Vec::new()));

  let (code, stdout, stderr) = run_cli(&["conversation".to_owned(), "corporate-site".to_owned()], server.addr);
  runtime.block_on(server.shutdown());

  assert_eq!(code, EXIT_FAILURE, "backend が応答した失敗は取得の失敗として終わる必要があります");
  assert!(
    stderr.contains("backend から会話を取得できませんでした"),
    "backend の失敗を会話の取得失敗として表示する必要があります: {stderr}"
  );
  assert!(
    stderr.contains("このダミーサーバーには会話のページが残っていません。"),
    "backend の失敗理由を表示する必要があります: {stderr}"
  );
  assert!(
    !stderr.contains("backend に接続できません。"),
    "backend が応答した失敗を接続不能として表示してはいけません: {stderr}"
  );
  assert!(!stdout.contains("会話:"), "取得に失敗したときは会話を表示してはいけません: {stdout}");
}
