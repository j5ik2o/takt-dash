//! CLI が扱うドメイン上の値を表す型（値オブジェクト）。
//!
//! 各型は size → lexical content → syntax → semantics の順で検証し、検証を通らない値からは
//! 生成できません。生成方法は [`TryFrom`] に統一し、無検査の公開コンストラクタは置きません。
//! 参照先の存在・一致のような外部状態を要する制約は、型ではなく backend の既存境界で検証します。

use std::fmt;

/// 値オブジェクトの生成に失敗した段と理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
  /// 長さ・件数・範囲の違反。
  Size(&'static str),
  /// 使える文字の種類の違反。
  Lexical(&'static str),
  /// 区切りや書式の違反。
  Syntax(&'static str),
  /// 意味上の制約の違反。
  Semantics(&'static str),
}

impl fmt::Display for DomainError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | DomainError::Size(label) => write!(formatter, "{label}が空か範囲外です"),
      | DomainError::Lexical(label) => write!(formatter, "{label}に使用できない文字が含まれています"),
      | DomainError::Syntax(label) => write!(formatter, "{label}の書式が不正です"),
      | DomainError::Semantics(label) => write!(formatter, "{label}が意味的に不正です"),
    }
  }
}

impl std::error::Error for DomainError {}

/// 空でなく、空白のみでなく、ASCII 制御文字を含まないことを検証する。
fn validate_text(label: &'static str, value: &str) -> Result<(), DomainError> {
  if value.is_empty() {
    return Err(DomainError::Size(label));
  }
  if value.trim().is_empty() || value.chars().any(|character| character.is_ascii_control()) {
    return Err(DomainError::Lexical(label));
  }
  Ok(())
}

macro_rules! text_value {
  ($name:ident, $label:literal, $doc:literal) => {
    #[doc = $doc]
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct $name(String);

    impl $name {
      /// 保持している文字列を返す。
      pub fn as_str(&self) -> &str {
        &self.0
      }
    }

    impl TryFrom<&str> for $name {
      type Error = DomainError;

      fn try_from(value: &str) -> Result<Self, Self::Error> {
        validate_text($label, value)?;
        Ok($name(value.to_owned()))
      }
    }

    impl fmt::Display for $name {
      fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
      }
    }
  };
}

text_value!(ServerId, "サーバー ID", "takt-mcp のサーバー ID。");
text_value!(ProjectName, "プロジェクト名", "集約対象のプロジェクト名。");
text_value!(GoalId, "ゴール ID", "ゴールの識別子。");
text_value!(InquiryId, "inquiry ID", "問い合わせの識別子。");

/// プルリクエスト番号。1 以上です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PullRequestNumber(i64);

impl PullRequestNumber {
  /// 保持している数値を返す。
  pub fn get(self) -> i64 {
    self.0
  }
}

impl TryFrom<i64> for PullRequestNumber {
  type Error = DomainError;

  fn try_from(value: i64) -> Result<Self, Self::Error> {
    if value <= 0 {
      return Err(DomainError::Size("プルリクエスト番号"));
    }
    Ok(PullRequestNumber(value))
  }
}

impl fmt::Display for PullRequestNumber {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}", self.0)
  }
}

/// ゴール登録の承認待ちの revision。1 以上です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegistrationRevision(i32);

impl RegistrationRevision {
  /// 保持している数値を返す。
  pub fn get(self) -> i32 {
    self.0
  }
}

impl TryFrom<i32> for RegistrationRevision {
  type Error = DomainError;

  fn try_from(value: i32) -> Result<Self, Self::Error> {
    if value <= 0 {
      return Err(DomainError::Size("revision"));
    }
    Ok(RegistrationRevision(value))
  }
}

impl fmt::Display for RegistrationRevision {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}", self.0)
  }
}

/// タスクの状態ごとの件数。0 以上です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskCount(i32);

impl TryFrom<i32> for TaskCount {
  type Error = DomainError;

  fn try_from(value: i32) -> Result<Self, Self::Error> {
    if value < 0 {
      return Err(DomainError::Size("タスク件数"));
    }
    Ok(TaskCount(value))
  }
}

impl fmt::Display for TaskCount {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}", self.0)
  }
}

/// 判断待ちの滞留とみなす分数。0 以上です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StaleMinutes(i32);

impl StaleMinutes {
  /// 保持している分数を返す。
  pub fn get(self) -> i32 {
    self.0
  }
}

