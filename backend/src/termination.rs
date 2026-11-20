//! manager 用接続への終了依頼（`takt_finish_goal`）。
//!
//! backend は検出・gh による状態確認・manager への終了依頼までを担い、実際の `takt_finish_goal`
//! は manager が実行します。終了の副作用は manager 用の接続だけに限定し、読み取り・書き込みの
//! レーンや stdio の道具集合（[`crate::mcp::WriteToolSet`]）へは manager 専用の道具を公開しません。
//! この関数は manager 用のクライアントへ `takt_finish_goal` を1回だけ投函します。

use std::path::Path;

use serde_json::json;

use crate::{domain::GoalId, mcp::McpClient};

/// manager 用の接続へ終了依頼を1回投函する。
///
/// `client` は manager レーン（`manager: true` を付けた stdio エントリ）の子プロセスです。
/// 成功すると manager が `takt_finish_goal` を実行し、失敗すると呼び出し側が理由を記録して
/// 表示を維持します。入力は `{cwd, goalId, reason}` で、`cwd` は対象プロジェクトの cwd、
/// `reason` は確認したプルリクエストの状態を含む終了の理由です。`reason` は実機の takt-mcp の
/// 入力契約で必須です（窓口が takt-custom の dist の実物で確認済みです）。
pub async fn request_finish_goal(
  client: &mut McpClient,
  cwd: &Path,
  goal_id: &GoalId,
  reason: &str,
) -> Result<(), String> {
  let arguments = json!({ "cwd": cwd.display().to_string(), "goalId": goal_id.as_str(), "reason": reason });
  client.call_tool("takt_finish_goal", arguments).await.map(|_| ()).map_err(|error| error.to_string())
}
