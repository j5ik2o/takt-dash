//! mock MCP サーバを使い、定期読み取りの1サイクル・出来事の記録・応答なしのキャッシュ保持・
//! 復旧・稼働中の設定変更の反映を、同じ Poller 実体の上で観測する。

mod common;

use std::{
  cell::RefCell,
  collections::BTreeMap,
  io::Write,
  os::unix::fs::PermissionsExt,
  path::{Path, PathBuf},
  sync::{Arc, Mutex, Once},
  time::{Duration, Instant},
};

use common::*;
use serde_json::{json, Value};
use takt_dash_backend::{
  activity::ActivityStore,
  config::Thresholds,
  domain::{SilentMinutes, StaleMinutes},
  mcp::McpClientRegistry,
  model::{ActivityEvent, EventKind, ProjectSnapshot},
  poller::{run_until_stopped, Poller},
  store::StateStore,
};

struct Harness {
  _directory:       tempfile::TempDir,
  config_path:      PathBuf,
  state_path:       PathBuf,
  call_log_path:    PathBuf,
  project_dir:      PathBuf,
  activity:         Arc<ActivityStore>,
  activity_db_path: PathBuf,
  store:            Arc<StateStore>,
  changes:          tokio::sync::broadcast::Sender<()>,
}

fn write_json(path: &Path, value: &Value) {
  std::fs::write(path, value.to_string()).expect("JSON ファイルを書けます");
}

/// 記録先の SQLite に、指定したサーバーの出来事の INSERT を中断させるトリガーを仕込む。
///
/// トリガーは保存処理そのものを既存のコネクション上で失敗させるため、権限や root 実行に
/// 依存せず、特定プロジェクトの出来事の記録だけを決定的に失敗させられます。
fn fail_event_recording_for(db_path: &Path, server_id: &str) {
  let connection = rusqlite::Connection::open(db_path).expect("記録先へ直接接続できます");
  connection
    .execute_batch(&format!(
      "CREATE TRIGGER reject_events BEFORE INSERT ON events WHEN NEW.server_id = '{server_id}'
       BEGIN SELECT RAISE(ABORT, '出来事の記録に失敗しました'); END;"
    ))
    .expect("記録を中断させるトリガーを作れます");
}

fn allow_event_recording(db_path: &Path) {
  let connection = rusqlite::Connection::open(db_path).expect("記録先へ直接接続できます");
  connection.execute_batch("DROP TRIGGER IF EXISTS reject_events").expect("記録の中断を解除できます");
}

/// 記録先の events テーブルを別名へ退避し、接続の出来事への照会を失敗させる。
///
/// テーブルが無い間も、出来事が空の記録（空のトランザクション）は成功するため、
/// 「照会だけが失敗し、読み取りと空の記録は成功する」状態を決定的に作れます。
/// 退避中は recent_events も失敗するため、検証は restore_events_table の後に行います。
fn hide_events_table(db_path: &Path) {
  let connection = rusqlite::Connection::open(db_path).expect("記録先へ直接接続できます");
  connection.execute_batch("ALTER TABLE events RENAME TO events_hidden").expect("テーブルを退避できます");
}

fn restore_events_table(db_path: &Path) {
  let connection = rusqlite::Connection::open(db_path).expect("記録先へ直接接続できます");
  connection.execute_batch("ALTER TABLE events_hidden RENAME TO events").expect("テーブルを戻せます");
}

/// 復帰イベントの本文から、切断していた秒数を読み取る。
fn outage_seconds_of(event: &ActivityEvent) -> i64 {
  event
    .body
    .split("切断していた時間は ")
    .nth(1)
    .and_then(|rest| rest.split(" 秒です。").next())
    .and_then(|value| value.parse::<i64>().ok())
    .expect("復帰イベントの本文に切断していた秒数が入ります")
}

/// mock MCP サーバが起動時に書いた PID を読む。
fn read_pid_file(path: &Path) -> u32 {
  std::fs::read_to_string(path).expect("PID ファイルを読めます").trim().parse().expect("PID を数値として読めます")
}

/// 指定した PID のプロセスが存在するか。終了済みなら偽です。
fn process_is_running(pid: u32) -> bool {
  matches!(
    std::process::Command::new("kill").arg("-0").arg(pid.to_string()).status(),
    Ok(status) if status.success()
  )
}

fn state_with_goal(goal_id: &str) -> Value {
  json!({ "goals": [ { "id": goal_id, "status": "created", "executionStatus": "active", "objective": format!("ゴール {goal_id}"), "integrationBranch": format!("takt/{goal_id}") } ] })
}

fn state_with_goals(count: usize, status: &str, execution_status: &str) -> Value {
  let goals: Vec<Value> = (0..count)
    .map(|index| {
      json!({
        "id": format!("g{index}"),
        "status": status,
        "executionStatus": execution_status,
        "objective": format!("ゴール {index}"),
        "integrationBranch": format!("takt/g{index}")
      })
    })
    .collect();
  json!({ "goals": goals })
}

/// running なタスクを持ち、takt_get_run には run の保存場所と食い違う値を返す mock の状態。
///
/// run の status・currentStep・stepLogs を保存場所（`meta.json` と本体ログ）と異なる値にし、
/// 一覧の run がどちらから作られたかを値で区別できるようにします。workflow の step 名一覧は
/// takt_get_workflow から返し、ステップ位置の補完に使います。
fn state_with_running_tasks(run_slugs: &[&str]) -> Value {
  let tasks: Vec<Value> = run_slugs
    .iter()
    .map(|run_slug| {
      json!({ "name": format!("task-{run_slug}"), "status": "running", "runSlug": run_slug, "startedAt": "2026-11-13T08:00:00Z" })
    })
    .collect();
  let runs: serde_json::Map<String, Value> = run_slugs
    .iter()
    .map(|run_slug| {
      (
        (*run_slug).to_owned(),
        json!({
          "runSlug": run_slug,
          "status": "completed",
          "workflow": "development-core",
          "currentStep": "decoy",
          "startedAt": "2026-11-13T07:00:00Z",
          "lastLogAt": "2026-11-13T07:30:00Z",
          "stepLogs": [ { "step": "decoy", "persona": "coder", "status": "done", "timestamp": "2026-11-13T07:30:00Z", "iteration": 1 } ],
          "reports": [ { "filename": "decoy.md", "sizeBytes": 1 } ]
        }),
      )
    })
    .collect();
  json!({
    "tasks": tasks,
    "runs": runs,
    "workflows": { "development-core": ["plan", "implement", "review"] }
  })
}

/// mock の takt_get_run を道具エラーにした状態を返す。
fn with_failing_get_run(mut state: Value) -> Value {
  state["isError"] = json!(["takt_get_run"]);
  state
}

/// 1 周期分の snapshot から、指定した run を取り出す。
fn stored_run(harness: &Harness, run_slug: &str) -> Option<takt_dash_backend::model::RunEntry> {
  harness
    .store
    .slot("takt-dash")
    .and_then(|slot| slot.snapshot)
    .and_then(|snapshot| snapshot.runs.into_iter().find(|run| run.run_slug == run_slug))
}

fn step_names(run: &takt_dash_backend::model::RunEntry) -> Vec<String> {
  run.steps.iter().map(|step| step.step_name.clone()).collect()
}

impl Harness {
  fn new(state: Value) -> Self {
    let directory = tempfile::tempdir().expect("一時ディレクトリを作れます");
    let config_path = directory.path().join("taktdash.config.json");
    let state_path = directory.path().join("state.json");
    let project_dir = directory.path().join("project");
    std::fs::create_dir_all(&project_dir).expect("プロジェクトの cwd を作れます");
    let call_log_path = directory.path().join("calls.log");
    write_json(&state_path, &state);
    write_json(
      &config_path,
      &json!({
        "mcpServers": { "mac-mini": { "command": "node", "args": [mock_script().display().to_string(), "--state", state_path.display().to_string(), "--call-log", call_log_path.display().to_string()], "env": {} } },
        "servers": { "mac-mini": { "name": "j5ik2o-mac-mini", "kind": "Mac mini", "concurrency": 4 } },
        "projects": [ { "name": "takt-dash", "server": "mac-mini", "about": "説明", "cwd": project_dir.display().to_string() } ],
        "intervalSeconds": 30,
        "thresholds": { "pendingDecisionStaleMinutes": 240, "runLogSilentMinutes": 10 },
        "sqlite": { "path": "data/taktdash.sqlite" }
      }),
    );
    let mut servers = BTreeMap::new();
    servers.insert("mac-mini".to_owned(), server_meta());
    let store = Arc::new(StateStore::new(servers, vec![project("takt-dash", &project_dir)], Thresholds {
      pending_decision_stale_minutes: StaleMinutes::try_from(240).expect("テストの閾値は有効です"),
      run_log_silent_minutes:         SilentMinutes::try_from(10).expect("テストの閾値は有効です"),
      connection_failure_count:       3,
      connection_delay_seconds:       120,
    }));
    let activity_db_path = directory.path().join("events.sqlite");
    let activity = Arc::new(ActivityStore::open(&activity_db_path).expect("SQLite を開けます"));
    let (changes, _) = tokio::sync::broadcast::channel(16);
    Harness {
      _directory: directory,
      config_path,
      state_path,
      call_log_path,
      project_dir,
      activity,
      activity_db_path,
      store,
      changes,
    }
  }

  fn poller(&self) -> Poller {
    Poller::new(
      self.config_path.clone(),
      self.activity.clone(),
      self.store.clone(),
      self.changes.clone(),
      McpClientRegistry::new(),
    )
  }

  /// プロセスの再起動を模した Poller を返す。状態ストアとレジストリは作り直し、
  /// 設定ファイルと出来事の記録先は同じものを使い回します。作り直した状態ストアも返します。
  fn restarted_poller(&self) -> (Poller, Arc<StateStore>) {
    let mut servers = BTreeMap::new();
    servers.insert("mac-mini".to_owned(), server_meta());
    let store = Arc::new(StateStore::new(servers, vec![project("takt-dash", &self.project_dir)], Thresholds {
      pending_decision_stale_minutes: StaleMinutes::try_from(240).expect("テストの閾値は有効です"),
      run_log_silent_minutes:         SilentMinutes::try_from(10).expect("テストの閾値は有効です"),
      connection_failure_count:       3,
      connection_delay_seconds:       120,
    }));
    let poller = Poller::new(
      self.config_path.clone(),
      self.activity.clone(),
      store.clone(),
      self.changes.clone(),
      McpClientRegistry::new(),
    );
    (poller, store)
  }

  fn rewrite_state(&self, state: Value) {
    write_json(&self.state_path, &state);
  }

  /// 設定したプロジェクトの cwd に、run の保存場所を作る fixture を返す。
  fn run_fixture(&self) -> RunFixture {
    RunFixture::new(&self._directory.path().join("project"))
  }

  fn calls(&self) -> Vec<String> {
    recorded_calls(&self.call_log_path)
  }

  fn clear_calls(&self) {
    clear_recorded_calls(&self.call_log_path);
  }

  /// 追加のプロジェクトを専用のサーバーと状態ファイルで設定へ加える。
  fn add_project_with_state(&self, name: &str, state: &Value) -> PathBuf {
    let directory = self._directory.path();
    let project_dir = directory.join(name);
    std::fs::create_dir_all(&project_dir).expect("追加プロジェクトの cwd を作れます");
    let state_path = directory.join(format!("{name}-state.json"));
    write_json(&state_path, state);
    let server_key = format!("server-{name}");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&self.config_path).expect("設定を読めます"))
      .expect("設定を解析できます");
    config["mcpServers"][&server_key] = json!({
      "command": "node",
      "args": [mock_script().display().to_string(), "--state", state_path.display().to_string()],
      "env": {}
    });
    config["servers"][&server_key] = json!({ "name": format!("j5ik2o-{name}"), "kind": "Mac mini", "concurrency": 4 });
    config["projects"].as_array_mut().expect("プロジェクト一覧があります").push(json!({
      "name": name,
      "server": server_key,
      "about": format!("{name} の説明"),
      "cwd": project_dir.display().to_string()
    }));
    write_json(&self.config_path, &config);
    state_path
  }

  fn rewrite_connection_thresholds(&self, failure_count: u32, delay_seconds: u32) {
    let current = std::fs::read_to_string(&self.config_path).expect("設定を読めます");
    let mut value: Value = serde_json::from_str(&current).expect("設定を解析できます");
    value["thresholds"]["connectionFailureCount"] = json!(failure_count);
    value["thresholds"]["connectionDelaySeconds"] = json!(delay_seconds);
    write_json(&self.config_path, &value);
  }

  fn rewrite_config(&self, mutate: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&self.config_path).expect("設定を読めます"))
      .expect("設定を解析できます");
    mutate(&mut value);
    write_json(&self.config_path, &value);
  }
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
  fn contents(&self) -> String {
    let bytes = self.0.lock().expect("ログのバッファを取得できます").clone();
    String::from_utf8(bytes).expect("ログは UTF-8 です")
  }
}

thread_local! {
  /// このスレッドで動くテストのログの取り込み先。取り込み中のテストが無ければ None です。
  static CURRENT_LOGS: RefCell<Option<CapturedLogs>> = const { RefCell::new(None) };
}

/// グローバルな購読者の書き込み先。書き込んだスレッドの取り込み先へ振り分け、取り込み中の
/// テストが無いスレッドのログは捨てる。
struct ThreadLocalWriter;

impl Write for ThreadLocalWriter {
  fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
    CURRENT_LOGS.with(|current| {
      if let Some(logs) = current.borrow().as_ref() {
        logs.0.lock().expect("ログのバッファを取得できます").extend_from_slice(buffer);
      }
    });
    Ok(buffer.len())
  }

  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ThreadLocalWriter {
  type Writer = ThreadLocalWriter;

  fn make_writer(&'a self) -> Self::Writer {
    ThreadLocalWriter
  }
}

/// drop したときに、このスレッドのログの取り込みを止める。
struct CaptureGuard;

impl Drop for CaptureGuard {
  fn drop(&mut self) {
    CURRENT_LOGS.with(|current| current.borrow_mut().take());
  }
}

/// このスレッドのログを取り込む。
///
/// tracing は callsite ごとの関心を全スレッドで共有してキャッシュする。テストごとに
/// `set_default` で購読者を差し替えると、購読者を持たない別のテストのスレッドが先に周期ログの
/// callsite を登録したときに「関心なし」がキャッシュされ、ログが届かなくなる。そのため購読者は
/// テストバイナリで1回だけグローバルに入れ、書き込み先をスレッドごとに振り分ける。
/// `#[tokio::test]` は current_thread で動くため、周期ログはテストのスレッドで出る。
fn capture_logs() -> (CapturedLogs, CaptureGuard) {
  static INITIALIZE: Once = Once::new();
  INITIALIZE.call_once(|| {
    tracing_subscriber::fmt()
      .with_max_level(tracing::Level::INFO)
      .with_ansi(false)
      .with_writer(ThreadLocalWriter)
      .init();
  });
  let logs = CapturedLogs::default();
  CURRENT_LOGS.with(|current| *current.borrow_mut() = Some(logs.clone()));
  (logs, CaptureGuard)
}

fn single_cycle_log_line(logs: &CapturedLogs) -> String {
  let output = logs.contents();
  let lines: Vec<&str> = output.lines().filter(|line| line.contains("cycle_ms=")).collect();
  assert_eq!(lines.len(), 1, "1周期につき集計ログを1本だけ出します: {output}");
  lines[0].to_owned()
}

/// 指定した順番（0 始まり）の周期ログ行を取り出す。
fn nth_cycle_log_line(logs: &CapturedLogs, index: usize) -> String {
  let output = logs.contents();
  let lines: Vec<&str> = output.lines().filter(|line| line.contains("cycle_ms=")).collect();
  assert!(lines.len() > index, "{index} 番目の周期ログがありません: {output}");
  lines[index].to_owned()
}

/// 周期ログの `tool_calls=<数>` を数値で取り出す。
fn tool_calls_value(line: &str) -> usize {
  let prefix = "tool_calls=";
  let start = line.find(prefix).unwrap_or_else(|| panic!("tool_calls がありません: {line}")) + prefix.len();
  let digits: String = line[start..].chars().take_while(|character| character.is_ascii_digit()).collect();
  digits.parse().expect("tool_calls の値が数値ではありません")
}

