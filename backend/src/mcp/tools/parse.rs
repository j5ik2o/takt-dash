//! mcp 応答のページ形式と基本フィールドの解析補助。

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::ToolError;
use crate::domain::DomainError;

/// 値オブジェクトの検証失敗を、takt 応答の解析エラーへ写す。
pub(super) fn domain_value<T>(label: &str, result: Result<T, DomainError>) -> Result<T, ToolError> {
  result.map_err(|error| ToolError::Parse(format!("{label}: {error}")))
}

pub(super) struct Page {
  pub(super) items:       Vec<Value>,
  pub(super) next_offset: Option<usize>,
  /// 応答が 64 KiB に収まらず、takt-mcp が要素の文を短くした項目。
  pub(super) shortened:   Shortened,
}

impl Page {
  pub(super) fn from_value(value: &Value, key: &str) -> Result<Self, ToolError> {
    let items = value
      .get(key)
      .and_then(Value::as_array)
      .ok_or_else(|| ToolError::Parse(format!("応答に {key} の配列がありません")))?
      .clone();
    let next_offset = value.get("nextOffset").and_then(Value::as_u64).map(|value| value as usize);
    Ok(Page { items, next_offset, shortened: Shortened::from_value(value, key) })
  }
}

/// ページの要素のうち、takt-mcp が応答の大きさの上限で短くした項目（issue #230）。
///
/// takt-mcp の一覧は 64 KiB に収まるまで要素の数を減らし、1 件でも収まらないときだけ文を
/// 短くします。そのとき応答の最上位に `omissions: [{ "path": "<配列名>[].<項目>" }]` を付け、
/// 書き切れなかった分の数を `omittedDetails` に入れます。path の番号は `[]` にまとめられて
/// いるため、どの要素かは分かりません。ページの要素はそのとき 1 件なので、ページのすべての
/// 要素の、その項目を短くしたとみなします。`omittedDetails` が正のときは、どの項目かを
/// takt-mcp が書き切れていないので、すべての項目を短くしたとみなします。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Shortened {
  fields: BTreeSet<String>,
  all:    bool,
}

impl Shortened {
  pub(super) fn from_value(value: &Value, key: &str) -> Self {
    let prefix = format!("{key}[].");
    let fields = value
      .get("omissions")
      .and_then(Value::as_array)
      .into_iter()
      .flatten()
      .filter_map(|omission| omission.get("path").and_then(Value::as_str))
      .filter_map(|path| path.strip_prefix(&prefix))
      .map(str::to_owned)
      .collect();
    let all = value.get("omittedDetails").and_then(Value::as_u64).is_some_and(|count| count > 0);
    Shortened { fields, all }
  }

  /// 要素の `field`（`body`、`answer.text` など、要素からの path）を短くしたか。
  pub(super) fn covers(&self, field: &str) -> bool {
    self.all || self.fields.contains(field)
  }

  /// 要素の文の項目を読み、短くしてあれば印を付ける。
  /// 空の文は短くしようがないので、印を付けません。
  pub(super) fn preview(&self, value: &Value, field: &str) -> crate::model::PreviewText {
    let text = optional_string(value, field).unwrap_or_default();
    let truncated = !text.is_empty() && self.covers(field);
    crate::model::PreviewText { text, truncated }
  }

  /// 文を、短くしてあれば末尾に省略記号を付けた表示用の文にする。空の文はそのままです。
  pub(super) fn display(&self, text: String, field: &str) -> String {
    let cut = !text.is_empty() && self.covers(field);
    crate::model::with_ellipsis(&text, cut)
  }
}

pub(super) fn string_field(value: &Value, key: &str) -> Result<String, ToolError> {
  value
    .get(key)
    .and_then(Value::as_str)
    .map(str::to_owned)
    .ok_or_else(|| ToolError::Parse(format!("{key} が文字列ではありません")))
}

pub(super) fn optional_string(value: &Value, key: &str) -> Option<String> {
  value.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub(super) fn parse_time(value: Option<&Value>) -> Result<Option<DateTime<Utc>>, ToolError> {
  match value {
    | None | Some(Value::Null) => Ok(None),
    | Some(Value::String(text)) => DateTime::parse_from_rfc3339(text)
      .map(|time| Some(time.with_timezone(&Utc)))
      .map_err(|error| ToolError::Parse(format!("時刻 {text} を解析できませんでした: {error}"))),
    | Some(_) => Err(ToolError::Parse("時刻が文字列ではありません".to_owned())),
  }
}

/// 応答の必須の整数フィールドを読み取る。欠落は解析エラーにします。
pub(super) fn required_i32(value: &Value, key: &str) -> Result<i32, ToolError> {
  checked_i32(value.get(key), key)
}

/// 任意の位置の整数を検査する。欠落・数値以外・i32 の範囲外は解析エラーにします。
///
/// `as i32` による切り詰めを避け、takt から受け取った値と異なる値を送らないようにします。
pub(super) fn checked_i32(value: Option<&Value>, key: &str) -> Result<i32, ToolError> {
  let value = value.ok_or_else(|| ToolError::Parse(format!("{key} が数値ではありません")))?;
  let number = value.as_i64().ok_or_else(|| ToolError::Parse(format!("{key} が数値ではありません")))?;
  i32::try_from(number).map_err(|_| ToolError::Parse(format!("{key} が i32 の範囲外です: {number}")))
}

/// 応答の必須の文字列フィールドを読み取る。欠落は解析エラーにします。
pub(super) fn required_string(value: &Value, key: &str) -> Result<String, ToolError> {
  value
    .get(key)
    .and_then(Value::as_str)
    .map(str::to_owned)
    .ok_or_else(|| ToolError::Parse(format!("{key} が文字列ではありません")))
}
