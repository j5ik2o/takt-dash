//! ダミーサーバーが返す固定の概要・ゴール・会話と、テストで使う定数。

use prost_types::Timestamp;

use crate::proto::v1::{
  self, ConversationMessage, DashboardState, GetConversationResponse, PendingDecision, PendingDecisionType, Project,
  Server as ServerInfo, TaskStatusCounts,
};

// proto の列挙値は生成コードの数値をそのまま使う。
const GOAL_STATUS_CREATED: i32 = v1::GoalStatus::Created as i32;
const GOAL_STATUS_AWAITING_MERGE: i32 = v1::GoalStatus::AwaitingMerge as i32;
const EXECUTION_STATUS_ACTIVE: i32 = v1::ExecutionStatus::Active as i32;
const EXECUTION_STATUS_PAUSED: i32 = v1::ExecutionStatus::Paused as i32;
const RUN_STATUS_RUNNING: i32 = v1::RunStatus::Running as i32;
const TASK_STATUS_PENDING: i32 = v1::TaskStatus::Pending as i32;
const TASK_STATUS_COMPLETED: i32 = v1::TaskStatus::Completed as i32;
const TASK_STATUS_FAILED: i32 = v1::TaskStatus::Failed as i32;
const INQUIRY_KIND_QUESTION: i32 = v1::InquiryKind::Question as i32;
pub(super) const INQUIRY_KIND_OPERATION_REQUEST: i32 = v1::InquiryKind::OperationRequest as i32;

/// タイムスタンプを組み立てる。`secs` は UNIX 時刻の秒。
pub(super) fn ts(secs: i64) -> Timestamp {
  Timestamp { seconds: secs, nanos: 0 }
}

// ----------------------------------------------------------------------------
// ダミーサーバーが返す固定の概要
// ----------------------------------------------------------------------------

pub(super) fn sample_overview() -> DashboardState {
  DashboardState {
    read_at:           Some(ts(1_731_600_000)),
    servers:           sample_overview_servers(),
    projects:          sample_overview_projects(),
    goals:             sample_overview_goals(),
    runs:              sample_overview_runs(),
    pending_decisions: sample_overview_pending_decisions(),
    thresholds:        Some(v1::AnomalyThresholds {
      pending_decision_stale_minutes: 240,
      run_log_silent_minutes:         10,
    }),
    build_info:        None,
  }
}

fn sample_overview_servers() -> Vec<ServerInfo> {
  vec![
    ServerInfo {
      server_id:    "mini".to_owned(),
      name:         "j5ik2o-mac-mini".to_owned(),
      kind:         "Mac mini".to_owned(),
      connected:    true,
      last_read_at: Some(ts(1_731_599_000)),
      write_only:   false,
    },
    ServerInfo {
      server_id:    "studio".to_owned(),
      name:         "j5ik2o-mac-studio".to_owned(),
      kind:         "Mac Studio".to_owned(),
      connected:    false,
      last_read_at: None,
      write_only:   false,
    },
  ]
}

fn sample_overview_projects() -> Vec<Project> {
  vec![Project {
    name:                "taktdash-cli".to_owned(),
    about:               "taktDash の概要を表示する閲覧コマンド".to_owned(),
    server_id:           "mini".to_owned(),
    completed_tasks_24h: 3,
    connected:           true,
    last_read_at:        Some(ts(1_731_599_000)),
    connection_state:    v1::ConnectionState::Connected as i32,
    concurrency:         2,
  }]
}

fn sample_overview_goals() -> Vec<v1::Goal> {
  vec![v1::Goal {
    goal_id:           "goal-cli".to_owned(),
    server_id:         "mini".to_owned(),
    project:           "taktdash-cli".to_owned(),
    name:              "CLI に閲覧コマンドを足す".to_owned(),
    status:            GOAL_STATUS_AWAITING_MERGE,
    execution_status:  EXECUTION_STATUS_ACTIVE,
    task_counts:       Some(TaskStatusCounts { pending: 1, running: 0, completed: 2, failed: 1, ..Default::default() }),
    paused_reason:     None,
    pull_request:      Some(v1::PullRequest { number: 42, url: "https://example.test/pr/42".to_owned() }),
    awaiting_merge_at: Some(ts(1_731_500_000)),
    merge_task_last:   None,
  }]
}

