//! ドメイン上の値を表す型（値オブジェクト）。
//!
//! 各型は size → lexical content → syntax → semantics の順で検証し、検証を通らない値からは
//! 生成できません。生成方法は [`TryFrom`] に統一し、無検査の公開コンストラクタは置きません。
//! 参照先の存在・一意性・一致のような外部状態を要する制約は、型ではなく既存の境界で検証します。

use std::fmt;

use chrono::NaiveDate;

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

/// 書き込み要求の重複を防ぐ鍵。1 から 128 文字の ASCII 表示文字です。
///
/// 利用者の 1 回の操作を表します。同じ鍵の要求は、同じ操作の再試行として扱います。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
  /// 鍵の長さの上限。
  pub const MAX_LEN: usize = 128;

  /// 保持している文字列を返す。
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl TryFrom<&str> for IdempotencyKey {
  type Error = DomainError;

  fn try_from(value: &str) -> Result<Self, Self::Error> {
    if value.is_empty() || value.len() > Self::MAX_LEN {
      return Err(DomainError::Size("idempotency_key"));
    }
    if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
      return Err(DomainError::Lexical("idempotency_key"));
    }
    Ok(IdempotencyKey(value.to_owned()))
  }
}

text_value!(ServerId, "サーバー ID", "takt-mcp のサーバー ID。");
text_value!(ProjectName, "プロジェクト名", "集約対象のプロジェクト名。");
text_value!(GoalId, "ゴール ID", "ゴールの識別子。");
text_value!(TaskName, "タスク名", "タスクの名前。");
text_value!(InquiryId, "inquiry ID", "問い合わせの識別子。");

/// run のディレクトリ名。パスの区切りや親ディレクトリを指す名前は受け付けません。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunSlug(String);

impl RunSlug {
  /// 保持している文字列を返す。
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl TryFrom<&str> for RunSlug {
  type Error = DomainError;

  fn try_from(value: &str) -> Result<Self, Self::Error> {
    validate_text("run slug", value)?;
    if value.contains('/') || value.contains('\\') || value == "." || value == ".." {
      return Err(DomainError::Syntax("run slug"));
    }
    Ok(RunSlug(value.to_owned()))
  }
}

impl fmt::Display for RunSlug {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(&self.0)
  }
}

/// run のレポートの名前。`reports/` からの `/` 区切りの相対パスで、名前は `.md` で終わります。
///
/// 絶対パス、`\`、空の区切り、`.` と `..` の区切りは受け付けません。この型は名前の書式だけを
/// 保証し、実在と `reports/` の下に収まることは読み取り時に実パスで確かめます。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReportFilename(String);

/// レポートの名前の長さの上限（バイト）。一般的なファイルシステムのパス長の上限に合わせます。
const MAX_REPORT_FILENAME_BYTES: usize = 1024;
const REPORT_FILENAME_SUFFIX: &str = ".md";

impl ReportFilename {
  /// 保持している相対パスを返す。
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl TryFrom<&str> for ReportFilename {
  type Error = DomainError;

  fn try_from(value: &str) -> Result<Self, Self::Error> {
    const LABEL: &str = "レポートの名前";
    if value.len() > MAX_REPORT_FILENAME_BYTES {
      return Err(DomainError::Size(LABEL));
    }
    validate_text(LABEL, value)?;
    if value.contains('\\') || value.starts_with('/') {
      return Err(DomainError::Syntax(LABEL));
    }
    if value.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
      return Err(DomainError::Syntax(LABEL));
    }
    if !value.ends_with(REPORT_FILENAME_SUFFIX) {
      return Err(DomainError::Semantics(LABEL));
    }
    Ok(ReportFilename(value.to_owned()))
  }
}

impl fmt::Display for ReportFilename {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(&self.0)
  }
}

/// run への指示書の ID。takt が 1 から順に振る正の整数です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstructionId(i32);

impl InstructionId {
  /// 保持している数値を返す。
  pub fn get(self) -> i32 {
    self.0
  }
}

impl TryFrom<i32> for InstructionId {
  type Error = DomainError;

  fn try_from(value: i32) -> Result<Self, Self::Error> {
    if value <= 0 {
      return Err(DomainError::Size("指示書の ID"));
    }
    Ok(InstructionId(value))
  }
}

impl TryFrom<i64> for InstructionId {
  type Error = DomainError;

  fn try_from(value: i64) -> Result<Self, Self::Error> {
    let value = i32::try_from(value).map_err(|_| DomainError::Size("指示書の ID"))?;
    InstructionId::try_from(value)
  }
}

impl fmt::Display for InstructionId {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}", self.0)
  }
}

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

/// プルリクエストの状態。gh の `state` の字面から作る値オブジェクトです。
///
/// `OPEN`・`CLOSED`・`MERGED` の大文字だけを受け付け、未知の字面・小文字・空は表現できません。
/// 確認できなかった状態を「閉じた」として扱わないため、字面の解釈はこの型に閉じます。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PullRequestState {
  Open,
  Closed,
  Merged,
}

