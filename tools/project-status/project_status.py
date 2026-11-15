#!/usr/bin/env python3
"""GitHub Projects のボード項目の Status を、takt の system step から変更する。

使い方:
    python3 tools/project-status/project_status.py --issue <番号 または issue の URL> \
        --status <Status の選択肢名> [--config <設定ファイル>]

設定ファイル（既定は本スクリプトと同じディレクトリの project-status.json）:
    owner            ボードの所有者（必須・非空文字列）
    project          ボード番号。数値（単一）または repository をキーとするオブジェクト
                     （各値は数値。リポジトリごとに切り替える）
    statusFieldName  Status フィールド名（必須・非空文字列）
    statusNames      状態名の対応（キー → 選択肢名。--status の値はキーとして厳密に引き、
                     未登録のキーは設定誤りとして扱う）
    repository       owner/name（必須）。issue 番号指定の解決に使う

方針:
  項目の有無は `gh project item-list` の内容で判定する。`item-edit` は項目が無い場合に
  失敗するだけで追加の要否を判定できないため、先に一覧を取り、対象 issue の項目が無ければ
  `gh project item-add` で追加してから `gh project item-edit` で Status を変更する。
  一覧は `--limit` で打ち切られることがある。`totalCount` と取得件数が一致しないときは
  「項目が無い」ことを確定できないため、追加も変更もせず未実施として記録する。

失敗時は異常終了せず、終了コード 0 を返す。失敗内容は必ず標準出力へ出す
（TAKT の system step は成功時の標準エラー出力を応答に含めないため）。gh が応答しない
場合も run を止めないよう、呼び出しにはタイムアウトを設ける。
"""
import argparse
import json
import os
import pwd
import re
import subprocess
import sys
from pathlib import Path

DEFAULT_CONFIG_NAME = "project-status.json"
COMMAND_TIMEOUT_SECONDS = 60
ITEM_LIST_LIMIT = 1000

REQUIRED_STRING_KEYS = ("owner", "statusFieldName", "repository")
ISSUE_PATH = "issues"


class StatusError(Exception):
    """Status を変更できない理由を表す。"""


def _run(command: list[str], cwd: str | None = None, timeout: float | None = None) -> dict:
    env = dict(os.environ)
    if "HOME" not in env:
        # TAKT の command 環境は PATH/SHELL/TMPDIR などしか渡さない。HOME が無いと gh が
        # keyring の認証情報を読めないため、ログインユーザーのホームを補う。
        env["HOME"] = pwd.getpwuid(os.getuid()).pw_dir
    try:
        proc = subprocess.run(
            command,
            cwd=cwd,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
        )
    except subprocess.TimeoutExpired:
        raise StatusError(
            f"{' '.join(command)} が {timeout} 秒以内に終わりませんでした"
        )
    return {"status": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}


run_command = _run


def _validate_config(raw: object, path: Path) -> dict:
    if not isinstance(raw, dict):
        raise StatusError(f"設定 {path} が JSON オブジェクトではありません")
    for key in REQUIRED_STRING_KEYS:
        value = raw.get(key)
        if not isinstance(value, str) or not value.strip():
            raise StatusError(f"設定 {path} の {key} が非空文字列ではありません")
    repository = raw["repository"].strip()
    owner_name = repository.split("/")
    if len(owner_name) != 2 or not owner_name[0] or not owner_name[1]:
        raise StatusError(f"設定 {path} の repository が owner/name の形式ではありません: {repository!r}")
    project = raw.get("project")
    if isinstance(project, int) and not isinstance(project, bool) and project >= 0:
        pass
    elif isinstance(project, dict):
        if not project or not all(
            isinstance(key, str) and isinstance(value, int) and not isinstance(value, bool) and value >= 0
            for key, value in project.items()
        ):
            raise StatusError(f"設定 {path} の project が repository をキーとする番号のオブジェクトではありません")
    else:
        raise StatusError(f"設定 {path} の project がボード番号（非負整数）でも repository をキーとするオブジェクトでもありません")
    status_names = raw.get("statusNames")
    if not isinstance(status_names, dict) or not all(
        isinstance(key, str) and isinstance(value, str) and value.strip()
        for key, value in status_names.items()
    ):
        raise StatusError(f"設定 {path} の statusNames が状態名の対応（文字列から文字列）ではありません")
    return raw


def load_config(path: Path) -> dict:
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        raise StatusError(f"設定ファイル {path} がありません")
    except OSError as error:
        raise StatusError(f"設定ファイル {path} を読めません: {error}")
    try:
        raw = json.loads(text)
    except ValueError as error:
        raise StatusError(f"設定ファイル {path} が JSON ではありません: {error}")
    return _validate_config(raw, path)


def resolve_project_number(config: dict, repository: str) -> int:
    project = config["project"]
    if isinstance(project, int):
        return project
    if repository not in project:
        raise StatusError(f"設定の project に repository {repository!r} のボード番号がありません")
    return project[repository]


