#!/usr/bin/env python3
"""project_status.py が gh へ渡す item-edit の argv を、完全一致で固定する。

使い方:
    python3 tools/project-status/project_status_argv_test.py

order.md の確認済み事項のとおり、gh 2.103.3 の item-edit は
`gh project item-edit <number> --owner <owner> --url <url> --field <name> --value <value>`
のフラグ構成を受け付ける。この構成を回帰させないよう、呼び出し引数をそのまま比較する。
実 gh は呼ばず、run_command をスタブに置き換える。
"""
import importlib.util
import io
import json
import shutil
import signal
import sys
import tempfile
import traceback
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "project_status.py"

OWNER = "example"
REPO = "example/owner"
PROJECT_NUMBER = 7
STATUS_FIELD = "Status"
ISSUE_NUMBER = 12
ISSUE_URL = f"https://github.com/{REPO}/issues/{ISSUE_NUMBER}"

EXPECTED_ITEM_EDIT = [
    "gh",
    "project",
    "item-edit",
    str(PROJECT_NUMBER),
    "--owner",
    OWNER,
    "--url",
    ISSUE_URL,
    "--field",
    STATUS_FIELD,
    "--value",
    "In Progress",
]

TEST_TIMEOUT_SECONDS = 10


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


def load_module():
    spec = importlib.util.spec_from_file_location("project_status", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Stub:
    def __init__(self):
        self.calls = []

    def run(self, command, cwd=None, timeout=None):
        self.calls.append(list(command))
        if list(command)[:3] == ["gh", "project", "item-list"]:
            payload = {"items": [{"id": "PVTI_item", "content": {"url": ISSUE_URL}}], "totalCount": 1}
            return {"status": 0, "stdout": json.dumps(payload), "stderr": ""}
        return {"status": 0, "stdout": "", "stderr": ""}


def test_item_edit_argv_is_fixed() -> None:
    base = Path(tempfile.mkdtemp(prefix="project-status-argv-test-"))
    config = base / "project-status.json"
    config.write_text(
        json.dumps(
            {
                "owner": OWNER,
                "project": PROJECT_NUMBER,
                "statusFieldName": STATUS_FIELD,
                "statusNames": {"In Progress": "In Progress"},
                "repository": REPO,
            }
        ),
        encoding="utf-8",
    )
    stub = Stub()
    module = load_module()
    original = module.run_command
    module.run_command = stub.run
    stdout = io.StringIO()
    saved = sys.stdout
    sys.stdout = stdout
    try:
        code = module.main(
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(config)]
        )
    finally:
        sys.stdout = saved
        module.run_command = original
        shutil.rmtree(base, ignore_errors=True)
    assert code == 0, f"exit 0 を期待しました: {code}; stdout={stdout.getvalue()!r}"
    edits = [call for call in stub.calls if call[:3] == ["gh", "project", "item-edit"]]
    assert len(edits) == 1, f"item-edit の呼び出しが 1 回ではありません: {stub.calls}"
    assert edits[0] == EXPECTED_ITEM_EDIT, f"item-edit の argv が契約と違います: {edits[0]}"


def main() -> int:
    tests = [test_item_edit_argv_is_fixed]
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
