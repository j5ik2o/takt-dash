use serde_json::{json, Value};

use super::{string_field, ToolError, ToolSet, MAX_PAGES};

pub(super) fn field_is_omitted(value: &Value, path: &str) -> bool {
  let Some(omissions) = value.get("omissions") else { return false };
  let omission = match omissions {
    | Value::Array(items) => items.iter().find(|item| item.get("path").and_then(Value::as_str) == Some(path)),
    | Value::Object(items) => items.get(path),
    | _ => None,
  };
  omission.is_some_and(|item| {
    item
      .get("totalBytes")
      .and_then(Value::as_u64)
      .zip(item.get("returnedBytes").and_then(Value::as_u64))
      .is_some_and(|(total, returned)| returned < total)
  })
}

impl ToolSet<'_> {
  pub(super) async fn restore_goal_record_body(
    &mut self,
    goal_id: &str,
    collection: &str,
    record: &mut Value,
  ) -> Result<(), ToolError> {
    let record_id = string_field(record, "id")?;
    let path = format!("{collection}.body");
    let mut body = String::new();
    let mut offset = 0;
    for _ in 0..MAX_PAGES {
      let page = self
        .call_json(
          "takt_get_goal",
          json!({
            "goalId": goal_id, "path": path, "recordId": record_id, "textOffset": offset
          }),
        )
        .await?;
      let chunk = string_field(&page, "content")?;
      body.push_str(&chunk);
      match page.get("nextOffset").and_then(Value::as_u64) {
        | Some(next) if next > offset => offset = next,
        | Some(_) => return Err(ToolError::Parse(format!("{path} の nextOffset が進みません"))),
        | None => {
          record["body"] = Value::String(body);
          return Ok(());
        },
      }
    }
    Err(ToolError::Parse(format!("{path} の全文取得が上限に達しました")))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn matches_only_the_requested_field_with_missing_bytes() {
    let value = json!({ "omissions": [
      { "path": "questions[].body", "totalBytes": 90, "returnedBytes": 9 },
      { "path": "summary", "totalBytes": 9, "returnedBytes": 9 }
    ]});
    assert!(field_is_omitted(&value, "questions[].body"));
    assert!(!field_is_omitted(&value, "inquiries[].body"));
    assert!(!field_is_omitted(&value, "summary"));
    assert!(!field_is_omitted(&json!({}), "summary"));
    assert!(field_is_omitted(
      &json!({ "omissions": { "summary": {
        "totalBytes": 10, "returnedBytes": 9
      }}}),
      "summary"
    ));
  }
}
