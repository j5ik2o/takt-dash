#!/usr/bin/env python3
"""タスクの公開を段階（commit / push / pr / status）に分けて実行する。

使い方:
    python3 tools/project-status/publish.py commit
    python3 tools/project-status/publish.py push
    python3 tools/project-status/publish.py pr
    python3 tools/project-status/publish.py status --status <Status の選択肢名> [--require-published]

段階の契約:
  commit  git add -A と git commit を行い、成功で <marker>/commit.ok を書く。
          変更が無いときはコミットせず成功として扱う。Python の bytecode
          （__pycache__ 配下と *.pyc）はコミット対象から外す。ステージ済みの
          一覧は -z で取得し、引用・エスケープされていない実パスで判定する。
  push    commit.ok があるときだけ git push を行う。成功で push.ok を書く。
  pr      push.ok があるときだけ gh pr create を行う。base は tasks.yaml の
          該当エントリの base_branch から解決する。既に PR がある場合だけ成功と
          して扱い、pr.ok を書く。
  status  order.md の issue を対象に Status を変更する。order.md や issue の
          記述が見つからないときは「未実施」を記録して終了コード 0 にする。
          --require-published を付けると commit.ok / push.ok / pr.ok が揃って
          いるときだけ変更し、揃わなければ「未実施」を記録して終了コード 0 にする。
          Status の失敗は公開の成否を止めない（常に終了コード 0）。

段階の前提はマーカー（<context>/.publish/{commit,push,pr}.ok）で表す。TAKT は
system step の command 入力を順にすべて実行するため、前段が失敗した後に後段の
コマンドが走っても、マーカーが無ければ後段は外部コマンドを呼ばずに止まる
（commit / push / pr は非ゼロ、status は前提を要求されたときだけ「未実施」で
終了コード 0）。
各段階は開始時に自分のマーカーと下流のマーカーを削除し、再試行で古い成功を残さない。

失敗の理由はすべて標準出力へ出す（終了コードで成否を伝える）。
"""
import argparse
import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

import project_status
import task_context

COMMIT_MARKER = "commit.ok"
PUSH_MARKER = "push.ok"
PR_MARKER = "pr.ok"


class PublishError(Exception):
    """公開の段階を進められない理由を表す。"""


def _root() -> Path:
    return Path.cwd()


def _markers(environ) -> Path:
    return task_context.marker_directory(_root(), environ)


def _clear(markers: Path, names) -> None:
    for name in names:
        try:
            (markers / name).unlink()
        except FileNotFoundError:
            continue


def _write_marker(markers: Path, name: str) -> None:
    markers.mkdir(parents=True, exist_ok=True)
    (markers / name).write_text("ok\n", encoding="utf-8")


def _has_marker(markers: Path, name: str) -> bool:
    return (markers / name).is_file()


def _run(command):
    try:
        return project_status.run_command(command)
    except project_status.StatusError as error:
        raise PublishError(str(error))


def _current_branch() -> str:
    result = _run(["git", "rev-parse", "--abbrev-ref", "HEAD"])
    if result["status"] != 0:
        raise PublishError(f"現在のブランチを取得できません: {result['stderr'].strip()}")
    branch = result["stdout"].strip()
    if not branch:
        raise PublishError("現在のブランチが空です")
    return branch


def _failure_reason(result) -> str:
    return (result["stderr"].strip() or result["stdout"].strip() or "理由の出力がありません")


def _order_run_slug(environ):
    try:
        order = task_context.resolve_order(_root(), environ)
    except task_context.ContextError:
        return None
    parts = Path(order).resolve().parts
    if "runs" in parts:
        index = parts.index("runs")
        if index + 1 < len(parts):
            return parts[index + 1]
    return None


def _is_bytecode(path: str) -> bool:
    """Python の bytecode（__pycache__ 配下のファイルと *.pyc）かどうかを返す。

    project-status のスクリプトを import すると作業ツリーに bytecode が
    生成される。公開のコミットへ混入させないため、stage_commit がコミット
    対象から外す。.gitignore の指定と二重に守る。
    """
    return "__pycache__" in path.split("/") or path.endswith(".pyc")


def stage_commit(environ) -> int:
    markers = _markers(environ)
    _clear(markers, (COMMIT_MARKER, PUSH_MARKER, PR_MARKER))

    added = _run(["git", "add", "-A"])
    if added["status"] != 0:
        print(f"失敗: git add -A に失敗しました: {_failure_reason(added)}")
        return 1

    # -z を付けると、パスが引用・エスケープされず NUL 区切りで返る。日本語など
    # を含むパスでも、実パスをそのまま判定と git reset に使える（引用された
    # 文字列では拡張子の判定が外れ、reset も実ファイルに届かない）。
    staged = _run(["git", "diff", "--cached", "--name-only", "-z"])
    if staged["status"] != 0:
        print(f"失敗: コミット対象の確認に失敗しました: {_failure_reason(staged)}")
        return 1

    names = [name for name in staged["stdout"].split("\x00") if name]
    bytecode = [name for name in names if _is_bytecode(name)]
    if bytecode:
        reset = _run(["git", "reset", "--", *bytecode])
        if reset["status"] != 0:
            print(f"失敗: bytecode をコミット対象から外せませんでした: {_failure_reason(reset)}")
            return 1
        excluded = set(bytecode)
        names = [name for name in names if name not in excluded]

    if not names:
        print("コミットする変更はありません")
        _write_marker(markers, COMMIT_MARKER)
        return 0

    slug = _order_run_slug(environ)
    message = f"takt: {slug}" if slug else "takt: workflow-result"
    committed = _run(["git", "commit", "--no-verify", "-m", message])
    if committed["status"] != 0:
        print(f"失敗: git commit に失敗しました: {_failure_reason(committed)}")
        return committed["status"] or 1

    _write_marker(markers, COMMIT_MARKER)
    print(f"コミットしました: {message}")
    return 0


