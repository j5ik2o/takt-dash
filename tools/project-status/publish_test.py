#!/usr/bin/env python3
"""publish.py の段階実行（commit / push / pr / status）を、偽の git と gh で確かめる。

使い方:
    python3 tools/project-status/publish_test.py

確かめる観点:
  1. commit は git add と git commit を行い、変更が無いときはコミットせず成功する。
     commit が失敗したら非ゼロで終わり、push は git push を呼ばない。
  2. push は commit の成功を前提にし、現在のブランチを origin へ push する。push が
     失敗したら非ゼロで終わり、push.ok を残さず、後続の pr も gh を呼ばずに非ゼロで
     止まる。
  3. pr は tasks.yaml の該当 run_slug の base_branch を --base に渡す。main ベースの
     タスクでは main を渡す。base を解決できないときは gh pr create を呼ばず非ゼロで
     理由を stdout に出す。既存 PR がある場合だけ成功として扱う。
  4. 新しい commit は前回の push を無効にし、push 無しで pr を実行できない。
  5. status は order.md の issue URL を --url に渡す。issue 番号は設定の repository で
     URL にする。pull URL しか無いときは gh を呼ばず「未実施: issue の記述が
     見つからない」と記録して終了コード 0 にする。order.md を特定できないときも gh を
     呼ばず未実施にする。
  6. status は TAKT_PREVIOUS_RESPONSE_FILE が無くても、.takt/runs/<slug>/ の
     meta.json から order.md を解決する（最初のステップでは応答ファイルが無い）。
  7. status は run ディレクトリも応答ファイルも無いとき、.takt/tasks.yaml の該当
     エントリの task_dir から order.md を解決する（開始時点の解決経路）。
  8. status の gh 失敗は非致命で、終了コード 0 と stdout の記録にする。
  9. commit は、ステージされた Python の bytecode（__pycache__ 配下と *.pyc）を
     コミット対象から外す。bytecode だけがステージされたときはコミットせず成功する。

実 gh・実 Git は呼ばない。publish.py と project_status.py を一時リポジトリへ複製し、
PATH の先頭に置いたスタブだけを呼ぶ。
"""
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import traceback
from pathlib import Path

SOURCE = Path(__file__).resolve().parent

SLUG = "20261115-000000-sample-run-abcdef"
BRANCH = "takt/head-branch"
GOAL_BASE = "takt/20261115T0000-goal-abc"
ISSUE_URL = "https://github.com/example/owner/issues/12"
REPOSITORY = "example/owner"

TEST_TIMEOUT_SECONDS = 30

GIT_STUB = '''#!/usr/bin/env python3
import json
import os
import sys

config = json.loads(open(os.environ["STUB_GIT_CONFIG"], encoding="utf-8").read())
with open(os.environ["STUB_CALLS"], "a", encoding="utf-8") as handle:
    handle.write("git " + " ".join(sys.argv[1:]) + "\\n")
if len(sys.argv) > 1 and sys.argv[1] == "diff" and "--quiet" in sys.argv:
    entry = config.get("diff-quiet", {})
else:
    entry = config.get(sys.argv[1] if len(sys.argv) > 1 else "", {})
sys.stdout.write(entry.get("stdout", ""))
sys.stderr.write(entry.get("stderr", ""))
raise SystemExit(entry.get("exit", 0))
'''

GH_STUB = '''#!/usr/bin/env python3
import json
import os
import sys

config = json.loads(open(os.environ["STUB_GH_CONFIG"], encoding="utf-8").read())
with open(os.environ["STUB_CALLS"], "a", encoding="utf-8") as handle:
    handle.write("gh " + " ".join(sys.argv[1:]) + "\\n")
key = " ".join(sys.argv[1:3])
entry = config.get(key, {})
sys.stdout.write(entry.get("stdout", ""))
sys.stderr.write(entry.get("stderr", ""))
raise SystemExit(entry.get("exit", 0))
'''


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


