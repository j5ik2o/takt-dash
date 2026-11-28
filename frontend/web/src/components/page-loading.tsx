import type { ReactElement } from "react";

export function PageLoading(): ReactElement {
  return (
    <main className="page">
      <p role="status">読み込んでいます…</p>
    </main>
  );
}
