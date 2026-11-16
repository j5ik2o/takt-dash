//! mcp 応答のページ形式と基本フィールドの解析補助。

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::ToolError;

pub(super) struct Page {
  pub(super) items:       Vec<Value>,
  pub(super) next_offset: Option<usize>,
}

impl Page {
  pub(super) fn from_value(value: &Value, key: &str) -> Result<Self, ToolError> {
    let items = value
      .get(key)
      .and_then(Value::as_array)
      .ok_or_else(|| ToolError::Parse(format!("応答に {key} の配列がありません")))?
      .clone();
    let next_offset = value.get("nextOffset").and_then(Value::as_u64).map(|value| value as usize);
    Ok(Page { items, next_offset })
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