def default_git_config() -> dict:
    return {
        "add": {},
        "diff": {"stdout": "changed.txt\n"},
        "diff-quiet": {"exit": 1},
        "status": {"stdout": "M  changed.txt\n"},
        "commit": {},
        "push": {},
        "rev-parse": {"stdout": BRANCH + "\n"},
        "symbolic-ref": {"stdout": BRANCH + "\n"},
        "branch": {"stdout": BRANCH + "\n"},
    }


def default_gh_config() -> dict:
    return {
        "pr create": {"stdout": "https://github.com/example/owner/pull/58\n"},
        "pr list": {"stdout": "[]\n"},
        "project item-list": {
            "stdout": json.dumps({"items": [], "totalCount": 0}),
        },
        "project item-add": {"stdout": json.dumps({"id": "PVTI_new"})},
        "project item-edit": {},
    }


def base_status_config(**overrides) -> dict:
    config = {
        "owner": "example",
        "project": 7,
        "statusFieldName": "Status",
        "statusNames": {"In Progress": "In Progress", "In Review": "In Review"},
        "repository": REPOSITORY,
    }
    config.update(overrides)
    return config


class Repo:
    """publish.py を動かす一時リポジトリ。git と gh は PATH 先頭のスタブだけを呼ぶ。"""

    def __init__(self) -> None:
        self.base = Path(tempfile.mkdtemp(prefix="publish-test-"))
        self.root = self.base / "repo"
        self.root.mkdir()
        self.home = self.base / "home"
        self.home.mkdir()
        self.bin = self.base / "bin"
        self.bin.mkdir()
        self.calls_path = self.base / "calls.log"
        self.context = self.root / ".takt" / "runs" / SLUG / "context"
        (self.context / "task").mkdir(parents=True)
        commands = self.context / "system-commands" / "uuid"
        commands.mkdir(parents=True)
        self.response = commands / "response.txt"
        self.response.write_text("", encoding="utf-8")
        self.write_meta(SLUG, "running")
        self.git_config = self.base / "git.json"
        self.gh_config = self.base / "gh.json"
        self.git_data = default_git_config()
        self.gh_data = default_gh_config()
        self._write_stub("git", GIT_STUB)
        self._write_stub("gh", GH_STUB)
        self.flush_tool_config()
        self.install_scripts()
        self.write_status_config(base_status_config())

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        shutil.rmtree(self.base, ignore_errors=True)
        return False

    def _write_stub(self, name, body):
        path = self.bin / name
        path.write_text(body, encoding="utf-8")
        path.chmod(0o755)

    def flush_tool_config(self):
        self.git_config.write_text(json.dumps(self.git_data), encoding="utf-8")
        self.gh_config.write_text(json.dumps(self.gh_data), encoding="utf-8")

    def set_git(self, data):
        self.git_data = data
        self.flush_tool_config()

    def set_gh(self, data):
        self.gh_data = data
        self.flush_tool_config()

    def gh_with_item(self, url):
        data = default_gh_config()
        data["project item-list"] = {
            "stdout": json.dumps(
                {"items": [{"id": "PVTI_item", "content": {"url": url}}], "totalCount": 1}
            )
        }
        self.set_gh(data)

    def install_scripts(self):
        dest = self.root / "tools" / "project-status"
        dest.mkdir(parents=True)
        for name in ("publish.py", "project_status.py", "task_context.py"):
            source = SOURCE / name
            assert source.exists(), f"{name} がありません（未実装のため失敗は想定内）"
            shutil.copy2(source, dest / name)

    def write_status_config(self, config):
        path = self.root / "tools" / "project-status" / "project-status.json"
        path.write_text(json.dumps(config), encoding="utf-8")

    def write_order(self, text):
        (self.context / "task" / "order.md").write_text(text, encoding="utf-8")

    def write_meta(self, slug, status):
        path = self.root / ".takt" / "runs" / slug / "meta.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"runSlug": slug, "status": status}), encoding="utf-8")

    def remove_runs(self):
        shutil.rmtree(self.root / ".takt" / "runs", ignore_errors=True)

    def write_tasks(self, entries):
        lines = ["tasks:"]
        for entry in entries:
            first = True
            for key, value in entry.items():
                prefix = "  - " if first else "    "
                lines.append(f"{prefix}{key}: {value}")
                first = False
        path = self.root / ".takt" / "tasks.yaml"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    def goal_task(self, base=GOAL_BASE, slug=SLUG):
        self.write_tasks(
            [
                {
                    "run_slug": slug,
                    "base_branch": base,
                    "worktree_path": self.root.resolve(),
                    "task_dir": ".takt/tasks/sample",
                    "status": "running",
                }
            ]
        )

    def mark_published(self):
        for base in (self.context / ".publish", self.root / ".takt" / ".publish"):
            base.mkdir(parents=True, exist_ok=True)
            for name in ("commit.ok", "push.ok", "pr.ok"):
                (base / name).write_text("ok\n", encoding="utf-8")

    def run(self, *args, response=True, path=None):
        env = {
            "PATH": path if path is not None else f"{self.bin}{os.pathsep}{os.environ.get('PATH', '')}",
            "HOME": str(self.home),
            "STUB_GIT_CONFIG": str(self.git_config),
            "STUB_GH_CONFIG": str(self.gh_config),
            "STUB_CALLS": str(self.calls_path),
        }
        if response:
            env["TAKT_PREVIOUS_RESPONSE_FILE"] = str(self.response)
        return subprocess.run(
            [sys.executable, str(self.root / "tools" / "project-status" / "publish.py"), *args],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            timeout=TEST_TIMEOUT_SECONDS,
        )

    def calls(self):
        if not self.calls_path.exists():
            return []
        return [line for line in self.calls_path.read_text(encoding="utf-8").splitlines() if line]

    def git_calls(self):
        return [line for line in self.calls() if line.startswith("git ")]

    def gh_calls(self):
        return [line for line in self.calls() if line.startswith("gh ")]


