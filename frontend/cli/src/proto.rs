//! proto/ の .proto から生成した gRPC の型とクライアントを公開するモジュール。
//!
//! バックエンドの gRPC へ接続する閲覧コマンドは、後続のタスクでこのクレートへ足します。

pub mod v1 {
  tonic::include_proto!("takt_dash.v1");
}

#[cfg(test)]
mod tests {
  use super::v1;

  #[test]
  fn cli_references_the_generated_client_type() {
    let _client: std::marker::PhantomData<
      v1::takt_dash_service_client::TaktDashServiceClient<tonic::transport::Channel>,
    > = std::marker::PhantomData;
  }

  #[test]
  fn cli_references_the_generated_pending_decision_type() {
    let decision = v1::PendingDecision {
      r#type:       v1::PendingDecisionType::AwaitingMerge as i32,
      server_id:    "mini".to_owned(),
      project:      "takt".to_owned(),
      goal_id:      Some("g-takt".to_owned()),
      inquiry_kind: v1::InquiryKind::Unspecified as i32,
      created_at:   None,
      body:         String::new(),
      options:      Vec::new(),
      pull_request: Some(v1::PullRequest { number: 218, url: "https://github.com/j5ik2o/takt/pull/218".to_owned() }),
      stale:        false,
    };

    assert_eq!(decision.r#type, v1::PendingDecisionType::AwaitingMerge as i32);
    assert_eq!(decision.server_id, "mini");
  }
}