struct TwoCycleObservation {
  first_calls:  Vec<String>,
  second_calls: Vec<String>,
  second_line:  String,
  snapshot:     ProjectSnapshot,
}

/// 同じ Harness で 2 周期読み取り、1 周期目と 2 周期目の呼び出し・2 周期目のログ行・
/// 最終スナップショットを返す。
async fn run_two_cycles(state: Value) -> TwoCycleObservation {
  let harness = Harness::new(state);
  let (logs, _guard) = capture_logs();
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1周期目を実行できます");
  let first_calls = harness.calls();
  harness.clear_calls();
  poller.read_cycle_once().await.expect("2周期目を実行できます");
  let second_calls = harness.calls();
  let second_line = nth_cycle_log_line(&logs, 1);
  let snapshot = harness.store.slot("takt-dash").and_then(|slot| slot.snapshot).expect("スナップショットがあります");
  poller.shutdown().await;
  TwoCycleObservation { first_calls, second_calls, second_line, snapshot }
}

/// 周期ログの `tools=` から、指定した道具1件分のエントリを取り出す。
fn tool_entry(line: &str, tool: &str) -> String {
  let prefix = format!("{tool}(");
  let start = line.find(&prefix).unwrap_or_else(|| panic!("道具 {tool} のエントリがありません: {line}"));
  let entry = &line[start..];
  // 複数あるときは `), ` の直前まで、最後の1件は行末までがエントリです。
  let end = entry.find("), ").map_or(entry.len(), |index| index + 1);
  entry[..end].to_owned()
}

#[tokio::test]
async fn first_cycle_reads_state_without_events_and_sets_read_time() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("1サイクルを実行できます");

  assert_eq!(report.projects_read, 1);
  assert_eq!(report.events_recorded, 0, "初回は基準化のみで出来事を出しません");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.is_connected());
  assert_eq!(slot.connection_state, Some(takt_dash_backend::model::ConnectionState::Connected));
  assert!(slot.last_read_at.is_some(), "読めた時刻を更新します");
  poller.shutdown().await;
}

#[tokio::test]
async fn task_source_run_slug_is_kept_separately_from_run_slug_in_the_snapshot() {
  // takt_list_tasks の sourceRunSlug は、元 run の解決に使う別の値として状態ストアへ
  // 保持します。run_slug へ混ぜると、実行中 run の照合や goal_id の補完が誤ります。
  let harness = Harness::new(json!({
    "goals": [],
    "tasks": [
      {
        "name": "implement",
        "goalId": "g1",
        "status": "completed",
        "startedAt": "2026-11-13T09:00:00Z",
        "sourceRunSlug": "run-src"
      }
    ]
  }));
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  let snapshot = harness.store.slot("takt-dash").expect("スロットがあります").snapshot.expect("状態があります");
  poller.shutdown().await;

  let task = snapshot.tasks.first().expect("タスクがあります");
  assert_eq!(task.source_run_slug.as_deref(), Some("run-src"), "sourceRunSlug は source_run_slug に保持します");
  assert!(task.run_slug.is_none(), "sourceRunSlug を run_slug へ誤って入れません");
}

#[tokio::test]
async fn state_change_between_cycles_emits_and_records_events() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_state(state_with_goal("g2"));
  let report = poller.read_cycle_once().await.expect("2回目のサイクルを実行できます");

  assert_eq!(report.events_recorded, 1);
  let events = harness.activity.recent_events(10).expect("記録した出来事を読めます");
  assert_eq!(events[0].kind, EventKind::GoalCreated);
  assert_eq!(events[0].project.as_deref(), Some("takt-dash"));
  assert_eq!(events[0].server_id, "mac-mini");
  poller.shutdown().await;
}

#[tokio::test]
async fn single_failure_keeps_last_state_as_delayed_without_connection_event() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let first_read_at = harness.store.slot("takt-dash").and_then(|slot| slot.last_read_at).expect("読めた時刻があります");

  // 状態ファイルを壊して、子プロセスを不正な応答に変える。
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  let failed = poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");

  assert_eq!(failed.projects_failed, 1);
  let delayed = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    delayed.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Delayed),
    "1回の失敗は遅延中です"
  );
  assert!(delayed.is_connected(), "遅延中は接続中として扱います");
  assert_eq!(delayed.last_read_at, Some(first_read_at), "最後に読めた時刻を保持します");
  assert_eq!(
    delayed.snapshot.expect("状態を保持します").goals[0].goal_id.as_str(),
    "g1",
    "最後に読めた状態を保持します"
  );
  let events: Vec<EventKind> =
    harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  assert!(events.is_empty(), "1回の失敗では接続断を記録しません: {events:?}");

  harness.rewrite_state(state_with_goal("g1"));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  let recovered = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(recovered.connection_state, Some(takt_dash_backend::model::ConnectionState::Connected));
  poller.shutdown().await;
}

#[tokio::test]
async fn threshold_breach_records_connection_lost_once_and_restore_records_restored() {
  let harness = Harness::new(state_with_goal("g1"));
  harness.rewrite_connection_thresholds(1, 1);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  // 最後に読めてから1秒以上待ってから失敗させ、回数と時間の両方のしきい値を超えさせる。
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");

  let unresponsive = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    unresponsive.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Unresponsive),
    "しきい値を超えた失敗で応答なしになります"
  );
  assert!(!unresponsive.is_connected(), "応答なしは接続中ではありません");

  // 応答なしが続いても切断イベントを重ねない。
  poller.read_cycle_once().await.expect("2回目の失敗でもサイクルは完了します");
  let lost: Vec<_> = harness
    .activity
    .recent_events(10)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .collect();
  assert_eq!(lost.len(), 1, "接続断は応答なしへ移ったときの1件だけです");

  // 復旧させ、同じ Poller の次のサイクルで復帰イベントと切断時間を記録する。
  harness.rewrite_state(state_with_goal("g1"));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  let restored: Vec<_> = harness
    .activity
    .recent_events(20)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .collect();
  assert_eq!(restored.len(), 1, "復帰イベントを1件だけ記録します");
  assert!(
    restored[0].body.contains("切断していた時間は") && restored[0].body.contains("秒です。"),
    "復帰イベントの本文に切断していた時間を入れます: {}",
    restored[0].body
  );
  poller.shutdown().await;
}

#[tokio::test]
async fn threshold_change_is_applied_on_the_same_running_poller() {
  // 「今」から120分前の問い合わせを作る。初期の240分基準では滞留せず、1分基準では滞留する。
  let created_at = (chrono::Utc::now() - chrono::Duration::minutes(120)).to_rfc3339();
  let harness = Harness::new(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": created_at } ]
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  let before =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::Inquiry);
  assert_eq!(before.len(), 1);
  assert!(!before[0].stale, "240分基準では120分前の問い合わせは滞留しません");

  // 同じ Poller 実体と状態ストアのまま、設定の閾値を1分に変える。
  let current = std::fs::read_to_string(&harness.config_path).expect("設定を読めます");
  let mut value: Value = serde_json::from_str(&current).expect("設定を解析できます");
  value["thresholds"]["pendingDecisionStaleMinutes"] = json!(1);
  write_json(&harness.config_path, &value);

  poller.read_cycle_once().await.expect("2回目のサイクルを実行できます");

  let after =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::Inquiry);
  poller.shutdown().await;
  assert_eq!(after.len(), 1);
  assert!(after[0].stale, "1分基準に変わったので同じ問い合わせが滞留になります");
}

#[tokio::test]
async fn interval_follows_config_file_changes_on_same_poller() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  let first = poller.read_cycle_once().await.expect("1サイクルを実行できます");
  assert_eq!(first.interval, std::time::Duration::from_secs(30));

  let current = std::fs::read_to_string(&harness.config_path).expect("設定を読めます");
  let mut value: Value = serde_json::from_str(&current).expect("設定を解析できます");
  value["intervalSeconds"] = json!(5);
  write_json(&harness.config_path, &value);

  let second = poller.read_cycle_once().await.expect("2サイクルを実行できます");
  poller.shutdown().await;
  assert_eq!(second.interval, std::time::Duration::from_secs(5), "稼働中でも設定変更に従います");
}

#[tokio::test]
async fn two_projects_read_independently_and_one_failure_does_not_stop_the_other() {
  let harness = Harness::new(state_with_goal("g1"));
  let directory = harness._directory.path();
  // 2つ目のプロジェクトは専用の state ファイルを持つ別サーバーに割り当てる。
  let project_two_dir = directory.join("project-two");
  std::fs::create_dir_all(&project_two_dir).expect("2つ目の cwd を作れます");
  let state_two = directory.join("state-two.json");
  write_json(&state_two, &state_with_goal("g2"));
  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&harness.config_path).unwrap()).unwrap();
  config["mcpServers"]["mini-two"] = json!({
    "command": "node",
    "args": [mock_script().display().to_string(), "--state", state_two.display().to_string()],
    "env": {}
  });
  config["servers"]["mini-two"] = json!({ "name": "j5ik2o-mac-mini-2", "kind": "Mac mini", "concurrency": 4 });
  config["projects"].as_array_mut().unwrap().push(json!({
    "name": "other", "server": "mini-two", "about": "説明2", "cwd": project_two_dir.display().to_string()
  }));
  write_json(&harness.config_path, &config);

  // 2つ目のプロジェクトの状態ファイルを壊し、その子プロセスだけを死亡させる。
  std::fs::write(&state_two, "{ this is not json").expect("壊れた状態ファイルを書けます");

  let mut poller = harness.poller();
  let report = poller.read_cycle_once().await.expect("1サイクルを実行できます");

  assert_eq!(report.projects_read, 1, "健常なプロジェクトだけを読めた件数に数えます");
  assert_eq!(report.projects_failed, 1, "失敗したプロジェクトを失敗件数に数えます");
  let healthy = harness.store.slot("takt-dash").expect("健常側のスロットがあります");
  assert!(healthy.is_connected(), "健常なプロジェクトは接続中のままです");
  let failed = harness.store.slot("other").expect("失敗側のスロットがあります");
  assert_eq!(
    failed.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Delayed),
    "1回失敗したプロジェクトは遅延中です"
  );
  assert!(failed.is_connected(), "遅延中は失敗したプロジェクトも接続中として扱います");
  poller.shutdown().await;
}

#[tokio::test]
async fn goal_pause_transition_emits_paused_event_through_the_cycle() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_state(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "paused", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ]
  }));
  let report = poller.read_cycle_once().await.expect("2回目のサイクルを実行できます");

  let events: Vec<EventKind> =
    harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  poller.shutdown().await;
  assert!(report.events_recorded >= 1);
  assert!(events.contains(&EventKind::Paused), "一時停止への遷移を出来事として記録します: {events:?}");
}

#[tokio::test]
async fn goal_read_failure_is_not_recorded_as_a_successful_partial_snapshot() {
  // ゴールの読み取りが isError で失敗したサイクルは全体の失敗として扱い、
  // ゴール質問が欠けた部分 snapshot を成功として記録しません。
  let harness = Harness::new(json!({ "goals": [], "isError": ["takt_list_goals"] }));
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");

  assert_eq!(report.projects_read, 0, "ゴールの読み取り失敗は成功件数に数えません");
  assert_eq!(report.projects_failed, 1, "ゴールの読み取り失敗は失敗件数に数えます");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    slot.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Delayed),
    "1回の読み取り失敗は遅延中です"
  );
  assert!(slot.snapshot.is_none(), "部分 snapshot を成功として記録しません");
  poller.shutdown().await;
}

#[tokio::test]
async fn goal_question_tool_error_keeps_previous_inquiries_without_duplicate_events_on_recovery() {
  // g1 のゴール質問だけが isError を返すサイクルでも、g1 の未回答質問を前回値で保持する。
  // 復旧しても同一 inquiry_id の Inquiry 出来事を重複記録しない。
  let harness = Harness::new(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] }
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  // ゴール質問の道具エラーを注入する。
  harness.rewrite_state(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] },
    "isError": ["takt_list_goal_questions:g1"]
  }));
  let during = poller.read_cycle_once().await.expect("道具エラーでもサイクルは完了します");
  assert_eq!(during.projects_read, 1, "ゴール質問の道具エラーは読み取り成功です");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.is_connected(), "ゴール質問の道具エラーは connected を偽にしません");
  let snapshot = slot.snapshot.expect("状態があります");
  assert!(
    snapshot.inquiries.iter().any(|inquiry| inquiry.inquiry_id.as_str() == "q1"),
    "道具エラー中も前回の未回答質問を保持します"
  );

  // 復旧させ、同じ q1 が再び読めるようになる。
  harness.rewrite_state(json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] }
  }));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  poller.shutdown().await;

  let inquiries: Vec<EventKind> = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::Inquiry)
    .map(|event| event.kind)
    .collect();
  assert!(
    inquiries.is_empty(),
    "q1 は初回から既知なので、道具エラーを挟んでも復旧で Inquiry 出来事を重複記録しません: {inquiries:?}"
  );
}

#[tokio::test]
async fn notification_tool_error_keeps_awaiting_merge_start_and_avoids_duplicate_blocked_events() {
  let past = (chrono::Utc::now() - chrono::Duration::minutes(300)).to_rfc3339();
  let state = json!({
    "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
    "tasks": [ { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true } ],
    "notifications": { "g1": [
      { "id": "n-merge", "kind": "awaiting_merge", "body": "マージ待ち", "recordedAt": past },
      { "id": "n-blocked", "kind": "blocked", "body": "依存解決に失敗しました", "recordedAt": past }
    ] }
  });
  let harness = Harness::new(state.clone());
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let expected_start = harness
    .store
    .pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge)[0]
    .created_at;

  // g1 の通知の道具エラーを注入する。
  let mut errored = state.clone();
  errored["isError"] = json!(["takt_get_goal:g1"]);
  harness.rewrite_state(errored);
  poller.read_cycle_once().await.expect("通知の道具エラーでもサイクルは完了します");

  let during =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert_eq!(during.len(), 1, "道具エラー中もマージ待ちの判断待ちを保持します");
  assert_eq!(during[0].created_at, expected_start, "道具エラー中も awaiting_merge の起点時刻を前回値で保持します");

  // 復旧させ、blocked 通知が再び読めるようになる。
  harness.rewrite_state(state);
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  poller.shutdown().await;

  let blocked: usize =
    harness.activity.recent_events(50).unwrap().into_iter().filter(|event| event.kind == EventKind::Blocked).count();
  assert_eq!(blocked, 0, "n-blocked は初回から既知なので、道具エラーを挟んでも復旧で Blocked 出来事を重複記録しません");
}

#[tokio::test]
async fn standalone_inquiry_tool_error_keeps_connected_and_previous_inquiry() {
  // takt_list_standalone_inquiries の isError は道具エラーであり接続断ではありません。
  // サイクルは成功し、前回の standalone 問い合わせを判断待ちに保持する。
  let harness = Harness::new(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "作業をお願いします", "createdAt": "2026-11-13T09:00:00Z" } ]
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_state(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "作業をお願いします", "createdAt": "2026-11-13T09:00:00Z" } ],
    "isError": ["takt_list_standalone_inquiries"]
  }));
  let report = poller.read_cycle_once().await.expect("standalone inquiries の道具エラーでサイクルは失敗しません");

  assert_eq!(report.projects_read, 1, "standalone inquiries の道具エラーは読み取り成功です");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.is_connected(), "道具エラーは connected を偽にしません");
  let events: Vec<EventKind> =
    harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  assert!(!events.contains(&EventKind::ConnectionLost), "偽の ConnectionLost を記録しません: {events:?}");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::Inquiry);
  assert!(
    decisions.iter().any(|decision| decision.body == "作業をお願いします"),
    "前回の standalone 問い合わせを保持します"
  );

  // 復旧させ、同じ standalone 問い合わせが再び読めるようになる。s1 は初回から既知なので、
  // 道具エラーを挟んでも復旧で同一 inquiry_id の Inquiry 出来事を重複記録しない。
  harness.rewrite_state(json!({
    "goals": [],
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "operation_request", "body": "作業をお願いします", "createdAt": "2026-11-13T09:00:00Z" } ]
  }));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  poller.shutdown().await;

  let inquiries: usize =
    harness.activity.recent_events(50).unwrap().into_iter().filter(|event| event.kind == EventKind::Inquiry).count();
  assert_eq!(inquiries, 0, "s1 は初回から既知なので、復旧で Inquiry 出来事を重複記録しません");
}

