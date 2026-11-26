#!/usr/bin/env python3
"""takt の実行コンテキストから、タスクの指示文（order.md）と base ブランチを解決する。

使い方（他モジュールから使う）:
    import task_context
    order = task_context.resolve_order(cwd, environ)
    base = task_context.resolve_base_branch(cwd, run_slug)

解決の根拠:
  order.md は次の順で探し、最初に 1 つへ絞れた候補を返す。
    1. TAKT_PREVIOUS_RESPONSE_FILE の祖先（最大 5 階層）にある context/task/order.md
       または task/order.md。応答ファイルは
       <run>/context/system-commands/<uuid>/response.txt に置かれる。
    2. cwd/.takt/runs/<slug>/meta.json が status == "running" の run の
       context/task/order.md。候補が複数なら、推測で選ばず ContextError にする。
    3. .takt/tasks.yaml の該当エントリ（worktree_path が cwd）の task_dir の order.md。
  どの候補も無ければ ContextError にする。呼び出し側はそれを「未実施」として扱う。

  base ブランチは .takt/tasks.yaml の該当エントリの base_branch から解決する。
  空のときだけ同じプロジェクトの .takt/config.yaml の base_branch を使う。どちらも
  無ければ ContextError にする（推測で main へ倒さない）。

  .takt/tasks.yaml は worktree のクローンに無いことがある。その場合は cwd の祖先と、
  祖先が takt-worktrees のときの兄弟プロジェクトを探し、見つかったすべての
  tasks.yaml から worktree_path または run_slug が一致するエントリを 1 件だけ採用する。

このモジュールは外部コマンドを呼ばない。すべてファイルの読み取りだけで完結する。
"""
import json
import os
import re
from pathlib import Path

RESPONSE_ANCESTOR_DEPTH = 5
DIRECTORY_ANCESTOR_DEPTH = 10
RUNS_DIRECTORY = Path(".takt") / "runs"
TASKS_YAML = Path(".takt") / "tasks.yaml"
PROJECT_CONFIG = Path(".takt") / "config.yaml"
WORKTREE_CONTAINER = "takt-worktrees"

ISSUE_URL = re.compile(
    r"https://github\.com/[^/\s]+/[^/\s]+/issues/\d+",
    re.IGNORECASE,
)
# 番号の直後が数字、または数字が続くピリオド（小数・バージョン表記の続き）のときは
# issue 番号として扱わない。文末・文中の句読点（ピリオド・読点・閉じ括弧）は許す。
ISSUE_NUMBER = re.compile(
    r"issue\s*#?\s*(\d+)(?!\d|\.\d)",
    re.IGNORECASE,
)


class ContextError(Exception):
    """実行コンテキストやタスクのメタデータを解決できないことを表す。"""


def _clean_value(raw):
    """tasks.yaml / config.yaml の 1 行の値を、引用符を外して返す。"""
    text = raw.strip()
    if text in ("", "~", "null", "Null", "NULL"):
        return ""
    if len(text) >= 2 and text[0] == text[-1] and text[0] in ("'", '"'):
        return text[1:-1]
    return text


def _clean_key(raw):
    return raw.strip().strip("'\"")


BLOCK_SCALAR_INDICATORS = (">", ">-", ">+", "|", "|-", "|+")


def _indent_of(line):
    return len(line) - len(line.lstrip(" "))


