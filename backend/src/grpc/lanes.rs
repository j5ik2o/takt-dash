//! gRPC の各 RPC が使う接続レーンの取っ手と、書き込みゲートと通し番号の記録。
//!
//! 書き込み道具を呼ぶ RPC は書き込みレーン、純粋な読み取り RPC は画面読み取り
//! レーンを使います。レーンごとに子プロセスが分かれるため、重い読み取りが
//! 書き込みを待たせることはありません。

use super::{
  resolve::WriteSource, tool_error_status, write_ledger::WriteFailure, ResolvedProject, TaktDashServiceImpl,
  READ_TIMEOUT,
};
use crate::{
  config::McpConnection,
  mcp::{client_config, ClientHandle, HttpReadHandle, Lane, McpClientRegistry},
};

impl TaktDashServiceImpl {
  /// 画面読み取りレーンのクライアントを返す。複数本ある場合は要求ごとに
  /// ラウンドロビンで1本を選びます。呼び出し側がロックして使います。
  pub(super) fn screen_handle(&self, resolved: &ResolvedProject) -> ClientHandle {
    let McpConnection::Stdio(server) = &resolved.connection else {
      panic!("画面読み取りレーンは stdio 接続先だけにあります。HTTP 接続先の読み取りは ReadSource 経由で行います。");
    };
    let config = client_config(server, &resolved.cwd);
    self.registry.screen_client(
      resolved.server_id.as_str(),
      resolved.project.as_str(),
      resolved.screen_read_lanes,
      || config.clone(),
    )
  }

  /// 1 回の書き込みが使う経路を作る。
  pub(super) fn write_lane(&self, resolved: ResolvedProject) -> WriteLane {
    WriteLane { registry: self.registry.clone(), resolved }
  }
}

/// 1 回の書き込みが使う、プロジェクトの書き込みゲート・書き込み元・通し番号の記録。
///
/// 書き込みは呼び出しの打ち切りと切り離して最後まで実行するため、サービスを借用せず、
/// レジストリの取っ手と解決済みの対象を所有します。
pub(super) struct WriteLane {
  registry:            McpClientRegistry,
  pub(super) resolved: ResolvedProject,
}

impl WriteLane {
  /// プロジェクトの書き込みゲートを取得する。
  ///
  /// 書き込み RPC を発行する前から、成功後の [`Self::record_write`] まで保持します。
  /// 定期読み取りは結果の反映の段で同じゲートを保持するため、書き込みの適用と
  /// 通し番号の前進が、読み取り結果の古さの確認と反映に対して原子的になります。
  pub(super) async fn gate(&self) -> tokio::sync::OwnedMutexGuard<()> {
    self.registry.write_gate(self.resolved.write_server_id.as_str(), self.resolved.project.as_str()).lock_owned().await
  }

  /// 書き込み元を作る。HTTP 接続先は常駐サーバーのハンドル、stdio 接続先は書き込みレーンの
  /// クライアントを使います。
  fn source(&self) -> WriteSource {
    match &self.resolved.write_connection {
      | McpConnection::Http(_) => {
        WriteSource::Http(Box::new(HttpReadHandle::new_for_write(self.resolved.write_connection.clone(), READ_TIMEOUT)))
      },
      | McpConnection::Stdio(server) => {
        let config = client_config(server, &self.resolved.cwd);
        let (handle, _) = self.registry.client(
          self.resolved.write_server_id.as_str(),
          self.resolved.project.as_str(),
          Lane::Write,
          || config.clone(),
        );
        WriteSource::Stdio(handle)
      },
    }
  }

  /// 接続を済ませた書き込み元を返す。接続の失敗は、書き込みを送る前の失敗として返します。
  pub(super) async fn connected_source(&self) -> Result<WriteSource, WriteFailure> {
    let mut source = self.source();
    source.connect().await.map_err(|error| WriteFailure::Failed(tool_error_status(error)))?;
    Ok(source)
  }

  /// 書き込みの成功を記録し、そのプロジェクトの書き込みの通し番号を進める。
  ///
  /// 定期読み取りは、読み取り開始時点の番号と現在の番号を比べ、進んでいたら
  /// 読み取り結果を捨てます。書き込み前の状態で新しい状態を上書きしないためです。
  pub(super) fn record_write(&self) {
    self.registry.record_write(self.resolved.write_server_id.as_str(), self.resolved.project.as_str());
  }
}