impl PullRequestState {
  /// この状態が、終了の対象になる閉じた PR か。`CLOSED` と `MERGED` だけが真です。
  pub fn is_closed(self) -> bool {
    matches!(self, PullRequestState::Closed | PullRequestState::Merged)
  }

  /// 復元・ログに使う字面を返す。
  pub fn as_str(self) -> &'static str {
    match self {
      | PullRequestState::Open => "OPEN",
      | PullRequestState::Closed => "CLOSED",
      | PullRequestState::Merged => "MERGED",
    }
  }
}

impl TryFrom<&str> for PullRequestState {
  type Error = DomainError;

  fn try_from(value: &str) -> Result<Self, Self::Error> {
    if value.is_empty() {
      return Err(DomainError::Size("プルリクエストの状態"));
    }
    if !value.chars().all(|character| character.is_ascii_uppercase()) {
      return Err(DomainError::Lexical("プルリクエストの状態"));
    }
    match value {
      | "OPEN" => Ok(PullRequestState::Open),
      | "CLOSED" => Ok(PullRequestState::Closed),
      | "MERGED" => Ok(PullRequestState::Merged),
      | _ => Err(DomainError::Syntax("プルリクエストの状態")),
    }
  }
}

impl fmt::Display for PullRequestState {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(self.as_str())
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

macro_rules! minutes_value {
  ($name:ident, $label:literal, $doc:literal) => {
    #[doc = $doc]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct $name(u32);

    impl $name {
      /// 保持している分数を `i32` で返す。上限は [`i32::MAX`] です。
      pub fn get(self) -> i32 {
        self.0 as i32
      }
    }

    impl TryFrom<u32> for $name {
      type Error = DomainError;

      fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value > i32::MAX as u32 {
          return Err(DomainError::Size($label));
        }
        Ok($name(value))
      }
    }

    impl fmt::Display for $name {
      fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
      }
    }
  };
}

minutes_value!(StaleMinutes, "滞留の閾値", "判断待ちの滞留とみなす分数。");
minutes_value!(SilentMinutes, "無音の閾値", "ログの無音とみなす分数。");

/// タスクの件数。0 以上 [`i32::MAX`] 以下です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskCount(i32);

impl TaskCount {
  /// 保持している件数を返す。
  pub fn get(self) -> i32 {
    self.0
  }
}

impl TryFrom<i64> for TaskCount {
  type Error = DomainError;

  fn try_from(value: i64) -> Result<Self, Self::Error> {
    if value < 0 || value > i64::from(i32::MAX) {
      return Err(DomainError::Size("タスク件数"));
    }
    Ok(TaskCount(value as i32))
  }
}

impl fmt::Display for TaskCount {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}", self.0)
  }
}

/// 画面読み取りレーンの本数。1〜3 本です。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScreenReadLanes(u32);

impl ScreenReadLanes {
  /// 本数の上限。子プロセスの増加を抑えるために設けます。
  pub const MAX: u32 = 3;
  /// 本数の下限。
  pub const MIN: u32 = 1;

  /// 保持している本数を返す。
  pub fn get(self) -> u32 {
    self.0
  }

  /// 範囲外の値を下限〜上限へ丸めて作る。設定ファイルの読み取り境界が使います。
  pub fn clamped(value: u32) -> Self {
    ScreenReadLanes(value.clamp(Self::MIN, Self::MAX))
  }
}

impl TryFrom<u32> for ScreenReadLanes {
  type Error = DomainError;

  fn try_from(value: u32) -> Result<Self, Self::Error> {
    if !(Self::MIN..=Self::MAX).contains(&value) {
      return Err(DomainError::Size("画面読み取りレーンの本数"));
    }
    Ok(ScreenReadLanes(value))
  }
}

impl fmt::Display for ScreenReadLanes {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{}", self.0)
  }
}

/// JST の日次集計の日付。`YYYY-MM-DD` の実在する日付です。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DailyDate(String);

impl DailyDate {
  /// 保持している文字列を返す。
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl TryFrom<&str> for DailyDate {
  type Error = DomainError;

  fn try_from(value: &str) -> Result<Self, Self::Error> {
    let label = "日次集計の日付";
    if value.len() != 10 {
      return Err(DomainError::Size(label));
    }
    if !value.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-') {
      return Err(DomainError::Lexical(label));
    }
    let bytes = value.as_bytes();
    let shape = bytes[4] == b'-'
      && bytes[7] == b'-'
      && bytes[..4].iter().all(u8::is_ascii_digit)
      && bytes[5..7].iter().all(u8::is_ascii_digit)
      && bytes[8..10].iter().all(u8::is_ascii_digit);
    if !shape {
      return Err(DomainError::Syntax(label));
    }
    let year = value[..4].parse::<i32>().map_err(|_| DomainError::Syntax(label))?;
    let month = value[5..7].parse::<u32>().map_err(|_| DomainError::Syntax(label))?;
    let day = value[8..10].parse::<u32>().map_err(|_| DomainError::Syntax(label))?;
    NaiveDate::from_ymd_opt(year, month, day).ok_or(DomainError::Semantics(label))?;
    Ok(DailyDate(value.to_owned()))
  }
}