def run_ok(repo, *args, response=True):
    result = repo.run(*args, response=response)
    assert result.returncode == 0, (
        f"{args} が失敗しました: exit={result.returncode} stdout={result.stdout!r} stderr={result.stderr!r}"
    )
    return result


def prepare_pushed(repo):
    run_ok(repo, "commit")
    run_ok(repo, "push")


def test_commit_commits_staged_changes() -> None:
    with Repo() as repo:
        result = run_ok(repo, "commit")
        calls = repo.git_calls()
        assert any(line == "git add -A" or line.startswith("git add -A ") for line in calls), calls
        assert any(line.startswith("git commit") for line in calls), calls
        assert "失敗" not in result.stdout, result.stdout


def test_commit_without_changes_succeeds_without_commit() -> None:
    with Repo() as repo:
        data = default_git_config()
        data["diff"] = {"stdout": ""}
        data["diff-quiet"] = {"exit": 0}
        data["status"] = {"stdout": ""}
        repo.set_git(data)
        run_ok(repo, "commit")
        assert not any(line.startswith("git commit") for line in repo.git_calls()), repo.git_calls()


def test_commit_excludes_staged_bytecode() -> None:
    with Repo() as repo:
        data = default_git_config()
        data["diff"] = {
            "stdout": "changed.txt\ntools/project-status/__pycache__/publish.cpython-315.pyc\n"
        }
        repo.set_git(data)
        run_ok(repo, "commit")
        calls = repo.git_calls()
        resets = [line for line in calls if line.startswith("git reset")]
        assert len(resets) == 1, calls
        assert "tools/project-status/__pycache__/publish.cpython-315.pyc" in resets[0], resets
        assert "changed.txt" not in resets[0], resets
        assert any(line.startswith("git commit") for line in calls), calls


