//! run の追従（takt_get_run の取得と、workflow の step 位置の補完）。

use serde_json::{json, Value};

use super::{
  parse_remote_run, step_entries, step_log_entries, step_log_next_offset, Page, RemoteRun, ToolError, ToolSetCore,
  MAX_PAGES,
};
use crate::{
  domain::RunSlug,
  model::RunEntry,
  workflow_progress::{build_progress, DefinitionNode, ExecutionEvidence, WorkflowDefinition},
};

impl ToolSetCore<'_> {
  /// takt_get_run から run を読む。stepLogs は先頭のウィンドウだけが返るため、
  /// stepLogPage.nextOffset が残っていれば追加のページを取得して全件を集めます。
  pub async fn read_remote_run(&mut self, run_slug: &RunSlug) -> Result<RemoteRun, ToolError> {
    let mut value = self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str() })).await?;
    let summary = value.get("summary").and_then(Value::as_str).filter(|text| !text.is_empty());
    if summary.is_none() && value.pointer("/taskOmission/truncated").and_then(Value::as_bool) == Some(true) {
      value["taskText"] = json!(self.read_remote_task_text(run_slug).await?);
    }
    self.read_remote_run_steps(run_slug, value).await
  }

  /// ログ更新は要約用の本文取得に依存せず、ステップの全ページだけを読みます。
  pub(crate) async fn read_remote_run_logs(&mut self, run_slug: &RunSlug) -> Result<RemoteRun, ToolError> {
    let value = self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str() })).await?;
    self.read_remote_run_steps(run_slug, value).await
  }

  async fn read_remote_run_steps(&mut self, run_slug: &RunSlug, value: Value) -> Result<RemoteRun, ToolError> {
    let mut steps = step_entries(&value);
    let mut step_logs = step_log_entries(&value);
    let mut next_offset = step_log_next_offset(&value);
    let mut pages_taken = 0;
    while let Some(offset) = next_offset {
      if pages_taken >= MAX_PAGES {
        return Err(ToolError::Parse("takt_get_run の stepLogs 追従が上限に達しました".to_owned()));
      }
      pages_taken += 1;
      let page = self.call_json("takt_get_run", json!({ "runSlug": run_slug.as_str(), "offset": offset })).await?;
      steps.extend(step_entries(&page));
      step_logs.extend(step_log_entries(&page));
      next_offset = step_log_next_offset(&page);
    }
    let mut run = parse_remote_run(&value, steps, step_logs)?;
    if super::omissions::field_is_omitted(&value, "summary") {
      run.summary.push('…');
    }
    Ok(run)
  }

  /// workflow の step 名一覧から、run の現在ステップの位置と全ステップ数を補う。
  ///
  /// workflow を読めないときや現在ステップが一覧に無いときは、位置と総数を設定しません。
  /// 道具エラー・解析不能な応答は位置不明として扱い、run 一覧全体を失敗させません。
  pub(super) async fn enrich_step_progress(
    &mut self,
    run: &mut RunEntry,
    workflow: &str,
    evidence: &ExecutionEvidence,
  ) -> Result<(), ToolError> {
    let Some(definition) = self.workflow_definition(workflow).await? else {
      return Ok(());
    };
    run.progress =
      Some(build_progress(definition.clone(), evidence, workflow, run.current_step.as_deref(), run.status));
    let steps: Vec<_> = definition.nodes.iter().map(|node| node.name.clone()).collect();
    // meta.json の currentStep は takt_get_workflow が返す workflow の step 名に対応する。
    // ステップの記録の末尾は入れ子 workflow の内側の step になり得るため、位置の算出には使わない。
    // currentStep が無い run だけ、従来どおりステップの記録の末尾で補う。
    let Some(current) = run
      .current_step
      .as_deref()
      .filter(|name| !name.is_empty())
      .or_else(|| run.steps.iter().rev().find(|step| !step.step_name.is_empty()).map(|step| step.step_name.as_str()))
    else {
      return Ok(());
    };
    let Some(index) = steps.iter().position(|name| name == current) else {
      return Ok(());
    };
    let Some(step_number) = i32::try_from(index + 1).ok() else {
      return Ok(());
    };
    let Some(total_steps) = i32::try_from(steps.len()).ok() else {
      return Ok(());
    };
    run.current_step_number = Some(step_number);
    run.total_steps = Some(total_steps);
    Ok(())
  }

  /// workflow の step 名と分岐を持つ定義を取得する。同じ workflow の結果は再利用します。
  pub async fn workflow_definition(&mut self, workflow: &str) -> Result<Option<WorkflowDefinition>, ToolError> {
    if let Some(cached) = self.workflow_definitions.get(workflow) {
      return Ok(cached.clone());
    }
    let definition = match self.fetch_workflow_definition(workflow).await {
      | Ok(definition) => definition,
      | Err(ToolError::Transport(error)) => return Err(ToolError::Transport(error)),
      | Err(_) => None,
    };
    self.workflow_definitions.insert(workflow.to_owned(), definition.clone());
    Ok(definition)
  }

  async fn fetch_workflow_definition(&mut self, workflow: &str) -> Result<Option<WorkflowDefinition>, ToolError> {
    let mut nodes = Vec::new();
    let mut initial_step = None;
    let mut offset = 0;
    let mut total_count = None;
    for _ in 0..MAX_PAGES {
      let value = self.call_json("takt_get_workflow", json!({ "workflow": workflow, "offset": offset })).await?;
      if initial_step.is_none() {
        initial_step = value.get("initialStep").and_then(Value::as_str).map(str::to_owned);
      }
      total_count = total_count.or_else(|| value.get("totalCount").and_then(Value::as_u64).map(|count| count as usize));
      let page = Page::from_value(&value, "steps")?;
      for item in &page.items {
        let Some(node) = DefinitionNode::from_value(item) else {
          return Ok(None);
        };
        nodes.push(node);
      }
      match page.next_offset {
        | Some(next) => offset = next,
        | None => {
          if total_count.is_some_and(|total| total != nodes.len()) {
            return Ok(None);
          }
          return Ok(if nodes.is_empty() { None } else { Some(WorkflowDefinition { initial_step, nodes }) });
        },
      }
    }
    Err(ToolError::Parse("takt_get_workflow のページ追従が上限に達しました".to_owned()))
  }

  /// SSH から進行証拠を読み、remote の evidence を補完する。
  pub(crate) async fn enrich_remote_progress(&mut self, run_slug: &RunSlug, remote: &mut crate::mcp::RemoteRun) {
    let Some(config) = self.client.config() else {
      return;
    };
    match crate::remote_run::read_evidence(config, &self.cwd, run_slug, &remote.step_logs).await {
      | Ok(Some(evidence)) => remote.evidence = evidence,
      | Ok(None) => {},
      | Err(error) => {
        tracing::warn!(run_slug = %run_slug.as_str(), error = %error, "SSH から進行情報を読めませんでした。取得済みの run を返します。")
      },
    }
  }
}
