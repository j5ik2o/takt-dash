// 操作の結果一覧。成功は status、失敗は alert として読み上げ可能にする。
"use client";
import { useId, type ReactElement } from "react";
import type { DecisionResult } from "../state/decision-actions";
import { formatClock } from "../state/format";

type DecisionResultsProps = {
  results: DecisionResult[];
  onClose?: (result: DecisionResult) => void;
};

function ResultRow({ result, onClose }: { result: DecisionResult; onClose?: (result: DecisionResult) => void }): ReactElement {
  const descriptionId = useId();
  const at = result.at instanceof Date ? result.at : undefined;
  const handleClose = (event: { currentTarget: HTMLButtonElement }): void => {
    // 閉じた後のフォーカス先を、行が消える前に移す。次の行（無ければ前の行）の
    // 「閉じる」は key で残る DOM なので、削除後もフォーカスが保たれる。
    const row = event.currentTarget.closest("li");
    const list = row?.parentElement;
    if (row !== null && row !== undefined && list !== null && list !== undefined) {
      const rows = Array.from(list.querySelectorAll(":scope > li"));
      const index = rows.indexOf(row);
      const targetRow = rows[index + 1] ?? rows[index - 1];
      const targetButton = targetRow?.querySelector("button");
      if (targetButton instanceof HTMLElement) {
        targetButton.focus();
      } else {
        const heading = document.querySelector("h1");
        if (heading instanceof HTMLElement) {
          heading.focus();
        }
      }
    }
    onClose?.(result);
  };
  return (
    <li role={result.ok ? "status" : "alert"} className={result.ok ? "result ok" : "result error"}>
      <span className="result-label">{result.label}</span>
      <span className="result-message">{result.message}</span>
      {at !== undefined ? <time dateTime={at.toISOString()}>{formatClock(at)}</time> : null}
      <span id={descriptionId} className="visually-hidden">
        {result.label}
      </span>
      <button type="button" aria-describedby={descriptionId} onClick={handleClose}>
        閉じる
      </button>
    </li>
  );
}

export function DecisionResults({ results, onClose }: DecisionResultsProps): ReactElement | null {
  if (results.length === 0) {
    return null;
  }
  return (
    <div className="decision-results">
      <ul>
        {results.map((result) => (
          <ResultRow key={result.key} result={result} onClose={onClose} />
        ))}
      </ul>
    </div>
  );
}