def test_commit_with_only_bytecode_succeeds_without_commit() -> None:
    with Repo() as repo:
        data = default_git_config()
        data["diff"] = {"stdout": "tools/project-status/__pycache__/task_context.cpython-315.pyc\n"}
        repo.set_git(data)
        result = run_ok(repo, "commit")
        calls = repo.git_calls()
        assert any(line.startswith("git reset") for line in calls), calls
        assert not any(line.startswith("git commit") for line in calls), calls
        assert "コミットする変更はありません" in result.stdout, result.stdout


def test_commit_failure_blocks_push() -> None:
    with Repo() as repo:
        data = default_git_config()
        data["commit"] = {"exit": 3, "stderr": "error: commit rejected\n"}
        repo.set_git(data)
        commit = repo.run("commit")
        assert commit.returncode != 0, f"commit 失敗は非ゼロである必要があります: {commit.stdout!r}"
        assert "commit rejected" in commit.stdout, f"失敗理由が stdout にありません: {commit.stdout!r}"
        push = repo.run("push")
        assert push.returncode != 0, f"commit 失敗後の push は非ゼロである必要があります: {push.stdout!r}"
        assert not any(line.startswith("git push") for line in repo.git_calls()), repo.git_calls()


def test_push_requires_successful_commit() -> None:
    with Repo() as repo:
        result = repo.run("push")
        assert result.returncode != 0, f"commit 無しの push は非ゼロである必要があります: {result.stdout!r}"
        assert result.stdout.strip(), "push を実行しない理由が stdout にありません"
        assert not any(line.startswith("git push") for line in repo.git_calls()), repo.git_calls()


def test_push_pushes_current_branch() -> None:
    with Repo() as repo:
        run_ok(repo, "commit")
        run_ok(repo, "push")
        pushes = [line for line in repo.git_calls() if line.startswith("git push")]
        assert len(pushes) == 1, pushes
        assert "origin" in pushes[0], pushes
        assert BRANCH in pushes[0], pushes


def test_push_failure_blocks_pr() -> None:
    with Repo() as repo:
        data = default_git_config()
        data["push"] = {"exit": 4, "stderr": "error: push rejected\n"}
        repo.set_git(data)
        run_ok(repo, "commit")
        push = repo.run("push")
        assert push.returncode != 0, f"push 失敗は非ゼロである必要があります: {push.stdout!r}"
        assert "push rejected" in push.stdout, f"失敗理由が stdout にありません: {push.stdout!r}"
        for markers in (repo.context / ".publish", repo.root / ".takt" / ".publish"):
            assert not (markers / "push.ok").is_file(), f"push 失敗で push.ok が生成されました: {markers}"
        pr = repo.run("pr")
        assert pr.returncode != 0, f"push 失敗後の pr は非ゼロである必要があります: {pr.stdout!r}"
        assert not any(line.startswith("gh pr create") for line in repo.gh_calls()), repo.gh_calls()


def test_pr_uses_base_branch_from_tasks_yaml() -> None:
    with Repo() as repo:
        repo.goal_task()
        prepare_pushed(repo)
        run_ok(repo, "pr")
        creates = [line for line in repo.gh_calls() if line.startswith("gh pr create")]
        assert len(creates) == 1, repo.gh_calls()
        assert f"--base {GOAL_BASE}" in creates[0], creates
        assert "--base main" not in creates[0], creates
        assert f"--head {BRANCH}" in creates[0], creates


def test_pr_uses_main_for_main_based_task() -> None:
    with Repo() as repo:
        repo.goal_task(base="main")
        prepare_pushed(repo)
        run_ok(repo, "pr")
        creates = [line for line in repo.gh_calls() if line.startswith("gh pr create")]
        assert len(creates) == 1, repo.gh_calls()
        assert "--base main" in creates[0], creates


def test_pr_fails_without_base_branch() -> None:
    with Repo() as repo:
        prepare_pushed(repo)
        result = repo.run("pr")
        assert result.returncode != 0, f"base を解決できない pr は非ゼロである必要があります: {result.stdout!r}"
        assert "base" in result.stdout and result.stdout.strip(), (
            f"base を解決できない理由が stdout にありません: {result.stdout!r}"
        )
        assert not any(line.startswith("gh pr create") for line in repo.gh_calls()), repo.gh_calls()


