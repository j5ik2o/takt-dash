//! 完了契約を、本番コードと同じ関数・同じ生成クライアント・ダミー gRPC サーバーで観測する。
//!
//! 表示の組み立ては `render_overview` を、引数と接続先の解決と終了コードは `run_with` を、
//! 実際の gRPC 通信は生成クライアントと本番と同じサービス定義のダミーサーバーを通して確かめる。

mod addr;
mod arguments;
mod conversation;
mod fixtures;
mod goals;
mod overview;
mod support;
mod truncation;
