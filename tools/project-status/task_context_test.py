#!/usr/bin/env python3
"""task_context.py の order と base ブランチの解決を、偽の実行ディレクトリで確かめる。

使い方:
    python3 tools/project-status/task_context_test.py

確かめる観点:
  1. TAKT が渡す応答ファイルのパス（<context>/system-commands/<uuid>/response.txt）から、
     run コンテキストの task/order.md を解決する。
  2. 応答ファイルが別の場所を指す場合でも、cwd/.takt/runs/<slug>/meta.json の
     runSlug と status == "running" から候補を絞って task/order.md を解決する。
  3. running の run が複数あり候補を 1 つに絞れないときは、推測で選ばず ContextError にする。
  4. .takt/tasks.yaml の該当エントリ（worktree_path が cwd）の task_dir から
     order.md を解決する。
  5. どの候補も無いときは ContextError にする。
  6. base ブランチは tasks.yaml の該当エントリの base_branch から解決する。main の
     エントリは main のまま返す。エントリが無い・複数一致する・tasks.yaml が無いときは
     main に倒さず ContextError にする。
  7. base_branch が空のエントリだけ、同じプロジェクトの .takt/config.yaml の
     base_branch を使う。
  8. worktree の直下・祖先に tasks.yaml が無くても、辞書順で先の兄弟プロジェクトが
     非該当エントリを持つ場合は、後続の兄弟プロジェクトの該当エントリを採用する。
     0 件のエラーは探した候補を、複数件のエラーは一致した場所を含む。
  9. ブロックスカラー（`>-` / `|-` など）の本文にある `- TR-00x:` 行ではエントリを
     分割せず、エントリの先頭と同じインデントの `- ` 行だけを境界にする。
     先頭のキーがブロックスカラーでも、本文の後に続く同じエントリのキーは読む。
     ブロックスカラーの印（`>-` など）は値にせず、空にする。`-` だけの行も項目の先頭にする。
  10. tasks.yaml を読めないとき（OSError・不正な UTF-8）は ContextError にする。
  11. issue 参照の抽出は、番号の直後の句読点（ピリオド・読点・閉じ括弧）を許し、
      数字が続くピリオド（小数・バージョン表記）は issue 番号として扱わない。
      URL があるときは番号より URL を優先し、pull request の URL は扱わない。

実 gh・実 Git は呼ばない。すべて一時ディレクトリで完結する。
"""
import importlib.util
import json
import shutil
import signal
import tempfile
import traceback
from pathlib import Path

MODULE = Path(__file__).resolve().parent / "task_context.py"

SLUG = "20261115-000000-sample-run-abcdef"
OTHER_SLUG = "20261115-000100-other-run-ghijkl"

TEST_TIMEOUT_SECONDS = 10


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


