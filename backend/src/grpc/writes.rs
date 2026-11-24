//! 書き込み系 RPC（AnswerInquiry・ApproveGoalMerge・ApproveGoalRegistration・
//! RejectGoalRegistration）の 入力の検証と、takt の書き込み道具を呼ぶ処理の組み立て。
//!
//! 各関数は入力を検証して対象を解決し、書き込みそのものを future として返します。future は
//! サービスを借用しないため、台帳（[`super::write_ledger`]）が呼び出しの打ち切りと切り離して
//! 最後まで実行できます。

use std::future::Future;

use tonic::Status;

use super::{answer_payload, write_failure, write_ledger::WriteFailure, TaktDashServiceImpl};
use crate::{
  domain::{GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, ServerId},
  model,
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
    // takt_answer_goal_question の入力契約では goalId は必須（UUID）です。要求が明示した
    // ゴールを優先し、無ければ引き当てた判断待ちのゴールを使います。要求の値の誤りは入力の
    // 誤り、状態ストア由来の値の誤りは内部の不整合として扱います。
    let requested_goal_id = request.goal_id.clone().filter(|goal_id| !goal_id.trim().is_empty());
    let goal_id = match requested_goal_id {
      | Some(goal_id) => GoalId::try_from(goal_id.as_str())
        .map_err(|error| Status::invalid_argument(format!("goal_id が不正です: {error}")))?,
      | None => {
        let goal_id = pending.goal_id.clone().filter(|goal_id| !goal_id.trim().is_empty()).ok_or_else(|| {
          Status::invalid_argument("goal_id を特定できません。ゴールに属さない問い合わせには回答できません。")
        })?;
        GoalId::try_from(goal_id.as_str())
          .map_err(|error| Status::internal(format!("状態ストアの goal_id が不正です: {error}")))?
      },
    };
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
      let answered = lane.settle(
        source.answer_goal_question(&lane.resolved.cwd, &goal_id, &question_id, answer).await.map_err(write_failure),
      );
      if !matches!(answered, Err(WriteFailure::Failed(_))) {
        // 回答した、または回答が届いたかもしれないゴールは、
        // 終端でも次の周期で質問を読み直させます。
        store.invalidate_goal_questions(lane.resolved.project.as_str(), goal_id);
      }
      answered?;
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
      let result = lane
        .settle(source.merge_goal_pr(&lane.resolved.cwd, &goal_id, pr_number, until).await.map_err(write_failure))?;
      Ok(ApproveGoalMergeResponse { task_name: result.task_name.as_str().to_owned(), workflow: result.workflow })
    })
  }

  /// ApproveGoalRegistration の入力を検証し、takt_approve_goal_registration
  /// を呼ぶ書き込みを組み立てる。
  pub(super) fn prepare_registration_approval(
    &self,
    request: ApproveGoalRegistrationRequest,
  ) -> Result<impl Future<Output = Result<ApproveGoalRegistrationResponse, WriteFailure>> + Send + 'static, Status> {
    if request.revision <= 0 {
      return Err(Status::invalid_argument("revision は正の整数で指定してください。"));
    }
    let revision = RegistrationRevision::try_from(request.revision)
      .map_err(|error| Status::invalid_argument(format!("revision が不正です: {error}")))?;
    if request.server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    let server_id = ServerId::try_from(request.server_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let lane = self.write_lane(self.resolve_write_target(&server_id, &project)?);
    Ok(async move {
      let _write_gate = lane.gate().await;
      let mut source = lane.connected_source().await?;
      let result =
        lane.settle(source.approve_goal_registration(&lane.resolved.cwd, revision).await.map_err(write_failure))?;
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
    if request.revision <= 0 {
      return Err(Status::invalid_argument("revision は正の整数で指定してください。"));
    }
    let revision = RegistrationRevision::try_from(request.revision)
      .map_err(|error| Status::invalid_argument(format!("revision が不正です: {error}")))?;
    if request.server_id.trim().is_empty() {
      return Err(Status::invalid_argument("server_id が空です。"));
    }
    if request.project.trim().is_empty() {
      return Err(Status::invalid_argument("project が空です。"));
    }
    let server_id = ServerId::try_from(request.server_id.as_str())
      .map_err(|error| Status::invalid_argument(format!("server_id が不正です: {error}")))?;
    let project = ProjectName::try_from(request.project.as_str())
      .map_err(|error| Status::invalid_argument(format!("project が不正です: {error}")))?;
    let lane = self.write_lane(self.resolve_write_target(&server_id, &project)?);
    Ok(async move {
      let _write_gate = lane.gate().await;
      let mut source = lane.connected_source().await?;
      let rejected =
        lane.settle(source.reject_goal_registration(&lane.resolved.cwd, revision).await.map_err(write_failure))?;
      Ok(RejectGoalRegistrationResponse { revision: rejected.get() })
    })
  }
}
