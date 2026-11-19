//! 設定済み SSH 接続で run の元記録を読む。MCP の公開応答に無い実行証拠だけを補完する。

use std::{
  path::{Path, PathBuf},
  process::Stdio,
};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  process::Command,
  time::timeout,
};

use crate::{
  domain::RunSlug,
  mcp::McpClientConfig,
  model::StepLogEntry,
  run_log::{parse_task_entries, run_paths, task_for_run},
  workflow_progress::{execution_counts, ExecutionEvidence},
};

const READER: &str = include_str!("remote_run_read.cjs");
const MAX_RESPONSE: u64 = 32 * 1024 * 1024;

/// 接続オプションと宛先までを取り出し、後続の MCP 起動コマンドを実行しない。
fn connection_args(config: &McpClientConfig) -> Result<Option<Vec<String>>, String> {
  if !config.is_ssh() {
    return Ok(None);
  }
  let mut index = 0;
  while let Some(arg) = config.args.get(index) {
    if arg == "--" {
      index += 1;
      break;
    }
    if !arg.starts_with('-') {
      break;
    }
    if arg == "-" {
      return Err("SSH の接続先が不正です".to_owned());
    }
    let mut options = arg[1..].chars();
    while let Some(option) = options.next() {
      if "BbcDEeFIiJLlmOopQRSWw".contains(option) {
        if options.as_str().is_empty() {
          index += 1;
        }
        break;
      }
      // -n/-f は stdin の要求を遮断し、-N/-s は固定の読取コマンドを実行しない。
      if !"46AaCqTtvXxYy".contains(option) {
        return Err(format!("記録読取に使えない SSH オプションです: -{option}"));
      }
    }
    index += 1;
  }
  let destination = config.args.get(index).ok_or("SSH の接続先がありません")?;
  if destination.starts_with('-') || destination.is_empty() {
    return Err("SSH の接続先が不正です".to_owned());
  }
  Ok(Some(config.args[..=index].to_vec()))
}

fn shell_quote(value: &str) -> String {
  format!("'{}'", value.replace('\'', "'\\''"))
}

async fn request(config: &McpClientConfig, connection: &[String], input: Value) -> Result<Value, String> {
  let node = format!("exec node -e {}", shell_quote(READER));
  let remote_command = format!("/bin/sh -lc {}", shell_quote(&node));
  let mut child = Command::new(&config.command)
    .args(connection)
    .arg(remote_command)
    .envs(&config.env)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true)
    .spawn()
    .map_err(|error| format!("SSH 読取を起動できません: {error}"))?;
  let result = timeout(config.request_timeout, async {
    let mut stdin = child.stdin.take().ok_or("SSH の標準入力がありません")?;
    stdin
      .write_all(&serde_json::to_vec(&input).map_err(|error| error.to_string())?)
      .await
      .map_err(|error| error.to_string())?;
    drop(stdin);
    let stdout = child.stdout.take().ok_or("SSH の標準出力がありません")?;
    let mut bytes = Vec::new();
    stdout.take(MAX_RESPONSE + 1).read_to_end(&mut bytes).await.map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_RESPONSE {
      return Err("SSH 読取の応答が上限を超えています".to_owned());
    }
    if !child.wait().await.map_err(|error| error.to_string())?.success() {
      return Err("SSH 側で記録を読めませんでした".to_owned());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("SSH 読取の応答が不正です: {error}"))
  })
  .await;
  match result {
    | Ok(value) => value,
    | Err(_) => Err("SSH 記録読取が期限を超えました".to_owned()),
  }
}

#[derive(Deserialize)]
struct Tasks {
  project: PathBuf,
  tasks:   String,
}

#[derive(Deserialize)]
struct Records {
  counts:  Option<Value>,
  records: Vec<Value>,
}

pub(crate) async fn read_evidence(
  config: &McpClientConfig,
  project: &Path,
  slug: &RunSlug,
  logs: &[StepLogEntry],
) -> Result<Option<ExecutionEvidence>, String> {
  let Some(connection) = connection_args(config)? else {
    return Ok(None);
  };
  let tasks: Tasks =
    serde_json::from_value(request(config, &connection, json!({"operation":"tasks", "project":project})).await?)
      .map_err(|error| error.to_string())?;
  let entries = parse_task_entries(&tasks.tasks)?;
  let entry = task_for_run(&entries, slug.as_str()).ok_or("対応表に run がありません")?;
  let paths = run_paths(entry, slug.as_str(), &tasks.project)?;
  let data: Records = serde_json::from_value(
    request(config, &connection, json!({"operation":"records", "project":tasks.project, "paths":paths})).await?,
  )
  .map_err(|error| error.to_string())?;
  let mut evidence = ExecutionEvidence { counts: execution_counts(data.counts), ..Default::default() };
  for record in data.records {
    if !evidence.history_complete && record.get("type").and_then(Value::as_str) != Some("workflow_start") {
      continue;
    }
    evidence.ingest(&record, None);
  }
  evidence.bind_logs(logs);
  Ok(Some(evidence))
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn separates_connection_options_from_the_remote_mcp_command() {
    let config = McpClientConfig {
      command:         "/usr/bin/ssh".to_owned(),
      args:            vec![
        "-p".into(),
        "2222".into(),
        "-oBatchMode=yes".into(),
        "server".into(),
        "exec takt mcp".into(),
      ],
      env:             Default::default(),
      cwd:             PathBuf::from("/project"),
      request_timeout: std::time::Duration::from_secs(1),
      tool_timeouts:   Default::default(),
    };
    assert_eq!(connection_args(&config).unwrap().unwrap(), config.args[..4]);
    let mut invalid = config.clone();
    invalid.args = vec!["-n".into(), "server".into()];
    assert!(connection_args(&invalid).is_err());
    invalid.command = "node".into();
    assert_eq!(connection_args(&invalid).unwrap(), None);
    assert_eq!(shell_quote("a'$(touch marker)"), "'a'\\''$(touch marker)'");
  }
}