def stage_push(environ) -> int:
    markers = _markers(environ)
    _clear(markers, (PUSH_MARKER, PR_MARKER))

    if not _has_marker(markers, COMMIT_MARKER):
        print("失敗: コミットが成功していないため push しません")
        return 1

    branch = _current_branch()
    pushed = _run(["git", "push", "-u", "origin", branch])
    if pushed["status"] != 0:
        print(f"失敗: git push に失敗しました: {_failure_reason(pushed)}")
        return pushed["status"] or 1

    _write_marker(markers, PUSH_MARKER)
    print(f"push しました: origin {branch}")
    return 0


def _existing_pull_request(branch: str):
    result = _run(
        ["gh", "pr", "list", "--head", branch, "--state", "open", "--json", "url", "--limit", "1"]
    )
    if result["status"] != 0:
        return None
    try:
        payload = json.loads(result["stdout"] or "[]")
    except ValueError:
        return None
    if not isinstance(payload, list) or not payload:
        return None
    item = payload[0]
    if not isinstance(item, dict):
        return None
    url = item.get("url")
    return url if isinstance(url, str) and url else None


def stage_pr(environ) -> int:
    markers = _markers(environ)
    _clear(markers, (PR_MARKER,))

    if not _has_marker(markers, PUSH_MARKER):
        print("失敗: push が成功していないため PR を作成しません")
        return 1

    branch = _current_branch()
    try:
        base_branch = task_context.resolve_base_branch(_root(), _order_run_slug(environ))
    except task_context.ContextError as error:
        print(f"失敗: PR の base ブランチを解決できません: {error}")
        return 1

    created = _run(
        ["gh", "pr", "create", "--base", base_branch, "--head", branch, "--fill"]
    )
    if created["status"] == 0:
        _write_marker(markers, PR_MARKER)
        url = created["stdout"].strip()
        print(f"PR を作成しました: {url}" if url else "PR を作成しました")
        return 0

    existing = _existing_pull_request(branch)
    if existing is not None:
        _write_marker(markers, PR_MARKER)
        print(f"既存の PR を確認しました: {existing}")
        return 0

    print(f"失敗: gh pr create に失敗しました: {_failure_reason(created)}")
    return created["status"] or 1


def stage_status(environ, status_name: str, require_published: bool = False) -> int:
    if require_published:
        markers = _markers(environ)
        missing = [
            name
            for name in (COMMIT_MARKER, PUSH_MARKER, PR_MARKER)
            if not _has_marker(markers, name)
        ]
        if missing:
            print(
                "未実施: 公開が成功していないため Status を変更しません"
                f"（不足: {' / '.join(missing)}）"
            )
            return 0

    try:
        order = task_context.resolve_order(_root(), environ)
    except task_context.ContextError as error:
        print(f"未実施: task/order.md を特定できないため Status を変更しません（{error}）")
        return 0

    try:
        text = Path(order).read_text(encoding="utf-8")
    except OSError as error:
        print(f"未実施: task/order.md を読めないため Status を変更しません（{error}）")
        return 0

    reference = task_context.extract_issue_reference(text)
    if reference is None:
        print("未実施: issue の記述が見つからない")
        return 0

    return project_status.main(["--issue", reference, "--status", status_name])


def main(argv) -> int:
    parser = argparse.ArgumentParser(
        description="タスクの公開を段階（commit / push / pr / status）に分けて実行します。"
    )
    parser.add_argument("stage", choices=("commit", "push", "pr", "status"))
    parser.add_argument("--status", help="status 段階で設定する Status の選択肢名")
    parser.add_argument(
        "--require-published",
        action="store_true",
        help="status 段階で commit / push / pr の成功マーカーが揃っていることを要求する",
    )
    try:
        args = parser.parse_args(argv)
    except SystemExit:
        print("失敗: 引数を解釈できません。stage と --status を確認してください。")
        return 1

    environ = os.environ
    try:
        if args.stage == "commit":
            return stage_commit(environ)
        if args.stage == "push":
            return stage_push(environ)
        if args.stage == "pr":
            return stage_pr(environ)
    except PublishError as error:
        print(f"失敗: {error}")
        return 1

    if not args.status:
        print("未実施: status には --status が必要です")
        return 0
    return stage_status(environ, args.status, require_published=args.require_published)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
