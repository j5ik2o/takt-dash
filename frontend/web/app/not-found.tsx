import type { ReactElement } from "react";

export default function NotFound(): ReactElement {
  return (
    <main className="page">
      <h1>ページが見つかりません</h1>
      <p>URL が正しいかを確かめてください。run やゴールは、終わってから時間がたつと消えることがあります。</p>
      <a href="/">概要に戻る</a>
    </main>
  );
}
