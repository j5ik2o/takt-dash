import type { NextRequest } from "next/server";
import { OperationAnswerReport } from "../../../../src/proto/taktdash";
import { badRequest, isNonEmptyString, readJsonObject, runWrite } from "../_shared";

// 問い合わせへの回答。text（自由記述・選択肢）と report（作業の依頼）のどちらか一方だけを
// 受け取り、そのまま AnswerInquiry へ渡す。
export async function POST(request: NextRequest): Promise<Response> {
  const body = await readJsonObject(request);
  if (body === undefined) {
    return badRequest("リクエストの本文が JSON オブジェクトではありません。");
  }

  const { serverId, project, inquiryId } = body;
  if (!isNonEmptyString(serverId)) {
    return badRequest("serverId がありません。");
  }
  if (!isNonEmptyString(project)) {
    return badRequest("project がありません。");
  }
  if (!isNonEmptyString(inquiryId)) {
    return badRequest("inquiryId がありません。");
  }

  if (body.goalId !== undefined && !isNonEmptyString(body.goalId)) {
    return badRequest("goalId が正しくありません。");
  }
  const goalId = body.goalId as string | undefined;

  const hasText = body.text !== undefined;
  const hasReport = body.report !== undefined;
  if (hasText === hasReport) {
    return badRequest("text と report はどちらか一方だけを指定してください。");
  }

  let text: string | undefined;
  let report: OperationAnswerReport | undefined;

  if (hasText) {
    if (typeof body.text !== "string") {
      return badRequest("text は文字列で指定してください。");
    }
    const trimmed = body.text.trim();
    if (trimmed === "") {
      return badRequest("回答の本文が空です。");
    }
    text = trimmed;
  } else {
    const reportValue = body.report;
    if (
      reportValue !== OperationAnswerReport.OPERATION_ANSWER_REPORT_COMPLETED &&
      reportValue !== OperationAnswerReport.OPERATION_ANSWER_REPORT_UNABLE
    ) {
      return badRequest("report は 1（実施した）か 2（対応できない）を指定してください。");
    }
    report = reportValue;
  }

  return runWrite((client) =>
    client.answerInquiry({ serverId, project, goalId, inquiryId, text, report }),
  );
}