impl From<NaiveDate> for DailyDate {
  fn from(value: NaiveDate) -> Self {
    DailyDate(value.format("%Y-%m-%d").to_string())
  }
}

impl fmt::Display for DailyDate {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(&self.0)
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

#[cfg(test)]
mod tests {
  use super::{
    DailyDate, DomainError, GoalId, InquiryId, MergeUntil, ProjectName, PullRequestNumber, PullRequestState,
    RegistrationRevision, RunSlug, ScreenReadLanes, ServerId, SilentMinutes, StaleMinutes, TaskCount, TaskName,
  };

  #[test]
  fn text_values_reject_empty_and_control_characters() {
    assert!(matches!(ServerId::try_from(""), Err(DomainError::Size(_))));
    assert!(matches!(ServerId::try_from("  "), Err(DomainError::Lexical(_))));
    assert!(matches!(ProjectName::try_from("a\n"), Err(DomainError::Lexical(_))));
    assert!(matches!(GoalId::try_from("g\u{7f}"), Err(DomainError::Lexical(_))));
    assert!(matches!(TaskName::try_from("\u{0}"), Err(DomainError::Lexical(_))));
    assert!(matches!(InquiryId::try_from(""), Err(DomainError::Size(_))));
    assert_eq!(ServerId::try_from("mac-mini").expect("有効です").as_str(), "mac-mini");
  }

  #[test]
  fn run_slug_rejects_path_components() {
    assert!(matches!(RunSlug::try_from("../x"), Err(DomainError::Syntax(_))));
    assert!(matches!(RunSlug::try_from("."), Err(DomainError::Syntax(_))));
    assert_eq!(RunSlug::try_from("20261116-run").expect("有効です").as_str(), "20261116-run");
  }

  #[test]
  fn numbers_reject_out_of_range_values() {
    assert!(matches!(PullRequestNumber::try_from(0), Err(DomainError::Size(_))));
    assert!(matches!(RegistrationRevision::try_from(-1), Err(DomainError::Size(_))));
    assert!(matches!(StaleMinutes::try_from(i32::MAX as u32 + 1), Err(DomainError::Size(_))));
    assert!(matches!(SilentMinutes::try_from(i32::MAX as u32 + 1), Err(DomainError::Size(_))));
    assert!(matches!(TaskCount::try_from(-1), Err(DomainError::Size(_))));
    assert_eq!(TaskCount::try_from(3).expect("有効です").get(), 3);
  }

  #[test]
  fn screen_read_lanes_guarantees_the_range() {
    assert!(matches!(ScreenReadLanes::try_from(0), Err(DomainError::Size(_))));
    assert!(matches!(ScreenReadLanes::try_from(4), Err(DomainError::Size(_))));
    assert_eq!(ScreenReadLanes::try_from(2).expect("有効です").get(), 2);
    assert_eq!(ScreenReadLanes::clamped(0).get(), 1, "下限未満は下限へ丸めます");
    assert_eq!(ScreenReadLanes::clamped(9).get(), 3, "上限超は上限へ丸めます");
  }

  #[test]
  fn daily_date_validates_each_stage() {
    assert!(matches!(DailyDate::try_from("2026-1-1"), Err(DomainError::Size(_))));
    assert!(matches!(DailyDate::try_from("2026/01/01"), Err(DomainError::Lexical(_))));
    assert!(matches!(DailyDate::try_from("20260-1-01"), Err(DomainError::Syntax(_))));
    assert!(matches!(DailyDate::try_from("2026-02-30"), Err(DomainError::Semantics(_))));
    assert_eq!(DailyDate::try_from("2024-02-29").expect("有効です").as_str(), "2024-02-29");
  }

  #[test]
  fn merge_until_accepts_only_known_lowercase_values() {
    assert!(matches!(MergeUntil::try_from(""), Err(DomainError::Size(_))));
    assert!(matches!(MergeUntil::try_from("Prepare"), Err(DomainError::Lexical(_))));
    assert!(matches!(MergeUntil::try_from("unknown"), Err(DomainError::Syntax(_))));
    assert_eq!(MergeUntil::try_from("prepare").expect("有効です").as_str(), "prepare");
  }

  #[test]
  fn pull_request_state_accepts_only_known_uppercase_values() {
    assert!(matches!(PullRequestState::try_from(""), Err(DomainError::Size(_))));
    assert!(matches!(PullRequestState::try_from("open"), Err(DomainError::Lexical(_))));
    assert!(matches!(PullRequestState::try_from("UNKNOWN"), Err(DomainError::Syntax(_))));
    assert!(matches!(PullRequestState::try_from("DRAFT"), Err(DomainError::Syntax(_))));
    assert_eq!(PullRequestState::try_from("OPEN").expect("有効です"), PullRequestState::Open);
    assert!(PullRequestState::try_from("CLOSED").expect("有効です").is_closed());
    assert!(PullRequestState::try_from("MERGED").expect("有効です").is_closed());
    assert!(!PullRequestState::try_from("OPEN").expect("有効です").is_closed());
  }
}