#[tokio::test]
async fn running_run_is_built_from_the_run_files_without_calling_takt_get_run() {
  // 一覧の run は meta.json と本体ログから作る。mock の takt_get_run は status・currentStep・
  // stepLogs を別の値で返すため、takt_get_run
  // から作る実装や両方を読む実装は値か呼び出し記録で落ちる。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 2, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record("development-core"),
    step_complete_record("plan", "architect", 1, "2026-11-13T10:00:00Z"),
  ]);
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r1").expect("running の run が一覧にあります");
  let calls = harness.calls();
  poller.shutdown().await;

  assert_eq!(report.projects_read, 1);
  assert_eq!(run.status, takt_dash_backend::model::RunStatus::Running, "status は meta.json の値です");
  assert_eq!(run.current_step.as_deref(), Some("implement"), "現在ステップは meta.json の値です");
  assert_eq!(run.current_iteration, Some(2), "現在の iteration は meta.json の値です");
  assert_eq!(run.started_at, Some(time("2026-11-13T09:00:00Z")), "開始時刻は meta.json の startTime です");
  assert_eq!(step_names(&run), vec!["plan".to_owned()], "steps は本体ログの step_complete です");
  assert_eq!(run.steps[0].started_at, time("2026-11-13T10:00:00Z"), "step の時刻は step_complete の timestamp です");
  assert_eq!(run.steps[0].ended_at, None, "終了時刻は持ちません");
  assert_eq!(run.steps[0].model, "", "model は空のままです");
  assert_eq!(run.task_name.as_str(), "task-r1", "タスク名は takt_list_tasks のタスクから補います");
  assert!(!calls.iter().any(|call| call == "takt_get_run"), "定期読み取りは takt_get_run を呼びません: {calls:?}");
}

#[tokio::test]
async fn step_position_uses_the_meta_workflow_and_the_current_step() {
  // ステップ位置は meta.json の workflow を takt_get_workflow で引き、meta.json の currentStep の
  // 位置で決める。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[workflow_start_record("development-core")]);
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r1").expect("running の run が一覧にあります");
  poller.shutdown().await;

  assert_eq!(run.current_step_number, Some(2), "implement は workflow の 2 番目です");
  assert_eq!(run.total_steps, Some(3), "workflow の step は 3 件です");
}

#[tokio::test]
async fn appended_step_complete_is_added_on_the_next_cycle_of_the_same_poller() {
  // 同じ Poller の上で、1周期目の後に追記した step_complete が2周期目に加わる。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record("development-core"),
    step_complete_record("plan", "architect", 1, "2026-11-13T10:00:00Z"),
  ]);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1周期目を実行できます");
  let first = stored_run(&harness, "r1").expect("1周期目の run があります");
  assert_eq!(step_names(&first), vec!["plan".to_owned()]);

  fixture.append_records("r1", &[step_complete_record("implement", "coder", 1, "2026-11-13T10:05:00Z")]);
  poller.read_cycle_once().await.expect("2周期目を実行できます");
  let second = stored_run(&harness, "r1").expect("2周期目の run があります");
  let calls = harness.calls();
  poller.shutdown().await;

  assert_eq!(
    step_names(&second),
    vec!["plan".to_owned(), "implement".to_owned()],
    "追記した step_complete だけが加わり、既読の step を重複させません"
  );
  assert!(!calls.iter().any(|call| call == "takt_get_run"), "どの周期も takt_get_run を呼びません: {calls:?}");
}

#[tokio::test]
async fn already_read_part_of_an_unchanged_run_log_is_not_read_again_on_the_same_poller() {
  // 1周期目で読んだ範囲を、inode と大きさを変えずに同じバイト長の別の文字列へ書き換える。
  // 毎周期 先頭から読み直す実装は2周期目に "PLAN" を返して落ちる。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record("development-core"),
    step_complete_record("plan", "architect", 1, "2026-11-13T10:00:00Z"),
  ]);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  fixture.overwrite_body_log_in_place("r1", "\"plan\"", "\"PLAN\"");
  poller.read_cycle_once().await.expect("2周期目を実行できます");
  let run = stored_run(&harness, "r1").expect("2周期目の run があります");
  poller.shutdown().await;

  assert_eq!(step_names(&run), vec!["plan".to_owned()], "既読範囲は読み直さず、1周期目の集計を保ちます");
}

#[tokio::test]
async fn top_level_step_start_of_the_current_iteration_sets_the_current_step_started_at() {
  // meta の currentStep・currentIteration・workflow に一致する最上位の step_start
  // だけが現在ステップの 開始時刻になる。iteration の異なる先の step_start を採る実装は 10:00
  // を返して落ちる。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 3, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record("development-core"),
    step_start_record("implement", "development-core", 2, "2026-11-13T10:00:00Z", &["implement"]),
    step_start_record("implement", "development-core", 3, "2026-11-13T10:10:00Z", &["implement"]),
  ]);
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r1").expect("running の run が一覧にあります");
  poller.shutdown().await;

  assert_eq!(run.current_step_started_at, Some(time("2026-11-13T10:10:00Z")));
  assert!(run.steps.is_empty(), "step_start はステップの記録になりません: {:?}", run.steps);
}

#[tokio::test]
async fn nested_step_start_with_the_same_name_is_not_the_current_step_start() {
  // stack の長さが 2 で外側の step が review の step_start は、名前が currentStep と同じでも
  // 現在ステップの開始ではない。step_start もステップの記録にはならない。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[
    workflow_start_record("development-core"),
    step_start_record("implement", "development-core", 1, "2026-11-13T10:10:00Z", &["review", "implement"]),
  ]);
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r1").expect("running の run が一覧にあります");
  poller.shutdown().await;

  assert!(run.steps.is_empty(), "step_start はステップの記録になりません: {:?}", run.steps);
  assert_eq!(run.current_step_started_at, None, "入れ子の同名 step_start は最上位の一致として数えません");
}

#[tokio::test]
async fn run_reports_are_listed_from_the_reports_directory() {
  // 一覧の reports は run の reports/ の通常ファイルを、/ 区切りの相対パスの昇順と大きさで返す。
  // takt_get_run の reports（decoy.md）は使わない。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[workflow_start_record("development-core")]);
  fixture.write_report("r1", "subworkflows/review.md", "レビュー");
  fixture.write_report("r1", "plan.md", "0123456789");
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r1").expect("running の run が一覧にあります");
  poller.shutdown().await;

  let reports: Vec<(String, i64)> =
    run.reports.iter().map(|report| (report.filename.clone(), report.size_bytes)).collect();
  assert_eq!(reports, vec![("plan.md".to_owned(), 10), ("subworkflows/review.md".to_owned(), "レビュー".len() as i64)]);
}

#[tokio::test]
async fn unreadable_run_keeps_its_previous_value_while_healthy_runs_use_the_new_value() {
  // 1周期目は両方の run を読めた。2周期目は r2 の meta.json を壊し、r1 に step を追記する。
  // r2 だけ前回値に戻り、r1 は今回値になり、プロジェクトは読み取り成功のまま接続断を記録しない。
  // takt_get_run も失敗させ、保存場所でも takt-mcp でも読めない run の扱いを確かめる。
  let harness = Harness::new(with_failing_get_run(state_with_running_tasks(&["r1", "r2"])));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }, RunTaskLine {
    run_slug:        Some("r2"),
    source_run_slug: None,
  }]);
  for run_slug in ["r1", "r2"] {
    fixture.write_meta(run_slug, &running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z"));
    fixture.append_records(run_slug, &[
      workflow_start_record("development-core"),
      step_complete_record("plan", "architect", 1, "2026-11-13T10:00:00Z"),
    ]);
  }
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1周期目を実行できます");
  let previous_r2 = stored_run(&harness, "r2").expect("1周期目の r2 があります");

  fixture.write_meta_raw("r2", "{ this is not json");
  fixture.append_records("r1", &[step_complete_record("implement", "coder", 1, "2026-11-13T10:05:00Z")]);
  let report = poller.read_cycle_once().await.expect("2周期目を実行できます");
  let r1 = stored_run(&harness, "r1").expect("2周期目の r1 があります");
  let r2 = stored_run(&harness, "r2").expect("読めない r2 は前回値で残ります");
  poller.shutdown().await;

  assert_eq!(report.projects_read, 1, "run を1件読めなくてもプロジェクトは読み取り成功です");
  assert_eq!(report.projects_failed, 0);
  assert_eq!(r2, previous_r2, "読めない run は前回の同じ run の値です");
  assert_eq!(step_names(&r1), vec!["plan".to_owned(), "implement".to_owned()], "健常な run は今回の値です");
  let events: Vec<EventKind> =
    harness.activity.recent_events(10).unwrap().into_iter().map(|event| event.kind).collect();
  assert!(!events.contains(&EventKind::ConnectionLost), "run の読み取り失敗で接続断を記録しません: {events:?}");
}

#[tokio::test]
async fn unreadable_run_without_a_previous_value_is_left_out_of_the_list() {
  // 前回値の無い run が保存場所でも takt-mcp でも読めないときは、その run だけ一覧から欠落し、
  // 健常な run は一覧に入る。
  let harness = Harness::new(with_failing_get_run(state_with_running_tasks(&["r1", "r2"])));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }, RunTaskLine {
    run_slug:        Some("r2"),
    source_run_slug: None,
  }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r1", &[workflow_start_record("development-core")]);
  fixture.write_meta_raw("r2", "{ this is not json");
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("1周期を実行できます");
  let r1 = stored_run(&harness, "r1");
  let r2 = stored_run(&harness, "r2");
  poller.shutdown().await;

  assert_eq!(report.projects_read, 1);
  assert!(r1.is_some(), "健常な run は一覧に入ります");
  assert!(r2.is_none(), "前回値の無い読めない run は一覧から欠落します");
}

/// running なタスク `run_slugs` を持ち、takt_get_run が r-remote だけを実行中の run として返す
/// mock の状態。r-remote の値はリモートのサーバーにある run を表し、ローカルの保存場所には無い。
fn state_with_remote_run(run_slugs: &[&str]) -> Value {
  let tasks: Vec<Value> = run_slugs
    .iter()
    .map(|run_slug| {
      json!({ "name": format!("task-{run_slug}"), "status": "running", "runSlug": run_slug, "startedAt": "2026-11-13T08:00:00Z" })
    })
    .collect();
  json!({
    "tasks": tasks,
    "runs": {
      "r-remote": {
        "runSlug": "r-remote",
        "status": "running",
        "workflow": "development-core",
        "currentStep": "implement",
        "currentIteration": 2,
        "startedAt": "2026-11-13T09:00:00Z",
        "stepLogs": [ { "step": "plan", "persona": "architect", "status": "done", "timestamp": "2026-11-13T09:10:00Z", "iteration": 1 } ],
        "reports": [ { "filename": "plan.md", "sizeBytes": 10 } ]
      }
    },
    "workflows": { "development-core": ["plan", "implement", "review"] }
  })
}

#[tokio::test]
async fn run_of_a_project_without_a_local_task_table_is_built_from_takt_get_run() {
  // リモートのサーバーのプロジェクトは、ローカルに tasks.yaml
  // が無く保存場所を読めない。その実行中の run は takt_get_run
  // の値で一覧に入る。ローカルの読み取りだけに頼る実装は run が一覧から消えて落ちる。
  let harness = Harness::new(state_with_remote_run(&["r-remote"]));
  let mut poller = harness.poller();

  let report = poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r-remote");
  let calls = harness.calls();
  poller.shutdown().await;

  assert_eq!(report.projects_read, 1);
  let run = run.expect("ローカルで読めない実行中の run も一覧に入ります");
  assert_eq!(run.status, takt_dash_backend::model::RunStatus::Running);
  assert_eq!(run.current_step.as_deref(), Some("implement"), "現在ステップは takt_get_run の値です");
  assert_eq!(run.current_iteration, Some(2));
  assert_eq!(run.started_at, Some(time("2026-11-13T09:00:00Z")));
  assert_eq!(step_names(&run), vec!["plan".to_owned()], "steps は takt_get_run の stepLogs です");
  let reports: Vec<(String, i64)> =
    run.reports.iter().map(|report| (report.filename.clone(), report.size_bytes)).collect();
  assert_eq!(reports, vec![("plan.md".to_owned(), 10)]);
  assert!(
    calls.iter().any(|call| call == "takt_get_run"),
    "ローカルで読めない run は takt_get_run で読みます: {calls:?}"
  );
}

#[tokio::test]
async fn remote_run_with_more_than_one_page_of_step_logs_collects_every_page_and_uses_the_last_step_time() {
  // takt_get_run は stepLogs を1ページ 50 件で返す（mock も実 takt-mcp と同じ大きさ）。51 件の
  // stepLogs を持つリモートの run（ローカルに tasks.yaml が無く Unreadable）では、2ページ目まで
  // 集めて steps が 51 件になり、最終ログ時刻は最後のページの最後のステップの時刻になる。
  // 1ページ目だけを読む実装は steps が 50 件で s49 の時刻になって落ちる。
  let mut state = state_with_remote_run(&["r-remote"]);
  let step_logs: Vec<Value> = (0..51)
    .map(|index| {
      json!({
        "step": format!("s{index}"),
        "persona": "coder",
        "status": "done",
        "timestamp": (time("2026-11-13T09:00:00Z") + chrono::Duration::minutes(index)).to_rfc3339(),
        "iteration": 1
      })
    })
    .collect();
  state["runs"]["r-remote"]["stepLogs"] = json!(step_logs);
  let harness = Harness::new(state);
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r-remote").expect("ローカルで読めない実行中の run も一覧に入ります");
  poller.shutdown().await;

  assert_eq!(run.steps.len(), 51, "2ページ目の stepLogs まで集めます: {:?}", step_names(&run));
  assert_eq!(run.steps.last().map(|step| step.step_name.as_str()), Some("s50"));
  assert_eq!(
    run.last_log_at,
    Some(time("2026-11-13T09:00:00Z") + chrono::Duration::minutes(50)),
    "最終ログ時刻は最後のページの最後のステップの時刻です"
  );
}

