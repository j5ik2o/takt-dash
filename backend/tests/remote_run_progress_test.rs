//! SSH トランスポート以外は実物を使い、生の記録から通常の概要・詳細応答までを確認する。
mod common;
mod grpc_harness;

use std::{
  fs,
  os::unix::fs::PermissionsExt,
  path::{Path, PathBuf},
};

use common::*;
use grpc_harness::{client, start_server, GrpcHarness};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use takt_dash_backend::{
  model::ProjectSnapshot,
  poller::Poller,
  takt_dash::v1::{
    get_run_content_request::Target, GetOverviewRequest, GetRunContentRequest, GetRunDetailRequest,
    GetRunUpdatesRequest, WorkflowNodeState,
  },
};

fn definition() -> Vec<Value> {
  vec![
    json!({"name":"start","rules":[{"next":"develop"}]}),
    json!({"name":"develop","kind":"workflow_call","rules":[{"next":"gate"}]}),
    json!({"name":"gate","rules":[{"next":"fix_gate_fmt"},{"next":"fix_gate_test"},{"next":"finish"},{"next":"ABORT"}]}),
    json!({"name":"finish","rules":[{"next":"gate"},{"next":"COMPLETE"}]}),
    json!({"name":"fix_gate_fmt","rules":[{"next":"gate"}]}),
    json!({"name":"fix_gate_test","rules":[{"next":"gate"}]}),
  ]
}

fn event(kind: &str, step: &str, iteration: i32, rule: Option<i32>) -> Value {
  json!({"type":kind,"workflow":"outer","step":step,"iteration":iteration,"matchedRuleIndex":rule,
    "timestamp":"2026-11-19T00:00:00Z", "status":"done", "error":null,
    "stack":[{"workflow":"outer","workflow_ref":"outer-ref","step":step}]})
}

fn counts(gate: u32, fmt: u32) -> Vec<Value> {
  [("start",1),("develop",1),("gate",gate),("fix_gate_fmt",fmt),("fix_gate_test",1)].into_iter().map(|(step,count)| json!({
    "identity":format!("{:x}",Sha256::digest(serde_json::to_vec(&(Vec::<String>::new(),"outer-ref",step)).unwrap())),
    "workflow":"outer","step":step,"count":count
  })).collect()
}

struct Fixture {
  harness:    GrpcHarness,
  directory:  tempfile::TempDir,
  run_dir:    PathBuf,
  records:    Vec<Value>,
  definition: Vec<Value>,
}