def test_pr_existing_pull_request_is_success() -> None:
    with Repo() as repo:
        repo.goal_task()
        prepare_pushed(repo)
        data = default_gh_config()
        data["pr create"] = {"exit": 1, "stderr": "a pull request for branch already exists\n"}
        data["pr list"] = {"stdout": json.dumps([{"url": "https://github.com/example/owner/pull/58"}]) + "\n"}
        repo.set_gh(data)
        run_ok(repo, "pr")
        lists = [line for line in repo.gh_calls() if line.startswith("gh pr list")]
        assert len(lists) == 1, repo.gh_calls()
        assert f"--head {BRANCH}" in lists[0], lists


def test_pr_failure_without_existing_pr_is_failure() -> None:
    with Repo() as repo:
        repo.goal_task()
        prepare_pushed(repo)
        data = default_gh_config()
        data["pr create"] = {"exit": 1, "stderr": "error: HTTP 422\n"}
        data["pr list"] = {"stdout": "[]\n"}
        repo.set_gh(data)
        result = repo.run("pr")
        assert result.returncode != 0, f"PR を作れない pr は非ゼロである必要があります: {result.stdout!r}"
        assert "HTTP 422" in result.stdout, f"失敗理由が stdout にありません: {result.stdout!r}"


def test_status_requires_published_after_pr_failure() -> None:
    with Repo() as repo:
        repo.goal_task()
        repo.write_order(f"対象: {ISSUE_URL}\n")
        prepare_pushed(repo)
        data = default_gh_config()
        data["pr create"] = {"exit": 1, "stderr": "error: HTTP 422\n"}
        data["pr list"] = {"stdout": "[]\n"}
        data["project item-list"] = {
            "stdout": json.dumps(
                {"items": [{"id": "PVTI_item", "content": {"url": ISSUE_URL}}], "totalCount": 1}
            )
        }
        repo.set_gh(data)
        pr = repo.run("pr")
        assert pr.returncode != 0, f"PR を作れない pr は非ゼロである必要があります: {pr.stdout!r}"
        result = run_ok(repo, "status", "--status", "In Review", "--require-published")
        assert not any(line.startswith("gh project") for line in repo.gh_calls()), repo.gh_calls()
        assert "未実施" in result.stdout, result.stdout


def test_pr_requires_successful_push() -> None:
    with Repo() as repo:
        repo.goal_task()
        run_ok(repo, "commit")
        result = repo.run("pr")
        assert result.returncode != 0, f"push 無しの pr は非ゼロである必要があります: {result.stdout!r}"
        assert not any(line.startswith("gh pr create") for line in repo.gh_calls()), repo.gh_calls()


def test_new_commit_invalidates_previous_push() -> None:
    with Repo() as repo:
        repo.goal_task()
        prepare_pushed(repo)
        run_ok(repo, "pr")
        created = len([line for line in repo.gh_calls() if line.startswith("gh pr create")])
        run_ok(repo, "commit")
        result = repo.run("pr")
        assert result.returncode != 0, f"新しい commit の後に push 無しで pr は通せません: {result.stdout!r}"
        after = len([line for line in repo.gh_calls() if line.startswith("gh pr create")])
        assert after == created, f"古い push の成功で pr が呼ばれました: {repo.gh_calls()}"


def test_status_changes_issue_url_from_order() -> None:
    with Repo() as repo:
        repo.write_order(f"対象: {ISSUE_URL}\n")
        repo.gh_with_item(ISSUE_URL)
        result = run_ok(repo, "status", "--status", "In Progress")
        edits = [line for line in repo.gh_calls() if line.startswith("gh project item-edit")]
        assert len(edits) == 1, repo.gh_calls()
        assert f"--url {ISSUE_URL}" in edits[0], edits
        assert "--field Status" in edits[0], edits
        assert "--value In Progress" in edits[0], edits
        assert "失敗" not in result.stdout, result.stdout


