"use client";

import type { ReactElement } from "react";

export default function ErrorPage({ reset }: { error: Error & { digest?: string }; reset: () => void }): ReactElement {
  return (
    <main className="page">
      <h1>画面を表示できませんでした</h1>
      <p>もう一度読み込んでください。続くときは、backend の状態を確かめてください。</p>
      <button type="button" onClick={() => reset()}>
        もう一度読み込む
      </button>
      <a href="/">概要に戻る</a>
    </main>
  );
}
