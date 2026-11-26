//! 書き込み系 RPC（AnswerInquiry・ApproveGoalMerge・ApproveGoalRegistration・
//! RejectGoalRegistration）の 入力の検証と、takt の書き込み道具を呼ぶ処理の組み立て。
//!
//! 各関数は入力を検証して対象を解決し、書き込みそのものを future として返します。future は
//! サービスを借用しないため、台帳（[`super::write_ledger`]）が呼び出しの打ち切りと切り離して
//! 最後まで実行できます。

use std::future::Future;

use tonic::Status;

use super::{
  answer_payload,
  failure::{log_detail, with_takt_reason},
  lanes::WriteLane,
  write_failure,
  write_ledger::WriteFailure,
  TaktDashServiceImpl,
};
use crate::{
  domain::{
    ConversationId, GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, ServerId,
  },
  mcp::{connection_key, tools::ToolError},
  model,
  model::ProposalSource,
  store::StateStore,
  takt_dash::v1::{
    AnswerInquiryRequest, AnswerInquiryResponse, ApproveGoalMergeRequest, ApproveGoalMergeResponse,
    ApproveGoalRegistrationRequest, ApproveGoalRegistrationResponse, RejectGoalRegistrationRequest,
    RejectGoalRegistrationResponse,
  },
};