def test_status_builds_issue_url_from_number() -> None:
    with Repo() as repo:
        repo.write_order("対象は issue #32 です。\n")
        repo.write_status_config(base_status_config(repository="j5ik2o/takt-dash"))
        expected = "https://github.com/j5ik2o/takt-dash/issues/32"
        repo.gh_with_item(expected)
        repo.mark_published()
        run_ok(repo, "status", "--status", "In Review", "--require-published")
        edits = [line for line in repo.gh_calls() if line.startswith("gh project item-edit")]
        assert len(edits) == 1, repo.gh_calls()
        assert f"--url {expected}" in edits[0], edits
        assert "--value In Review" in edits[0], edits


def test_status_builds_issue_url_from_number_with_trailing_period() -> None:
    with Repo() as repo:
        repo.write_order("Fix issue #32.\n")
        repo.write_status_config(base_status_config(repository="j5ik2o/takt-dash"))
        expected = "https://github.com/j5ik2o/takt-dash/issues/32"
        repo.gh_with_item(expected)
        repo.mark_published()
        run_ok(repo, "status", "--status", "In Review", "--require-published")
        edits = [line for line in repo.gh_calls() if line.startswith("gh project item-edit")]
        assert len(edits) == 1, repo.gh_calls()
        assert f"--url {expected}" in edits[0], edits
        assert "--value In Review" in edits[0], edits


def test_status_require_published_changes_after_publish() -> None:
    with Repo() as repo:
        repo.goal_task()
        repo.write_order(f"対象: {ISSUE_URL}\n")
        repo.gh_with_item(ISSUE_URL)
        prepare_pushed(repo)
        run_ok(repo, "pr")
        result = run_ok(repo, "status", "--status", "In Review", "--require-published")
        edits = [line for line in repo.gh_calls() if line.startswith("gh project item-edit")]
        assert len(edits) == 1, repo.gh_calls()
        assert "--value In Review" in edits[0], edits
        assert "失敗" not in result.stdout, result.stdout


def test_status_without_require_published_ignores_markers() -> None:
    with Repo() as repo:
        repo.write_order(f"対象: {ISSUE_URL}\n")
        repo.gh_with_item(ISSUE_URL)
        result = run_ok(repo, "status", "--status", "In Review")
        edits = [line for line in repo.gh_calls() if line.startswith("gh project item-edit")]
        assert len(edits) == 1, repo.gh_calls()
        assert "--value In Review" in edits[0], edits
        assert "失敗" not in result.stdout, result.stdout


def test_status_pull_url_is_unprocessed() -> None:
    with Repo() as repo:
        repo.write_order(f"対象: https://github.com/{REPOSITORY}/pull/58\n")
        result = run_ok(repo, "status", "--status", "In Progress")
        assert repo.gh_calls() == [], f"pull URL で gh を呼んではいけません: {repo.gh_calls()}"
        assert "未実施: issue の記述が見つからない" in result.stdout, result.stdout


def test_status_without_order_is_unprocessed() -> None:
    with Repo() as repo:
        repo.remove_runs()
        result = run_ok(repo, "status", "--status", "In Progress")
        assert repo.gh_calls() == [], f"order を特定できないのに gh を呼びました: {repo.gh_calls()}"
        assert "未実施" in result.stdout, result.stdout


def test_status_resolves_order_without_environment() -> None:
    with Repo() as repo:
        repo.write_order(f"対象: {ISSUE_URL}\n")
        repo.gh_with_item(ISSUE_URL)
        run_ok(repo, "status", "--status", "In Progress", response=False)
        edits = [line for line in repo.gh_calls() if line.startswith("gh project item-edit")]
        assert len(edits) == 1, repo.gh_calls()
        assert f"--url {ISSUE_URL}" in edits[0], edits


