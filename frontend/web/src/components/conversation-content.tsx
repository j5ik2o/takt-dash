import type { ReactElement } from "react";
import { formatDateTime } from "../state/format";
import {
  conversationSpeakerLabel,
  sortConversationMessages,
  type ConversationPage,
} from "../state/conversation";

type ConversationContentProps = {
  project: string;
  page?: ConversationPage;
  loading?: boolean;
  error?: string;
  picker?: ReactElement;
};

// 会話の表示は読み取り専用とし、発話者・時刻・本文を seq の昇順で出す。
// 出来事や質問への回答から始まったターンの元の入力は、本文の下の折りたたみに入れる。
// unavailable_reason は会話が無い理由として、skipped は理由と件数として補足表示する。
// 取得中・取得失敗は本文を出さず、ヘッダのプロジェクトと食い違わせない。
// picker は対象プロジェクトの選択 UI で、main の先頭に置く（補助技術が main の中として読む）。
export function ConversationContent({
  project,
  page,
  loading = false,
  error,
  picker,
}: ConversationContentProps): ReactElement {
  const messages = page === undefined ? [] : sortConversationMessages(page.messages);
  return (
    <main className="page">
      {picker}
      <header className="page-head">
        <div>
          <h1>会話</h1>
          <p>
            {project} の manager の会話です。{page === undefined ? "" : `プロバイダは ${page.provider} です。`}
          </p>
        </div>
      </header>
      {error !== undefined ? (
        <p className="banner warn" role="status">
          会話を取得できません: {error}
        </p>
      ) : null}
      {loading && page === undefined ? (
        <p className="note" role="status">
          会話を読み込んでいます。
        </p>
      ) : null}
      {page?.unavailableReason !== undefined ? (
        <p className="banner warn" role="status">
          会話を表示できません: {page.unavailableReason}
        </p>
      ) : null}
      {page !== undefined && page.skipped.length > 0 ? (
        <ul className="conversation-skipped">
          {page.skipped.map((item) => (
            <li key={item.reason}>{`スキップした行: ${item.reason} ${item.count}件`}</li>
          ))}
        </ul>
      ) : null}
      <ol className="conversation">
        {messages.map((message) => (
          <li className="conversation-message" key={message.seq}>
            <p className="conversation-meta">
              <span className="conversation-seq">{`seq ${message.seq}`}</span>
              <span className="conversation-speaker">{conversationSpeakerLabel(message.speaker)}</span>
              <time className="conversation-at">
                {message.occurredAt !== undefined ? formatDateTime(message.occurredAt) : "時刻不明"}
              </time>
            </p>
            {message.notice !== undefined ? (
              <p className="conversation-notice" role="note">
                {message.notice}
              </p>
            ) : null}
            <p className="conversation-text">{message.text}</p>
            {message.detail !== undefined ? (
              <details className="conversation-detail">
                <summary>元の内容を表示します。</summary>
                {message.detail.question !== undefined ? (
                  <>
                    <p className="conversation-detail-label">質問</p>
                    <p className="conversation-text">{message.detail.question}</p>
                    <p className="conversation-detail-label">元の JSON</p>
                  </>
                ) : null}
                <pre>{message.detail.sourceJson}</pre>
              </details>
            ) : null}
          </li>
        ))}
      </ol>
      {page !== undefined && messages.length === 0 && page.unavailableReason === undefined && !loading && error === undefined ? (
        <p className="note">会話はありません。</p>
      ) : null}
    </main>
  );
}