#[tokio::test]
async fn run_missing_from_the_local_task_table_is_built_from_takt_get_run_while_local_runs_are_not() {
  // ローカルの tasks.yaml に載っていない run（NotListed）は takt_get_run で読み、載っている run は
  // 保存場所から読む。takt_get_run は r-remote の1回だけで、ローカルで読めた run には呼ばない。
  let harness = Harness::new(state_with_remote_run(&["r-local", "r-remote"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r-local"), source_run_slug: None }]);
  fixture.write_meta("r-local", &running_meta("review", 1, "development-core", "2026-11-13T09:00:00Z"));
  fixture.append_records("r-local", &[workflow_start_record("development-core")]);
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1周期を実行できます");
  let local = stored_run(&harness, "r-local").expect("ローカルの run は一覧にあります");
  let remote = stored_run(&harness, "r-remote");
  let calls = harness.calls();
  poller.shutdown().await;

  assert_eq!(local.current_step.as_deref(), Some("review"), "ローカルの run は meta.json の値です");
  let remote = remote.expect("tasks.yaml に無い実行中の run も一覧に入ります");
  assert_eq!(remote.current_step.as_deref(), Some("implement"), "対応表に無い run は takt_get_run の値です");
  assert_eq!(
    calls.iter().filter(|call| *call == "takt_get_run").count(),
    1,
    "takt_get_run はローカルで読めない run の分だけ呼びます: {calls:?}"
  );
}

#[tokio::test]
async fn run_summary_is_the_first_line_of_the_meta_task() {
  // 一覧の run の要約は meta.json の task の1行目です。空のままにする実装は落ちる。
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  let mut meta = running_meta("implement", 1, "development-core", "2026-11-13T09:00:00Z");
  meta["task"] = json!("認証処理を実装する\n詳細: トークンの検証を加える");
  fixture.write_meta("r1", &meta);
  fixture.append_records("r1", &[workflow_start_record("development-core")]);
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1周期を実行できます");
  let run = stored_run(&harness, "r1").expect("running の run が一覧にあります");
  poller.shutdown().await;

  assert_eq!(run.summary, "認証処理を実装する");
}

#[tokio::test]
async fn merge_wait_uses_notification_recorded_at_and_becomes_stale_through_the_cycle() {
  // awaiting_merge 通知の recordedAt を十分に過去にすると、読み取り後に人のマージ待ちが滞留になる。
  let past = (chrono::Utc::now() - chrono::Duration::minutes(300)).to_rfc3339();
  let harness = Harness::new(json!({
    "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
    "tasks": [ { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true } ],
    "notifications": { "g1": [ { "id": "n-merge", "kind": "awaiting_merge", "body": "マージ待ち", "recordedAt": past } ] }
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1サイクルを実行できます");

  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  poller.shutdown().await;

  assert_eq!(decisions.len(), 1, "人のマージ待ちを判断待ちに含めます");
  assert!(decisions[0].stale, "通知の recordedAt が240分より前なので滞留になります");
}

#[tokio::test]
async fn merge_wait_without_notification_keeps_first_observed_start_through_the_cycle() {
  // awaiting_merge 通知が無いゴールでも、poller の初回サイクル（prev=None）で起点を確定し、
  // 以降の投影で同じ created_at を保ち、時間経過で滞留になる。起点を都度 now にすると
  // created_at が投影ごとに進み、300 分後の投影でも滞留にならない。
  let harness = Harness::new(json!({
    "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
    "tasks": [ { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true } ],
    "notifications": { "g1": [ { "id": "n-progress", "kind": "progress", "body": "進捗", "recordedAt": "2026-11-13T08:00:00Z" } ] }
  }));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  let first =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert_eq!(first.len(), 1, "通知が無くても人のマージ待ちを判断待ちに含めます");
  let started_at = first[0].created_at;
  assert!(!first[0].stale, "確定直後の起点は滞留ではありません");

  let later = harness.store.pending_decisions(
    chrono::Utc::now() + chrono::Duration::minutes(300),
    takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge,
  );
  poller.shutdown().await;

  assert_eq!(later.len(), 1);
  assert_eq!(later[0].created_at, started_at, "時間が経っても起点は初回観測のまま固定されます");
  assert!(later[0].stale, "初回観測の起点から240分を超えた待ちは滞留になります");
}

#[tokio::test]
async fn change_notification_is_sent_once_when_the_state_changes_between_cycles() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();

  // 未読から読めた状態への初回の遷移も状態の変化なので通知します。
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  assert!(changes.try_recv().is_ok(), "初回の読み取りは未読からの変化なので通知します");

  // 状態を変えないサイクルでは通知しません。
  poller.read_cycle_once().await.expect("状態を変えないサイクルを実行できます");
  assert!(changes.try_recv().is_err(), "状態を変えないサイクルでは通知しません");

  harness.rewrite_state(state_with_goal("g2"));
  let required = poller.read_cycle_once().await.expect("状態を変えるサイクルを実行できます");

  assert!(required.projects_read > 0);
  assert!(changes.try_recv().is_ok(), "状態が変わったサイクルでは通知を1回送ります");
  assert!(changes.try_recv().is_err(), "同じサイクルで通知を重ねて送りません");

  poller.shutdown().await;
}

#[tokio::test]
async fn change_notification_is_sent_when_a_project_becomes_delayed() {
  let harness = Harness::new(state_with_goal("g1"));
  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let _ = changes.try_recv();

  // 状態ファイルを壊して、子プロセスを不正な応答に変える。遅延中への変化も通知します。
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");

  assert!(changes.try_recv().is_ok(), "遅延中への変化も通知します");
  poller.shutdown().await;
}

#[tokio::test]
async fn cycle_log_reports_duration_projects_and_per_tool_call_counts() {
  let harness = Harness::new(state_with_goal("g1"));
  let (logs, _guard) = capture_logs();
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("1サイクルを実行できます");
  poller.shutdown().await;

  let line = single_cycle_log_line(&logs);
  assert!(line.contains("cycle_ms="), "周期の所要時間を出します: {line}");
  assert!(line.contains("projects=1"), "設定上のプロジェクト数を出します: {line}");
  assert!(line.contains("tool_calls=6"), "道具呼び出しの総数を出します: {line}");
  assert!(line.contains("tool_timeouts=0"), "タイムアウト件数を出します: {line}");
  assert!(line.contains("takt_list_goals(calls=1,"), "道具名ごとの回数を出します: {line}");
  assert!(line.contains("takt_get_goal(calls=1,"), "道具名ごとの回数を出します: {line}");
  assert!(
    line.contains("takt_list_goal_registration_proposals(calls=1,"),
    "ゴール登録提案も1サイクルに1回だけ読みます: {line}"
  );

  let goals = tool_entry(&line, "takt_list_goals");
  assert!(goals.contains("calls=1,"), "道具エントリに回数を出します: {goals}");
  assert!(goals.contains("failures=0,"), "道具エントリに失敗件数を出します: {goals}");
  assert!(goals.contains("timeouts=0,"), "道具エントリにタイムアウト件数を出します: {goals}");
  assert!(goals.contains("total_ms="), "道具エントリに合計時間を出します: {goals}");
  assert!(goals.contains("max_ms="), "道具エントリに最大時間を出します: {goals}");
  assert!(goals.contains("p95_ms="), "道具エントリに p95 を出します: {goals}");
}

#[tokio::test]
async fn cycle_log_records_failed_tool_calls_and_separates_projects_from_projects_read() {
  let harness = Harness::new(json!({ "goals": [], "isError": ["takt_list_goals"] }));
  let (logs, _guard) = capture_logs();
  let mut poller = harness.poller();

  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");
  poller.shutdown().await;

  let line = single_cycle_log_line(&logs);
  assert!(line.contains("projects=1"), "設定上のプロジェクト数を出します: {line}");
  assert!(line.contains("projects_read=0"), "読み取り成功数と区別します: {line}");
  assert!(line.contains("projects_failed=1"), "失敗したプロジェクト数を出します: {line}");
  assert!(line.contains("tool_calls=1"), "失敗した呼び出しも1回として記録します: {line}");
  assert!(line.contains("takt_list_goals(calls=1,"), "失敗した呼び出しも道具名ごとに記録します: {line}");
  assert!(line.contains("failures=1"), "失敗した呼び出しを失敗として数えます: {line}");

  assert!(line.contains("tool_failures=1"), "周期全体の失敗件数を出します: {line}");
  let goals = tool_entry(&line, "takt_list_goals");
  assert!(goals.contains("calls=1,"), "道具エントリに回数を出します: {goals}");
  assert!(goals.contains("failures=1,"), "道具別の失敗件数を出します: {goals}");
  assert!(goals.contains("timeouts=0,"), "道具エントリにタイムアウト件数を出します: {goals}");
  assert!(goals.contains("total_ms="), "道具エントリに合計時間を出します: {goals}");
  assert!(goals.contains("max_ms="), "道具エントリに最大時間を出します: {goals}");
  assert!(goals.contains("p95_ms="), "道具エントリに p95 を出します: {goals}");
}

#[tokio::test]
async fn completed_goals_do_not_increase_tool_calls_between_cycles() {
  let without_goals = run_two_cycles(state_with_goals(0, "completed", "active")).await;
  let with_completed = run_two_cycles(state_with_goals(20, "completed", "active")).await;

  assert_eq!(
    tool_calls_value(&without_goals.second_line),
    tool_calls_value(&with_completed.second_line),
    "完了ゴールが 20 件あっても、状態が変わらない 2 周期目の呼び出し回数は 0 件のときと同じです"
  );
  assert!(with_completed.second_line.contains("projects_read=1"), "{}", with_completed.second_line);
  assert_eq!(with_completed.snapshot.goals.len(), 20, "20 件の完了ゴールを保持します");

  let first_questions = with_completed.first_calls.iter().filter(|call| *call == "takt_list_goal_questions").count();
  assert_eq!(first_questions, 20, "1 周期目は各ゴールの質問を読んで基準化します: {:?}", with_completed.first_calls);
  assert!(
    with_completed.first_calls.iter().any(|call| call == "takt_get_goal"),
    "1 周期目は各ゴールの通知を読んで基準化します: {:?}",
    with_completed.first_calls
  );

  assert!(
    !with_completed.second_calls.iter().any(|call| call == "takt_list_goal_questions"),
    "状態が変わらない完了ゴールの質問は 2 周期目に読みません: {:?}",
    with_completed.second_calls
  );
  assert!(
    !with_completed.second_calls.iter().any(|call| call == "takt_get_goal"),
    "状態が変わらない完了ゴールの通知は 2 周期目に読みません: {:?}",
    with_completed.second_calls
  );
}

#[tokio::test]
async fn terminal_goal_skips_aux_reads_and_keeps_pending_items_without_new_events() {
  let state = json!({
    "goals": [ { "id": "g1", "status": "completed", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] },
    "notifications": { "g1": [ { "id": "n-blocked", "kind": "blocked", "body": "依存解決に失敗しました", "recordedAt": "2026-11-13T09:30:00Z" } ] }
  });
  let harness = Harness::new(state);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1 周期目を実行できます");
  harness.clear_calls();
  let report = poller.read_cycle_once().await.expect("2 周期目を実行できます");
  let snapshot = harness.store.slot("takt-dash").and_then(|slot| slot.snapshot).expect("スナップショットがあります");
  let second_calls = harness.calls();
  poller.shutdown().await;

  assert_eq!(report.events_recorded, 0, "状態が変わらない周期では新しい出来事を記録しません");
  assert!(
    snapshot.inquiries.iter().any(|inquiry| inquiry.inquiry_id.as_str() == "q1"),
    "スキップしたゴールの未回答問い合わせを前回値から引き継ぎます"
  );
  assert!(
    snapshot.notifications.iter().any(|notification| notification.notification_id == "n-blocked"),
    "スキップしたゴールの blocked 通知を前回値から引き継ぎます"
  );
  assert!(
    !second_calls.iter().any(|call| call == "takt_list_goal_questions"),
    "完了ゴールの質問は 2 周期目に読みません: {second_calls:?}"
  );
  assert!(
    !second_calls.iter().any(|call| call == "takt_get_goal"),
    "完了ゴールの通知は 2 周期目に読みません: {second_calls:?}"
  );

  let blocked: usize =
    harness.activity.recent_events(50).unwrap().into_iter().filter(|event| event.kind == EventKind::Blocked).count();
  assert_eq!(blocked, 0, "引き継いだ通知を新しい出来事として重複記録しません");
}

#[tokio::test]
async fn awaiting_merge_goal_is_read_on_every_cycle() {
  let state = json!({
    "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T09:00:00Z" } ] },
    "notifications": { "g1": [ { "id": "n-merge", "kind": "awaiting_merge", "body": "マージ待ち", "recordedAt": "2026-11-13T09:30:00Z" } ] }
  });
  let harness = Harness::new(state);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1 周期目を実行できます");
  harness.clear_calls();
  poller.read_cycle_once().await.expect("2 周期目を実行できます");
  let second_calls = harness.calls();
  poller.shutdown().await;

  assert!(
    second_calls.iter().any(|call| call == "takt_list_goal_questions"),
    "人のマージ待ちは終端ではないため、2 周期目も質問を読みます: {second_calls:?}"
  );
  assert!(
    second_calls.iter().any(|call| call == "takt_get_goal"),
    "人のマージ待ちは終端ではないため、2 周期目も通知を読みます: {second_calls:?}"
  );
}

#[tokio::test]
async fn status_transition_to_completed_reads_auxiliary_data_again() {
  let created = json!({
    "goals": [ { "id": "g1", "status": "created", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "1 周期目の質問", "createdAt": "2026-11-13T09:00:00Z" } ] },
    "notifications": { "g1": [ { "id": "n1", "kind": "blocked", "body": "1 周期目の通知", "recordedAt": "2026-11-13T09:30:00Z" } ] }
  });
  let harness = Harness::new(created);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1 周期目を実行できます");
  harness.clear_calls();

  harness.rewrite_state(json!({
    "goals": [ { "id": "g1", "status": "completed", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [
      { "id": "q1", "status": "pending", "kind": "question", "body": "1 周期目の質問", "createdAt": "2026-11-13T09:00:00Z" },
      { "id": "q2", "status": "pending", "kind": "question", "body": "2 周期目の質問", "createdAt": "2026-11-13T10:00:00Z" }
    ] },
    "notifications": { "g1": [ { "id": "n2", "kind": "blocked", "body": "2 周期目の通知", "recordedAt": "2026-11-13T10:30:00Z" } ] }
  }));
  poller.read_cycle_once().await.expect("2 周期目を実行できます");
  let second_calls = harness.calls();
  let snapshot = harness.store.slot("takt-dash").and_then(|slot| slot.snapshot).expect("スナップショットがあります");
  poller.shutdown().await;

  assert!(
    second_calls.iter().any(|call| call == "takt_list_goal_questions"),
    "終端へ変わった周期は質問を読みます: {second_calls:?}"
  );
  assert!(
    second_calls.iter().any(|call| call == "takt_get_goal"),
    "終端へ変わった周期は通知を読みます: {second_calls:?}"
  );
  assert!(
    snapshot.inquiries.iter().any(|inquiry| inquiry.inquiry_id.as_str() == "q2"),
    "遷移周期の新しい質問を snapshot に入れます"
  );
  assert!(
    snapshot.notifications.iter().any(|notification| notification.notification_id == "n2"),
    "遷移周期の新しい通知を snapshot に入れます"
  );
}

#[tokio::test]
async fn aborted_goals_do_not_increase_tool_calls_between_cycles() {
  let without_goals = run_two_cycles(state_with_goals(0, "created", "aborted")).await;
  let with_aborted = run_two_cycles(state_with_goals(20, "created", "aborted")).await;

  assert_eq!(
    tool_calls_value(&without_goals.second_line),
    tool_calls_value(&with_aborted.second_line),
    "中止ゴールが 20 件あっても、状態が変わらない 2 周期目の呼び出し回数は 0 件のときと同じです"
  );
  assert_eq!(with_aborted.snapshot.goals.len(), 20, "20 件の中止ゴールを保持します");
  assert!(
    !with_aborted.second_calls.iter().any(|call| call == "takt_list_goal_questions"),
    "状態が変わらない中止ゴールの質問は 2 周期目に読みません: {:?}",
    with_aborted.second_calls
  );
  assert!(
    !with_aborted.second_calls.iter().any(|call| call == "takt_get_goal"),
    "状態が変わらない中止ゴールの通知は 2 周期目に読みません: {:?}",
    with_aborted.second_calls
  );
}

/// ゴールの補助読み取りの項目。
#[derive(Clone, Copy, Debug)]
enum AuxRead {
  Questions,
  Notifications,
}

impl AuxRead {
  fn tool(self) -> &'static str {
    match self {
      | AuxRead::Questions => "takt_list_goal_questions",
      | AuxRead::Notifications => "takt_get_goal",
    }
  }

  fn other(self) -> AuxRead {
    match self {
      | AuxRead::Questions => AuxRead::Notifications,
      | AuxRead::Notifications => AuxRead::Questions,
    }
  }

  /// 終端状態になってから現れる項目（q2 または n2）が snapshot にあるか。
  fn has_terminal_item(self, snapshot: &ProjectSnapshot) -> bool {
    match self {
      | AuxRead::Questions => snapshot.inquiries.iter().any(|inquiry| inquiry.inquiry_id.as_str() == "q2"),
      | AuxRead::Notifications => {
        snapshot.notifications.iter().any(|notification| notification.notification_id == "n2")
      },
    }
  }
}

/// ゴール g1 の状態と、指定した補助読み取りの道具エラーを持つ mock の状態。終端（completed）の
/// ときだけ、質問 q2 と通知 n2 が加わる。
fn aux_state(status: &str, failing: Option<AuxRead>) -> Value {
  let terminal = status == "completed";
  let mut questions = vec![
    json!({ "id": "q1", "status": "pending", "kind": "question", "body": "質問 1", "createdAt": "2026-11-13T09:00:00Z" }),
  ];
  let mut notifications =
    vec![json!({ "id": "n1", "kind": "blocked", "body": "通知 1", "recordedAt": "2026-11-13T09:30:00Z" })];
  if terminal {
    questions.push(
      json!({ "id": "q2", "status": "pending", "kind": "question", "body": "質問 2", "createdAt": "2026-11-13T10:00:00Z" }),
    );
    notifications
      .push(json!({ "id": "n2", "kind": "blocked", "body": "通知 2", "recordedAt": "2026-11-13T10:30:00Z" }));
  }
  let is_error: Vec<String> = failing.into_iter().map(|aux| format!("{}:g1", aux.tool())).collect();
  json!({
    "goals": [ { "id": "g1", "status": status, "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": questions },
    "notifications": { "g1": notifications },
    "isError": is_error
  })
}

/// 終端のゴールで `failing` の補助読み取りだけが失敗した後、同じ Poller の上で
/// 「失敗 → 次周期に読み直す → 以降は読まない」を観測する。
///
/// `from_transition` が真なら、1周期目は created で読み、2周期目に completed へ変わった周期で
/// 失敗させる。偽なら、1周期目に completed を初めて観測した周期で失敗させる。
async fn assert_terminal_aux_failure_is_retried(failing: AuxRead, from_transition: bool) {
  let harness =
    Harness::new(if from_transition { aux_state("created", None) } else { aux_state("completed", Some(failing)) });
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1周期目を実行できます");
  if from_transition {
    harness.rewrite_state(aux_state("completed", Some(failing)));
    poller.read_cycle_once().await.expect("終端へ変わる周期を実行できます");
  }
  let failed = harness.store.slot("takt-dash").and_then(|slot| slot.snapshot).expect("スナップショットがあります");
  assert!(!failing.has_terminal_item(&failed), "失敗した周期は {failing:?} の新しい項目を読めていない前提です");
  assert!(failing.other().has_terminal_item(&failed), "失敗しなかった {:?} は終端の値を読めています", failing.other());

  harness.rewrite_state(aux_state("completed", None));
  harness.clear_calls();
  poller.read_cycle_once().await.expect("読み直す周期を実行できます");
  let retry_calls = harness.calls();
  let retried = harness.store.slot("takt-dash").and_then(|slot| slot.snapshot).expect("スナップショットがあります");
  harness.clear_calls();
  poller.read_cycle_once().await.expect("状態が変わらない周期を実行できます");
  let steady_calls = harness.calls();
  poller.shutdown().await;

  assert!(
    retry_calls.iter().any(|call| call == failing.tool()),
    "終端のゴールで読めなかった {failing:?} は次の周期に読み直します: {retry_calls:?}"
  );
  assert!(failing.has_terminal_item(&retried), "読み直した {failing:?} の値を snapshot に入れます");
  assert!(
    !retry_calls.iter().any(|call| call == failing.other().tool()),
    "読めていた {:?} は読み直しません: {retry_calls:?}",
    failing.other()
  );
  assert!(
    !steady_calls.iter().any(|call| call == AuxRead::Questions.tool() || call == AuxRead::Notifications.tool()),
    "読み直しに成功した後は、状態が変わらない終端のゴールの補助読み取りを省きます: {steady_calls:?}"
  );
}

#[tokio::test]
async fn terminal_goal_question_read_failure_on_the_transition_is_retried_on_the_next_cycle() {
  assert_terminal_aux_failure_is_retried(AuxRead::Questions, true).await;
}

#[tokio::test]
async fn terminal_goal_question_read_failure_on_the_first_observation_is_retried_on_the_next_cycle() {
  assert_terminal_aux_failure_is_retried(AuxRead::Questions, false).await;
}

#[tokio::test]
async fn terminal_goal_notification_read_failure_on_the_transition_is_retried_on_the_next_cycle() {
  assert_terminal_aux_failure_is_retried(AuxRead::Notifications, true).await;
}

#[tokio::test]
async fn terminal_goal_notification_read_failure_on_the_first_observation_is_retried_on_the_next_cycle() {
  assert_terminal_aux_failure_is_retried(AuxRead::Notifications, false).await;
}

/// ゴール g1 の状態と、指定した補助読み取りの道具エラーを持つ mock の状態。質問は q1、通知は n1
/// だけで、終端になっても増えない。読み直しても画面に出る内容は変わらない。
fn unchanged_aux_state(status: &str, failing: Option<AuxRead>) -> Value {
  let is_error: Vec<String> = failing.into_iter().map(|aux| format!("{}:g1", aux.tool())).collect();
  json!({
    "goals": [ { "id": "g1", "status": status, "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ],
    "goalQuestions": { "g1": [ { "id": "q1", "status": "pending", "kind": "question", "body": "質問 1", "createdAt": "2026-11-13T09:00:00Z" } ] },
    "notifications": { "g1": [ { "id": "n1", "kind": "blocked", "body": "通知 1", "recordedAt": "2026-11-13T09:30:00Z" } ] },
    "isError": is_error
  })
}

/// 受け取り済みの変化通知を捨てる。
fn drain_changes(changes: &mut tokio::sync::broadcast::Receiver<()>) {
  while changes.try_recv().is_ok() {}
}

/// completed へ変わった周期で `failing` の補助読み取りだけが失敗し、次の周期で読み直しに成功する。
/// 読み直した内容が前回値と同じなら、その周期は画面に出る状態を変えないため、変化通知を送らない。
async fn assert_retry_with_unchanged_content_sends_no_change_notification(failing: AuxRead) {
  let harness = Harness::new(unchanged_aux_state("created", None));
  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("1周期目を実行できます");
  harness.rewrite_state(unchanged_aux_state("completed", Some(failing)));
  poller.read_cycle_once().await.expect("終端へ変わる周期を実行できます");
  drain_changes(&mut changes);

  harness.rewrite_state(unchanged_aux_state("completed", None));
  harness.clear_calls();
  poller.read_cycle_once().await.expect("読み直す周期を実行できます");
  let retry_calls = harness.calls();
  let notified = changes.try_recv();
  poller.shutdown().await;

  assert!(
    retry_calls.iter().any(|call| call == failing.tool()),
    "終端のゴールで読めなかった {failing:?} は次の周期に読み直します: {retry_calls:?}"
  );
  assert!(notified.is_err(), "読み直した {failing:?} の内容が同じ周期では変化通知を送りません: {notified:?}");
}

#[tokio::test]
async fn retried_terminal_goal_questions_with_unchanged_content_send_no_change_notification() {
  assert_retry_with_unchanged_content_sends_no_change_notification(AuxRead::Questions).await;
}

#[tokio::test]
async fn retried_terminal_goal_notifications_with_unchanged_content_send_no_change_notification() {
  assert_retry_with_unchanged_content_sends_no_change_notification(AuxRead::Notifications).await;
}

#[tokio::test]
async fn projects_are_read_concurrently() {
  // 子プロセスの起動時間は、並列に動くほかのテストの負荷で大きく揺れる。遅延なしの1周期目で
  // 3 プロジェクトの子プロセスを起動しておき、遅延を入れた2周期目の読み取りだけを測る。
  let delayed = json!({ "delayMs": { "takt_list_goals": 1000 } });
  let harness = Harness::new(json!({}));
  let second_state = harness.add_project_with_state("project-two", &json!({}));
  let third_state = harness.add_project_with_state("project-three", &json!({}));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("子プロセスを起動する 1 サイクル目を実行できます");
  for state_path in [&harness.state_path, &second_state, &third_state] {
    write_json(state_path, &delayed);
  }

  let started = Instant::now();
  let report = poller.read_cycle_once().await.expect("1 サイクルを実行できます");
  let elapsed = started.elapsed();
  poller.shutdown().await;

  assert_eq!(report.projects_read, 3, "3 プロジェクトを読み取ります");
  assert!(
    elapsed < Duration::from_millis(2500),
    "プロジェクト間の読み取りを並行にします。直列なら 1 秒の遅延が 3 回で 3 秒以上かかります: {elapsed:?}"
  );
}

#[tokio::test]
async fn changed_client_settings_are_applied_on_the_same_poller() {
  // 同じ Poller 実体のまま、既存サーバーの起動方法（args）を変えると、
  // 次のサイクルから新しい設定で読む。 プロジェクト名だけで既存クライアントを再利用する実装では、
  // 起動設定の変更が反映されず失敗する。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  let alternate = harness._directory.path().join("state-alternate.json");
  write_json(&alternate, &state_with_goal("g-changed"));
  harness.rewrite_config(|value| {
    value["mcpServers"]["mac-mini"]["args"] =
      json!([mock_script().display().to_string(), "--state", alternate.display().to_string()]);
  });

  poller.read_cycle_once().await.expect("設定変更後のサイクルを実行できます");
  poller.shutdown().await;

  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  let goals: Vec<String> = slot
    .snapshot
    .expect("読み取った状態があります")
    .goals
    .into_iter()
    .map(|goal| goal.goal_id.as_str().to_owned())
    .collect();
  assert!(goals.contains(&"g-changed".to_owned()), "起動設定の変更が次のサイクルから反映されます: {goals:?}");
}

#[tokio::test]
async fn server_meta_change_is_applied_on_the_same_running_poller() {
  // 同じ Poller 実体と状態ストアのまま、サーバーの表示情報を改名・追加する。
  // 起動時だけ server_meta を読み込む実装では、稼働中の変更が投影へ反映されない。
  let harness = Harness::new(state_with_goal("g1"));
  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let _ = changes.try_recv();
  assert_eq!(harness.store.servers().get("mac-mini").expect("サーバーの表示情報があります").name, "j5ik2o-mac-mini");

  harness.rewrite_config(|value| {
    value["servers"]["mac-mini"]["name"] = json!("renamed-mini");
    value["servers"]["mac-mini"]["kind"] = json!("Mac Studio");
    value["servers"]["mac-studio"] = json!({ "name": "j5ik2o-mac-studio", "kind": "Mac Studio" });
  });
  poller.read_cycle_once().await.expect("サーバー表示情報を変えたサイクルを実行できます");

  let servers = harness.store.servers();
  let mini = servers.get("mac-mini").expect("改名後もサーバーがあります");
  assert_eq!(mini.name, "renamed-mini", "稼働中でも改名を同じストアへ反映します");
  assert_eq!(mini.kind, "Mac Studio", "種類の変更も反映します");
  assert_eq!(
    servers.get("mac-studio").expect("追加したサーバーがあります").name,
    "j5ik2o-mac-studio",
    "追加したサーバーも同じストアへ反映します"
  );
  assert!(changes.try_recv().is_ok(), "サーバー表示情報だけが変わったサイクルでも変化を通知します");

  poller.shutdown().await;
}

#[tokio::test]
async fn project_concurrency_change_is_applied_and_notified_on_the_same_running_poller() {
  // 同じ Poller 実体と状態ストアのまま、プロジェクトの .takt/config.yaml の
  // concurrency だけを変える。変化指紋がプロジェクトの concurrency を見ない実装では、
  // ストアの投影へ新しい上限が反映されても変化が通知されず、画面の上限が古いままになる。
  let harness = Harness::new(state_with_goal("g1"));
  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let _ = changes.try_recv();

  // .takt/config.yaml が無い間は、takt の既定値が上限になる。
  let view = harness
    .store
    .project_views()
    .into_iter()
    .find(|view| view.name == "takt-dash")
    .expect("プロジェクトの投影があります");
  assert_eq!(view.concurrency, 1, "takt 設定が無い間は既定値の上限です");

  let takt_dir = harness._directory.path().join("project").join(".takt");
  std::fs::create_dir_all(&takt_dir).expect("プロジェクトの .takt を作れます");
  std::fs::write(takt_dir.join("config.yaml"), "concurrency: 4\n").expect("プロジェクトの takt 設定を書けます");
  poller.read_cycle_once().await.expect("concurrency を変えたサイクルを実行できます");

  let view = harness
    .store
    .project_views()
    .into_iter()
    .find(|view| view.name == "takt-dash")
    .expect("プロジェクトの投影があります");
  assert_eq!(view.concurrency, 4, "稼働中でも新しい上限を同じストアへ反映します");
  assert!(changes.try_recv().is_ok(), "concurrency だけが変わったサイクルでも変化を通知します");

  // 続く無変更のサイクルでは通知しません。
  poller.read_cycle_once().await.expect("状態を変えないサイクルを実行できます");
  assert!(changes.try_recv().is_err(), "状態を変えないサイクルでは通知しません");

  poller.shutdown().await;
}

#[tokio::test]
async fn changed_cwd_rebuilds_the_client_and_reads_from_the_new_directory() {
  // 同じ Poller 実体のまま、プロジェクトの cwd を存在しないディレクトリへ変える。
  // cwd を比較しない実装では旧クライアントを再利用して成功し続けるため、
  // 次のサイクルの失敗で検出する。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  let missing = harness._directory.path().join("missing-project-dir");
  harness.rewrite_config(|value| {
    value["projects"][0]["cwd"] = json!(missing.display().to_string());
  });
  let report = poller.read_cycle_once().await.expect("cwd を変えたサイクルは完了します");
  poller.shutdown().await;

  assert_eq!(report.projects_failed, 1, "存在しない cwd では子プロセスを起動できず失敗します");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    slot.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Delayed),
    "新しい cwd の初回失敗は遅延中です"
  );
  assert!(slot.snapshot.is_none(), "旧 cwd の状態を新しい所属へ持ち越しません");
}

#[tokio::test]
async fn changed_server_settings_read_new_state_without_stale_slot_on_failure() {
  // 接続設定を変えた直後の読み取りが失敗しても、旧サーバー由来の snapshot をスロットに残さない。
  // 差分基準とスロットをリセットしない実装では、旧サーバーの状態が新接続先の所属のまま残る。
  let harness = Harness::new(state_with_goal("g1"));
  let directory = harness._directory.path();
  let broken = directory.join("state-broken.json");
  std::fs::write(&broken, "{ this is not json").expect("壊れた状態ファイルを書けます");

  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let before = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(before.snapshot.expect("旧サーバーの状態があります").goals[0].goal_id.as_str(), "g1");

  harness.rewrite_config(|value| {
    value["mcpServers"]["mac-mini"]["args"] =
      json!([mock_script().display().to_string(), "--state", broken.display().to_string()]);
  });
  let report = poller.read_cycle_once().await.expect("起動設定変更後のサイクルは完了します");
  poller.shutdown().await;

  assert_eq!(report.projects_failed, 1, "新接続先の初回読み取りは失敗します");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    slot.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Delayed),
    "新接続先の初回失敗は遅延中です"
  );
  assert!(
    slot.snapshot.is_none(),
    "接続先を変えたら旧サーバー由来の snapshot を残しません: {:?}",
    slot.snapshot.map(|snapshot| snapshot.goals.into_iter().map(|goal| goal.goal_id).collect::<Vec<_>>())
  );
}

