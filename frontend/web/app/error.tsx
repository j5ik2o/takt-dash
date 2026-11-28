"use client";

import { startTransition, type ReactElement } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";

export default function ErrorPage({ reset }: { error: Error & { digest?: string }; reset: () => void }): ReactElement {
  const router = useRouter();
  return (
    <main className="page">
      <h1>画面を表示できませんでした</h1>
      <p>もう一度読み込んでください。続くときは、backend の状態を確かめてください。</p>
      <button
        type="button"
        onClick={() => {
          startTransition(() => {
            router.refresh();
            reset();
          });
        }}
      >
        もう一度読み込む
      </button>
      <Link href="/">概要に戻る</Link>
    </main>
  );
}