def load_module():
    spec = importlib.util.spec_from_file_location("task_context", MODULE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Workspace:
    """偽のプロジェクトと run ディレクトリを持つ一時ディレクトリ。"""

    def __init__(self) -> None:
        self.base = Path(tempfile.mkdtemp(prefix="task-context-test-"))
        self.root = self.base / "project"
        self.root.mkdir()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        shutil.rmtree(self.base, ignore_errors=True)
        return False

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def make_run(self, slug, order_text, status="running"):
        context = self.root / ".takt" / "runs" / slug / "context"
        (context / "task").mkdir(parents=True, exist_ok=True)
        (context / "task" / "order.md").write_text(order_text, encoding="utf-8")
        commands = context / "system-commands" / "uuid"
        commands.mkdir(parents=True, exist_ok=True)
        (commands / "response.txt").write_text("", encoding="utf-8")
        (self.root / ".takt" / "runs" / slug / "meta.json").write_text(
            json.dumps({"runSlug": slug, "status": status}),
            encoding="utf-8",
        )
        return context

    def tasks_yaml(self, entries):
        lines = ["tasks:"]
        for entry in entries:
            first = True
            for key, value in entry.items():
                prefix = "  - " if first else "    "
                lines.append(f"{prefix}{key}: {value}")
                first = False
        return self.write(".takt/tasks.yaml", "\n".join(lines) + "\n")


def expect_context_error(module, function, *args):
    try:
        function(*args)
    except module.ContextError:
        return
    raise AssertionError(f"{function.__name__} が ContextError を送出しませんでした")


def test_response_file_resolves_context_order() -> None:
    with Workspace() as workspace:
        context = workspace.make_run(SLUG, "対象は issue #12 です。\n")
        module = load_module()
        environ = {
            "TAKT_PREVIOUS_RESPONSE_FILE": str(context / "system-commands" / "uuid" / "response.txt"),
        }
        found = Path(module.resolve_order(workspace.root, environ)).resolve()
    expected = (workspace.root / ".takt" / "runs" / SLUG / "context" / "task" / "order.md").resolve()
    assert found == expected, f"order の解決先が違います: {found} != {expected}"


def test_unrelated_response_file_falls_back_to_run_directory() -> None:
    with Workspace() as workspace:
        context = workspace.make_run(SLUG, "対象は issue #12 です。\n")
        unrelated = workspace.base / "other" / "response.txt"
        unrelated.parent.mkdir(parents=True, exist_ok=True)
        unrelated.write_text("", encoding="utf-8")
        module = load_module()
        found = Path(module.resolve_order(workspace.root, {"TAKT_PREVIOUS_RESPONSE_FILE": str(unrelated)})).resolve()
    expected = (context / "task" / "order.md").resolve()
    assert found == expected, f"応答ファイルの外側から run ディレクトリへ戻れません: {found} != {expected}"


def test_run_directory_resolves_order_without_environment() -> None:
    with Workspace() as workspace:
        context = workspace.make_run(SLUG, "対象は issue #12 です。\n")
        module = load_module()
        found = Path(module.resolve_order(workspace.root, {})).resolve()
    expected = (context / "task" / "order.md").resolve()
    assert found == expected, f"環境変数が無いときの order の解決先が違います: {found} != {expected}"


def test_multiple_running_runs_are_ambiguous() -> None:
    with Workspace() as workspace:
        workspace.make_run(SLUG, "一つ目\n")
        workspace.make_run(OTHER_SLUG, "二つ目\n")
        module = load_module()
        expect_context_error(module, module.resolve_order, workspace.root, {})


def test_tasks_yaml_task_dir_resolves_order_without_environment() -> None:
    with Workspace() as workspace:
        order = workspace.write(".takt/tasks/sample-task/order.md", "対象は issue #12 です。\n")
        workspace.tasks_yaml(
            [
                {
                    "run_slug": "unknown-run-slug",
                    "base_branch": "main",
                    "worktree_path": workspace.root.resolve(),
                    "task_dir": ".takt/tasks/sample-task",
                }
            ]
        )
        module = load_module()
        found = Path(module.resolve_order(workspace.root, {})).resolve()
    assert found == order.resolve(), f"tasks.yaml の task_dir から order を解決できません: {found}"


def test_missing_every_candidate_is_context_error() -> None:
    with Workspace() as workspace:
        module = load_module()
        expect_context_error(module, module.resolve_order, workspace.root, {})


def test_base_branch_comes_from_matching_run_slug() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml(
            [
                {"run_slug": OTHER_SLUG, "base_branch": "main"},
                {"run_slug": SLUG, "base_branch": "takt/20261115T0000-goal-abc"},
            ]
        )
        module = load_module()
        found = module.resolve_base_branch(workspace.root, SLUG)
    assert found == "takt/20261115T0000-goal-abc", f"base_branch が違います: {found!r}"


def test_main_base_branch_is_preserved() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml([{"run_slug": SLUG, "base_branch": "main"}])
        module = load_module()
        found = module.resolve_base_branch(workspace.root, SLUG)
    assert found == "main", f"main ベースのタスクは main のまま返す必要があります: {found!r}"


def test_missing_entry_is_context_error() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml([{"run_slug": OTHER_SLUG, "base_branch": "main"}])
        module = load_module()
        expect_context_error(module, module.resolve_base_branch, workspace.root, SLUG)


def test_duplicate_entries_are_context_error() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml(
            [
                {"run_slug": SLUG, "base_branch": "takt/a"},
                {"run_slug": SLUG, "base_branch": "takt/b"},
            ]
        )
        module = load_module()
        expect_context_error(module, module.resolve_base_branch, workspace.root, SLUG)


def test_missing_tasks_yaml_is_context_error() -> None:
    with Workspace() as workspace:
        module = load_module()
        expect_context_error(module, module.resolve_base_branch, workspace.root, SLUG)


def test_empty_base_branch_uses_project_config() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml([{"run_slug": SLUG, "base_branch": '""'}])
        workspace.write(".takt/config.yaml", "base_branch: main\n")
        module = load_module()
        found = module.resolve_base_branch(workspace.root, SLUG)
    assert found == "main", f"空の base_branch はプロジェクト設定へ委ねる必要があります: {found!r}"


def test_empty_base_branch_without_config_is_error() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml([{"run_slug": SLUG, "base_branch": '""'}])
        module = load_module()
        expect_context_error(module, module.resolve_base_branch, workspace.root, SLUG)


def test_sibling_project_tasks_yaml_is_found_from_worktree() -> None:
    base = Path(tempfile.mkdtemp(prefix="task-context-test-"))
    project = base / "project"
    worktree = base / "takt-worktrees" / "worktree"
    (project / ".takt").mkdir(parents=True)
    (project / ".takt" / "tasks.yaml").write_text(
        "tasks:\n  - run_slug: " + SLUG + "\n    base_branch: takt/20261115T0000-goal-abc\n",
        encoding="utf-8",
    )
    worktree.mkdir(parents=True)
    try:
        module = load_module()
        found = module.resolve_base_branch(worktree, SLUG)
    finally:
        shutil.rmtree(base, ignore_errors=True)
    assert found == "takt/20261115T0000-goal-abc", f"worktree から兄弟プロジェクトの tasks.yaml を使えません: {found!r}"


def test_sibling_project_without_entry_does_not_shadow() -> None:
    base = Path(tempfile.mkdtemp(prefix="task-context-test-"))
    worktree = base / "takt-worktrees" / "worktree"
    worktree.mkdir(parents=True)
    for name, text in (
        ("alpha", "tasks:\n  - run_slug: " + OTHER_SLUG + "\n    base_branch: wrong-branch\n"),
        ("bravo", "tasks:\n  - run_slug: " + SLUG + "\n    base_branch: takt/20261115T0000-goal-abc\n"),
    ):
        directory = base / name / ".takt"
        directory.mkdir(parents=True)
        (directory / "tasks.yaml").write_text(text, encoding="utf-8")
    try:
        module = load_module()
        found = module.resolve_base_branch(worktree, SLUG)
    finally:
        shutil.rmtree(base, ignore_errors=True)
    assert found == "takt/20261115T0000-goal-abc", (
        f"辞書順で先の兄弟プロジェクトの非該当エントリに引きずられました: {found!r}"
    )


def test_missing_entry_error_lists_scanned_candidates() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml([{"run_slug": OTHER_SLUG, "base_branch": "main"}])
        module = load_module()
        try:
            module.resolve_base_branch(workspace.root, SLUG)
        except module.ContextError as error:
            message = str(error)
        else:
            raise AssertionError("該当エントリが無いのに ContextError になりませんでした")
        expected = str((workspace.root / ".takt" / "tasks.yaml").resolve())
    assert expected in message, f"探した候補がエラーに含まれていません: {message}"


def test_duplicate_entries_error_lists_locations() -> None:
    with Workspace() as workspace:
        workspace.tasks_yaml(
            [
                {"run_slug": SLUG, "base_branch": "takt/a"},
                {"run_slug": SLUG, "base_branch": "takt/b"},
            ]
        )
        module = load_module()
        try:
            module.resolve_base_branch(workspace.root, SLUG)
        except module.ContextError as error:
            message = str(error)
        else:
            raise AssertionError("一致が複数なのに ContextError になりませんでした")
        expected = str((workspace.root / ".takt" / "tasks.yaml").resolve())
    assert expected in message, f"一致したエントリの場所がエラーに含まれていません: {message}"


def test_block_scalar_body_does_not_split_entry() -> None:
    with Workspace() as workspace:
        path = workspace.write(
            ".takt/tasks.yaml",
            "tasks:\n"
            f"  - run_slug: {SLUG}\n"
            "    base_branch: takt/20261115T0000-goal-abc\n"
            f"    worktree_path: {workspace.root.resolve()}\n"
            "    retry_note: >-\n"
            "      失敗した点:\n"
            "      - TR-001: 対象を確認する\n"
            "      - TR-002: 再実行する\n"
            "    task_dir: .takt/tasks/sample\n",
        )
        module = load_module()
        entries = module.parse_tasks_yaml(path)
        root = str(workspace.root.resolve())
        assert len(entries) == 1, f"ブロックスカラー本文でエントリが分割されました: {entries}"
        assert entries[0].get("base_branch") == "takt/20261115T0000-goal-abc", entries
        assert entries[0].get("run_slug") == SLUG, entries
        assert entries[0].get("worktree_path") == root, entries


def test_entry_start_indent_keeps_second_entry() -> None:
    with Workspace() as workspace:
        path = workspace.write(
            ".takt/tasks.yaml",
            f"tasks:\n  - run_slug: {SLUG}\n    base_branch: main\n"
            f"  - run_slug: {OTHER_SLUG}\n    base_branch: other\n",
        )
        module = load_module()
        entries = module.parse_tasks_yaml(path)
        slugs = [entry.get("run_slug") for entry in entries]
        assert len(entries) == 2, f"エントリの境界が違います: {entries}"
        assert slugs == [SLUG, OTHER_SLUG], f"エントリの並びが違います: {slugs}"


def test_block_scalar_first_key_keeps_following_keys() -> None:
    with Workspace() as workspace:
        path = workspace.write(
            ".takt/tasks.yaml",
            "tasks:\n"
            "  - retry_note: >-\n"
            "      run_slug: bogus\n"
            f"    run_slug: {SLUG}\n"
            "    base_branch: main\n",
        )
        module = load_module()
        entries = module.parse_tasks_yaml(path)
        assert len(entries) == 1, f"エントリの境界が違います: {entries}"
        assert entries[0].get("run_slug") == SLUG, f"本文の後のキーを読み飛ばしました: {entries}"
        assert entries[0].get("base_branch") == "main", entries


def test_block_scalar_value_is_empty() -> None:
    with Workspace() as workspace:
        path = workspace.write(
            ".takt/tasks.yaml",
            "tasks:\n"
            "  - run_slug: >-\n"
            f"      {SLUG}\n"
            "    base_branch: |\n"
            "      main\n",
        )
        module = load_module()
        entries = module.parse_tasks_yaml(path)
        assert entries == [{"run_slug": "", "base_branch": ""}], f"ブロックスカラーの印を値にしました: {entries}"


def test_hyphen_only_line_starts_entry() -> None:
    with Workspace() as workspace:
        path = workspace.write(
            ".takt/tasks.yaml",
            f"tasks:\n  -\n    run_slug: {SLUG}\n  -\n    run_slug: {OTHER_SLUG}\n",
        )
        module = load_module()
        entries = module.parse_tasks_yaml(path)
        slugs = [entry.get("run_slug") for entry in entries]
        assert slugs == [SLUG, OTHER_SLUG], f"`-` だけの行で項目を分けていません: {entries}"


def test_unreadable_tasks_yaml_is_context_error() -> None:
    with Workspace() as workspace:
        path = workspace.root / ".takt" / "tasks.yaml"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"tasks:\n  - run_slug: \xff\xfe\n")
        module = load_module()
        expect_context_error(module, module.parse_tasks_yaml, path)


