import type { ReactElement } from "react";

// backend に接続できないときの表示。概要はレイアウト、他のページは各ローダーが使う。
// hard-coded な文言を散在させないため、文言と要素はここ1か所で持つ。
export const BACKEND_UNAVAILABLE_MESSAGE =
  "backend に接続できません。backend の起動と TAKTDASH_BACKEND_ADDR の設定を確認してください。";

export function BackendUnavailableNotice(): ReactElement {
  return (
    <p className="banner warn" role="alert">
      {BACKEND_UNAVAILABLE_MESSAGE}
    </p>
  );
}