impl Fixture {
  fn new() -> Self {
    let harness = GrpcHarness::builder()
      .single_project(project("takt-dash", Path::new("/remote/project")), server_meta(), json!({}))
      .build();
    let directory = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(directory.path()).unwrap();
    let project = root.join("project ' $(touch forbidden)");
    let run_dir = root.join("takt-worktrees/wt/.takt/runs/run-1");
    fs::create_dir_all(project.join(".takt")).unwrap();
    fs::create_dir_all(run_dir.join("logs")).unwrap();
    fs::write(
      project.join(".takt/tasks.yaml"),
      format!(
        "tasks:\n  - name: implement\n    run_slug: run-1\n    source_run_slug: old-run\n    worktree_path: {}\n",
        root.join("takt-worktrees/wt").display()
      ),
    )
    .unwrap();
    let script = root.join("ssh");
    fs::write(&script, include_str!("fake_ssh.cjs")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let mut config: Value = serde_json::from_str(&fs::read_to_string(harness.config_path()).unwrap()).unwrap();
    let mcp = config["mcpServers"]["mac-mini"].clone();
    config["mcpServers"]["mac-mini"] = json!({"command":script,"args":["-p","2222","-oBatchMode=yes","fixture-host","fixture-mcp"],"env":{
      "FIXTURE_MCP":mock_script(),"FIXTURE_MCP_ARGS":serde_json::to_string(&mcp["args"].as_array().unwrap()[1..]).unwrap(),
      "FIXTURE_PROJECT":project,"FIXTURE_READ_CALLS":root.join("reads.jsonl")
    }});
    config["projects"][0]["cwd"] = json!("/remote/project");
    fs::write(harness.config_path(), config.to_string()).unwrap();
    let records = vec![
      json!({"type":"workflow_start"}),
      event("step_complete", "start", 1, Some(0)),
      event("step_complete", "develop", 2, Some(0)),
      event("step_complete", "gate", 3, Some(0)),
      event("step_complete", "fix_gate_fmt", 4, Some(0)),
      event("step_start", "gate", 9, None),
    ];
    let fixture = Self { harness, directory, run_dir, records, definition: definition() };
    fixture.write("gate", 3, 1, false);
    fixture
  }

  fn write(&self, current: &str, gate: u32, fmt: u32, ended: bool) {
    fs::write(self.run_dir.join("meta.json"), json!({"stepExecutions":counts(gate,fmt), "workflow":"outer", "currentStep":current,"currentIteration":9,"status":if ended {"completed"} else {"running"},"task":"本文を保持する"}).to_string()).unwrap();
    fs::write(self.run_dir.join("logs/body.jsonl"), self.records.iter().map(|r| format!("{r}\n")).collect::<String>())
      .unwrap();
    let logs:Vec<_>=self.records.iter().filter(|r|r["type"]=="step_complete").map(|r|json!({"step":r["step"],"timestamp":r["timestamp"],"iteration":r["iteration"],"status":"done","persona":"coder"})).collect();
    self.harness.rewrite_state(json!({"tasks":[{"name":"implement","runSlug":"run-1","status":if ended {"completed"} else {"running"}}],
      "runs":{"run-1":{"runSlug":"run-1","workflow":"outer","status":if ended {"completed"} else {"running"},"currentStep":current,"currentIteration":9,"taskText":"本文を保持する","stepLogs":logs,
        "reports":[{"filename":"report.md","content":"リモートのレポート"}],
        "instructions":[{"instructionId":1,"issuedAt":"2026-11-19T00:00:00Z","state":"pending","content":"リモートの指示書"}]}},
      "workflows":{"outer":{"initialStep":"start","steps":self.definition}}
    }));
  }

  fn use_local(&self) {
    let mut config: Value = serde_json::from_str(&fs::read_to_string(self.harness.config_path()).unwrap()).unwrap();
    let old = &config["mcpServers"]["mac-mini"];
    let args: Vec<Value> = serde_json::from_str(old["env"]["FIXTURE_MCP_ARGS"].as_str().unwrap()).unwrap();
    let mut command_args = vec![json!(mock_script())];
    command_args.extend(args);
    config["projects"][0]["cwd"] = old["env"]["FIXTURE_PROJECT"].clone();
    config["mcpServers"]["mac-mini"] = json!({"command":"node","args":command_args});
    fs::write(self.harness.config_path(), config.to_string()).unwrap();
  }

  async fn cycle(&self) {
    let mut poller = Poller::new(
      self.harness.config_path().to_path_buf(),
      self.harness.activity.clone(),
      self.harness.store.clone(),
      self.harness.changes.clone(),
      self.harness.registry.clone(),
    );
    poller.read_cycle_once().await.unwrap();
    poller.shutdown().await;
  }
}

#[tokio::test]
async fn remote_records_update_overview_and_detail_through_the_same_acquisition_path() {
  let mut fixture = Fixture::new();
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  for (current, gate, fmt) in [("gate", 3, 1), ("fix_gate_fmt", 3, 2), ("gate", 4, 2)] {
    fixture.write(current, gate, fmt, false);
    fixture.cycle().await;
    let state = api.get_overview(GetOverviewRequest {}).await.unwrap().into_inner();
    assert_eq!(state.runs.len(), 1);
    let overview = state.runs[0].progress.as_ref().unwrap();
    let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
    let progress = detail.progress.unwrap();
    assert_eq!(&progress, overview);
    assert_eq!(progress.current_position.unwrap().value, 3);
    assert_eq!(progress.backbone.len(), 4);
    assert_eq!(progress.nodes.iter().find(|n| n.name == "gate").unwrap().execution_count.unwrap().value, gate);
    assert_eq!(progress.loops[0].execution_count.unwrap().value, fmt + 1);
    let edge = progress.edges.iter().find(|e| e.source == "gate" && e.target == "fix_gate_fmt").unwrap();
    assert_eq!(edge.traversals.unwrap().value, fmt);
    assert!(progress.edges.iter().all(|e| e.target != "ABORT"));
    let node = progress.nodes.iter().find(|n| n.name == "fix_gate_fmt").unwrap();
    assert_eq!(detail.step_logs[node.log_index.unwrap() as usize].step_name, "fix_gate_fmt");
    fixture.records.push(event("step_complete", current, 10, Some(0)));
    fixture.records.push(event("step_start", if current == "gate" { "fix_gate_fmt" } else { "gate" }, 11, None));
  }
  server.stop().await;
  let reads = fs::read_to_string(fixture.directory.path().join("reads.jsonl")).unwrap();
  assert!(reads.lines().count() >= 12);
  assert!(!fixture.directory.path().join("forbidden").exists());
}

#[tokio::test]
async fn remote_detail_preserves_body_and_logs_when_ssh_records_are_unreadable() {
  let fixture = Fixture::new();
  fixture.cycle().await;
  let mut config: Value = serde_json::from_str(&fs::read_to_string(fixture.harness.config_path()).unwrap()).unwrap();
  config["mcpServers"]["mac-mini"]["env"]["FIXTURE_READ_FAIL"] = json!("yes");
  fs::write(fixture.harness.config_path(), config.to_string()).unwrap();
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  assert_eq!(detail.task_text, "本文を保持する");
  assert!(!detail.step_logs.is_empty());
  let progress = detail.progress.unwrap();
  assert!(progress.nodes.iter().all(|n| n.execution_count.is_none()));
  assert!(progress.edges.iter().all(|e| e.traversals.is_none()));
  let overview = api.get_overview(GetOverviewRequest {}).await.unwrap().into_inner();
  assert_eq!(overview.runs.len(), 1);
  assert_eq!(overview.runs[0].progress.as_ref(), Some(&progress));
  server.stop().await;
}

#[tokio::test]
async fn child_rule_and_parallel_activity_do_not_advance_outer_nodes() {
  let mut fixture = Fixture::new();
  fixture.records.retain(|r| r["step"] != "gate" || r["type"] != "step_complete");
  fixture.records.push(event("workflow_call_start", "develop", 11, None));
  let mut child = event("step_complete", "gate", 12, Some(0));
  child["workflow"] = json!("child");
  child["stack"] =
    json!([{"step":"develop","workflow":"outer","workflow_ref":"outer-ref"},{"step":"gate","workflow":"child"}]);
  fixture.records.push(child);
  let mut activity = event("step_start", "plan", 13, None);
  activity["workflow"] = json!("child");
  activity["stack"] =
    json!([{"step":"develop","workflow":"outer","workflow_ref":"outer-ref"},{"step":"plan","workflow":"child"}]);
  fixture.records.push(activity);
  let mut parallel = event("step_start", "finish", 14, None);
  parallel["parallel"] = json!({"role":"child"});
  parallel["stack"][0]["step"] = json!("develop");
  fixture.records.push(parallel);
  fixture.write("develop", 3, 1, false);
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  let progress = detail.progress.unwrap();
  assert_eq!(
    progress.edges.iter().find(|e| e.source == "gate" && e.target == "fix_gate_fmt").unwrap().traversals.unwrap().value,
    0
  );
  assert!(progress.nodes.iter().find(|n| n.name == "develop").unwrap().active_children.contains(&"plan".into()));
  assert_eq!(progress.nodes.iter().find(|n| n.name == "finish").unwrap().state, WorkflowNodeState::Unvisited as i32);
  server.stop().await;
}

#[tokio::test]
async fn completed_remote_run_is_read_even_after_it_leaves_the_running_list() {
  let fixture = Fixture::new();
  fixture.cycle().await;
  fixture.write("gate", 3, 1, true);
  let mut task = task("implement", takt_dash_backend::model::TaskStatus::Completed);
  task.run_slug = Some(takt_dash_backend::domain::RunSlug::try_from("run-1").unwrap());
  fixture.harness.store.record_success(
    "takt-dash",
    ProjectSnapshot { tasks: vec![task], ..ProjectSnapshot::empty() },
    time("2026-11-19T00:00:00Z"),
  );
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  assert!(api.get_overview(GetOverviewRequest {}).await.unwrap().into_inner().runs.is_empty());
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  assert_eq!(detail.progress.unwrap().terminal.as_deref(), Some("COMPLETE"));
  server.stop().await;
}

#[tokio::test]
async fn remote_symlinks_cannot_read_meta_or_logs_outside_the_run() {
  for target in ["meta.json", "logs", "run"] {
    let fixture = Fixture::new();
    fixture.cycle().await;
    let external = fixture.directory.path().join("external");
    fs::create_dir_all(&external).unwrap();
    fs::write(external.join("meta.json"), json!({"stepExecutions":counts(99,99)}).to_string()).unwrap();
    let path = if target == "run" { fixture.run_dir.clone() } else { fixture.run_dir.join(target) };
    if target != "meta.json" {
      fs::remove_dir_all(&path).unwrap();
    } else {
      fs::remove_file(&path).unwrap();
    }
    std::os::unix::fs::symlink(
      if target != "meta.json" { external.clone() } else { external.join("meta.json") },
      &path,
    )
    .unwrap();
    let server = start_server(&fixture.harness).await;
    let mut api = client(server.address).await;
    let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
    assert_eq!(detail.task_text, "本文を保持する");
    assert!(detail.progress.unwrap().nodes.iter().all(|node| node.execution_count.is_none()));
    server.stop().await;
  }
}

#[tokio::test]
async fn remote_read_uses_worktree_before_project_and_ignores_unfinished_log_lines() {
  let fixture = Fixture::new();
  let project = fixture.directory.path().join("project ' $(touch forbidden)");
  let project_run = project.join(".takt/runs/run-1");
  fs::create_dir_all(&project_run).unwrap();
  fs::write(project_run.join("meta.json"), json!({"stepExecutions":counts(99,99)}).to_string()).unwrap();
  use std::io::Write;
  let mut body = fs::OpenOptions::new().append(true).open(fixture.run_dir.join("logs/body.jsonl")).unwrap();
  write!(body, "{}", event("step_complete", "finish", 42, Some(0))).unwrap();
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  let progress = detail.progress.unwrap();
  assert_eq!(progress.nodes.iter().find(|node| node.name == "gate").unwrap().execution_count.unwrap().value, 3);
  assert_eq!(
    progress
      .edges
      .iter()
      .find(|edge| edge.source == "finish" && edge.target == "gate")
      .unwrap()
      .traversals
      .unwrap()
      .value,
    0
  );
  server.stop().await;
}

#[tokio::test]
async fn remote_source_run_uses_the_project_copy_when_the_worktree_has_no_records() {
  let fixture = Fixture::new();
  let project = fixture.directory.path().join("project ' $(touch forbidden)");
  fs::write(
    project.join(".takt/tasks.yaml"),
    format!(
      "tasks:\n  - name: resumed\n    run_slug: new-run\n    source_run_slug: run-1\n    worktree_path: {}\n",
      fixture.directory.path().join("takt-worktrees/wt").display()
    ),
  )
  .unwrap();
  fs::create_dir_all(project.join(".takt/runs")).unwrap();
  fs::rename(&fixture.run_dir, project.join(".takt/runs/run-1")).unwrap();
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  assert_eq!(
    detail.progress.unwrap().nodes.iter().find(|n| n.name == "gate").unwrap().execution_count.unwrap().value,
    3
  );
  server.stop().await;
}

#[tokio::test]
async fn log_links_only_target_completions_returned_by_the_mcp_response() {
  let fixture = Fixture::new();
  use std::io::Write;
  let mut body = fs::OpenOptions::new().append(true).open(fixture.run_dir.join("logs/body.jsonl")).unwrap();
  writeln!(body, "{}", event("step_complete", "finish", 42, Some(0))).unwrap();
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  let progress = detail.progress.unwrap();
  let finish = progress.nodes.iter().find(|node| node.name == "finish").unwrap();
  assert_eq!(finish.state, WorkflowNodeState::Completed as i32);
  assert_eq!(finish.log_index, None, "MCP の取得後に追記された完了記録には空リンクを作りません");
  for node in progress.nodes.iter().filter(|node| node.log_index.is_some()) {
    assert_eq!(detail.step_logs[node.log_index.unwrap() as usize].step_name, node.name);
  }
  server.stop().await;
}

#[tokio::test]
async fn ssh_project_never_uses_a_same_named_run_on_the_backend_host() {
  let fixture = Fixture::new();
  let local = fixture.harness.project_dir();
  fs::create_dir_all(local.join(".takt/runs/run-1/logs")).unwrap();
  fs::write(local.join(".takt/tasks.yaml"), "tasks:\n  - name: local\n    run_slug: run-1\n").unwrap();
  fs::write(
    local.join(".takt/runs/run-1/meta.json"),
    json!({"workflow":"wrong-host","status":"failed","currentStep":"finish","stepExecutions":counts(99,99)})
      .to_string(),
  )
  .unwrap();
  let mut config: Value = serde_json::from_str(&fs::read_to_string(fixture.harness.config_path()).unwrap()).unwrap();
  config["projects"][0]["cwd"] = json!(local);
  config["mcpServers"]["mac-mini"]["env"]["FIXTURE_LOGICAL_PROJECT"] = json!(local);
  fs::write(fixture.harness.config_path(), config.to_string()).unwrap();
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let overview = api.get_overview(GetOverviewRequest {}).await.unwrap().into_inner();
  assert_eq!(overview.runs[0].current_step_name.as_deref(), Some("gate"));
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  assert_eq!(detail.workflow, "outer");
  assert_eq!(detail.current_step.as_deref(), Some("gate"));
  assert_eq!(
    detail.progress.unwrap().nodes.iter().find(|n| n.name == "gate").unwrap().execution_count.unwrap().value,
    3
  );
  let updates = api
    .get_run_updates(GetRunUpdatesRequest {
      run_slug:       "run-1".into(),
      epoch:          None,
      after_sequence: None,
    })
    .await
    .unwrap()
    .into_inner();
  assert!(updates.lines.iter().any(|line| line.text.contains("fix_gate_fmt")));
  for (target, expected) in
    [(Target::ReportFilename("report.md".into()), "リモートのレポート"), (Target::InstructionId(1), "リモートの指示書")]
  {
    let content = api
      .get_run_content(GetRunContentRequest { run_slug: "run-1".into(), target: Some(target) })
      .await
      .unwrap()
      .into_inner();
    assert_eq!(content.content, expected);
  }
  server.stop().await;
}

#[tokio::test]
async fn remote_body_records_count_only_actual_backbone_returns() {
  let mut fixture = Fixture::new();
  fixture.records.push(event("step_complete", "finish", 10, Some(0)));
  fixture.records.push(event("step_complete", "finish", 12, Some(0)));
  fixture.write("gate", 3, 1, false);
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let progress =
    api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner().progress.unwrap();
  let returned = progress.edges.iter().find(|edge| edge.source == "finish" && edge.target == "gate").unwrap();
  assert_eq!(returned.traversals.unwrap().value, 2);
  assert!(returned.backward);
  let unused = progress.edges.iter().find(|edge| edge.source == "gate" && edge.target == "finish").unwrap();
  assert_eq!(unused.traversals.unwrap().value, 0);
  assert!(progress.edges.iter().all(|edge| edge.target != "ABORT"));
  server.stop().await;
}

#[tokio::test]
async fn invalid_remote_counts_keep_the_states_known_from_the_body_log() {
  let fixture = Fixture::new();
  fs::write(
    fixture.run_dir.join("meta.json"),
    json!({"stepExecutions":[{"identity":"bad","workflow":"outer","step":"gate","count":-1}]}).to_string(),
  )
  .unwrap();
  fixture.cycle().await;
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
  let progress = detail.progress.unwrap();
  let start = progress.nodes.iter().find(|node| node.name == "start").unwrap();
  assert_eq!(start.state, WorkflowNodeState::Completed as i32);
  assert_eq!(start.execution_count, None);
  assert_eq!(
    progress
      .edges
      .iter()
      .find(|edge| edge.source == "gate" && edge.target == "fix_gate_fmt")
      .unwrap()
      .traversals
      .unwrap()
      .value,
    1
  );
  server.stop().await;
}

#[tokio::test]
async fn workflow_call_frames_results_and_uncertainty_survive_local_and_ssh_acquisition() {
  for local in [false, true] {
    for mode in [
      "normal",
      "returned",
      "ambiguous",
      "duplicate",
      "failed",
      "later-rule",
      "compound",
      "aborted",
      "aborted-returned",
      "child-aborted",
      "terminal-abort",
    ] {
      let mut fixture = Fixture::new();
      if local {
        fixture.use_local();
      }
      fixture.definition[1]["rules"] = if mode == "ambiguous" {
        json!([{"next":"gate"},{"next":"fix_gate_fmt"}])
      } else {
        json!([{"condition":{"kind":"semantic","label":"COMPLETE"},"next":"gate"},{"condition":{"kind":"semantic","label":"need_replan"},"next":"fix_gate_fmt"},{"condition":{"kind":"semantic","label":"ABORT"},"next":"gate"}])
      };
      if mode == "later-rule" {
        fixture.definition[1]["rules"] = json!([{"condition":{"kind":"semantic","label":"success"},"next":"gate"},{"condition":{"kind":"semantic","label":"skipped"},"next":"gate"}]);
      }
      if mode == "compound" {
        fixture.definition[1]["rules"][0]["condition"] = json!({"kind":"and","left":{"kind":"semantic","label":"COMPLETE"},"right":{"kind":"when","expression":"true"}});
      }
      if mode == "terminal-abort" {
        fixture.definition[1]["rules"][2]["next"] = json!("ABORT");
      }
      let mut start = event("workflow_call_start", "develop", 2, None);
      start["workflow"] = json!("outer-ref");
      start["timestamp"] = json!("2026-11-19T00:00:10Z");
      start["stack"][0]["kind"] = json!("workflow_call");
      start["stack"][0]["occurrence"] = json!(1);
      let mut child = start.clone();
      child["timestamp"] = json!("2026-11-19T00:00:11Z");
      child["stack"]
        .as_array_mut()
        .unwrap()
        .push(json!({"workflow":"inner","workflow_ref":"inner-ref","step":"develop","occurrence":1}));
      child["workflow"] = json!("inner-ref");
      if mode == "child-aborted" {
        child["type"] = json!("workflow_call_complete");
        child["status"] = json!("aborted");
      }
      let mut complete = start.clone();
      complete["type"] = json!("workflow_call_complete");
      complete["status"] = json!(
        if mode == "failed" {
          "failed"
        } else if mode.starts_with("aborted") || mode == "terminal-abort" {
          "aborted"
        } else {
          "completed"
        }
      );
      complete["timestamp"] = json!("2026-11-19T00:00:12Z");
      if mode == "later-rule" {
        complete["matchedRuleIndex"] = json!(1);
      }
      if mode == "returned" || mode == "aborted-returned" {
        complete["returnValue"] = json!("need_replan");
      }
      fixture.records = vec![
        json!({"type":"workflow_start"}),
        event("step_complete", "start", 1, Some(0)),
        start,
        child,
        complete.clone(),
      ];
      if mode == "duplicate" {
        complete["type"] = json!("step_complete");
        complete["matchedRuleIndex"] = json!(1);
        fixture.records.push(complete);
      }
      fixture.records.push(event("step_start", "gate", 9, None));
      fixture.write("gate", 3, 1, false);
      fixture.cycle().await;
      let server = start_server(&fixture.harness).await;
      let mut api = client(server.address).await;
      let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
      let graph = detail.progress.unwrap();
      let node = graph.nodes.iter().find(|node| node.name == "develop").unwrap();
      assert_eq!(
        node.started_at,
        Some(prost_types::Timestamp { seconds: time("2026-11-19T00:00:10Z").timestamp(), nanos: 0 }),
        "local={local},mode={mode}"
      );
      assert_eq!(
        node.state,
        if mode == "failed" {
          WorkflowNodeState::Failed
        } else if mode.starts_with("aborted") || mode == "terminal-abort" {
          WorkflowNodeState::Aborted
        } else {
          WorkflowNodeState::Completed
        } as i32
      );
      let edges: Vec<_> = graph
        .edges
        .iter()
        .filter(|edge| edge.source == "develop")
        .map(|edge| edge.traversals.map(|count| count.value))
        .collect();
      assert_eq!(
        edges,
        match mode {
          | "ambiguous" => vec![None, None],
          | "failed" | "compound" => vec![None, None, None],
          | "later-rule" => vec![Some(0), Some(1)],
          | "returned" | "aborted-returned" | "duplicate" => vec![Some(0), Some(1), Some(0)],
          | "aborted" => vec![Some(0), Some(0), Some(1)],
          | "terminal-abort" => vec![Some(0), Some(0)],
          | _ => vec![Some(1), Some(0), Some(0)],
        },
        "local={local},mode={mode}"
      );
      if mode == "later-rule" {
        assert_eq!(
          graph.connections.iter().find(|edge| edge.source == "develop" && edge.target == "gate").unwrap().traversed,
          Some(true)
        );
      }
      let overview = api.get_overview(GetOverviewRequest {}).await.unwrap().into_inner();
      let overview_graph = overview.runs[0].progress.as_ref().unwrap();
      assert_eq!(overview_graph.edges, graph.edges, "概要と詳細の規則別回数 local={local},mode={mode}");
      assert_eq!(overview_graph.nodes.iter().find(|node| node.name == "develop").unwrap().state, node.state);
      assert_eq!(graph.current_step.as_deref(), Some("gate"));
      assert_eq!(detail.status, takt_dash_backend::takt_dash::v1::RunStatus::Running as i32);
      assert_eq!(
        overview.runs[0]
          .progress
          .as_ref()
          .unwrap()
          .nodes
          .iter()
          .find(|node| node.name == "develop")
          .unwrap()
          .started_at,
        node.started_at
      );
      server.stop().await;
    }
  }
}

#[tokio::test]
async fn participant_lifetimes_are_independent_of_outer_progress_in_local_and_ssh_runs() {
  for local in [false, true] {
    let mut fixture = Fixture::new();
    if local {
      fixture.use_local();
    }
    fixture.records = vec![json!({"type":"workflow_start"}), event("step_start", "gate", 9, None)];
    fixture.records[1]["stack"][0]["occurrence"] = json!(1);
    let server = start_server(&fixture.harness).await;
    let mut api = client(server.address).await;
    fixture.write("gate", 3, 1, false);
    fixture.cycle().await;
    let baseline = api
      .get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() })
      .await
      .unwrap()
      .into_inner()
      .progress
      .unwrap();
    for (kind, name, id, expected) in [
      ("phase_start", "gate", "participant:a", vec!["gate"]),
      ("phase_start", "gate", "participant:c", vec!["gate", "gate"]),
      ("phase_start", "frontend-review", "participant:b", vec!["frontend-review", "gate", "gate"]),
      ("phase_complete", "gate", "participant:a", vec!["frontend-review", "gate"]),
      ("phase_complete", "gate", "participant:c", vec!["frontend-review"]),
      ("step_complete", "gate", "", vec![]),
    ] {
      let mut record = event(kind, name, 9, None);
      record["stack"] = fixture.records[1]["stack"].clone();
      if !id.is_empty() {
        record["parallel"] =
          json!({"role":"direct_participant","participationId":id,"parentParticipationId":"parallel:a"});
        record["phase"] = json!(1);
      }
      fixture.records.push(record);
      fixture.write("gate", 3, 1, false);
      fixture.cycle().await;
      let detail = api.get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() }).await.unwrap().into_inner();
      let graph = detail.progress.unwrap();
      let node = graph.nodes.iter().find(|node| node.name == "gate").unwrap();
      let mut children = node.active_children.clone();
      children.sort();
      let mut expected = expected;
      expected.sort();
      assert_eq!(children, expected, "local={local},kind={kind}");
      if kind.starts_with("phase_") {
        assert_eq!(graph.edges, baseline.edges);
      }
      assert_eq!(node.execution_count.unwrap().value, 3);
      assert_eq!(graph.current_position.unwrap().value, 3);
      assert!(graph
        .edges
        .iter()
        .filter(|edge| edge.source == "gate")
        .all(|edge| edge.traversals.is_none_or(|count| count.value == 0)));
    }
    server.stop().await;
  }
}

