# proto/

このディレクトリは、taktDash の API 定義の正となる `.proto` ファイルを置く場所です。

バックエンドとコマンドラインと Next.js は、このディレクトリの `.proto` から生成したコードを使います。

## 生成の手順

`.proto` を変更したときは、次の手順で Rust と TypeScript の両方の生成コードを再生成します。

- Rust（バックエンドとコマンドライン）: それぞれのクレートの `build.rs` が `tonic-prost-build` でビルドのたびに生成するため、`cargo build` または `cargo test` を実行すれば再生成されます。生成物はリポジトリへコミットしません。
- TypeScript（Next.js）: リポジトリのルートで `make proto-gen` を実行すると `frontend/web/src/proto/` へ生成されます。生成物はリポジトリへコミットします。

## 生成コードの扱い

生成したコードは手で編集しません。変更は必ず `.proto` に対して行います。

`make done` は `proto-check` を通じて TypeScript の生成物を再生成し、`.proto` と生成物が同期していること（再生成後の差分が無いこと）を検査します。Rust の生成物はビルドのたびに作られるため、`.proto` と構造的に同期します。
