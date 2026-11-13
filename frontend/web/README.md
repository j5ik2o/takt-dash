このディレクトリは、taktDash のフロントエンドである Next.js アプリを置く場所です。
このアプリは、バックエンドの gRPC API から取得した状態を画面に表示します。

## 開発サーバーを起動する

開発サーバーを起動するには、次のコマンドを実行します。

```bash
npm run dev
```

開発サーバーを起動したら、ブラウザで http://localhost:3000 を開くと、画面を確認できます。

## 画面を編集する

画面を編集するには、`app/page.tsx` を書き換えます。`app/page.tsx` を保存すると、開発サーバーが変更を反映して画面を更新します。

このアプリは、フォントの最適化に `next/font` を使っています。

## 検査を実行する

次のコマンドで、型検査と lint とテストを実行します。

```bash
npm run typecheck
npm run lint
npm run test
```

## 詳しい情報を調べる

Next.js の詳しい情報は、次の資料を参照してください。

- [Next.js のドキュメント](https://nextjs.org/docs)
- [Next.js を学ぶ](https://nextjs.org/learn)

## Vercel へデプロイする

このアプリをデプロイする方法は、[Next.js のデプロイのドキュメント](https://nextjs.org/docs/app/building-your-application/deploying) を参照してください。