#[tokio::test]
async fn changed_server_baseline_reset_prevents_cross_server_transition_events() {
  // 接続設定が変わったら旧接続先の差分基準をリセットする。リセットしない実装では、新サーバーの
  // 同じ goal id の状態を旧サーバーの状態との差分として扱い、遷移の出来事を誤記録する。
  let harness = Harness::new(state_with_goal("g1"));
  let directory = harness._directory.path();
  let alternate = directory.join("state-alternate.json");
  write_json(
    &alternate,
    &json!({ "goals": [ { "id": "g1", "status": "created", "executionStatus": "paused", "objective": "ゴール g1", "integrationBranch": "takt/g1" } ] }),
  );

  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_config(|value| {
    value["mcpServers"]["mac-mini"]["args"] =
      json!([mock_script().display().to_string(), "--state", alternate.display().to_string()]);
  });
  let report = poller.read_cycle_once().await.expect("起動設定変更後のサイクルを実行できます");
  poller.shutdown().await;

  assert_eq!(report.events_recorded, 0, "新接続先の初回は基準化のみで出来事を出しません");
  let events: Vec<EventKind> =
    harness.activity.recent_events(50).expect("記録した出来事を読めます").into_iter().map(|event| event.kind).collect();
  assert!(!events.contains(&EventKind::Paused), "旧接続先との差分で一時停止を誤記録しません: {events:?}");
}

