//! ゴール登録案の道具（一覧、承認、却下）と、登録案の本文の組み立て。
//!
//! takt-mcp は、登録案の持ち主の会話（conversationId）を一覧で返し、承認・却下と本文の読み取りで
//! 必須にします。登録案を読む道具はこのモジュールに置きます。

use serde_json::{json, Value};

use super::{
  parse::{domain_value, required_i32, required_string},
  ToolError, ToolSetCore,
};
use crate::{
  domain::{ConversationId, GoalId, RegistrationRevision},
  model::{ProposalSource, RegistrationProposalEntry},
};

/// 本文の一部が省略されたときに、本文の末尾へ足す注記。
const TRUNCATED_SUMMARY_NOTE: &str = "\n（長い項目は先頭だけを表示しています）";

/// 省略された項目の続きを読む道具呼び出しの、登録案 1 件あたりの上限。1 回は最大 4096 バイト
/// なので、本文はおよそ 256 KiB まで全文で出します。続きは revision が変わったときだけ読みます。
const MAX_FIELD_READS: usize = 64;

/// `summary` の項目のうち、文字列の一覧であるもの。
const LIST_FIELDS: [&str; 3] = ["outOfScope", "acceptanceCriteria", "constraints"];

/// 登録案 1 件の項目の続きを読む間の、持ち主と revision と残りの読み取り回数。
struct FieldReader<'a> {
  revision:        RegistrationRevision,
  conversation_id: &'a ConversationId,
  remaining:       usize,
}

/// 本文の応答の omissions が `summary.<key>` を含むか。takt-mcp は一覧の要素の省略を
/// `proposal.summary.<key>[]` とまとめて書きます。omissions が無い・読めない応答は、
/// どの項目が省略されたか分からないため、含むとみなします。
fn is_omitted(value: &Value, key: &str) -> bool {
  let Some(omissions) = value.get("omissions").and_then(Value::as_array) else {
    return true;
  };
  let field = format!("proposal.summary.{key}");
  omissions
    .iter()
    .filter_map(|omission| omission.get("path").and_then(Value::as_str))
    .any(|path| path == field || path.strip_prefix(field.as_str()).is_some_and(|rest| rest.starts_with(['.', '['])))
}

/// 上限で読み切れなかった文字列は、先頭だけの候補のうち長いほうを残す。
fn keep_longer(text: &mut String, candidate: String) {
  if candidate.len() > text.len() {
    *text = candidate;
  }
}