fn sample_overview_runs() -> Vec<v1::Run> {
  vec![
    v1::Run {
      run_slug:                "run-1".to_owned(),
      session_id:              "session-1".to_owned(),
      server_id:               "mini".to_owned(),
      project:                 "taktdash-cli".to_owned(),
      goal_id:                 "goal-cli".to_owned(),
      task_name:               "閲覧コマンドの実装".to_owned(),
      summary:                 "概要を表示する実装を進めています".to_owned(),
      started_at:              Some(ts(1_731_590_000)),
      steps:                   Vec::new(),
      current_step_started_at: None,
      current_step_number:     None,
      total_steps:             None,
      current_iteration:       None,
      current_step_name:       None,
      last_log_at:             Some(ts(1_731_599_500)),
      reports:                 Vec::new(),
      status:                  RUN_STATUS_RUNNING,
      log_silent:              false,
    },
    // 実行中でない run を混ぜ、実行中の件数が全 run の件数と取り違えられないようにする。
    v1::Run {
      run_slug:                "run-2".to_owned(),
      session_id:              "session-2".to_owned(),
      server_id:               "studio".to_owned(),
      project:                 "taktdash-cli".to_owned(),
      goal_id:                 "goal-cli".to_owned(),
      task_name:               "終わったタスク".to_owned(),
      summary:                 "完了済みの run".to_owned(),
      started_at:              Some(ts(1_731_580_000)),
      steps:                   Vec::new(),
      current_step_started_at: None,
      current_step_number:     None,
      total_steps:             None,
      current_iteration:       None,
      current_step_name:       None,
      last_log_at:             Some(ts(1_731_590_000)),
      reports:                 Vec::new(),
      status:                  v1::RunStatus::Completed as i32,
      log_silent:              false,
    },
  ]
}

fn sample_overview_pending_decisions() -> Vec<PendingDecision> {
  vec![
    PendingDecision {
      r#type:          PendingDecisionType::Inquiry as i32,
      server_id:       "mini".to_owned(),
      project:         "taktdash-cli".to_owned(),
      goal_id:         Some("goal-cli".to_owned()),
      inquiry_kind:    INQUIRY_KIND_QUESTION,
      created_at:      Some(ts(1_731_300_000)),
      body:            "表示の並び順はどれがよいですか。".to_owned(),
      options:         vec!["待ち時間の長い順".to_owned()],
      pull_request:    None,
      stale:           false,
      inquiry_id:      String::new(),
      revision:        None,
      merge_task_last: None,
    },
    PendingDecision {
      r#type:          PendingDecisionType::AwaitingMerge as i32,
      server_id:       "mini".to_owned(),
      project:         "taktdash-cli".to_owned(),
      goal_id:         Some("goal-cli".to_owned()),
      inquiry_kind:    v1::InquiryKind::Unspecified as i32,
      created_at:      Some(ts(1_731_400_000)),
      body:            "プルリクエスト 42 をマージしてください。".to_owned(),
      options:         Vec::new(),
      pull_request:    Some(v1::PullRequest { number: 42, url: "https://example.test/pr/42".to_owned() }),
      stale:           false,
      inquiry_id:      String::new(),
      revision:        None,
      merge_task_last: None,
    },
    PendingDecision {
      r#type:          PendingDecisionType::GoalRegistration as i32,
      server_id:       "mini".to_owned(),
      project:         "taktdash-cli".to_owned(),
      goal_id:         None,
      inquiry_kind:    v1::InquiryKind::Unspecified as i32,
      created_at:      Some(ts(1_731_500_000)),
      body:            "新しいゴールを登録してください。".to_owned(),
      options:         Vec::new(),
      pull_request:    None,
      stale:           false,
      inquiry_id:      String::new(),
      revision:        Some(3),
      merge_task_last: None,
    },
  ]
}

// ----------------------------------------------------------------------------
// ダミーサーバーが返す固定のゴール一覧とゴール詳細
// ----------------------------------------------------------------------------

/// 全サーバー・全プロジェクトのゴール一覧を組み立てる。
///
/// マージ待ちは `awaiting_merge_at` の有無で決まる契約なので、状態が作成済みでも
/// `awaiting_merge_at` を持つゴールと、状態が人のマージ待ちでも `awaiting_merge_at` を
/// 持たないゴールを混ぜ、状態からの推測では通らないようにする。
pub(super) fn sample_goals() -> Vec<v1::Goal> {
  vec![
    v1::Goal {
      goal_id:           "goal-cli".to_owned(),
      server_id:         "mini".to_owned(),
      project:           "taktdash-cli".to_owned(),
      name:              "CLI に閲覧コマンドを足す".to_owned(),
      status:            GOAL_STATUS_CREATED,
      execution_status:  EXECUTION_STATUS_ACTIVE,
      task_counts:       Some(TaskStatusCounts {
        pending:   1,
        running:   2,
        completed: 3,
        failed:    1,
        exceeded:  9,
        pr_failed: 9,
      }),
      paused_reason:     None,
      pull_request:      Some(v1::PullRequest { number: 42, url: "https://example.test/pr/42".to_owned() }),
      awaiting_merge_at: Some(ts(1_731_500_000)),
      merge_task_last:   None,
    },
    v1::Goal {
      goal_id:           "goal-web".to_owned(),
      server_id:         "studio".to_owned(),
      project:           "corporate-site".to_owned(),
      name:              "ゴール詳細を表示する".to_owned(),
      status:            GOAL_STATUS_AWAITING_MERGE,
      execution_status:  EXECUTION_STATUS_PAUSED,
      task_counts:       Some(TaskStatusCounts {
        pending:   0,
        running:   0,
        completed: 5,
        failed:    2,
        exceeded:  0,
        pr_failed: 0,
      }),
      paused_reason:     Some("保守作業のため".to_owned()),
      pull_request:      None,
      awaiting_merge_at: None,
      merge_task_last:   Some(TASK_STATUS_PENDING),
    },
  ]
}

