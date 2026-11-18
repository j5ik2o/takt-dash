//! CLI のサブコマンドの定義。コマンド名と、引数を解釈した結果の型を持ちます。

use std::path::PathBuf;

use crate::{
  domain::{GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, ServerId},
  proto::v1::OperationAnswerReport,
};

/// 閲覧のみを持つコマンドの名前。位置引数の既定値でもあります。
pub(crate) const OVERVIEW_COMMAND: &str = "overview";

/// manager の会話を閲覧するコマンドの名前。
pub(crate) const CONVERSATION_COMMAND: &str = "conversation";

/// ゴールの一覧を表示するコマンドの名前。
pub(crate) const GOALS_COMMAND: &str = "goals";
/// ゴールの詳細を表示するコマンドの名前。
pub(crate) const GOAL_COMMAND: &str = "goal";

/// 問い合わせへ回答するコマンド。
pub(crate) const ANSWER_COMMAND: &str = "answer";
/// プルリクエストのマージを承認するコマンド。
pub(crate) const MERGE_COMMAND: &str = "merge";
/// ゴール登録の承認待ちを一覧表示するコマンド。
pub(crate) const REGISTRATIONS_COMMAND: &str = "registrations";
/// 判断待ちの操作対象を一覧表示するコマンド。
pub(crate) const DECISIONS_COMMAND: &str = "decisions";
/// ゴール登録を承認するコマンド。
pub(crate) const APPROVE_REGISTRATION_COMMAND: &str = "approve-registration";
/// ゴール登録を却下するコマンド。
pub(crate) const REJECT_REGISTRATION_COMMAND: &str = "reject-registration";

/// 引数を解釈した結果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Invocation {
  /// 概要を表示する。
  Overview(OverviewOptions),
  /// 判断待ちの操作対象を表示する。
  Decisions(DecisionsOptions),
  /// 書き込み操作を実行する。
  Write(WriteInvocation),
  /// manager の会話を表示する。
  Conversation(ConversationOptions),
  /// ゴールの一覧を表示する。
  Goals(GoalsOptions),
  /// ゴールの詳細を表示する。
  Goal(GoalOptions),
  /// 使い方を表示して正常終了する。
  Help,
}

/// 判断待ちの一覧の表示に使う解決前の入力。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DecisionsOptions {
  pub(crate) server_id: ServerId,
  pub(crate) project:   ProjectName,
  pub(crate) addr:      Option<String>,
  pub(crate) config:    Option<PathBuf>,
}

/// 概要の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct OverviewOptions {
  pub(crate) addr:   Option<String>,
  pub(crate) config: Option<PathBuf>,
}

/// 会話の表示に使う解決前の入力。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ConversationOptions {
  pub(crate) project: ProjectName,
  pub(crate) session: Option<String>,
  pub(crate) addr:    Option<String>,
  pub(crate) config:  Option<PathBuf>,
}

/// ゴール一覧の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GoalsOptions {
  pub(crate) addr:   Option<String>,
  pub(crate) config: Option<PathBuf>,
}

/// ゴール詳細の表示に使う解決前の入力。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct GoalOptions {
  pub(crate) goal_id: String,
  pub(crate) addr:    Option<String>,
  pub(crate) config:  Option<PathBuf>,
}

/// 書き込みコマンドと、その接続先の解決前の入力。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct WriteInvocation {
  pub(crate) command: WriteCommand,
  pub(crate) addr:    Option<String>,
  pub(crate) config:  Option<PathBuf>,
}

/// 書き込みコマンド。引数は解釈時に検証済みです。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WriteCommand {
  /// 問い合わせへ回答する。`text` か `report` のどちらか一方を指定します。
  Answer {
    server_id:  ServerId,
    project:    ProjectName,
    goal_id:    Option<GoalId>,
    inquiry_id: InquiryId,
    answer:     Answer,
  },
  /// プルリクエストのマージを承認する。
  Merge {
    server_id: ServerId,
    project:   ProjectName,
    goal_id:   GoalId,
    pr_number: PullRequestNumber,
    until:     MergeUntil,
  },
  /// ゴール登録の承認待ちを一覧表示する。
  Registrations { server_id: ServerId, project: ProjectName },
  /// ゴール登録を承認する。
  ApproveRegistration { server_id: ServerId, project: ProjectName, revision: RegistrationRevision },
  /// ゴール登録を却下する。
  RejectRegistration { server_id: ServerId, project: ProjectName, revision: RegistrationRevision },
}

/// 問い合わせへの回答の種類。質問は自由記述（`text`）、作業の依頼は実施結果（`report`）を使います。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Answer {
  Text(String),
  Report(OperationAnswerReport),
}

impl WriteCommand {
  /// 失敗の表示に使う操作名。
  pub(crate) fn operation(&self) -> &'static str {
    match self {
      | WriteCommand::Answer { .. } => "回答の記録",
      | WriteCommand::Merge { .. } => "マージの承認",
      | WriteCommand::Registrations { .. } => "ゴール登録の承認待ちの取得",
      | WriteCommand::ApproveRegistration { .. } => "ゴール登録の承認",
      | WriteCommand::RejectRegistration { .. } => "ゴール登録の却下",
    }
  }
}