impl TaktDashServiceImpl {
  /// AnswerInquiry の入力を検証し、takt_answer_goal_question を呼ぶ書き込みを組み立てる。
  pub(super) fn prepare_answer(
    &self,
    request: AnswerInquiryRequest,
  ) -> Result<impl Future<Output = Result<AnswerInquiryResponse, WriteFailure>> + Send + 'static, Status> {
    if request.inquiry_id.trim().is_empty() {
      return Err(Status::invalid_argument("inquiry_id が空です。"));
    }
    if request.server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    // 保留項目は (server_id, project, inquiry_id) の3条件一致で引き当てる。同じ id が別の
    // プロジェクトや別のサーバーにあっても、対象の判断だけを選ぶ。
    let matches: Vec<model::PendingDecision> = self
      .store
      .pending_decisions(self.now(), model::PendingDecisionFilter::Inquiry)
      .into_iter()
      .filter(|decision| {
        decision.server_id == request.server_id
          && decision.project == request.project
          && decision.inquiry_id.as_ref().map(InquiryId::as_str) == Some(request.inquiry_id.as_str())
      })
      .collect();
    let pending = match matches.len() {
      | 0 => None,
      | 1 => matches.into_iter().next(),
      | _ => {
        return Err(Status::invalid_argument(format!(
          "問い合わせ {} はサーバー {} のプロジェクト {} 内で一意に特定できません。",
          request.inquiry_id, request.server_id, request.project
        )));
      },
    };
    let inquiry_kind = pending.as_ref().and_then(|decision| decision.inquiry_kind);
    let answer = answer_payload(inquiry_kind, request.text, request.report)?;
    // 保留項目を引き当てられない問い合わせは、要求がゴールを明示していても回答しません。
    // 回答本文の検証を先に済ませることで、空の回答は入力の誤りとして扱います。
    let Some(pending) = pending else {
      return Err(Status::not_found(format!(
        "問い合わせ {} はサーバー {} のプロジェクト {} にありません。",
        request.inquiry_id, request.server_id, request.project
      )));
    };
    // takt_answer_goal_question の入力契約では goalId は必須（UUID）です。書き込み先は、引き当てた
    // 判断待ちのゴールに限ります。要求が goal_id を明示したときは、判断待ちのゴールと照合し、
    // 食い違えば takt へ送らずに断ります（issue #319）。要求の値の誤りは入力の誤り、状態ストア
    // 由来の値の誤りは内部の不整合として扱います。
    let requested_goal_id = request
      .goal_id
      .as_deref()
      .filter(|goal_id| !goal_id.trim().is_empty())
      .map(GoalId::try_from)
      .transpose()
      .map_err(|error| Status::invalid_argument(format!("goal_id が不正です: {error}")))?;
    let pending_goal_id = pending
      .goal_id
      .as_deref()
      .filter(|goal_id| !goal_id.trim().is_empty())
      .map(GoalId::try_from)
      .transpose()
      .map_err(|error| Status::internal(format!("状態ストアの goal_id が不正です: {error}")))?;
    if requested_goal_id.as_ref().is_some_and(|requested| Some(requested) != pending_goal_id.as_ref()) {
      return Err(Status::failed_precondition(format!(
        "問い合わせ {} は、指定されたゴールの判断待ちではありません。画面を読み込み直してから、もう一度操作してください。",
        request.inquiry_id
      )));
    }
    let goal_id = pending_goal_id.ok_or_else(|| {
      Status::invalid_argument("goal_id を特定できません。ゴールに属さない問い合わせには回答できません。")
    })?;
    let question_id = InquiryId::try_from(request.inquiry_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("inquiry_id が不正です: {error}")))?;
    let server_id = ServerId::try_from(request.server_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let lane = self.write_lane(self.resolve_write_target(&server_id, &project)?);
    let store = self.store.clone();
    Ok(async move {
      let _write_gate = lane.gate().await;
      let mut source = lane.connected_source().await?;
      let answered =
        source.answer_goal_question(&lane.resolved.cwd, &goal_id, &question_id, answer).await.map_err(write_failure);
      if !matches!(answered, Err(WriteFailure::Failed(_))) {
        // 回答した、または回答が届いたかもしれないゴールは、
        // 終端でも次の周期で質問を読み直させます。
        store.invalidate_goal_questions(lane.resolved.project.as_str(), goal_id);
      }
      answered?;
      lane.record_write();
      Ok(AnswerInquiryResponse { inquiry_id: question_id.as_str().to_owned() })
    })
  }

  /// ApproveGoalMerge の入力を検証し、takt_merge_goal_pr を呼ぶ書き込みを組み立てる。
  pub(super) fn prepare_merge(
    &self,
    request: ApproveGoalMergeRequest,
  ) -> Result<impl Future<Output = Result<ApproveGoalMergeResponse, WriteFailure>> + Send + 'static, Status> {
    if request.goal_id.trim().is_empty() {
      return Err(Status::invalid_argument("goal_id が空です。"));
    }
    if request.pr_number <= 0 {
      return Err(Status::invalid_argument("pr_number は正の整数で指定してください。"));
    }
    if !matches!(request.until.as_str(), "prepare" | "merge") {
      return Err(Status::invalid_argument("until は \"prepare\" か \"merge\" を指定してください。"));
    }
    if request.server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    let goal_id = GoalId::try_from(request.goal_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("goal_id が不正です: {error}")))?;
    let pr_number = PullRequestNumber::try_from(request.pr_number)
      .map_err(|error| Status::invalid_argument(format!("pr_number が不正です: {error}")))?;
    let until = MergeUntil::try_from(request.until.as_str())
      .map_err(|error| Status::invalid_argument(format!("until が不正です: {error}")))?;
    let server_id = ServerId::try_from(request.server_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let lane = self.write_lane(self.resolve_write_target(&server_id, &project)?);
    Ok(async move {
      let _write_gate = lane.gate().await;
      let mut source = lane.connected_source().await?;
      let result = source.merge_goal_pr(&lane.resolved.cwd, &goal_id, pr_number, until).await.map_err(write_failure)?;
      lane.record_write();
      Ok(ApproveGoalMergeResponse { task_name: result.task_name.as_str().to_owned(), workflow: result.workflow })
    })
  }

  /// ApproveGoalRegistration の入力を検証し、takt_approve_goal_registration
  /// を呼ぶ書き込みを組み立てる。
  pub(super) fn prepare_registration_approval(
    &self,
    request: ApproveGoalRegistrationRequest,
  ) -> Result<impl Future<Output = Result<ApproveGoalRegistrationResponse, WriteFailure>> + Send + 'static, Status> {
    let target = self.registration_target(request.revision, &request.server_id, &request.project)?;
    Ok(async move {
      let RegistrationTarget { revision, ref lane, .. } = target;
      let _write_gate = lane.gate().await;
      let conversation_id = target.check_pending()?;
      let mut source = lane.connected_source().await?;
      let result = source
        .approve_goal_registration(&lane.resolved.cwd, revision, &conversation_id)
        .await
        .map_err(registration_write_failure)?;
      lane.record_write();
      Ok(ApproveGoalRegistrationResponse {
        revision: result.revision.get(),
        goal_id:  result.goal_id.as_str().to_owned(),
      })
    })
  }

  /// RejectGoalRegistration の入力を検証し、takt_reject_goal_registration
  /// を呼ぶ書き込みを組み立てる。
  pub(super) fn prepare_registration_rejection(
    &self,
    request: RejectGoalRegistrationRequest,
  ) -> Result<impl Future<Output = Result<RejectGoalRegistrationResponse, WriteFailure>> + Send + 'static, Status> {
    let target = self.registration_target(request.revision, &request.server_id, &request.project)?;
    Ok(async move {
      let RegistrationTarget { revision, ref lane, .. } = target;
      let _write_gate = lane.gate().await;
      let conversation_id = target.check_pending()?;
      let mut source = lane.connected_source().await?;
      let rejected = source
        .reject_goal_registration(&lane.resolved.cwd, revision, &conversation_id)
        .await
        .map_err(registration_write_failure)?;
      lane.record_write();
      Ok(RejectGoalRegistrationResponse { revision: rejected.get() })
    })
  }

  /// ゴール登録の承認・却下の対象を検証し、書き込み先を解決する。
  ///
  /// 承認待ちの一覧との照合（[`RegistrationTarget::check_pending`]）は、書き込みゲートを
  /// 取ってから行います。
  fn registration_target(&self, revision: i32, server_id: &str, project: &str) -> Result<RegistrationTarget, Status> {
    if revision <= 0 {
      return Err(Status::invalid_argument("revision は正の整数で指定してください。"));
    }
    let revision = RegistrationRevision::try_from(revision)
      .map_err(|error| Status::invalid_argument(format!("revision が不正です: {error}")))?;
    if server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    let server_id = ServerId::try_from(server_id)
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(project)
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let lane = self.write_lane(self.resolve_write_target(&server_id, &project)?);
    Ok(RegistrationTarget { revision, server_id, project, lane, store: self.store.clone() })
  }
}