impl ToolSetCore<'_> {
  /// ゴール登録の承認待ちを取得する。提案が無ければ空の並びを返します。
  ///
  /// 一覧（takt_list_goal_registration_proposals）は、登録案の本文（summary）を返さず、項目の
  /// 一覧（fields）だけを返します。本文は takt_get_goal_registration_proposal で読みます。
  /// `known` は前回読んだ登録案です。同じ接続先・同じ持ち主・同じ revision の登録案は、前回の
  /// 本文を使い回し、本文を読み直しません。takt は登録案を作り直すたびに revision を進めるため、
  /// revision が同じなら本文も同じです。
  pub async fn list_goal_registration_proposals(
    &mut self,
    known: &[RegistrationProposalEntry],
  ) -> Result<Vec<RegistrationProposalEntry>, ToolError> {
    let value = self.call_json("takt_list_goal_registration_proposals", json!({})).await?;
    let proposals = value.get("proposals").and_then(Value::as_array).ok_or_else(|| {
      ToolError::Parse("takt_list_goal_registration_proposals の応答に proposals の配列がありません".to_owned())
    })?;
    let source = ProposalSource { connection: self.client.connection_key(), cwd: self.cwd.clone() };
    let mut entries = Vec::with_capacity(proposals.len());
    for proposal in proposals {
      let revision = domain_value("revision", RegistrationRevision::try_from(required_i32(proposal, "revision")?))?;
      let conversation_id = domain_value(
        "conversation ID",
        ConversationId::try_from(required_string(proposal, "conversationId")?.as_str()),
      )?;
      let read_before = known.iter().find(|entry| {
        entry.revision == revision
          && entry.conversation_id == conversation_id
          && entry.source == source
          && !entry.summary.is_empty()
      });
      let summary = match (proposal.get("summary"), read_before) {
        // 本文を一覧に載せる応答（以前の takt-mcp）は、そのまま使います。
        | (Some(summary), _) if !summary.is_null() => registration_summary_text(Some(summary))?,
        | (_, Some(entry)) => entry.summary.clone(),
        | (_, None) => self.read_registration_summary(revision, &conversation_id).await?,
      };
      entries.push(RegistrationProposalEntry {
        source: source.clone(),
        revision,
        conversation_id,
        repository_directory: required_string(proposal, "repositoryDirectory")?,
        summary,
      });
    }
    Ok(entries)
  }

  /// 登録案の本文を takt_get_goal_registration_proposal で読み、表示用の本文にする。
  ///
  /// 一覧のあとに登録案が作り直されると、読んだ本文は別の revision のものになります。その
  /// 本文を一覧の revision に付けないよう、revision が違えば解析エラーにします。定期読み取りは
  /// 前回の登録案へ縮退し、次の周期で読み直します。
  async fn read_registration_summary(
    &mut self,
    revision: RegistrationRevision,
    conversation_id: &ConversationId,
  ) -> Result<String, ToolError> {
    let value = self
      .call_json("takt_get_goal_registration_proposal", json!({ "conversationId": conversation_id.as_str() }))
      .await?;
    let proposal = value.get("proposal").ok_or_else(|| {
      ToolError::Parse("takt_get_goal_registration_proposal の応答に proposal がありません".to_owned())
    })?;
    let read = domain_value("revision", RegistrationRevision::try_from(required_i32(proposal, "revision")?))?;
    if read != revision {
      return Err(ToolError::Parse(format!(
        "登録案が一覧の取得後に作り直されました（一覧 revision {}、本文 revision {}）",
        revision.get(),
        read.get()
      )));
    }
    // 応答が 64 KiB に収まらないと、takt-mcp は長い文字列を先頭だけにして truncated を立てます。
    if value.get("truncated").and_then(Value::as_bool) != Some(true) {
      return registration_summary_text(proposal.get("summary"));
    }
    let Some(Value::Object(object)) = proposal.get("summary") else {
      // 文字列の本文（以前の takt-mcp）は、path で読む項目を持ちません。
      let mut summary = registration_summary_text(proposal.get("summary"))?;
      summary.push_str(TRUNCATED_SUMMARY_NOTE);
      return Ok(summary);
    };
    let mut object = object.clone();
    let mut reader = FieldReader { revision, conversation_id, remaining: MAX_FIELD_READS };
    let mut complete = true;
    for (key, _) in REGISTRATION_SUMMARY_FIELDS {
      if !object.contains_key(key) || !is_omitted(&value, key) {
        continue;
      }
      let path = format!("summary.{key}");
      let first = object.get(key);
      let (full, read_all) = if LIST_FIELDS.contains(&key) {
        let first_items = first
          .and_then(Value::as_array)
          .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
          .unwrap_or_default();
        let (items, read_all) = self.read_proposal_list(&mut reader, &path, first_items).await?;
        (Value::Array(items.into_iter().map(Value::String).collect()), read_all)
      } else {
        let first_text = first.and_then(Value::as_str).unwrap_or_default().to_owned();
        let (mut text, read_all) = self.read_proposal_text(&mut reader, &path).await?;
        if !read_all {
          keep_longer(&mut text, first_text);
        }
        (Value::String(text), read_all)
      };
      complete &= read_all;
      object.insert(key.to_owned(), full);
    }
    let mut summary = compose_registration_summary(&object);
    if !complete {
      summary.push_str(TRUNCATED_SUMMARY_NOTE);
    }
    Ok(summary)
  }

  /// 登録案の文字列の項目を textOffset で読み、全文を返す。読み取りの上限に達したら、読めた分と
  /// false を返します。
  async fn read_proposal_text(
    &mut self,
    reader: &mut FieldReader<'_>,
    path: &str,
  ) -> Result<(String, bool), ToolError> {
    let mut content = String::new();
    let mut offset = 0;
    while let Some(page) = self.read_proposal_field(reader, json!({ "path": path, "textOffset": offset })).await? {
      let chunk = page
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::Parse(format!("{path} の content が文字列ではありません")))?;
      content.push_str(chunk);
      match page.get("nextOffset").and_then(Value::as_u64) {
        | Some(next) => offset = next,
        | None => return Ok((content, true)),
      }
    }
    Ok((content, false))
  }

  /// 登録案の一覧の項目を offset で読み、全要素を返す。takt-mcp が要素を先頭だけにしたページは、
  /// そのページの要素を 1 つずつ textOffset で読み直します。`entries` は最初の応答の要素で、
  /// 読めた要素だけを同じ番号で置き換えます。読み取りの上限に達したら、まだ読んでいない要素は
  /// 最初の応答のまま残し、false を返します。
  async fn read_proposal_list(
    &mut self,
    reader: &mut FieldReader<'_>,
    path: &str,
    mut entries: Vec<String>,
  ) -> Result<(Vec<String>, bool), ToolError> {
    let mut offset = 0;
    while let Some(page) = self.read_proposal_field(reader, json!({ "path": path, "offset": offset })).await? {
      let items = page
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::Parse(format!("{path} の items が配列ではありません")))?;
      let shortened = page.get("omissions").and_then(Value::as_array).is_some_and(|omissions| !omissions.is_empty());
      for (index, item) in items.iter().enumerate() {
        let at = offset as usize + index;
        let mut text =
          item.as_str().ok_or_else(|| ToolError::Parse(format!("{path} の要素が文字列ではありません")))?.to_owned();
        let mut read_all = true;
        if shortened {
          let (full, read) = self.read_proposal_text(reader, &format!("{path}.{at}")).await?;
          read_all = read;
          if read_all {
            text = full;
          } else {
            keep_longer(&mut text, full);
            if let Some(first) = entries.get(at) {
              keep_longer(&mut text, first.clone());
            }
          }
        }
        match entries.get_mut(at) {
          | Some(entry) => *entry = text,
          | None => entries.push(text),
        }
        if !read_all {
          return Ok((entries, false));
        }
      }
      match page.get("nextOffset").and_then(Value::as_u64) {
        | Some(next) => offset = next,
        | None => {
          entries.truncate(offset as usize + items.len());
          return Ok((entries, true));
        },
      }
    }
    Ok((entries, false))
  }

  /// 登録案の項目を 1 回読む。読み取りの上限に達していたら None を返します。一覧のあとに
  /// 登録案が作り直されて revision が違えば、本文を混ぜないよう解析エラーにします。
  async fn read_proposal_field(
    &mut self,
    reader: &mut FieldReader<'_>,
    mut extra: Value,
  ) -> Result<Option<Value>, ToolError> {
    if reader.remaining == 0 {
      return Ok(None);
    }
    reader.remaining -= 1;
    extra["conversationId"] = json!(reader.conversation_id.as_str());
    let value = self.call_json("takt_get_goal_registration_proposal", extra).await?;
    let read = domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))?;
    if read != reader.revision {
      return Err(ToolError::Parse(format!(
        "登録案が読み取りの途中で作り直されました（一覧 revision {}、本文 revision {}）",
        reader.revision.get(),
        read.get()
      )));
    }
    Ok(Some(value))
  }

  /// ゴール登録の承認待ちを承認する。承認した revision と、登録されたゴールの ID を返します。
  ///
  /// `conversation_id` は、一覧が返した登録案の持ち主です。takt-mcp は必須の入力として受け取り、
  /// 持ち主でなければ断ります。
  pub async fn approve_goal_registration(
    &mut self,
    revision: RegistrationRevision,
    conversation_id: &ConversationId,
  ) -> Result<ApproveGoalRegistrationResult, ToolError> {
    let value = self
      .call_json(
        "takt_approve_goal_registration",
        json!({ "revision": revision.get(), "conversationId": conversation_id.as_str() }),
      )
      .await?;
    let goal = value.get("goal").ok_or_else(|| ToolError::Parse("応答に goal がありません".to_owned()))?;
    let goal_id = domain_value("ゴール ID", GoalId::try_from(required_string(goal, "id")?.as_str()))?;
    let revision = domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))?;
    Ok(ApproveGoalRegistrationResult { revision, goal_id })
  }

  /// ゴール登録の承認待ちを却下する。却下した revision を返します。
  /// `conversation_id` の扱いは承認と同じです。
  pub async fn reject_goal_registration(
    &mut self,
    revision: RegistrationRevision,
    conversation_id: &ConversationId,
  ) -> Result<RegistrationRevision, ToolError> {
    let value = self
      .call_json(
        "takt_reject_goal_registration",
        json!({ "revision": revision.get(), "conversationId": conversation_id.as_str() }),
      )
      .await?;
    domain_value("revision", RegistrationRevision::try_from(required_i32(&value, "revision")?))
  }
}

