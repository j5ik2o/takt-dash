//! proto/ の .proto から生成した gRPC の型とクライアントを公開するモジュール。
//!
//! 閲覧コマンドは、このモジュールが公開する生成クライアントで backend の gRPC へ接続します。
//! 生成物は手で編集しません。

pub mod v1 {
  tonic::include_proto!("takt_dash.v1");
}
