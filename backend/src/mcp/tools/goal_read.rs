//! ゴール詳細・タスク詳細の読み取りと、問い合わせの回答・ゴール登録の承認却下・マージの操作。

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{
  domain_value, is_missing_goal_field, optional_string, parse_goal_execution_status, parse_goal_question,
  parse_goal_status, parse_goal_work_log, parse_task_detail, registration_summary_text, required_i32, required_string,
  string_field, Page, TaskLinkFields, ToolError, ToolSet, MAX_PAGES, SPECIFICATION_NOTATIONS,
};
use crate::{
  domain::{GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, RegistrationRevision, TaskName},
  model::{GoalDetail, GoalQuestionEntry, GoalTaskEntry, GoalWorkLogEntry, RegistrationProposalEntry, TaskDetail},
};

impl ToolSet<'_> {
  /// 指定した goalId の詳細を takt_get_goal と補助道具から読み取る。
  ///
  /// 目的・対象外・受け入れ条件・制約・仕様は takt_get_goal の path/textOffset を全ページ追従して
  /// 全文を集めます。作業記録と質問は専用の一覧道具を全ページ追従します。紐づくタスクは
  /// takt_list_tasks を全ページ取得して goal_id で絞ります。
  pub async fn read_goal_detail(&mut self, goal_id: &GoalId) -> Result<GoalDetail, ToolError> {
    let value = self.call_json("takt_get_goal", json!({ "goalId": goal_id.as_str() })).await?;
    let goal =
      value.get("goal").ok_or_else(|| ToolError::Parse("takt_get_goal の応答に goal がありません".to_owned()))?;
    let returned_id = string_field(goal, "id")?;
    if returned_id != goal_id.as_str() {
      return Err(ToolError::Parse(format!("要求したゴール {goal_id} と応答のゴール {returned_id} が一致しません")));
    }
    let status = parse_goal_status(goal.get("status"))?;
    let execution_status = parse_goal_execution_status(goal.get("executionStatus"))?;
    let branch = optional_string(goal, "branch").unwrap_or_default();
    let integration_branch = optional_string(goal, "integrationBranch").unwrap_or_default();
    let objective = self.read_goal_text(goal_id.as_str(), "objective").await?;
    let out_of_scope = self.read_goal_array(goal_id.as_str(), "outOfScope").await?;
    let acceptance_criteria = self.read_goal_array(goal_id.as_str(), "acceptanceCriteria").await?;
    let constraints = self.read_goal_array(goal_id.as_str(), "constraints").await?;
    let mut specifications = BTreeMap::new();
    for notation in SPECIFICATION_NOTATIONS {
      let content = self.read_goal_text(goal_id.as_str(), &format!("specifications.{notation}")).await?;
      if !content.is_empty() {
        specifications.insert(notation.to_owned(), content);
      }
    }
    let work_log = self.read_goal_work_log(goal_id.as_str()).await?;
    let questions = self.read_goal_questions(goal_id.as_str()).await?;
    let tasks = self
      .list_tasks()
      .await?
      .into_iter()
      .filter(|task| task.goal_id.as_ref() == Some(goal_id))
      .map(|task| GoalTaskEntry { task_name: task.name, status: task.status, run_slug: task.run_slug })
      .collect();
    Ok(GoalDetail {
      goal_id: goal_id.clone(),
      status,
      execution_status,
      objective,
      out_of_scope,
      acceptance_criteria,
      constraints,
      specifications,
      branch,
      integration_branch,
      work_log,
      questions,
      tasks,
    })
  }

  /// takt_get_goal の文字列フィールドを textOffset で全ページ連結する。
  ///
  /// 空または省略された任意項目は空文字として扱います。`content` は必須項目ではないため、
  /// 応答に無ければ空文字です（実 takt-mcp は項目そのものが無い場合に
  /// `Selected field does not exist` の道具エラーを返します）。`content` が存在して型が
  /// 文字列でない場合は解析エラーにします。
  async fn read_goal_text(&mut self, goal_id: &str, path: &str) -> Result<String, ToolError> {
    let mut content = String::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value =
        match self.call_json("takt_get_goal", json!({ "goalId": goal_id, "path": path, "textOffset": offset })).await {
          | Ok(value) => value,
          | Err(ToolError::Tool(error)) if is_missing_goal_field(&error) => return Ok(content),
          | Err(error) => return Err(error),
        };
      if let Some(chunk) = value.get("content") {
        let chunk =
          chunk.as_str().ok_or_else(|| ToolError::Parse(format!("{path} の content が文字列ではありません")))?;
        content.push_str(chunk);
      }
      match value.get("nextOffset").and_then(Value::as_u64) {
        | Some(next) => offset = next,
        | None => return Ok(content),
      }
    }
    Err(ToolError::Parse(format!("takt_get_goal の {path} の追従が上限に達しました")))
  }

  /// takt_get_goal の配列フィールドを offset で全ページ集める。
  ///
  /// 空または省略された任意項目は空配列として扱います。`items` は必須項目ではないため、
  /// 応答に無ければ空配列です（実 takt-mcp は項目そのものが無い場合に
  /// `Selected field does not exist` の道具エラーを返します）。`items` が存在して配列でない
  /// 場合は解析エラーにします。
  async fn read_goal_array(&mut self, goal_id: &str, path: &str) -> Result<Vec<String>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value =
        match self.call_json("takt_get_goal", json!({ "goalId": goal_id, "path": path, "offset": offset })).await {
          | Ok(value) => value,
          | Err(ToolError::Tool(error)) if is_missing_goal_field(&error) => return Ok(entries),
          | Err(error) => return Err(error),
        };
      let Some(items) = value.get("items") else {
        return Ok(entries);
      };
      let items = items.as_array().ok_or_else(|| ToolError::Parse(format!("{path} の items が配列ではありません")))?;
      for item in items {
        let text = item.as_str().ok_or_else(|| ToolError::Parse(format!("{path} の要素が文字列ではありません")))?;
        entries.push(text.to_owned());
      }
      match value.get("nextOffset").and_then(Value::as_u64) {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse(format!("takt_get_goal の {path} の追従が上限に達しました")))
  }

  /// takt_list_goal_work_log を全ページ追従して作業記録を集める。
  async fn read_goal_work_log(&mut self, goal_id: &str) -> Result<Vec<GoalWorkLogEntry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_goal_work_log", json!({ "goalId": goal_id, "offset": offset })).await?;
      let page = Page::from_value(&value, "workLog")?;
      for item in &page.items {
        entries.push(parse_goal_work_log(item)?);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_goal_work_log のページ追従が上限に達しました".to_owned()))
  }

  /// takt_list_goal_questions を全ページ追従して質問を集める。
  async fn read_goal_questions(&mut self, goal_id: &str) -> Result<Vec<GoalQuestionEntry>, ToolError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_goal_questions", json!({ "goalId": goal_id, "offset": offset })).await?;
      let page = Page::from_value(&value, "questions")?;
      for item in &page.items {
        entries.push(parse_goal_question(item)?);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(entries),
      }
    }
    Err(ToolError::Parse("takt_list_goal_questions のページ追従が上限に達しました".to_owned()))
  }

  /// takt_get_goal（path なし）の goal summary から、タスク詳細に載せる
  /// ブランチとプルリクエスト URL を読む。
  async fn read_task_link_fields(&mut self, goal_id: &str) -> Result<TaskLinkFields, ToolError> {
    let value = self.call_json("takt_get_goal", json!({ "goalId": goal_id })).await?;
    let goal =
      value.get("goal").ok_or_else(|| ToolError::Parse("takt_get_goal の応答に goal がありません".to_owned()))?;
    Ok(TaskLinkFields {
      branch:           optional_string(goal, "branch"),
      pull_request_url: goal
        .get("pullRequest")
        .and_then(|pull_request| pull_request.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned),
    })
  }

  /// takt_list_tasks を全ページ取得し、タスク名（と任意の
  /// goalId）が一致する最初のタスク詳細を返す。
  ///
  /// 一致するタスクが goalId を持つ場合は、そのゴールの summary から
  /// ブランチとプルリクエスト URL を補います。
  /// 一致するタスクが無ければ Ok(None)、ページ追従が上限に達したら Parse エラーにします。
  pub async fn read_task_detail(
    &mut self,
    project: &ProjectName,
    task_name: &TaskName,
    goal_id: Option<&GoalId>,
  ) -> Result<Option<TaskDetail>, ToolError> {
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_list_tasks", json!({ "offset": offset })).await?;
      let page = Page::from_value(&value, "tasks")?;
      for item in &page.items {
        if item.get("name").and_then(Value::as_str) != Some(task_name.as_str()) {
          continue;
        }
        if let Some(goal_id) = goal_id {
          if item.get("goalId").and_then(Value::as_str) != Some(goal_id.as_str()) {
            continue;
          }
        }
        // goalId が空文字のタスクはゴール未所属（parse_task と同じ扱い）とし、
        // takt_get_goal を呼ばずに、ID が欠落する場合と同じ既定値を使う。
        let links = match optional_string(item, "goalId") {
          | Some(task_goal_id) if !task_goal_id.is_empty() => self.read_task_link_fields(&task_goal_id).await?,
          | _ => TaskLinkFields::default(),
        };
        return Ok(Some(parse_task_detail(project, item, links)?));
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => return Ok(None),
      }
    }
    Err(ToolError::Parse("takt_list_tasks のページ追従が上限に達しました".to_owned()))
  }

  /// 問い合わせへ回答する。`answer` は takt の `answer` オブジェクト（例: `{"text": "..."}`）。
  /// 応答の `questionId` を返します。takt の入力契約では `goalId` は必須（UUID）なので、
  /// `goal_id` は必須の引数として受け取ります。
  pub async fn answer_goal_question(
    &mut self,
    goal_id: &GoalId,
    question_id: &InquiryId,
    answer: Value,
  ) -> Result<InquiryId, ToolError> {
    let value = self
      .call_json(
        "takt_answer_goal_question",
        json!({ "goalId": goal_id.as_str(), "questionId": question_id.as_str(), "answer": answer }),
      )
      .await?;
    let question_id = optional_string(&value, "questionId")
      .ok_or_else(|| ToolError::Parse("takt_answer_goal_question の応答に questionId がありません".to_owned()))?;
    domain_value("inquiry ID", InquiryId::try_from(question_id.as_str()))
  }

  /// ゴール登録の承認待ちを取得する。提案が無ければ空の並びを返します。
  pub async fn list_goal_registration_proposals(&mut self) -> Result<Vec<RegistrationProposalEntry>, ToolError> {
    let value = self.call_json("takt_list_goal_registration_proposals", json!({})).await?;
    let proposals = value.get("proposals").and_then(Value::as_array).ok_or_else(|| {
      ToolError::Parse("takt_list_goal_registration_proposals の応答に proposals の配列がありません".to_owned())
    })?;
    proposals
      .iter()
      .map(|proposal| {
        Ok(RegistrationProposalEntry {
          revision:             domain_value(
            "revision",
            RegistrationRevision::try_from(required_i32(proposal, "revision")?),
          )?,
          repository_directory: required_string(proposal, "repositoryDirectory")?,
          summary:              registration_summary_text(proposal.get("summary"))?,
        })
      })
      .collect()
  }

  /// ゴール登録の承認待ちを承認する。承認した revision と、登録されたゴールの ID を返します。
  pub async fn approve_goal_registration(
    &mut self,
    revision: RegistrationRevision,
  ) -> Result<ApproveGoalRegistrationResult, ToolError> {
    let value = self.call_json("takt_approve_goal_registration", json!({ "revision": revision.get() })).await?;
    let goal = value.get("goal").ok_or_else(|| ToolError::Parse("応答に goal がありません".to_owned()))?;
    let goal_id = domain_value("ゴール ID", GoalId::try_from(required_string(goal, "id")?.as_str()))?;
    let revision = domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))?;
    Ok(ApproveGoalRegistrationResult { revision, goal_id })
  }

  /// ゴール登録の承認待ちを却下する。却下した revision を返します。
  pub async fn reject_goal_registration(
    &mut self,
    revision: RegistrationRevision,
  ) -> Result<RegistrationRevision, ToolError> {
    let value = self.call_json("takt_reject_goal_registration", json!({ "revision": revision.get() })).await?;
    domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))
  }

  /// プルリクエストのマージを承認する。依頼したタスク名とワークフローを返します。
  pub async fn merge_goal_pr(
    &mut self,
    goal_id: &GoalId,
    pr_number: PullRequestNumber,
    until: MergeUntil,
  ) -> Result<MergeGoalPrResult, ToolError> {
    let value = self
      .call_json(
        "takt_merge_goal_pr",
        json!({ "goalId": goal_id.as_str(), "prNumber": pr_number.get(), "until": until.as_str() }),
      )
      .await?;
    Ok(MergeGoalPrResult {
      task_name: domain_value("タスク名", TaskName::try_from(required_string(&value, "taskName")?.as_str()))?,
      workflow:  required_string(&value, "workflow")?,
    })
  }
}

/// takt_approve_goal_registration の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveGoalRegistrationResult {
  pub revision: RegistrationRevision,
  pub goal_id:  GoalId,
}

/// takt_merge_goal_pr の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeGoalPrResult {
  pub task_name: TaskName,
  pub workflow:  String,
}
