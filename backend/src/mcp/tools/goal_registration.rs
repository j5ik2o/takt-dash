//! ゴール登録案の道具（一覧、承認、却下）と、登録案の本文の組み立て。
//!
//! takt-mcp は、登録案の持ち主の会話（conversationId）を一覧で返し、承認・却下と本文の読み取りで
//! 必須にします。登録案を読む道具はこのモジュールに置きます。

use serde_json::{json, Value};

use super::{
  parse::{domain_value, required_i32, required_string},
  ToolError, ToolSetCore,
};
use crate::{
  domain::{ConversationId, GoalId, RegistrationRevision},
  model::{ProposalSource, RegistrationProposalEntry},
};

impl ToolSetCore<'_> {
  /// ゴール登録の承認待ちを取得する。提案が無ければ空の並びを返します。
  pub async fn list_goal_registration_proposals(&mut self) -> Result<Vec<RegistrationProposalEntry>, ToolError> {
    let value = self.call_json("takt_list_goal_registration_proposals", json!({})).await?;
    let proposals = value.get("proposals").and_then(Value::as_array).ok_or_else(|| {
      ToolError::Parse("takt_list_goal_registration_proposals の応答に proposals の配列がありません".to_owned())
    })?;
    let source = ProposalSource { connection: self.client.connection_key(), cwd: self.cwd.clone() };
    proposals
      .iter()
      .map(|proposal| {
        Ok(RegistrationProposalEntry {
          source:               source.clone(),
          revision:             domain_value(
            "revision",
            RegistrationRevision::try_from(required_i32(proposal, "revision")?),
          )?,
          conversation_id:      domain_value(
            "conversation ID",
            ConversationId::try_from(required_string(proposal, "conversationId")?.as_str()),
          )?,
          repository_directory: required_string(proposal, "repositoryDirectory")?,
          summary:              registration_summary_text(proposal.get("summary"))?,
        })
      })
      .collect()
  }

  /// ゴール登録の承認待ちを承認する。承認した revision と、登録されたゴールの ID を返します。
  ///
  /// `conversation_id` は、一覧が返した登録案の持ち主です。takt-mcp は必須の入力として受け取り、
  /// 持ち主でなければ断ります。
  pub async fn approve_goal_registration(
    &mut self,
    revision: RegistrationRevision,
    conversation_id: &ConversationId,
  ) -> Result<ApproveGoalRegistrationResult, ToolError> {
    let value = self
      .call_json(
        "takt_approve_goal_registration",
        json!({ "revision": revision.get(), "conversationId": conversation_id.as_str() }),
      )
      .await?;
    let goal = value.get("goal").ok_or_else(|| ToolError::Parse("応答に goal がありません".to_owned()))?;
    let goal_id = domain_value("ゴール ID", GoalId::try_from(required_string(goal, "id")?.as_str()))?;
    let revision = domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))?;
    Ok(ApproveGoalRegistrationResult { revision, goal_id })
  }

  /// ゴール登録の承認待ちを却下する。却下した revision を返します。
  /// `conversation_id` の扱いは承認と同じです。
  pub async fn reject_goal_registration(
    &mut self,
    revision: RegistrationRevision,
    conversation_id: &ConversationId,
  ) -> Result<RegistrationRevision, ToolError> {
    let value = self
      .call_json(
        "takt_reject_goal_registration",
        json!({ "revision": revision.get(), "conversationId": conversation_id.as_str() }),
      )
      .await?;
    domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))
  }
}

/// takt_approve_goal_registration の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveGoalRegistrationResult {
  pub revision: RegistrationRevision,
  pub goal_id:  GoalId,
}

/// ゴール登録提案の `summary` を表示本文へ変換する。
///
/// 文字列はそのまま本文にします。オブジェクトは既知の項目だけを日本語の文として組み立て、
/// 未知のキーと欠落は無視します。どちらでもない値だけを解析エラーにします。
fn registration_summary_text(value: Option<&Value>) -> Result<String, ToolError> {
  match value {
    | None | Some(Value::Null) => Ok(String::new()),
    | Some(Value::String(text)) => Ok(text.clone()),
    | Some(Value::Object(object)) => Ok(compose_registration_summary(object)),
    | Some(_) => Err(ToolError::Parse("summary が文字列でもオブジェクトでもありません".to_owned())),
  }
}

/// `summary` オブジェクトから本文へ含める既知の項目と見出し。
const REGISTRATION_SUMMARY_FIELDS: [(&str, &str); 6] = [
  ("objective", "目的"),
  ("outOfScope", "対象外"),
  ("acceptanceCriteria", "受入条件"),
  ("constraints", "制約"),
  ("startBranch", "開始ブランチ"),
  ("integrationBranch", "統合ブランチ"),
];

fn compose_registration_summary(object: &serde_json::Map<String, Value>) -> String {
  let mut lines = Vec::new();
  for (key, label) in REGISTRATION_SUMMARY_FIELDS {
    if let Some(text) = registration_summary_field(object.get(key)) {
      lines.push(format!("{label}: {text}"));
    }
  }
  lines.join("\n")
}

/// `summary` オブジェクトの項目値を本文用の文字列にする。文字列と文字列の配列だけを受け付け、
/// それ以外の型と空の配列は無視します。
fn registration_summary_field(value: Option<&Value>) -> Option<String> {
  match value? {
    | Value::String(text) => Some(text.clone()),
    | Value::Array(items) => {
      let texts: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
      if texts.is_empty() {
        None
      } else {
        Some(texts.join("、"))
      }
    },
    | _ => None,
  }
}
