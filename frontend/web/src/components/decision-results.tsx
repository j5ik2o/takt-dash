// 操作の結果一覧。成功は status、失敗は alert として読み上げ可能にする。
import type { ReactElement } from "react";
import type { DecisionResult } from "../state/decision-actions";

type DecisionResultsProps = {
  results: DecisionResult[];
};

export function DecisionResults({ results }: DecisionResultsProps): ReactElement | null {
  if (results.length === 0) {
    return null;
  }

  return (
    <div className="decision-results">
      <ul>
        {results.map((result) => (
          <li key={result.key} role={result.ok ? "status" : "alert"} className={result.ok ? "result ok" : "result error"}>
            <span className="result-label">{result.label}</span>
            <span className="result-message">{result.message}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