def test_status_resolves_order_from_tasks_yaml_task_dir() -> None:
    with Repo() as repo:
        repo.remove_runs()
        order = repo.root / ".takt" / "tasks" / "sample" / "order.md"
        order.parent.mkdir(parents=True, exist_ok=True)
        order.write_text(f"対象は {ISSUE_URL} です。\n", encoding="utf-8")
        repo.write_tasks(
            [
                {
                    "run_slug": SLUG,
                    "base_branch": GOAL_BASE,
                    "worktree_path": repo.root.resolve(),
                    "task_dir": ".takt/tasks/sample",
                }
            ]
        )
        repo.gh_with_item(ISSUE_URL)
        run_ok(repo, "status", "--status", "In Progress", response=False)
        edits = [line for line in repo.gh_calls() if line.startswith("gh project item-edit")]
        assert len(edits) == 1, repo.gh_calls()
        assert f"--url {ISSUE_URL}" in edits[0], edits


def test_status_failure_is_non_fatal() -> None:
    with Repo() as repo:
        repo.write_order(f"対象: {ISSUE_URL}\n")
        data = default_gh_config()
        data["project item-list"] = {
            "stdout": json.dumps(
                {"items": [{"id": "PVTI_item", "content": {"url": ISSUE_URL}}], "totalCount": 1}
            )
        }
        data["project item-edit"] = {"exit": 1, "stderr": "error: HTTP 401\n"}
        repo.set_gh(data)
        result = run_ok(repo, "status", "--status", "In Progress")
        assert "HTTP 401" in result.stdout, f"失敗理由が stdout にありません: {result.stdout!r}"


def test_commit_without_git_is_failure_recorded() -> None:
    with Repo() as repo:
        (repo.bin / "git").unlink()
        result = repo.run("commit", path=str(repo.bin))
        assert result.returncode != 0, f"git を実行できない commit は非ゼロである必要があります: {result.stdout!r}"
        assert "失敗" in result.stdout, f"失敗理由が stdout にありません: {result.stdout!r}"
        assert "Traceback" not in result.stderr, f"traceback を出してはいけません: {result.stderr!r}"


def test_status_without_gh_is_recorded_with_zero_exit() -> None:
    with Repo() as repo:
        repo.write_order(f"対象: {ISSUE_URL}\n")
        (repo.bin / "gh").unlink()
        result = repo.run("status", "--status", "In Progress", path=str(repo.bin))
        assert result.returncode == 0, f"gh を実行できない status は exit 0 である必要があります: {result.stdout!r}"
        assert "失敗" in result.stdout, f"失敗理由が stdout にありません: {result.stdout!r}"
        assert "Traceback" not in result.stderr, f"traceback を出してはいけません: {result.stderr!r}"
        assert repo.gh_calls() == [], repo.gh_calls()


def main() -> int:
    tests = [
        test_commit_commits_staged_changes,
        test_commit_without_changes_succeeds_without_commit,
        test_commit_excludes_staged_bytecode,
        test_commit_with_only_bytecode_succeeds_without_commit,
        test_commit_failure_blocks_push,
        test_push_requires_successful_commit,
        test_push_pushes_current_branch,
        test_push_failure_blocks_pr,
        test_pr_uses_base_branch_from_tasks_yaml,
        test_pr_uses_main_for_main_based_task,
        test_pr_fails_without_base_branch,
        test_pr_existing_pull_request_is_success,
        test_pr_failure_without_existing_pr_is_failure,
        test_status_requires_published_after_pr_failure,
        test_pr_requires_successful_push,
        test_new_commit_invalidates_previous_push,
        test_status_changes_issue_url_from_order,
        test_status_builds_issue_url_from_number,
        test_status_builds_issue_url_from_number_with_trailing_period,
        test_status_require_published_changes_after_publish,
        test_status_without_require_published_ignores_markers,
        test_status_pull_url_is_unprocessed,
        test_status_without_order_is_unprocessed,
        test_status_resolves_order_without_environment,
        test_status_resolves_order_from_tasks_yaml_task_dir,
        test_status_failure_is_non_fatal,
        test_commit_without_git_is_failure_recorded,
        test_status_without_gh_is_recorded_with_zero_exit,
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
