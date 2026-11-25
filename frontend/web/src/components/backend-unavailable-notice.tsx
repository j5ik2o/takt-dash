import type { ReactElement, ReactNode } from "react";
import type { BackendUnavailableSituation } from "../grpc/client";

// backend に接続できないときの表示。概要と判断待ちは画面側の内容が、それ以外の
// ページは各ローダーが使う（issue #92 で表示元を画面側の内容へ一本化した）。
// hard-coded な文言を散在させないため、文言と要素はここ1か所で持つ。
export const BACKEND_UNAVAILABLE_MESSAGE =
  "backend に接続できません。backend の起動と TAKTDASH_BACKEND_ADDR の設定を確認してください。";

// 期限内に応答が無いときの文面。接続できない場合とは分ける（issue #152）。
export const BACKEND_NO_RESPONSE_MESSAGE =
  "backend が応答しません。読み取りが遅れている可能性があります。";

// 一度応答を受けた後の接続不可で、切断が短時間のうちはこの文面にする。
// 設定は正しいはずなので、設定の確認は促さない。
export const BACKEND_UNREACHABLE_MESSAGE = "backend に接続できません。";

// 接続できない状態がこの時間を超えたら、起動の確認を促す（issue #152）。
export const BACKEND_UNREACHABLE_LONG_THRESHOLD_MS = 60_000;

export const BACKEND_UNREACHABLE_LONG_MESSAGE =
  "backend に接続できません。backend の起動を確認してください。";

/**
 * backend 不通の文面を状況から決める。
 *
 * 応答なしは接続不可と別の文面にする。設定の確認を促すのは、一度も応答を
 * 受けていないときだけに限る。一度でも応答を受けていれば設定は正しいはずで、
 * 接続できない状態が長く続く場合は起動の確認を促す。
 */
export function backendUnavailableMessage(situation: BackendUnavailableSituation): string {
  if (situation.failure === "no-response") {
    return BACKEND_NO_RESPONSE_MESSAGE;
  }
  if (!situation.respondedBefore) {
    return BACKEND_UNAVAILABLE_MESSAGE;
  }
  if (
    situation.durationMs !== undefined &&
    situation.durationMs >= BACKEND_UNREACHABLE_LONG_THRESHOLD_MS
  ) {
    return BACKEND_UNREACHABLE_LONG_MESSAGE;
  }
  return BACKEND_UNREACHABLE_MESSAGE;
}

// props の既定値は置かない。React は props オブジェクトを必ず渡すため実行時に
// 既定値は使われず、既定値があると createElement の props 型推論が崩れて
// situation を渡す呼び出しが型検査を通らなくなる。
export function BackendUnavailableNotice({
  situation,
}: {
  situation?: BackendUnavailableSituation;
}): ReactElement {
  return (
    <p className="banner warn" role="alert">
      {situation === undefined ? BACKEND_UNAVAILABLE_MESSAGE : backendUnavailableMessage(situation)}
    </p>
  );
}

/**
 * 切断の通知と、前に読めた内容を重ねて描画する（issue #309）。前に読めた内容が
 * 無いページは children を渡さず、通知だけを出す。概要と判断待ちと同じく、
 * 通知は見出しと内容の上に置く。
 */
export function BackendUnavailableOver({
  situation,
  children,
}: {
  situation: BackendUnavailableSituation;
  children?: ReactNode;
}): ReactElement {
  return (
    <>
      <BackendUnavailableNotice situation={situation} />
      {children}
    </>
  );
}