/// takt_approve_goal_registration の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApproveGoalRegistrationResult {
  pub revision: RegistrationRevision,
  pub goal_id:  GoalId,
}

/// ゴール登録提案の `summary` を表示本文へ変換する。
///
/// 文字列はそのまま本文にします。オブジェクトは既知の項目だけを日本語の文として組み立て、
/// 未知のキーと欠落は無視します。どちらでもない値だけを解析エラーにします。
fn registration_summary_text(value: Option<&Value>) -> Result<String, ToolError> {
  match value {
    | None | Some(Value::Null) => Ok(String::new()),
    | Some(Value::String(text)) => Ok(text.clone()),
    | Some(Value::Object(object)) => Ok(compose_registration_summary(object)),
    | Some(_) => Err(ToolError::Parse("summary が文字列でもオブジェクトでもありません".to_owned())),
  }
}

/// `summary` オブジェクトから本文へ含める既知の項目と見出し。
const REGISTRATION_SUMMARY_FIELDS: [(&str, &str); 6] = [
  ("objective", "目的"),
  ("outOfScope", "対象外"),
  ("acceptanceCriteria", "受入条件"),
  ("constraints", "制約"),
  ("startBranch", "開始ブランチ"),
  ("integrationBranch", "統合ブランチ"),
];

fn compose_registration_summary(object: &serde_json::Map<String, Value>) -> String {
  let mut lines = Vec::new();
  for (key, label) in REGISTRATION_SUMMARY_FIELDS {
    if let Some(text) = registration_summary_field(object.get(key)) {
      lines.push(format!("{label}: {text}"));
    }
  }
  lines.join("\n")
}

/// `summary` オブジェクトの項目値を本文用の文字列にする。文字列と文字列の配列だけを受け付け、
/// それ以外の型と空の配列は無視します。
fn registration_summary_field(value: Option<&Value>) -> Option<String> {
  match value? {
    | Value::String(text) => Some(text.clone()),
    | Value::Array(items) => {
      let texts: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
      if texts.is_empty() {
        None
      } else {
        Some(texts.join("、"))
      }
    },
    | _ => None,
  }
}