#[tokio::test]
async fn baseline_is_reset_only_once_after_a_client_rebuild() {
  // 設定変更でクライアントを作り直したサイクルは、新接続先の初回として基準化のみにする。
  // 作り直しの記録を消費しない実装では、次サイクル以降も毎回基準化され、
  // 新接続先で起きた状態の変化が出来事として記録されない。
  let harness = Harness::new(state_with_goal("g1"));
  let alternate = harness._directory.path().join("state-alternate.json");
  write_json(&alternate, &state_with_goal("g1"));

  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_config(|value| {
    value["mcpServers"]["mac-mini"]["args"] =
      json!([mock_script().display().to_string(), "--state", alternate.display().to_string()]);
  });
  let rebuilt = poller.read_cycle_once().await.expect("起動設定変更後のサイクルを実行できます");
  assert_eq!(rebuilt.events_recorded, 0, "作り直したクライアントの初回は基準化のみで出来事を出しません");

  // 同じ接続先のまま状態だけを変える。作り直しの記録が消費済みなら、この差分は基準化されず
  // 出来事として記録されます。
  write_json(&alternate, &state_with_goal("g2"));
  let changed = poller.read_cycle_once().await.expect("作り直しの次のサイクルを実行できます");
  poller.shutdown().await;

  assert_eq!(changed.events_recorded, 1, "作り直しの次のサイクルは基準化せず、状態の変化を記録します");
  let events = harness.activity.recent_events(10).expect("記録した出来事を読めます");
  assert_eq!(events[0].kind, EventKind::GoalCreated, "状態の変化に対応する出来事を記録します");
  assert_eq!(events[0].project.as_deref(), Some("takt-dash"));
}

#[tokio::test]
async fn unchanged_client_settings_keep_the_baseline_and_emit_no_duplicate_events() {
  // 設定が変わっていないプロジェクトはクライアントも差分基準も保持する。
  // 毎サイクル再作成する実装では、 2サイクル目が基準化からやり直しになり、
  // 内容が同じでも出来事を取りこぼしたり重複させたりする。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  let second = poller.read_cycle_once().await.expect("設定を変えない2回目のサイクルを実行できます");

  assert_eq!(second.events_recorded, 0, "内容が同じなら設定を変えないサイクルでは出来事を出しません");
  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(slot.is_connected(), "クライアントを保持したまま読み続けます");
  poller.shutdown().await;
}

#[tokio::test]
async fn unchanged_client_settings_reuse_the_child_process_between_cycles() {
  // 設定を変えないサイクルでは同じ子プロセスを再利用する。毎サイクル起動し直す実装では、
  // 起動時にだけ書かれる PID ファイルが2サイクル目に再生成され、ここで検出できる。
  let harness = Harness::new(state_with_goal("g1"));
  let pid_path = harness._directory.path().join("mock.pid");
  let pid_for_config = pid_path.clone();
  harness.rewrite_config(|value| {
    let args = value["mcpServers"]["mac-mini"]["args"].as_array_mut().expect("起動引数があります");
    args.push(json!("--pid-file"));
    args.push(json!(pid_for_config.display().to_string()));
  });

  let mut poller = harness.poller();
  let first = poller.read_cycle_once().await.expect("1回目のサイクルを実行できます");
  assert_eq!(first.projects_read, 1, "1回目は子プロセスを起動して読み取ります");
  let pid = read_pid_file(&pid_path);

  // mock は起動時にしか PID
  // ファイルを書かない。消しておけば、2サイクル目の再生成が再起動の証拠になる。
  std::fs::remove_file(&pid_path).expect("PID ファイルを消せます");

  let second = poller.read_cycle_once().await.expect("設定を変えない2回目のサイクルを実行できます");
  assert_eq!(second.projects_read, 1, "再利用した子プロセスからも読み取れます");
  assert_eq!(second.projects_failed, 0, "2回目の読み取りは失敗しません");
  assert!(!pid_path.exists(), "設定が同じなら子プロセスを起動し直しません");
  assert!(process_is_running(pid), "1回目に起動した子プロセスが同じ PID のまま動いています: pid={pid}");

  poller.shutdown().await;
}

#[tokio::test]
async fn failed_event_record_keeps_the_baseline_and_records_the_event_once_after_recovery() {
  // 読み取りは成功したが出来事の記録に失敗したサイクルでは、差分基準を進めません。
  // 記録が回復した後のサイクルで、未記録だった変化をちょうど1回だけ記録します。
  // 記録の成功前に基準を進める実装では、失敗したサイクルの変化が失われ、回復後にも記録されません。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_state(state_with_goal("g2"));
  fail_event_recording_for(&harness.activity_db_path, "mac-mini");
  poller.read_cycle_once().await.expect_err("記録できないサイクルは記録の失敗で終わります");

  // 失敗したサイクルの時点で、差分基準も状態ストアも進んでいないことを観測する。
  let failed_slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    failed_slot.snapshot.expect("状態があります").goals[0].goal_id.as_str(),
    "g1",
    "記録に失敗した状態を成功として残しません"
  );

  allow_event_recording(&harness.activity_db_path);
  let recovered = poller.read_cycle_once().await.expect("記録が回復したサイクルを実行できます");
  assert_eq!(recovered.events_recorded, 1, "未記録だった変化を回復後に記録します");
  poller.shutdown().await;

  let created: Vec<String> = harness
    .activity
    .recent_events(50)
    .expect("記録した出来事を読めます")
    .into_iter()
    .filter(|event| event.kind == EventKind::GoalCreated)
    .map(|event| event.body)
    .collect();
  assert_eq!(created.len(), 1, "記録に失敗したサイクルの変化が失われず、二重にもなりません: {created:?}");
  assert!(created[0].contains("g2"), "回復後に記録された変化は失敗したサイクルの状態です: {created:?}");
}

#[tokio::test]
async fn failed_event_record_on_the_failure_path_still_marks_the_project_disconnected() {
  // 接続断の出来事の記録に失敗したサイクルでも、そのプロジェクトの connected は偽にします。
  // 記録エラーを `?` で先に伝えて record_failure に到達しない実装では、応答がないプロジェクトが
  // connected 真のまま残ります。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_connection_thresholds(1, 1);
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  fail_event_recording_for(&harness.activity_db_path, "mac-mini");
  poller.read_cycle_once().await.expect_err("接続断の出来事を記録できないサイクルは失敗で終わります");

  let slot = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    slot.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Unresponsive),
    "記録に失敗しても応答なしの状態は確定します"
  );
  assert!(slot.snapshot.is_some(), "最後に読めた状態は保持します");
  poller.shutdown().await;
}

#[tokio::test]
async fn failed_event_record_does_not_stop_the_remaining_projects() {
  // 先頭のプロジェクトの出来事の記録に失敗しても、
  // 後続のプロジェクトは読み取りと状態更新を続けます。 記録エラーを `?`
  // でループから抜ける実装では、後続プロジェクトの状態が更新されず、通知も届きません。
  let harness = Harness::new(state_with_goal("g1"));
  let directory = harness._directory.path();
  let project_two_dir = directory.join("project-two");
  std::fs::create_dir_all(&project_two_dir).expect("2つ目の cwd を作れます");
  let state_two = directory.join("state-two.json");
  write_json(&state_two, &state_with_goal("x1"));
  let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&harness.config_path).unwrap()).unwrap();
  config["mcpServers"]["mini-two"] = json!({
    "command": "node",
    "args": [mock_script().display().to_string(), "--state", state_two.display().to_string()],
    "env": {}
  });
  config["servers"]["mini-two"] = json!({ "name": "j5ik2o-mac-mini-2", "kind": "Mac mini", "concurrency": 4 });
  config["projects"].as_array_mut().unwrap().push(json!({
    "name": "other", "server": "mini-two", "about": "説明2", "cwd": project_two_dir.display().to_string()
  }));
  write_json(&harness.config_path, &config);

  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let _ = changes.try_recv();

  harness.rewrite_state(state_with_goal("g2"));
  write_json(&state_two, &state_with_goal("x2"));
  fail_event_recording_for(&harness.activity_db_path, "mac-mini");
  poller.read_cycle_once().await.expect_err("記録できないサイクルは失敗で終わります");

  assert!(changes.try_recv().is_ok(), "記録に失敗しても状態変化を通知します");
  let second = harness.store.slot("other").expect("後続のスロットがあります");
  assert!(second.is_connected(), "後続プロジェクトは記録の失敗に巻き込まれません");
  assert_eq!(
    second.snapshot.expect("後続の状態があります").goals[0].goal_id.as_str(),
    "x2",
    "後続プロジェクトの状態を更新します"
  );

  // 記録が回復した次のサイクルでは、先頭プロジェクトの未記録の変化だけが記録されます。
  allow_event_recording(&harness.activity_db_path);
  let recovered = poller.read_cycle_once().await.expect("記録が回復したサイクルを実行できます");
  assert_eq!(recovered.events_recorded, 1, "未記録だった変化だけを回復後に記録します");
  poller.shutdown().await;

  let mut recorded: Vec<String> =
    harness.activity.recent_events(50).unwrap().into_iter().map(|event| event.body).collect();
  recorded.sort();
  // 後続プロジェクトの変化は失敗したサイクルで記録され、先頭プロジェクトの変化は回復後に
  // 1回だけ記録されます。どちらも欠落も二重記録もありません。
  assert_eq!(recorded.len(), 2, "両プロジェクトの変化をちょうど1件ずつ記録します: {recorded:?}");
  assert!(recorded.iter().any(|body| body.contains("g2")), "先頭プロジェクトの未記録の変化を記録します: {recorded:?}");
  assert!(
    recorded.iter().any(|body| body.contains("x2")),
    "後続プロジェクトの変化を失敗サイクルで記録します: {recorded:?}"
  );
}

#[tokio::test]
async fn failed_event_record_keeps_the_connection_lost_event_once_after_recovery() {
  // 接続断を検出したがその出来事を記録できなかったサイクルでは、connected 偽を確定させます。
  // 回復したサイクルでは接続断を二重に記録せず、その後の変化を通常どおり記録します。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  harness.rewrite_connection_thresholds(1, 1);
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  fail_event_recording_for(&harness.activity_db_path, "mac-mini");
  poller.read_cycle_once().await.expect_err("接続断の出来事を記録できないサイクルは失敗で終わります");
  let down = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    down.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Unresponsive),
    "記録に失敗しても応答なしの状態は確定します"
  );

  allow_event_recording(&harness.activity_db_path);
  harness.rewrite_state(state_with_goal("g1"));
  poller.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  let recovered = harness.store.slot("takt-dash").expect("スロットがあります");
  assert!(recovered.is_connected(), "復旧すれば connected 真です");
  poller.shutdown().await;

  let lost: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost, 0, "記録できなかった接続断を、復旧時に遡って二重に記録しません");
}

#[tokio::test]
async fn failed_connection_lost_record_keeps_the_baseline_and_records_it_once_while_reads_keep_failing() {
  // 接続断の記録に失敗したサイクルでは接続状態の比較基準を据え置き、読み取り失敗が続く中で
  // 記録だけが回復したとき、未記録だった接続断をちょうど1回記録します。基準を偽へ進める
  // 実装では回復後も再検出できず欠落し、毎サイクル記録する実装では重複します。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  // サイクル2: 読み取り失敗かつ接続断の記録も失敗させる。
  harness.rewrite_connection_thresholds(1, 1);
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  harness.rewrite_state(json!({ "goals": [], "isError": ["takt_list_goals"] }));
  fail_event_recording_for(&harness.activity_db_path, "mac-mini");
  poller.read_cycle_once().await.expect_err("接続断の出来事を記録できないサイクルは失敗で終わります");
  let down = harness.store.slot("takt-dash").expect("スロットがあります");
  assert_eq!(
    down.connection_state,
    Some(takt_dash_backend::model::ConnectionState::Unresponsive),
    "記録に失敗しても応答なしの状態は確定します"
  );
  assert!(down.snapshot.is_some(), "最後に読めた状態は保持します");

  // サイクル3: 記録だけを回復する。読み取り失敗は続くので、未記録だった接続断を記録する。
  allow_event_recording(&harness.activity_db_path);
  let recovered = poller.read_cycle_once().await.expect("記録が回復したサイクルを実行できます");
  assert_eq!(recovered.events_recorded, 1, "未記録だった接続断だけを回復後に記録します");

  // サイクル4: 読み取り失敗が続いても、記録済みの接続断を再検出しない。
  let continuing = poller.read_cycle_once().await.expect("読み取り失敗が続くサイクルを実行できます");
  assert_eq!(continuing.events_recorded, 0, "記録済みの接続断を再記録しません");
  poller.shutdown().await;

  let lost: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost, 1, "接続断は欠落も重複もなくちょうど1回記録します");
}

#[tokio::test]
async fn config_server_changes_are_reflected_after_a_cycle_and_notify_watchers() {
  // 設定のサーバー情報だけを変えたサイクルでも、状態ストアの servers が新しい内容になり、
  // WatchState の変化通知が届く。起動時の servers を返し続ける実装や、変化指紋が servers を
  // 見ない実装では失敗する。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  let mut watch = harness.changes.subscribe();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let _ = watch.try_recv();

  // プロジェクトと状態はそのままに、サーバーの表示情報と並列度だけを変える。
  harness.rewrite_config(|value| {
    value["servers"]["mac-mini"] = json!({ "name": "j5ik2o-mac-mini-renamed", "kind": "Mac mini" });
  });
  poller.read_cycle_once().await.expect("サーバー情報を変えたサイクルを実行できます");
  poller.shutdown().await;

  let servers = harness.store.servers();
  let meta = servers.get("mac-mini").expect("既存サーバーの情報があります");
  assert_eq!(meta.name, "j5ik2o-mac-mini-renamed", "再読み込み後の servers は新しい設定の表示名になります");
  assert!(watch.try_recv().is_ok(), "サーバー情報だけが変わったサイクルでも変化通知を1回送ります");
}

#[tokio::test]
async fn log_silent_threshold_crossing_notifies_once_on_the_same_poller() {
  // 同じ Poller 実体のまま時刻だけを進め、run のログ無音が閾値（既定 10 分）を跨いだ
  // サイクルでだけ変化通知を 1 回送る。時刻は注入し、実時間の待機に依存しない。
  // 最後のログは step_complete の時刻と本体ログの更新時刻を同じ 10:00 に揃えて固定する。
  let last_log_at = "2026-11-13T10:00:00Z";
  let harness = Harness::new(state_with_running_tasks(&["r1"]));
  let fixture = harness.run_fixture();
  fixture.write_tasks(&[RunTaskLine { run_slug: Some("r1"), source_run_slug: None }]);
  fixture.write_meta("r1", &running_meta("implement", 1, "development-core", last_log_at));
  fixture.append_records("r1", &[
    workflow_start_record("development-core"),
    step_complete_record("plan", "architect", 1, last_log_at),
  ]);
  fixture.set_body_log_modified("r1", time(last_log_at));
  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();

  poller.read_cycle_once_at(time("2026-11-13T10:01:00Z")).await.expect("閾値内の初回サイクルを実行できます");
  assert!(changes.try_recv().is_ok(), "未読から読めた状態への初回の遷移も通知します");

  poller.read_cycle_once_at(time("2026-11-13T10:05:00Z")).await.expect("閾値内のサイクルを実行できます");
  assert!(changes.try_recv().is_err(), "無音が閾値内のままのサイクルでは通知しません");

  poller.read_cycle_once_at(time("2026-11-13T10:31:00Z")).await.expect("閾値を跨いだサイクルを実行できます");
  assert!(changes.try_recv().is_ok(), "無音が閾値を跨いだサイクルで通知します");
  assert!(changes.try_recv().is_err(), "同じサイクルで通知を重ねて送りません");

  poller.read_cycle_once_at(time("2026-11-13T11:00:00Z")).await.expect("閾値外のままのサイクルを実行できます");
  assert!(changes.try_recv().is_err(), "無音が真のままのサイクルでは通知しません");

  poller.shutdown().await;
}