impl TryFrom<i32> for StaleMinutes {
  type Error = DomainError;

  fn try_from(value: i32) -> Result<Self, Self::Error> {
    if value < 0 {
      return Err(DomainError::Size("滞留の閾値"));
    }
    Ok(StaleMinutes(value))
  }
}

impl fmt::Display for StaleMinutes {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}", self.0)
  }
}

/// プルリクエストのマージを承認するときの until の指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MergeUntil {
  Prepare,
  Merge,
}

impl MergeUntil {
  /// takt へ渡す文字列を返す。
  pub fn as_str(self) -> &'static str {
    match self {
      | MergeUntil::Prepare => "prepare",
      | MergeUntil::Merge => "merge",
    }
  }
}

impl TryFrom<&str> for MergeUntil {
  type Error = DomainError;

  fn try_from(value: &str) -> Result<Self, Self::Error> {
    validate_text("until", value)?;
    if !value.chars().all(|character| character.is_ascii_lowercase()) {
      return Err(DomainError::Lexical("until"));
    }
    match value {
      | "prepare" => Ok(MergeUntil::Prepare),
      | "merge" => Ok(MergeUntil::Merge),
      | _ => Err(DomainError::Syntax("until")),
    }
  }
}

impl fmt::Display for MergeUntil {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(self.as_str())
  }
}

/// run の状態。既知の値だけを受け付けます。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunStatus {
  Running,
  Completed,
  Aborted,
  Failed,
}

impl TryFrom<i32> for RunStatus {
  type Error = DomainError;

  fn try_from(value: i32) -> Result<Self, Self::Error> {
    match crate::proto::v1::RunStatus::try_from(value) {
      | Ok(crate::proto::v1::RunStatus::Running) => Ok(RunStatus::Running),
      | Ok(crate::proto::v1::RunStatus::Completed) => Ok(RunStatus::Completed),
      | Ok(crate::proto::v1::RunStatus::Aborted) => Ok(RunStatus::Aborted),
      | Ok(crate::proto::v1::RunStatus::Failed) => Ok(RunStatus::Failed),
      | _ => Err(DomainError::Semantics("run の状態")),
    }
  }
}

/// 判断待ちの種類。既知の値だけを受け付けます。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PendingDecisionType {
  Inquiry,
  AwaitingMerge,
  GoalRegistration,
}

impl TryFrom<i32> for PendingDecisionType {
  type Error = DomainError;

  fn try_from(value: i32) -> Result<Self, Self::Error> {
    match crate::proto::v1::PendingDecisionType::try_from(value) {
      | Ok(crate::proto::v1::PendingDecisionType::Inquiry) => Ok(PendingDecisionType::Inquiry),
      | Ok(crate::proto::v1::PendingDecisionType::AwaitingMerge) => Ok(PendingDecisionType::AwaitingMerge),
      | Ok(crate::proto::v1::PendingDecisionType::GoalRegistration) => Ok(PendingDecisionType::GoalRegistration),
      | _ => Err(DomainError::Semantics("判断待ちの種類")),
    }
  }
}

/// 判断待ちの問い合わせの種類。既知の値だけを受け付けます。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InquiryKind {
  Question,
  OperationRequest,
}

impl TryFrom<i32> for InquiryKind {
  type Error = DomainError;

