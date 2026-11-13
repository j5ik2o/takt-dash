taktDash のコード（Rust のバックエンドと CLI、Next.js のフロントエンド、.proto の API 定義）を計画、作成、実装、修正、またはレビューする場合は、次のコーディングルールに従ってください。

これらは CI とレビューのゲートで判定されるルールです。ゲートで落とされてから直すのではなく、書く時点で守ってください。機械判定のルールの正はリポジトリの CI 定義であり、ここに書くのは事前提示用の要約です。食い違いがあれば CI 定義に合わせてこちらを直します。

**機械判定されるルール**

- **rust-format**：Rust のコードは rustfmt で整形します。整形の正はリポジトリの rustfmt.toml です
- **rust-clippy**：clippy の警告を残しません。抑制（allow 属性）は理由を添えた箇所だけに使います
- **rust-test**：バックエンドと CLI のテストは cargo test で全件を通します
- **ts-checks**：Next.js 側は型検査と lint とテストを通します。各検査の正は package.json の scripts です
- **proto-sync**：.proto を変更したら、Rust（tonic）と TypeScript の生成物を両方とも再生成します。生成物は手で編集しません

**設計原則（レビューで判定）**

この節は機械判定ではなく、ワークフローの AI レビューで判定します。

- **api-source-of-truth**：API の正は .proto です。バックエンドとフロントエンドの間で .proto に無い形のデータをやり取りしません
- **backend-only-access**：フロントエンド（Next.js と CLI）はバックエンドの gRPC API だけを使います。フロントエンドが takt-mcp や .takt のファイルや SQLite へ直接触れる実装は差し戻しの理由になります
- **no-auth-by-design**：認証の仕組みを足す変更は要件の外です（要件定義 2.6 節）。認証が必要に見える場面があれば、実装せずに報告します
- **review-bot-comments**：レビューボット（CodeRabbit や Bugbot など）のコメントは無視しません。直すか、理由を述べて反論して、スレッドを解決します