#[tokio::test]
async fn pending_decision_stale_threshold_crossing_notifies_once_on_the_same_poller() {
  // 問い合わせと人のマージ待ちの起点を別々の固定時刻にし、滞留が閾値（既定 240 分）を
  // 跨ぐサイクルでだけ通知する。2回目は問い合わせだけ、3回目はマージ待ちだけが跨ぐため、
  // 片方の種別だけを比較する実装でも検出できる。
  let harness = Harness::new(json!({
    "goals": [ { "id": "g1", "status": "awaiting_merge", "executionStatus": "active", "objective": "ゴール g1", "integrationBranch": "takt/g1", "pullRequest": { "number": 5, "url": "https://example.test/pr/5" } } ],
    "tasks": [ { "name": "merge", "goalId": "g1", "status": "completed", "mergeTask": true } ],
    "standaloneInquiries": [ { "id": "s1", "status": "pending", "kind": "question", "body": "どうしますか", "createdAt": "2026-11-13T10:00:00Z" } ],
    "notifications": { "g1": [ { "id": "n-merge", "kind": "awaiting_merge", "body": "マージ待ち", "recordedAt": "2026-11-13T11:00:00Z" } ] }
  }));
  let mut changes = harness.changes.subscribe();
  let mut poller = harness.poller();

  poller.read_cycle_once_at(time("2026-11-13T12:00:00Z")).await.expect("閾値内の初回サイクルを実行できます");
  assert!(changes.try_recv().is_ok(), "未読から読めた状態への初回の遷移も通知します");

  poller
    .read_cycle_once_at(time("2026-11-13T14:10:00Z"))
    .await
    .expect("問い合わせだけが閾値を跨いだサイクルを実行できます");
  assert!(changes.try_recv().is_ok(), "問い合わせの滞留が閾値を跨いだサイクルで通知します");

  poller
    .read_cycle_once_at(time("2026-11-13T15:20:00Z"))
    .await
    .expect("マージ待ちだけが閾値を跨いだサイクルを実行できます");
  assert!(changes.try_recv().is_ok(), "人のマージ待ちの滞留が閾値を跨いだサイクルで通知します");

  poller.read_cycle_once_at(time("2026-11-13T16:00:00Z")).await.expect("両方とも滞留のままのサイクルを実行できます");
  assert!(changes.try_recv().is_err(), "滞留が真のままのサイクルでは通知しません");

  poller.shutdown().await;
}

#[tokio::test]
async fn stop_request_during_the_wait_returns_promptly() {
  // interval 30 秒の待機中に停止要求を送ると、interval の満了を待たずに戻る。
  // 子プロセスの起動時間は並列に動くほかのテストの負荷で揺れるため、先に1周期読んで起動しておき、
  // 期限には起動済みの子プロセスでの周期と待機だけを含める。
  let harness = Harness::new(state_with_goal("g1"));
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("子プロセスを起動する 1 サイクル目を実行できます");
  let warmed_up_at = harness.store.slot("takt-dash").and_then(|slot| slot.last_read_at);
  let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
  let (sent_at_tx, sent_at_rx) = tokio::sync::oneshot::channel();
  let store = harness.store.clone();
  let signal = tokio::spawn(async move {
    // run_until_stopped の最初のサイクルが終わって待機に入ったことを、読めた時刻が進んだことで
    // 観測してから停止を送る。
    for _ in 0..500 {
      if store.slot("takt-dash").is_some_and(|slot| slot.is_connected() && slot.last_read_at != warmed_up_at) {
        break;
      }
      tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let sent = std::time::Instant::now();
    stop_tx.send(true).expect("停止要求を送れます");
    let _ = sent_at_tx.send(sent);
  });

  tokio::time::timeout(std::time::Duration::from_secs(2), run_until_stopped(&mut poller, stop_rx))
    .await
    .expect("停止要求で待機を抜けて戻ります");
  signal.await.expect("停止要求の送信タスクが完了します");

  let sent_at = sent_at_rx.await.expect("停止要求を送った時刻を受け取れます");
  assert!(
    sent_at.elapsed() < std::time::Duration::from_secs(1),
    "30秒の interval を待たずに戻ります: {:?}",
    sent_at.elapsed()
  );
  poller.shutdown().await;
}

#[tokio::test]
async fn stop_request_shuts_down_the_mcp_child_process() {
  // 停止要求で run_until_stopped が戻ったとき、起動済みの MCP 子プロセスが終了している。
  // ループを抜けるだけで shutdown を呼ばない実装では、子プロセスが残る。
  let harness = Harness::new(state_with_goal("g1"));
  let pid_path = harness._directory.path().join("mock.pid");
  let pid_for_config = pid_path.clone();
  harness.rewrite_config(|value| {
    let args = value["mcpServers"]["mac-mini"]["args"].as_array_mut().expect("起動引数があります");
    args.push(json!("--pid-file"));
    args.push(json!(pid_for_config.display().to_string()));
  });

  let mut poller = harness.poller();
  let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
  let store = harness.store.clone();
  let pid_wait = pid_path.clone();
  let signal = tokio::spawn(async move {
    // 子プロセスの起動（PID ファイル）と最初のサイクル完了を観測してから停止を送る。
    for _ in 0..500 {
      if store.slot("takt-dash").is_some_and(|slot| slot.is_connected()) && pid_wait.exists() {
        break;
      }
      tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    stop_tx.send(true).expect("停止要求を送れます");
  });

  tokio::time::timeout(std::time::Duration::from_secs(5), run_until_stopped(&mut poller, stop_rx))
    .await
    .expect("停止要求で待機を抜けて戻ります");
  signal.await.expect("停止要求の送信タスクが完了します");

  let pid = read_pid_file(&pid_path);
  assert!(!process_is_running(pid), "停止後は MCP 子プロセスが終了しています: pid={pid}");
  poller.shutdown().await;
}

#[tokio::test]
async fn changing_the_project_server_assignment_reads_the_new_server_state() {
  // 同じ Poller 実体のままプロジェクトの server 割り当てを別サーバーへ変えると、
  // 次のサイクルで新しい接続先の状態を読み、投影の server_id も新しい割り当てになる。
  let harness = Harness::new(state_with_goal("g1"));
  let directory = harness._directory.path();
  let state_two = directory.join("state-two.json");
  write_json(&state_two, &state_with_goal("g2"));
  harness.rewrite_config(|value| {
    value["mcpServers"]["mini-two"] = json!({
      "command": "node",
      "args": [mock_script().display().to_string(), "--state", state_two.display().to_string()],
      "env": {}
    });
    value["servers"]["mini-two"] = json!({ "name": "j5ik2o-mac-mini-2", "kind": "Mac mini", "concurrency": 4 });
  });

  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");
  let before = harness.store.project_views();
  assert_eq!(before[0].server_id, "mac-mini", "初回は元のサーバー割り当てです");

  harness.rewrite_config(|value| {
    value["projects"][0]["server"] = json!("mini-two");
  });
  poller.read_cycle_once().await.expect("server 割り当てを変えたサイクルを実行できます");
  poller.shutdown().await;

  let views = harness.store.project_views();
  let view = &views[0];
  assert_eq!(view.server_id, "mini-two", "投影の server_id は新しい割り当てになります");
  let goals: Vec<String> = view
    .snapshot
    .as_ref()
    .expect("読み取った状態があります")
    .goals
    .iter()
    .map(|goal| goal.goal_id.as_str().to_owned())
    .collect();
  assert!(goals.contains(&"g2".to_owned()), "新しいサーバーの状態を読みます: {goals:?}");
  assert!(!goals.contains(&"g1".to_owned()), "旧サーバーの状態を持ち越しません: {goals:?}");
}

#[tokio::test]
async fn restart_during_a_recorded_outage_records_the_matching_restore_once() {
  // 切断を記録した後にプロセスが再起動し、再起動後の最初の読み取りで既に回復していた
  // 場合でも、記録済みの切断に対応する復帰を記録します。再起動後の Poller がメモリ上の
  // 比較基準と状態ストアだけに頼る実装では、基準もスロットも無いまま成功するため
  // 復帰を記録できず、切断だけが残ります。再起動は、新しい Poller と新しい状態ストアに
  // 同じ設定ファイルと同じ記録先を使い回して模します。
  let harness = Harness::new(state_with_goal("g1"));
  harness.rewrite_connection_thresholds(1, 1);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");
  let lost_before: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost_before, 1, "再起動前に切断を1件記録します");
  poller.shutdown().await;

  // 再起動後の最初の読み取りで既に回復している状態にします。
  harness.rewrite_state(state_with_goal("g1"));
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  let (mut restarted, _restarted_store) = harness.restarted_poller();
  restarted.read_cycle_once().await.expect("再起動後のサイクルを実行できます");
  let restored: Vec<_> = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .collect();
  assert_eq!(restored.len(), 1, "再起動前の切断に対応する復帰を1件だけ記録します");
  assert!(
    outage_seconds_of(&restored[0]) >= 1,
    "復帰イベントの本文に、記録済みの切断から数えた切断していた時間を入れます: {}",
    restored[0].body
  );

  // 続く成功サイクルでも、復帰を二重に記録しません。
  restarted.read_cycle_once().await.expect("続くサイクルを実行できます");
  let restored_after: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .count();
  assert_eq!(restored_after, 1, "復帰を二重に記録しません");
  restarted.shutdown().await;
}

#[tokio::test]
async fn restart_with_continuing_failures_does_not_duplicate_the_recorded_lost() {
  // 再起動後も読み取りの失敗が続く間は、記録済みの切断を二重に記録しません。
  // 記録から基準化した状態で状態ストア側が応答なしへ跨いでも、切断は1件のままです。
  let harness = Harness::new(state_with_goal("g1"));
  harness.rewrite_connection_thresholds(1, 1);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");
  poller.shutdown().await;

  let (mut restarted, _restarted_store) = harness.restarted_poller();
  restarted.read_cycle_once().await.expect("再起動後の失敗でもサイクルは完了します");
  let lost: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost, 1, "再起動後も切断を二重に記録しません");

  // 回復したら、再起動前の切断に対応する復帰を1件だけ記録します。
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  harness.rewrite_state(state_with_goal("g1"));
  restarted.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  let restored: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .count();
  assert_eq!(restored, 1, "再起動前の切断に対応する復帰を1件だけ記録します");
  restarted.shutdown().await;
}

#[tokio::test]
async fn client_rebuild_during_a_recorded_outage_records_the_matching_restore() {
  // クライアントの作り直しで差分基準をリセットしても、記録済みで未対応の切断は
  // 残ります。リセットした基準だけに頼る実装では、回復しても復帰を記録できません。
  let harness = Harness::new(state_with_goal("g1"));
  harness.rewrite_connection_thresholds(1, 1);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");
  let lost: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost, 1, "作り直し前に切断を1件記録します");

  // cwd を別の存在するディレクトリへ変えてクライアントを作り直し、読み取りも回復させます。
  let alternate_dir = harness._directory.path().join("project-alternate");
  std::fs::create_dir_all(&alternate_dir).expect("作り直し先の cwd を作れます");
  harness.rewrite_config(|value| {
    value["projects"][0]["cwd"] = json!(alternate_dir.display().to_string());
  });
  harness.rewrite_state(state_with_goal("g1"));
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  let report = poller.read_cycle_once().await.expect("作り直し後のサイクルを実行できます");
  assert_eq!(report.events_recorded, 1, "復帰の出来事だけを記録します");
  let restored: Vec<_> = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .collect();
  assert_eq!(restored.len(), 1, "作り直し前の切断に対応する復帰を1件だけ記録します");
  assert!(
    outage_seconds_of(&restored[0]) >= 1,
    "復帰イベントの本文に、記録済みの切断から数えた切断していた時間を入れます: {}",
    restored[0].body
  );
  poller.shutdown().await;
}
#[tokio::test]
async fn restart_with_a_failed_first_history_lookup_still_records_the_matching_restore() {
  // 再起動直後の種付け照会が一時的に失敗しても、その後の成功サイクルで比較基準を
  // 確定してしまわず、照会が成功したサイクルで未対応の切断に対応する復帰を記録します。
  // 照会の失敗を「履歴なし」と同じに扱う実装では、最初の成功サイクルが空の出来事群を
  // 保存して基準を接続中へ確定させるため、以後は照会を試し直せず復帰を記録できません。
  // 照会の失敗は、events テーブルを別名へ退避して決定的に作ります。
  let harness = Harness::new(state_with_goal("g1"));
  harness.rewrite_connection_thresholds(1, 1);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");
  let lost_before: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost_before, 1, "再起動前に切断を1件記録します");
  poller.shutdown().await;

  // 再起動後の最初のサイクルでは、種付け照会だけを失敗させます。読み取り自体は
  // 回復している状態にします。切断していた時間が1秒以上になるよう間を空けます。
  harness.rewrite_state(state_with_goal("g1"));
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  hide_events_table(&harness.activity_db_path);
  let (mut restarted, _restarted_store) = harness.restarted_poller();
  restarted.read_cycle_once().await.expect("照会が失敗してもサイクルは完了します");
  restore_events_table(&harness.activity_db_path);

  // 照会が成功する次のサイクルで、未対応の切断に対応する復帰を1件だけ記録します。
  restarted.read_cycle_once().await.expect("照会が成功するサイクルを実行できます");
  let restored: Vec<_> = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .collect();
  assert_eq!(restored.len(), 1, "照会の失敗をまたいでも復帰を1件だけ記録します");
  assert!(
    outage_seconds_of(&restored[0]) >= 1,
    "復帰イベントの本文に、記録済みの切断から数えた切断していた時間を入れます: {}",
    restored[0].body
  );

  // 続く成功サイクルでも、復帰を二重に記録しません。
  restarted.read_cycle_once().await.expect("続くサイクルを実行できます");
  let restored_after: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .count();
  assert_eq!(restored_after, 1, "復帰を二重に記録しません");
  restarted.shutdown().await;
}

#[tokio::test]
async fn restart_with_a_delayed_phase_does_not_duplicate_the_recorded_lost() {
  // 再起動後に「遅延中」を経由して応答なしへ跨いでも、記録済みの切断を二重に記録しません。
  // 記録から基準化した「応答なし」の基準が、遅延中の失敗で弱い状態へ上書きされると、
  // しきい値を跨いだときに切断をもう1件記録してしまいます。
  let harness = Harness::new(state_with_goal("g1"));
  harness.rewrite_connection_thresholds(1, 1);
  let mut poller = harness.poller();
  poller.read_cycle_once().await.expect("初回のサイクルを実行できます");

  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  poller.read_cycle_once().await.expect("失敗してもサイクルは完了します");
  let lost_before: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost_before, 1, "再起動前に切断を1件記録します");
  poller.shutdown().await;

  // 失敗回数のしきい値を3にして再起動し、失敗を3回続けます。1回目と2回目は遅延中、
  // 3回目で応答なしへ跨ぎますが、切断は1件のままです。
  harness.rewrite_connection_thresholds(3, 1);
  let (mut restarted, _restarted_store) = harness.restarted_poller();
  for _ in 0..3 {
    restarted.read_cycle_once().await.expect("再起動後の失敗でもサイクルは完了します");
    let lost: usize = harness
      .activity
      .recent_events(50)
      .unwrap()
      .into_iter()
      .filter(|event| event.kind == EventKind::ConnectionLost)
      .count();
    assert_eq!(lost, 1, "再起動後も切断を二重に記録しません");
  }

  // 回復したら、再起動前の切断に対応する復帰を1件だけ記録します。
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  harness.rewrite_state(state_with_goal("g1"));
  restarted.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  let restored: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .count();
  assert_eq!(restored, 1, "再起動前の切断に対応する復帰を1件だけ記録します");
  restarted.shutdown().await;
}