def resolve_issue(reference: str, repository: str) -> str:
    """--issue の値を issue の URL へ解決する。番号は repository で解決する。"""
    value = reference.strip()
    if re.fullmatch(r"\d+", value):
        return f"https://github.com/{repository}/{ISSUE_PATH}/{value}"
    match = re.fullmatch(
        r"https://github\.com/(?P<owner>[^/]+)/(?P<name>[^/]+)/(?P<kind>[^/]+)/(?P<number>\d+)",
        value,
    )
    if match is None:
        raise StatusError(f"issue の URL でない値は受け付けません: {reference!r}")
    kind = match.group("kind")
    if kind != ISSUE_PATH:
        raise StatusError(f"issue 以外（{kind}）の URL は対象外です: {reference!r}")
    found = f"{match.group('owner')}/{match.group('name')}"
    if found != repository:
        raise StatusError(f"設定の repository {repository!r} と異なるリポジトリの issue は対象外です: {reference!r}")
    return value


def resolve_status_name(config: dict, status: str) -> str:
    status_names = config["statusNames"]
    if status not in status_names:
        raise StatusError(
            f"設定の statusNames に {status!r} のキーがありません（定義済み: {sorted(status_names)}）"
        )
    return status_names[status]


def _json_payload(result: dict, label: str) -> object:
    if result["status"] != 0:
        raise StatusError(f"{label} が失敗しました: {result['stderr'].strip()}")
    try:
        return json.loads(result["stdout"])
    except ValueError as error:
        raise StatusError(f"{label} の出力が JSON ではありません: {error}")


def _find_item_id(payload: object, issue_url: str) -> str | None:
    if not isinstance(payload, dict):
        raise StatusError("item-list の出力が JSON オブジェクトではありません")
    items = payload.get("items")
    if not isinstance(items, list):
        raise StatusError("item-list の出力に items がありません")
    for item in items:
        content = (item or {}).get("content") or {}
        if content.get("url") == issue_url:
            item_id = item.get("id")
            if not isinstance(item_id, str) or not item_id:
                raise StatusError(f"item-list の項目に id がありません: {item}")
            return item_id
    return None


def _is_truncated(payload: dict, items: list) -> bool:
    total = payload.get("totalCount")
    return isinstance(total, int) and total > len(items)


def ensure_item(number: int, owner: str, issue_url: str) -> str | None:
    """対象 issue の項目 ID を返す。無ければ追加する。確定できなければ None を返す。"""
    listing = run_command(
        ["gh", "project", "item-list", str(number), "--owner", owner, "--format", "json",
         "--limit", str(ITEM_LIST_LIMIT)],
        timeout=COMMAND_TIMEOUT_SECONDS,
    )
    payload = _json_payload(listing, "gh project item-list")
    items = payload.get("items") if isinstance(payload, dict) else None
    if not isinstance(items, list):
        raise StatusError("item-list の出力に items がありません")
    item_id = _find_item_id(payload, issue_url)
    if item_id is not None:
        return item_id
    if _is_truncated(payload, items):
        print(f"未実施: item-list が打ち切られており（totalCount={payload.get('totalCount')}）、項目の有無を確定できません")
        return None
    added = run_command(
        ["gh", "project", "item-add", str(number), "--owner", owner, "--url", issue_url, "--format", "json"],
        timeout=COMMAND_TIMEOUT_SECONDS,
    )
    if added["status"] != 0:
        print(f"未実施: gh project item-add に失敗しました: {added['stderr'].strip()}")
        return None
    payload = _json_payload(added, "gh project item-add")
    item_id = payload.get("id") if isinstance(payload, dict) else None
    if not isinstance(item_id, str) or not item_id:
        print("未実施: gh project item-add の出力に id がありません")
        return None
    return item_id


def change_status(config_path: Path, issue_reference: str, status: str) -> None:
    config = load_config(config_path)
    repository = config["repository"].strip()
    owner = config["owner"].strip()
    number = resolve_project_number(config, repository)
    issue_url = resolve_issue(issue_reference, repository)
    status_name = resolve_status_name(config, status)
    item_id = ensure_item(number, owner, issue_url)
    if item_id is None:
        return
    field_name = config["statusFieldName"]
    edit = run_command(
        ["gh", "project", "item-edit", str(number), "--owner", owner, "--url", issue_url,
         "--field", field_name, "--value", status_name],
        timeout=COMMAND_TIMEOUT_SECONDS,
    )
    if edit["status"] != 0:
        raise StatusError(f"gh project item-edit が失敗しました: {edit['stderr'].strip()}")
    print(f"対象: {issue_url}")
    print(f"変更: {field_name}={status_name}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="GitHub Projects のボード項目の Status を変更します。")
    parser.add_argument("--issue", required=True, help="対象 issue の番号または URL")
    parser.add_argument("--status", required=True, help="Status の選択肢名")
    parser.add_argument(
        "--config",
        default=str(Path(__file__).resolve().parent / DEFAULT_CONFIG_NAME),
        help="設定ファイルのパス（既定: 本スクリプトと同じディレクトリの project-status.json）",
    )
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_error:
        # 引数の不足などは run を止めない。理由を標準出力へ出して終了コード 0 で抜ける。
        print(f"失敗: 引数を解釈できません（{exit_error.code}）。--issue と --status を指定してください。")
        return 0
    try:
        change_status(Path(args.config), args.issue, args.status)
    except StatusError as error:
        print(f"失敗: {error}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
