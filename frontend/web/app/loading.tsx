import type { ReactElement } from "react";

export default function Loading(): ReactElement {
  return (
    <main className="page">
      <p role="status">読み込んでいます…</p>
    </main>
  );
}
