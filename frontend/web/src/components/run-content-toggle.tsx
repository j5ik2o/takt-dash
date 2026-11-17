"use client";

import { useState, type ReactElement, type SyntheticEvent } from "react";
import {
  initialRunContentView,
  restoreRunContentResponse,
  runContentFailed,
  runContentLoaded,
  runContentOpened,
  runContentUrl,
  type RunContentBody,
  type RunContentRequestTarget,
  type RunContentView,
} from "../state/run-content";

type RunContentToggleProps = {
  runSlug: string;
  target: RunContentRequestTarget;
};

const requestContent = async (runSlug: string, target: RunContentRequestTarget): Promise<RunContentBody> => {
  const response = await fetch(runContentUrl(runSlug, target), { cache: "no-store" });
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error ?? `本文を取得できません（${response.status}）。`);
  }
  return restoreRunContentResponse(await response.json());
};

// レポートや指示書の本文を、開いたときに BFF の Route Handler から取得して表示する。
export function RunContentToggle({ runSlug, target }: RunContentToggleProps): ReactElement {
  const [view, setView] = useState<RunContentView>(initialRunContentView);

  const onToggle = (event: SyntheticEvent<HTMLDetailsElement>): void => {
    if (!event.currentTarget.open) {
      return;
    }
    const opened = runContentOpened(view);
    setView(opened.view);
    if (!opened.fetch) {
      return;
    }
    requestContent(runSlug, target)
      .then((body) => setView(runContentLoaded(body)))
      .catch((error: unknown) =>
        setView(runContentFailed(error instanceof Error ? error.message : "本文を取得できませんでした。")),
      );
  };

  return (
    <details className="content-toggle" onToggle={onToggle}>
      <summary>本文を開く</summary>
      {view.status === "loading" ? <p className="note">本文を読み込んでいます。</p> : null}
      {view.status === "failed" ? <p className="note">本文を取得できませんでした。{view.error}</p> : null}
      {view.status === "loaded" ? (
        <>
          {view.body.truncated ? (
            <p className="note">本文が長いため、画面は本文の先頭の部分だけを表示しています。</p>
          ) : null}
          <pre className="rbody">{view.body.content}</pre>
        </>
      ) : null}
    </details>
  );
}
