//! gRPC の各 RPC が使う接続レーンの取っ手と、書き込みの通し番号の記録。
//!
//! 書き込み道具を呼ぶ RPC は書き込みレーン、純粋な読み取り RPC は画面読み取り
//! レーンを使います。レーンごとに子プロセスが分かれるため、重い読み取りが
//! 書き込みを待たせることはありません。

use super::{ResolvedProject, TaktDashServiceImpl};
use crate::mcp::{ClientHandle, Lane};

impl TaktDashServiceImpl {
  /// 画面読み取りレーンのクライアントを返す。複数本ある場合は要求ごとに
  /// ラウンドロビンで1本を選びます。呼び出し側がロックして使います。
  pub(super) fn screen_handle(&self, resolved: &ResolvedProject) -> ClientHandle {
    self.registry.screen_client(
      resolved.server_id.as_str(),
      resolved.project.as_str(),
      resolved.screen_read_lanes,
      || resolved.client_config.clone(),
    )
  }

  /// 書き込みレーンのクライアントを返す。呼び出し側がロックして使います。
  pub(super) fn write_handle(&self, resolved: &ResolvedProject) -> ClientHandle {
    self
      .registry
      .client(resolved.server_id.as_str(), resolved.project.as_str(), Lane::Write, || resolved.client_config.clone())
      .0
  }

  /// 書き込みの成功を記録し、そのプロジェクトの書き込みの通し番号を進める。
  ///
  /// 定期読み取りは、読み取り開始時点の番号と現在の番号を比べ、進んでいたら
  /// 読み取り結果を捨てます。書き込み前の状態で新しい状態を上書きしないためです。
  pub(super) fn record_write(&self, resolved: &ResolvedProject) {
    self.registry.record_write(resolved.server_id.as_str(), resolved.project.as_str());
  }
}