#[tokio::test]
async fn log_updates_never_read_progress_evidence_and_keep_their_cursor() {
  let fixture = Fixture::new();
  fixture.cycle().await;
  fs::write(fixture.directory.path().join("reads.jsonl"), "").unwrap();
  let server = start_server(&fixture.harness).await;
  let mut api = client(server.address).await;
  let first = api
    .get_run_updates(GetRunUpdatesRequest { run_slug: "run-1".into(), ..Default::default() })
    .await
    .unwrap()
    .into_inner();
  assert!(first.reset);
  assert_eq!(first.lines.iter().map(|line| (line.sequence, line.text.as_str())).collect::<Vec<_>>(), vec![
    (1, "2026-11-19T00:00:00Z step_complete start"),
    (2, "2026-11-19T00:00:00Z step_complete develop"),
    (3, "2026-11-19T00:00:00Z step_complete gate"),
    (4, "2026-11-19T00:00:00Z step_complete fix_gate_fmt"),
  ]);
  let next = api
    .get_run_updates(GetRunUpdatesRequest {
      run_slug:       "run-1".into(),
      epoch:          Some(first.epoch),
      after_sequence: first.lines.last().map(|line| line.sequence),
    })
    .await
    .unwrap()
    .into_inner();
  assert!(next.lines.is_empty());
  assert_eq!(next.epoch, first.epoch);
  assert!(!next.reset);
  assert_eq!(fs::read_to_string(fixture.directory.path().join("reads.jsonl")).unwrap(), "");
  server.stop().await;
}