def test_missing_tasks_yaml_read_is_context_error() -> None:
    with Workspace() as workspace:
        module = load_module()
        expect_context_error(module, module.parse_tasks_yaml, workspace.root / ".takt" / "tasks.yaml")


def test_issue_reference_allows_trailing_punctuation() -> None:
    module = load_module()
    cases = [
        "Fix issue #123.",
        "issue #123. 次の文に進みます。",
        "対象は issue #123, あわせて確認してください。",
        "(issue #123)",
        "issue #123",
        "issue 123",
    ]
    for text in cases:
        found = module.extract_issue_reference(text)
        assert found == "123", f"{text!r} から issue 番号 123 を取り出す必要があります: {found!r}"


def test_issue_reference_rejects_version_like_number() -> None:
    module = load_module()
    found = module.extract_issue_reference("issue #123.5 を参照してください。")
    assert found is None, f"数字が続くピリオドはバージョン表記として issue 番号にしない必要があります: {found!r}"


def test_issue_reference_prefers_url_and_ignores_pull_url() -> None:
    module = load_module()
    url = "https://github.com/j5ik2o/takt-dash/issues/45"
    found = module.extract_issue_reference(f"対象は {url} です。issue #99 も関係します。")
    assert found == url, f"URL があるときは番号より URL を優先する必要があります: {found!r}"
    found = module.extract_issue_reference("対象は https://github.com/j5ik2o/takt-dash/pull/45 です。")
    assert found is None, f"pull request の URL は issue 参照として扱いません: {found!r}"