  fn try_from(value: i32) -> Result<Self, Self::Error> {
    match crate::proto::v1::InquiryKind::try_from(value) {
      | Ok(crate::proto::v1::InquiryKind::Question) => Ok(InquiryKind::Question),
      | Ok(crate::proto::v1::InquiryKind::OperationRequest) => Ok(InquiryKind::OperationRequest),
      | _ => Err(DomainError::Semantics("問い合わせの種類")),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{
    DomainError, GoalId, InquiryId, InquiryKind, MergeUntil, PendingDecisionType, ProjectName, PullRequestNumber,
    RegistrationRevision, RunStatus, ServerId, StaleMinutes, TaskCount,
  };

  #[test]
  fn text_values_reject_empty_blank_and_control_characters() {
    assert!(matches!(ServerId::try_from(""), Err(DomainError::Size(_))));
    assert!(matches!(ServerId::try_from("  "), Err(DomainError::Lexical(_))));
    assert!(matches!(ProjectName::try_from("a\n"), Err(DomainError::Lexical(_))));
    assert!(matches!(GoalId::try_from("g\u{7f}"), Err(DomainError::Lexical(_))));
    assert!(matches!(InquiryId::try_from("\u{0}"), Err(DomainError::Lexical(_))));
    assert_eq!(ServerId::try_from("mac-mini").expect("有効です").as_str(), "mac-mini");
    assert_eq!(ProjectName::try_from("takt-dash").expect("有効です").as_str(), "takt-dash");
  }

  #[test]
  fn numbers_reject_out_of_range_values() {
    assert!(matches!(PullRequestNumber::try_from(0), Err(DomainError::Size(_))));
    assert!(matches!(PullRequestNumber::try_from(-1), Err(DomainError::Size(_))));
    assert!(matches!(RegistrationRevision::try_from(0), Err(DomainError::Size(_))));
    assert!(matches!(RegistrationRevision::try_from(-1), Err(DomainError::Size(_))));
    assert!(matches!(TaskCount::try_from(-1), Err(DomainError::Size(_))));
    assert!(matches!(StaleMinutes::try_from(-1), Err(DomainError::Size(_))));
    assert_eq!(PullRequestNumber::try_from(1).expect("有効です").get(), 1);
    assert_eq!(PullRequestNumber::try_from(i64::MAX).expect("有効です").get(), i64::MAX);
    assert_eq!(RegistrationRevision::try_from(i32::MAX).expect("有効です").get(), i32::MAX);
    assert_eq!(TaskCount::try_from(0).expect("有効です").to_string(), "0");
    assert_eq!(TaskCount::try_from(i32::MAX).expect("有効です").to_string(), i32::MAX.to_string());
    assert_eq!(StaleMinutes::try_from(0).expect("有効です").get(), 0);
    assert_eq!(StaleMinutes::try_from(i32::MAX).expect("有効です").get(), i32::MAX);
  }

  #[test]
  fn merge_until_accepts_only_known_lowercase_values() {
    assert!(matches!(MergeUntil::try_from(""), Err(DomainError::Size(_))));
    assert!(matches!(MergeUntil::try_from("Prepare"), Err(DomainError::Lexical(_))));
    assert!(matches!(MergeUntil::try_from("unknown"), Err(DomainError::Syntax(_))));
    assert_eq!(MergeUntil::try_from("prepare").expect("有効です").as_str(), "prepare");
    assert_eq!(MergeUntil::try_from("merge").expect("有効です").as_str(), "merge");
  }

  #[test]
  fn status_kinds_accept_only_known_wire_values() {
    assert_eq!(RunStatus::try_from(crate::proto::v1::RunStatus::Running as i32).expect("有効です"), RunStatus::Running);
    assert_eq!(
      RunStatus::try_from(crate::proto::v1::RunStatus::Completed as i32).expect("有効です"),
      RunStatus::Completed
    );
    assert!(matches!(
      RunStatus::try_from(crate::proto::v1::RunStatus::Unspecified as i32),
      Err(DomainError::Semantics(_))
    ));
    assert!(matches!(RunStatus::try_from(999), Err(DomainError::Semantics(_))));
    assert_eq!(RunStatus::try_from(crate::proto::v1::RunStatus::Aborted as i32).expect("有効です"), RunStatus::Aborted);
    assert_eq!(RunStatus::try_from(crate::proto::v1::RunStatus::Failed as i32).expect("有効です"), RunStatus::Failed);

    assert_eq!(
      PendingDecisionType::try_from(crate::proto::v1::PendingDecisionType::AwaitingMerge as i32).expect("有効です"),
      PendingDecisionType::AwaitingMerge
    );
    assert!(matches!(
      PendingDecisionType::try_from(crate::proto::v1::PendingDecisionType::Unspecified as i32),
      Err(DomainError::Semantics(_))
    ));
    assert!(matches!(PendingDecisionType::try_from(999), Err(DomainError::Semantics(_))));

    assert_eq!(
      InquiryKind::try_from(crate::proto::v1::InquiryKind::OperationRequest as i32).expect("有効です"),
      InquiryKind::OperationRequest
    );
    assert!(matches!(
      InquiryKind::try_from(crate::proto::v1::InquiryKind::Unspecified as i32),
      Err(DomainError::Semantics(_))
    ));
    assert!(matches!(InquiryKind::try_from(999), Err(DomainError::Semantics(_))));
  }
}