#[tokio::test]
async fn restart_with_pending_history_lookup_across_the_threshold_records_the_lost_once() {
  // 再起動直後の種付け照会が失敗し続けている間に読み取りの失敗が応答なしの閾値を
  // 跨ぐと、保留中の遷移は捨てられるため切断が記録されません。その後、照会が成功
  // して（履歴に未対応の切断は無い）保留が解けると、比較基準だけが応答なしとして
  // 確定し、回復したときに切断の無い復帰だけが残ります。保留中に跨いだ切断は、
  // 照会が成功した時点で1件だけ記録して、復帰と対応させます。
  // 照会の失敗は、events テーブルを別名へ退避して決定的に作ります。
  let harness = Harness::new(state_with_goal("g1"));
  harness.rewrite_connection_thresholds(3, 1);
  std::fs::write(&harness.state_path, "{ this is not json").expect("壊れた状態ファイルを書けます");
  hide_events_table(&harness.activity_db_path);
  let (mut restarted, _restarted_store) = harness.restarted_poller();

  // 照会が失敗し続ける間に失敗を3回続けます。1回目と2回目は遅延中で、
  // 3回目で応答なしへ跨ぎます。
  for _ in 0..3 {
    restarted.read_cycle_once().await.expect("照会が失敗してもサイクルは完了します");
  }
  restore_events_table(&harness.activity_db_path);

  // 照会が成功するサイクルでも、読み取りは失敗したままにします。この時点で、
  // 保留中に跨いだ切断が1件だけ記録されます。
  restarted.read_cycle_once().await.expect("照会が成功するサイクルを実行できます");
  let lost: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionLost)
    .count();
  assert_eq!(lost, 1, "保留中に跨いだ切断を、照会が成功した時点で1件だけ記録します");

  // 回復したら、保留中に跨いだ切断に対応する復帰を1件だけ記録します。
  tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
  harness.rewrite_state(state_with_goal("g1"));
  restarted.read_cycle_once().await.expect("復旧後のサイクルを実行できます");
  let restored: Vec<_> = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .collect();
  assert_eq!(restored.len(), 1, "保留中に跨いだ切断に対応する復帰を1件だけ記録します");
  assert!(
    outage_seconds_of(&restored[0]) >= 1,
    "復帰イベントの本文に、保留中に跨いだ切断から数えた切断していた時間を入れます: {}",
    restored[0].body
  );

  // 続く成功サイクルでも、復帰を二重に記録しません。
  restarted.read_cycle_once().await.expect("続くサイクルを実行できます");
  let restored_after: usize = harness
    .activity
    .recent_events(50)
    .unwrap()
    .into_iter()
    .filter(|event| event.kind == EventKind::ConnectionRestored)
    .count();
  assert_eq!(restored_after, 1, "復帰を二重に記録しません");
  restarted.shutdown().await;
}

/// 偽の gh を書く。`state_file` の内容を `{"state":"<内容>"}` として標準出力へ出し、
/// 内容が `EXIT:<code>` のときはその終了コードで終わる。`state_file` が無ければ空の出力で
/// 終了する（状態を確認できない経路）。
fn write_fake_gh(directory: &Path, state_file: &Path) -> PathBuf {
  let script = directory.join("fake-gh.sh");
  let body = format!(
    "#!/bin/sh\nstate_file='{state}'\nif [ ! -f \"$state_file\" ]; then\n  exit 0\nfi\nvalue=$(cat \"$state_file\")\ncase \"$value\" in\n  EXIT:*) exit \"${{value#EXIT:}}\" ;;\nesac\nprintf '{{\"state\":\"%s\"}}' \"$value\"\n",
    state = state_file.display()
  );
  std::fs::write(&script, body).expect("偽の gh を書けます");
  std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("偽の gh を実行可能にできます");
  script
}

/// 自動終了のテスト用に、manager 用の stdio エントリを設定へ加える。
///
/// manager 用エントリは `backend/tests/mock_mcp_server.mjs` を `--calls` で起動し、
/// `takt_finish_goal` の投函を記録します。`--state` は空の状態にします。
fn add_manager_server(harness: &Harness, calls_path: &Path) -> PathBuf {
  let state_path = harness._directory.path().join("manager-state.json");
  write_json(&state_path, &json!({}));
  harness.rewrite_config(|config| {
    config["mcpServers"]["manager"] = json!({
      "command": "node",
      "args": [
        mock_script().display().to_string(),
        "--state", state_path.display().to_string(),
        "--calls", calls_path.display().to_string()
      ],
      "env": {},
      "manager": true
    });
  });
  state_path
}

/// 中止済み（aborted）のマージ待ちゴールの状態を作る。
fn aborted_awaiting_merge_state(goal_id: &str, pull_request: i64) -> Value {
  json!({ "goals": [ {
    "id": goal_id,
    "status": "awaiting_merge",
    "executionStatus": "aborted",
    "objective": format!("ゴール {goal_id}"),
    "integrationBranch": format!("takt/{goal_id}"),
    "pullRequest": { "number": pull_request, "url": format!("https://example.test/pr/{pull_request}") }
  } ] })
}

fn recorded_tool_names(path: &Path) -> Vec<String> {
  match std::fs::read_to_string(path) {
    | Ok(text) => text
      .lines()
      .filter(|line| !line.is_empty())
      .filter_map(|line| serde_json::from_str::<Value>(line).ok())
      .filter_map(|value| value.get("name").and_then(Value::as_str).map(str::to_owned))
      .collect(),
    | Err(_) => Vec::new(),
  }
}

/// CLOSED の PR を持つ中止済みマージ待ちゴールは、読み取りサイクルで manager へ終了依頼され、
/// 判断待ちから消える（SCN-C2-P1、AC1）。
#[tokio::test]
async fn closed_aborted_goal_is_finished_by_manager_and_leaves_the_pending_list() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "CLOSED").expect("gh の状態を書けます");
  let fake_gh = write_fake_gh(harness._directory.path(), &gh_state);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert_eq!(
    recorded_tool_names(&manager_calls),
    vec!["takt_finish_goal".to_owned()],
    "manager 用エントリへ takt_finish_goal を1回投函します"
  );
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert!(decisions.is_empty(), "終了依頼済みのゴールは判断待ちに出ません");

  // 2周期目でも再投函しません（C7）。
  poller.read_cycle_once().await.expect("2周期目を実行できます");
  assert_eq!(
    recorded_tool_names(&manager_calls),
    vec!["takt_finish_goal".to_owned()],
    "依頼済みのゴールは再投函しません"
  );
  poller.shutdown().await;
}

/// OPEN の PR を持つ中止済みマージ待ちゴールは、終了依頼されず判断待ちに残る（SCN-C2-N1、AC2）。
#[tokio::test]
async fn open_aborted_goal_stays_without_finish_request() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "OPEN").expect("gh の状態を書けます");
  let fake_gh = write_fake_gh(harness._directory.path(), &gh_state);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert!(recorded_tool_names(&manager_calls).is_empty(), "OPEN の PR では終了依頼しません");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert_eq!(decisions.len(), 1, "OPEN の PR を持つ中止済みゴールは判断待ちに残ります");
  poller.shutdown().await;
}

/// gh が失敗するときは終了依頼せず判断待ちに残る（SCN-C4-N1、AC4）。
#[tokio::test]
async fn unknown_pull_request_state_keeps_the_goal() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "EXIT:1").expect("gh の状態を書けます");
  let fake_gh = write_fake_gh(harness._directory.path(), &gh_state);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert!(recorded_tool_names(&manager_calls).is_empty(), "状態を確認できないときは終了依頼しません");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert_eq!(decisions.len(), 1, "状態を確認できないゴールは判断待ちに残ります");
  poller.shutdown().await;
}

/// `autoTerminateClosedGoals: false` のときは、表示補正の PR 確認は行い、終了依頼だけを
/// 行わない（SCN-C5-N1、AC3、要件24）。
#[tokio::test]
async fn auto_termination_disabled_corrects_the_display_without_requesting() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  harness.rewrite_config(|config| {
    config["autoTerminateClosedGoals"] = json!(false);
  });
  let gh_state = harness._directory.path().join("gh-state");
  // CLOSED のとき、設定 false でも表示補正のために gh を起動する。偽 gh は起動時に印を残す。
  std::fs::write(&gh_state, "CLOSED").expect("gh の状態を書けます");
  let gh_marker = harness._directory.path().join("gh-invoked");
  let fake_gh = write_fake_gh_with_marker(harness._directory.path(), &gh_state, &gh_marker);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert!(gh_marker.exists(), "設定 false でも表示補正のために gh を起動します");
  assert!(recorded_tool_names(&manager_calls).is_empty(), "設定 false では終了依頼しません");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert!(decisions.is_empty(), "CLOSED の PR を持つゴールは、設定 false でも判断待ちに出ません");
  poller.shutdown().await;
}

/// MERGED の PR を持つ中止済みマージ待ちゴールも、読み取りサイクルで manager へ終了依頼され、
/// 判断待ちから消える（SCN-C2-P1 の MERGED 経路、AC1）。
#[tokio::test]
async fn merged_aborted_goal_is_finished_by_manager_and_leaves_the_pending_list() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "MERGED").expect("gh の状態を書けます");
  let fake_gh = write_fake_gh(harness._directory.path(), &gh_state);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert_eq!(
    recorded_tool_names(&manager_calls),
    vec!["takt_finish_goal".to_owned()],
    "MERGED の PR でも manager 用エントリへ takt_finish_goal を1回投函します"
  );
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert!(decisions.is_empty(), "終了依頼済みのゴールは判断待ちに出ません");
  poller.shutdown().await;
}

/// 終了依頼は manager 用エントリだけで行い、読み取り・書き込み用エントリからは
/// `takt_finish_goal` を呼ばない（C3-manager の否定契約、order.md 末尾の担保要求）。
#[tokio::test]
async fn finish_request_goes_only_to_the_manager_lane() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "CLOSED").expect("gh の状態を書けます");
  let fake_gh = write_fake_gh(harness._directory.path(), &gh_state);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert_eq!(
    recorded_tool_names(&manager_calls),
    vec!["takt_finish_goal".to_owned()],
    "manager 用エントリへだけ takt_finish_goal を投函します"
  );
  assert!(
    !harness.calls().iter().any(|name| name == "takt_finish_goal"),
    "読み取り・書き込み用エントリの呼び出し記録に takt_finish_goal は出ません"
  );
  poller.shutdown().await;
}

/// manager 用エントリが未設定のときは、表示補正の PR 確認は行い、終了依頼だけを行わない
/// （C4 の manager 未設定経路、AC4）。
#[tokio::test]
async fn missing_manager_server_corrects_the_display_without_requesting() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "CLOSED").expect("gh の状態を書けます");
  let gh_marker = harness._directory.path().join("gh-invoked");
  let fake_gh = write_fake_gh_with_marker(harness._directory.path(), &gh_state, &gh_marker);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert!(gh_marker.exists(), "manager 未設定でも表示補正のために gh を起動します");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert!(decisions.is_empty(), "CLOSED の PR を持つゴールは、manager 未設定でも判断待ちに出ません");
  poller.shutdown().await;
}

/// 中止でない（active）のマージ待ちゴールを、状態と実行状態を指定して作る。
fn awaiting_merge_state(goal_id: &str, pull_request: i64, status: &str, execution_status: &str) -> Value {
  json!({ "goals": [ {
    "id": goal_id,
    "status": status,
    "executionStatus": execution_status,
    "objective": format!("ゴール {goal_id}"),
    "integrationBranch": format!("takt/{goal_id}"),
    "pullRequest": { "number": pull_request, "url": format!("https://example.test/pr/{pull_request}") }
  } ] })
}

/// 中止でない（active）の awaiting_merge ゴールは、PR が CLOSED でも読み取りサイクルで
/// 終了依頼されません。表示補正は中止済みゴールに限らないため、判断待ちには出ません
/// （issue #112 の基準、AC2、P2）。
#[tokio::test]
async fn active_merge_goal_is_not_auto_terminated() {
  let harness = Harness::new(awaiting_merge_state("g1", 5, "awaiting_merge", "active"));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "CLOSED").expect("gh の状態を書けます");
  let gh_marker = harness._directory.path().join("gh-invoked");
  let fake_gh = write_fake_gh_with_marker(harness._directory.path(), &gh_state, &gh_marker);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert!(recorded_tool_names(&manager_calls).is_empty(), "active のゴールは終了依頼しません");
  assert!(gh_marker.exists(), "表示補正のために gh を起動します");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert!(decisions.is_empty(), "CLOSED の PR を持つゴールは判断待ちに出ません");
  poller.shutdown().await;
}

/// awaiting_merge でない中止済みゴールは、PR が CLOSED でも読み取りサイクルで終了依頼されず、
/// 判断待ちに残る（AC2、P2）。
///
/// `completed` は既存パーサが受理する状態値なので、読み取りは成功し `status` 除外を通る。
#[tokio::test]
async fn non_awaiting_merge_aborted_goal_is_not_auto_terminated() {
  let harness = Harness::new(awaiting_merge_state("g1", 5, "completed", "aborted"));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  add_manager_server(&harness, &manager_calls);
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "CLOSED").expect("gh の状態を書けます");
  let gh_marker = harness._directory.path().join("gh-invoked");
  let fake_gh = write_fake_gh_with_marker(harness._directory.path(), &gh_state, &gh_marker);

  let mut poller = harness.poller().with_gh_program(fake_gh);
  poller.read_cycle_once().await.expect("1周期目を実行できます");

  assert!(recorded_tool_names(&manager_calls).is_empty(), "awaiting_merge でないゴールは終了依頼しません");
  assert!(!gh_marker.exists(), "awaiting_merge でないゴールでは gh を起動しません");
  assert!(
    harness.calls().iter().any(|name| name == "takt_list_goals"),
    "読み取りサイクルでゴール一覧の読み取りが成功しています"
  );
  poller.shutdown().await;
}

/// 終了依頼が失敗しても、WARN ログで観測でき、次のサイクルで再評価される（P1 受入条件 (2)(3)）。
/// `takt_finish_goal` は1サイクルに1回だけ投函されます。
///
/// 表示方針は、PR が閉じていると確認できたゴールは、終了依頼の成否にかかわらず判断待ちに
/// 出さないことです（issue #112 の基準）。依頼の失敗は表示へ影響せず、依頼だけが次の
/// サイクルで再試行されます。PR を確認できない場合の表示維持は
/// `unknown_pull_request_state_keeps_the_goal` が観測します。
#[tokio::test]
async fn failed_finish_request_warns_and_retries_next_cycle() {
  let harness = Harness::new(aborted_awaiting_merge_state("g1", 5));
  let manager_calls = harness._directory.path().join("manager-calls.jsonl");
  let manager_state = add_manager_server(&harness, &manager_calls);
  // manager の `takt_finish_goal` 呼び出しを失敗させる（引数の goalId で限定）。
  write_json(&manager_state, &json!({ "isError": ["takt_finish_goal:g1"] }));
  let gh_state = harness._directory.path().join("gh-state");
  std::fs::write(&gh_state, "CLOSED").expect("gh の状態を書けます");
  let fake_gh = write_fake_gh(harness._directory.path(), &gh_state);

  let (logs, _guard) = capture_logs();
  let mut poller = harness.poller().with_gh_program(fake_gh);

  poller.read_cycle_once().await.expect("1周期目を実行できます");
  assert_eq!(
    recorded_tool_names(&manager_calls),
    vec!["takt_finish_goal".to_owned()],
    "失敗しても1サイクルに1回だけ投函します"
  );
  let output = logs.contents();
  assert!(output.contains("WARN"), "依頼失敗を WARN で記録します: {output}");
  assert!(output.contains("goal_id=g1"), "WARN にゴール ID を含めます: {output}");
  let decisions =
    harness.store.pending_decisions(chrono::Utc::now(), takt_dash_backend::model::PendingDecisionFilter::AwaitingMerge);
  assert!(decisions.is_empty(), "依頼が失敗しても、CLOSED の PR を持つゴールは判断待ちに出ません");

  // 2サイクル目も同じ失敗状態で、次の読み取りサイクルで再評価され再投函されます（合計2回）。
  poller.read_cycle_once().await.expect("2周期目を実行できます");
  assert_eq!(
    recorded_tool_names(&manager_calls),
    vec!["takt_finish_goal".to_owned(), "takt_finish_goal".to_owned()],
    "次の読み取りサイクルで再評価します"
  );
  poller.shutdown().await;
}

/// 偽の gh で、起動されたことをマーカーファイルへ記録する版。
fn write_fake_gh_with_marker(directory: &Path, state_file: &Path, marker: &Path) -> PathBuf {
  let script = directory.join("fake-gh-marker.sh");
  let body = format!(
    "#!/bin/sh\ntouch '{marker}'\nstate_file='{state}'\nif [ ! -f \"$state_file\" ]; then\n  exit 0\nfi\nvalue=$(cat \"$state_file\")\ncase \"$value\" in\n  EXIT:*) exit \"${{value#EXIT:}}\" ;;\nesac\nprintf '{{\"state\":\"%s\"}}' \"$value\"\n",
    marker = marker.display(),
    state = state_file.display()
  );
  std::fs::write(&script, body).expect("偽の gh を書けます");
  std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("偽の gh を実行可能にできます");
  script
}