/// ゴール登録の承認・却下の、検証済みの対象。
struct RegistrationTarget {
  revision:  RegistrationRevision,
  server_id: ServerId,
  project:   ProjectName,
  lane:      WriteLane,
  store:     std::sync::Arc<StateStore>,
}

impl RegistrationTarget {
  /// revision を、状態ストアが持つ対象プロジェクトの承認待ちと照合する（issue #319）。
  ///
  /// 書き込みゲートを保持したまま呼びます。定期読み取りは同じゲートを保持して一覧を反映する
  /// ため、照合から送信までのあいだに一覧は変わりません。
  ///
  /// 照合に使うのは、書き込み先と同じ接続先・同じ cwd から読んだ承認待ちだけです。設定で
  /// 接続先や cwd を切り替えた直後は、状態ストアに切替前の一覧が残ります。revision は cwd ごとの
  /// 小さな整数なので、切替前の一覧の revision を切替後の接続先へ送ると、同じ番号の別の登録案に
  /// 当たりえます。
  ///
  /// 一覧に無い revision（作り直される前の登録案、決着済みの登録案、まだ読めていない
  /// プロジェクト）は、takt へ送らずに断ります。状態ストアが takt より古いときの最終の判定は
  /// takt-mcp が行います。
  ///
  /// 照合で引き当てた登録案の持ち主（conversationId）を返します。takt-mcp は、承認・却下で
  /// この値を必須にします（issue #363）。
  fn check_pending(&self) -> Result<ConversationId, Status> {
    let resolved = &self.lane.resolved;
    let source = ProposalSource { connection: connection_key(&resolved.connection), cwd: resolved.cwd.clone() };
    let listed: Vec<_> = self
      .store
      .registration_proposals(&self.server_id, &self.project)
      .into_iter()
      .filter(|proposal| proposal.revision == self.revision)
      .collect();
    if let Some(proposal) = listed.iter().find(|proposal| proposal.source == source) {
      return Ok(proposal.conversation_id.clone());
    }
    let reason = if listed.is_empty() {
      "いまの承認待ちにありません"
    } else {
      "接続先の設定が変わる前に読んだものです"
    };
    Err(Status::failed_precondition(format!(
      "ゴール登録案 revision {} は、{reason}。画面を読み込み直してから、もう一度操作してください。",
      self.revision.get()
    )))
  }
}

/// ゴール登録の承認・却下の失敗を、台帳の扱いと gRPC のステータスへ変換する。
///
/// takt-mcp が「登録案が変わった」「会話が登録案の持ち主でない」と断ったときは、状態ストアの
/// 承認待ちが takt より古かったことを表します。書き込みは適用されていないので、一覧との
/// 食い違い（issue #319）と同じ FAILED_PRECONDITION で返します。
pub(super) fn registration_write_failure(error: ToolError) -> WriteFailure {
  match error {
    | ToolError::Tool(error) if is_stale_registration(&error.message) => {
      log_detail("takt-mcp がゴール登録案の食い違いで書き込みを断りました。", &error);
      WriteFailure::Failed(Status::failed_precondition(with_takt_reason(
        "ゴール登録案は、作り直されたか、すでに承認・却下されています。画面を読み込み直してから、もう一度操作してください。"
          .to_owned(),
        &error.message,
      )))
    },
    | error => write_failure(error),
  }
}

/// takt-mcp の道具エラーの文言が、登録案の食い違いを表すかどうか。
///
/// `stale_summary` は manager の会話が返すエラーコード、後者は takt-mcp が持ち主を照合して返す
/// 文言です（takt-custom features/mcp/goalRegistrationProposalOperations）。
fn is_stale_registration(message: &str) -> bool {
  message.contains("stale_summary") || message.contains("does not own the pending registration proposal")
}