def parse_tasks_yaml(path) -> list:
    """tasks.yaml の tasks 配列を、キーと値の辞書のリストとして読む。

    PyYAML に依存しない最小の読み取り。タスクの各エントリは `- key: value` で始まり、
    続く行は同じエントリの別キーとして読む。`retry_note: >-` などのブロックスカラーの
    本文は、キーの桁より深いインデントの行として読み飛ばし、エントリの境界にもキーにも
    しない。ブロックスカラーの値は読まずに空にする。エントリの境界は、先頭のエントリと
    同じインデントの `- ` 行だけに限る。読み取りに失敗したときは ContextError にする。

    backend/src/run_log.rs の parse_task_entries は、この手順を移したもの。手順を
    変えるときは両方を直す（run_log_tasks_yaml_tests.rs が、同じ入力で結果を比べる）。
    """
    try:
        text = Path(path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise ContextError(f"tasks.yaml を読めません: {path}: {error}") from error

    entries = []
    current = None
    entry_indent = None
    block_indent = None
    for line in text.splitlines():
        if block_indent is not None:
            if not line.strip() or _indent_of(line) > block_indent:
                continue
            block_indent = None
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or stripped == "tasks:":
            continue
        indent = _indent_of(line)
        if stripped.startswith("- ") and (entry_indent is None or indent == entry_indent):
            if current is not None:
                entries.append(current)
            if entry_indent is None:
                entry_indent = indent
            current = {}
            body = stripped[2:]
            key_column = indent + 2 + _indent_of(body)
        elif current is not None and entry_indent is not None and indent > entry_indent:
            body = stripped
            key_column = indent
        else:
            continue
        if ":" not in body:
            continue
        key, _, value = body.partition(":")
        if value.strip() in BLOCK_SCALAR_INDICATORS:
            current[_clean_key(key)] = ""
            block_indent = key_column
        else:
            current[_clean_key(key)] = _clean_value(value)
    if current is not None:
        entries.append(current)
    return entries


def _read_config_value(path, key):
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or ":" not in stripped:
            continue
        found, _, value = stripped.partition(":")
        if _clean_key(found) == key:
            return _clean_value(value)
    return ""


def _tasks_yaml_candidates(cwd):
    directory = Path(cwd).resolve()
    candidates = []

    def add(path):
        if path not in candidates:
            candidates.append(path)

    for _ in range(DIRECTORY_ANCESTOR_DEPTH):
        add(directory / TASKS_YAML)
        parent = directory.parent
        if parent == directory:
            break
        directory = parent

    for ancestor in Path(cwd).resolve().parents:
        if ancestor.name != WORKTREE_CONTAINER:
            continue
        container = ancestor.parent
        if not container.is_dir():
            break
        for sibling in sorted(container.iterdir()):
            if sibling.is_dir():
                add(sibling / TASKS_YAML)
        break

    return candidates


def find_task_entry(cwd, run_slug):
    """cwd と run slug に一致する tasks.yaml のエントリと、その tasks.yaml を返す。

    候補の tasks.yaml をすべて走査し、worktree_path が cwd のエントリと run_slug が
    一致するエントリのうち、ちょうど 1 件に絞れたものを採用する。0 件のときは探した
    候補のパスを、複数件のときは一致したエントリの場所を ContextError に含める。
    """
    checked = _tasks_yaml_candidates(cwd)
    root = Path(cwd).resolve()
    matched = []
    for tasks_path in checked:
        if not tasks_path.is_file():
            continue
        for index, entry in enumerate(parse_tasks_yaml(tasks_path)):
            raw = entry.get("worktree_path", "")
            by_worktree = False
            if raw:
                try:
                    by_worktree = Path(raw).resolve() == root
                except OSError:
                    by_worktree = False
            by_slug = bool(run_slug) and entry.get("run_slug") == run_slug
            if by_worktree or by_slug:
                matched.append((entry, tasks_path, index))

    if len(matched) == 1:
        entry, tasks_path, _ = matched[0]
        return entry, tasks_path
    if len(matched) > 1:
        locations = ", ".join(
            f"{tasks_path} の {index + 1} 件目" for _, tasks_path, index in matched
        )
        raise ContextError(
            f"tasks.yaml に worktree_path が {root} または run_slug が {run_slug!r} の"
            f"エントリが複数あります: {locations}"
        )
    raise ContextError(
        f"tasks.yaml に worktree_path が {root} または run_slug が {run_slug!r} の"
        "エントリがありません。探した場所: "
        + ", ".join(str(path) for path in checked)
    )


def resolve_base_branch(cwd, run_slug=None) -> str:
    entry, tasks_path = find_task_entry(cwd, run_slug)
    base = _clean_value(entry.get("base_branch", ""))
    if base:
        return base

    project = tasks_path.parent.parent
    config = project / PROJECT_CONFIG
    if config.is_file():
        configured = _read_config_value(config, "base_branch")
        if configured:
            return configured
    raise ContextError(
        f"base ブランチを解決できません: tasks.yaml {tasks_path} の base_branch が空で、"
        f"{config} にも base_branch がありません"
    )


def _response_order(response_file):
    path = Path(response_file)
    directories = [path.parent, *path.parent.parents][:RESPONSE_ANCESTOR_DEPTH]
    for directory in directories:
        for relative in (Path("context") / "task" / "order.md", Path("task") / "order.md"):
            candidate = directory / relative
            if candidate.is_file():
                return candidate
    return None


def _running_run_orders(cwd):
    runs = Path(cwd) / RUNS_DIRECTORY
    if not runs.is_dir():
        return []
    found = []
    for meta in sorted(runs.glob("*/meta.json")):
        try:
            data = json.loads(meta.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict) or data.get("status") != "running":
            continue
        slug = data.get("runSlug")
        if not isinstance(slug, str) or not slug:
            continue
        order = runs / slug / "context" / "task" / "order.md"
        if order.is_file():
            found.append(order)
    return found


def _tasks_yaml_order(cwd):
    try:
        entry, tasks_path = find_task_entry(cwd, None)
    except ContextError:
        return None
    task_dir = _clean_value(entry.get("task_dir", ""))
    if not task_dir:
        return None
    project = tasks_path.parent.parent
    order = project / task_dir / "order.md"
    return order if order.is_file() else None


def resolve_order(cwd, environ=None) -> str:
    """タスクの指示文（order.md）のパスを返す。特定できなければ ContextError。"""
    environ = os.environ if environ is None else environ
    response_file = (environ.get("TAKT_PREVIOUS_RESPONSE_FILE") or "").strip()
    if response_file:
        found = _response_order(response_file)
        if found is not None:
            return str(found)

    candidates = _running_run_orders(cwd)
    if len(candidates) > 1:
        raise ContextError(
            "status が running の run が複数あり order.md を一つに絞れません: "
            + ", ".join(str(path) for path in candidates)
        )
    if len(candidates) == 1:
        return str(candidates[0])

    found = _tasks_yaml_order(cwd)
    if found is not None:
        return str(found)

    raise ContextError(
        f"task/order.md を特定できません（応答ファイル={response_file or 'なし'}、cwd={cwd}）"
    )


def extract_issue_reference(text):
    """order.md の本文から issue の参照を取り出す。

    issue の URL を優先し、無ければ `issue #N` / `issue N` の番号を返す。
    pull request の URL は issue ではないため扱わない。
    """
    match = ISSUE_URL.search(text)
    if match is not None:
        return match.group(0)
    match = ISSUE_NUMBER.search(text)
    if match is not None:
        return match.group(1)
    return None


def marker_directory(cwd, environ=None) -> Path:
    """公開段階のマーカーを置くディレクトリを返す。

    order.md が解決できるときは run の context 配下（<context>/.publish）、
    できないときは cwd/.takt/.publish にする。
    """
    try:
        order = resolve_order(cwd, environ)
    except ContextError:
        return Path(cwd) / ".takt" / ".publish"
    return Path(order).parent.parent / ".publish"