#[tokio::test]
async fn nested_parallel_parent_completion_clears_its_participants_in_both_acquisition_paths() {
  for local in [false, true] {
    let mut fixture = Fixture::new();
    if local {
      fixture.use_local();
    }
    let mut outer = event("workflow_call_start", "develop", 2, None);
    outer["stack"][0]["occurrence"] = json!(1);
    fixture.records =
      vec![json!({"type":"workflow_start"}), event("step_complete", "start", 1, Some(0)), outer.clone()];
    let server = start_server(&fixture.harness).await;
    let mut api = client(server.address).await;
    for (kind, step, id, group, occurrence, expected) in [
      ("step_start", "review", "", "parallel:a", 1, vec!["review"]),
      ("phase_start", "architecture-review", "participant:a", "parallel:a", 1, vec!["architecture-review", "review"]),
      ("phase_start", "frontend-review", "participant:b", "parallel:a", 1, vec![
        "architecture-review",
        "frontend-review",
        "review",
      ]),
      ("phase_complete", "architecture-review", "participant:a", "parallel:a", 1, vec!["frontend-review", "review"]),
      ("step_complete", "review", "", "parallel:a", 1, vec![]),
      ("step_start", "review", "", "parallel:b", 2, vec!["review"]),
      ("phase_start", "architecture-review", "participant:c", "parallel:b", 2, vec!["architecture-review", "review"]),
      ("step_complete", "review", "", "parallel:b", 2, vec![]),
    ] {
      let mut record = event(kind, step, 10, None);
      record["workflow"] = json!("inner");
      record["stack"] = outer["stack"].clone();
      record["stack"]
        .as_array_mut()
        .unwrap()
        .push(json!({"workflow":"inner","workflow_ref":"inner-ref","step":"review","kind":"parallel","occurrence":occurrence}));
      record["parallel"] = if id.is_empty() {
        json!({"role":"parent","participationId":group})
      } else {
        json!({"role":"direct_participant","participationId":id,"parentParticipationId":group})
      };
      record["phase"] = json!(1);
      fixture.records.push(record);
      fixture.write("develop", 3, 1, false);
      fixture.cycle().await;
      let graph = api
        .get_run_detail(GetRunDetailRequest { run_slug: "run-1".into() })
        .await
        .unwrap()
        .into_inner()
        .progress
        .unwrap();
      let node = graph.nodes.iter().find(|node| node.name == "develop").unwrap();
      let mut children = node.active_children.clone();
      children.sort();
      assert_eq!(children, expected, "local={local},kind={kind}");
      assert_eq!(node.execution_count.unwrap().value, 1);
      assert_eq!(graph.current_position.unwrap().value, 2);
      assert!(graph
        .edges
        .iter()
        .filter(|edge| edge.source == "develop")
        .all(|edge| edge.traversals.unwrap().value == 0));
    }
    server.stop().await;
  }
}