/// ゴール詳細の固定応答を組み立てる。
///
/// 質問は未回答・回答済み・取り下げを1件ずつ含め、回答のある質問だけに回答本文を持たせる。
/// タスクは run_slug のあるものと無いものを含める。
pub(super) fn sample_goal_detail() -> v1::GetGoalDetailResponse {
  v1::GetGoalDetailResponse {
    goal_id:             "goal-cli".to_owned(),
    server_id:           "mini".to_owned(),
    project:             "taktdash-cli".to_owned(),
    status:              GOAL_STATUS_AWAITING_MERGE,
    execution_status:    EXECUTION_STATUS_ACTIVE,
    objective:           "CLI にゴールの一覧と詳細の表示を足す。".to_owned(),
    out_of_scope:        vec!["backend の変更".to_owned()],
    acceptance_criteria: vec![
      "goals コマンドで一覧を表示できる。".to_owned(),
      "goal コマンドで詳細を表示できる。".to_owned(),
    ],
    constraints:         vec!["proto を変更しない。".to_owned()],
    specifications:      Default::default(),
    branch:              "takt/20261115T0040-goal-5984be7d".to_owned(),
    integration_branch:  "main".to_owned(),
    work_log:            vec![v1::GoalWorkLog {
      work_log_id: "work-1".to_owned(),
      recorded_at: Some(ts(1_731_600_000)),
      summary:     "一覧の表示を実装しました。".to_owned(),
    }],
    questions:           vec![
      v1::GoalQuestion {
        question_id:     "q-1".to_owned(),
        kind:            "question".to_owned(),
        status:          "pending".to_owned(),
        body:            "並び順はどれがよいですか。".to_owned(),
        options:         vec!["新しい順".to_owned()],
        answer_text:     None,
        answer_decision: None,
        answer_report:   None,
        created_at:      Some(ts(1_731_300_000)),
        answered_at:     None,
      },
      v1::GoalQuestion {
        question_id:     "q-2".to_owned(),
        kind:            "approval".to_owned(),
        status:          "answered".to_owned(),
        body:            "この方針で進めてよいですか。".to_owned(),
        options:         Vec::new(),
        answer_text:     Some("はい".to_owned()),
        answer_decision: None,
        answer_report:   None,
        created_at:      Some(ts(1_731_310_000)),
        answered_at:     Some(ts(1_731_320_000)),
      },
      v1::GoalQuestion {
        question_id:     "q-3".to_owned(),
        kind:            "operation_request".to_owned(),
        status:          "withdrawn".to_owned(),
        body:            "ログを取得してください。".to_owned(),
        options:         Vec::new(),
        answer_text:     None,
        answer_decision: None,
        answer_report:   None,
        created_at:      Some(ts(1_731_330_000)),
        answered_at:     None,
      },
    ],
    tasks:               vec![
      v1::GoalTask {
        task_name: "一覧の実装".to_owned(),
        status:    TASK_STATUS_COMPLETED,
        run_slug:  Some("run-1".to_owned()),
      },
      v1::GoalTask { task_name: "詳細の実装".to_owned(), status: TASK_STATUS_PENDING, run_slug: None },
      v1::GoalTask {
        task_name: "失敗したタスク".to_owned(),
        status:    TASK_STATUS_FAILED,
        run_slug:  Some("run-2".to_owned()),
      },
    ],
  }
}

// ----------------------------------------------------------------------------
// ダミーサーバーが返す固定の会話
// ----------------------------------------------------------------------------

/// 会話の1件を組み立てる。`secs` は UNIX 時刻の秒。
pub(super) fn conversation_message(
  seq: i64,
  secs: i64,
  speaker: v1::ConversationSpeaker,
  text: &str,
) -> ConversationMessage {
  ConversationMessage { seq, occurred_at: Some(ts(secs)), speaker: speaker as i32, text: text.to_owned(), notice: None }
}

/// 会話の1ページを組み立てる。
pub(super) fn conversation_page(
  project: &str,
  session_id: &str,
  messages: Vec<ConversationMessage>,
  next_after_seq: i64,
  has_more: bool,
) -> GetConversationResponse {
  GetConversationResponse {
    project: project.to_owned(),
    session_id: session_id.to_owned(),
    provider: "opencode".to_owned(),
    messages,
    next_after_seq,
    has_more,
    skipped: Vec::new(),
    unavailable_reason: None,
  }
}

/// 表示に使う UTC の日時表記を、本番と同じ書式で組み立てる。
pub(super) fn formatted_time(secs: i64) -> String {
  chrono::DateTime::from_timestamp(secs, 0)
    .expect("テストの秒は有効な日時である必要があります")
    .format("%Y-%m-%d %H:%M:%S UTC")
    .to_string()
}