def main() -> int:
    tests = [
        test_response_file_resolves_context_order,
        test_unrelated_response_file_falls_back_to_run_directory,
        test_run_directory_resolves_order_without_environment,
        test_multiple_running_runs_are_ambiguous,
        test_tasks_yaml_task_dir_resolves_order_without_environment,
        test_missing_every_candidate_is_context_error,
        test_base_branch_comes_from_matching_run_slug,
        test_main_base_branch_is_preserved,
        test_missing_entry_is_context_error,
        test_duplicate_entries_are_context_error,
        test_missing_tasks_yaml_is_context_error,
        test_empty_base_branch_uses_project_config,
        test_empty_base_branch_without_config_is_error,
        test_sibling_project_tasks_yaml_is_found_from_worktree,
        test_sibling_project_without_entry_does_not_shadow,
        test_missing_entry_error_lists_scanned_candidates,
        test_duplicate_entries_error_lists_locations,
        test_block_scalar_body_does_not_split_entry,
        test_entry_start_indent_keeps_second_entry,
        test_block_scalar_first_key_keeps_following_keys,
        test_block_scalar_value_is_empty,
        test_hyphen_only_line_starts_entry,
        test_unreadable_tasks_yaml_is_context_error,
        test_missing_tasks_yaml_read_is_context_error,
        test_issue_reference_allows_trailing_punctuation,
        test_issue_reference_rejects_version_like_number,
        test_issue_reference_prefers_url_and_ignores_pull_url,
    ]
    failures = 0
    for test in tests:
        signal.signal(signal.SIGALRM, _on_timeout)
        signal.alarm(TEST_TIMEOUT_SECONDS)
        try:
            test()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {test.__name__}: {error}")
        except TestTimeout:
            failures += 1
            print(f"FAIL {test.__name__}: did not finish within {TEST_TIMEOUT_SECONDS}s")
        except Exception:
            failures += 1
            print(f"FAIL {test.__name__}:")
            traceback.print_exc()
        else:
            print(f"ok   {test.__name__}")
        finally:
            signal.alarm(0)
    print(f"{len(tests) - failures} passed, {failures} failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
