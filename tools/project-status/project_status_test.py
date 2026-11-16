#!/usr/bin/env python3
"""project_status.py の引数組み立て・設定検証・分岐・失敗記録を、固定の応答で確かめる。

使い方:
    python3 tools/project-status/project_status_test.py

gh の呼び出しをスタブに置き換え、`main(argv)` の戻り値・標準出力・呼ばれた引数だけを
観測する。本物の gh は呼ばない。

確かめる観点:
  1. 番号指定と URL 指定の両方で、対象 issue の URL が `--url` に渡る。
  2. 設定どおりに `item-list` / `item-edit` が組み立てられ、`--field` / `--value` /
     `--owner` / 番号が設定値になる。`--config` で別ファイルを渡すと切り替わる。
  3. 設定誤り（repository の欠落・形式不正、owner の欠落）は、gh を 1 回も呼ばず、
     stdout に理由を出して終了コード 0 になる。
  4. 項目が無ければ `item-add` を呼んでから `item-edit` を呼ぶ。`item-add` が失敗、
     または一覧が打ち切られているときは `item-edit` を呼ばず未実施を記録する。
  5. PR の URL と他リポジトリの issue は対象外として、`item-list` を呼ばず失敗記録する。
  6. `statusNames` の別名が `--value` に置き換わる。
  7. `item-edit` の失敗・タイムアウトでも終了コード 0 で、stdout に失敗内容を出す。
  8. 成功時は変更内容（対象 URL と状態名）を stdout に出す。
"""
import importlib.util
import io
import json
import shutil
import signal
import subprocess
import sys
import tempfile
import traceback
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "project_status.py"

# 対象 issue の URL・番号・状態名はテストの契約値として固定する。
OWNER = "example"
REPO = "example/owner"
PROJECT_NUMBER = 7
STATUS_FIELD = "Status"
ISSUE_NUMBER = 12
ISSUE_URL = f"https://github.com/{REPO}/issues/{ISSUE_NUMBER}"

# スタブの応答が固まらないよう、1 テストの上限を置く。
TEST_TIMEOUT_SECONDS = 10


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


def base_config(**overrides) -> dict:
    config = {
        "owner": OWNER,
        "project": PROJECT_NUMBER,
        "statusFieldName": STATUS_FIELD,
        "statusNames": {
            "in progress": "In Progress",
            "in review": "In Review",
            "In Progress": "In Progress",
            "In Review": "In Review",
        },
        "repository": REPO,
    }
    config.update(overrides)
    return config


class ConfigDir:
    """設定ファイルを置く一時ディレクトリ。テスト間で使い回さない。"""

    def __init__(self, *configs: dict) -> None:
        self.base = Path(tempfile.mkdtemp(prefix="project-status-test-"))
        self.paths = []
        for index, config in enumerate(configs):
            path = self.base / f"config-{index}.json"
            path.write_text(json.dumps(config), encoding="utf-8")
            self.paths.append(path)

    def write(self, name: str, content: str) -> Path:
        path = self.base / name
        path.write_text(content, encoding="utf-8")
        return path

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        shutil.rmtree(self.base, ignore_errors=True)
        return False


class GhStub:
    """`gh project ...` のサブコマンドごとに固定の応答を返し、呼び出しを記録する。"""

    def __init__(self, responses: dict | None = None) -> None:
        self.responses = responses or {}
        self.calls: list[list[str]] = []

    def run(self, command, cwd=None, timeout=None):
        command = list(command)
        self.calls.append(command)
        handler = self._select(command)
        if handler is None:
            raise AssertionError(f"unexpected command: {command}")
        if isinstance(handler, BaseException):
            raise handler
        return handler

    def _select(self, command):
        if command[:2] != ["gh", "project"]:
            return None
        subcommand = command[2]
        if subcommand in self.responses:
            return self.responses[subcommand]
        return None

    def subcommands(self) -> list[str]:
        return [call[2] for call in self.calls if call[:2] == ["gh", "project"]]


class Runner:
    """スクリプトを import し、gh 呼び出しと標準出力を差し替えて main を呼ぶ。"""

    def __init__(self, stub: GhStub) -> None:
        spec = importlib.util.spec_from_file_location("project_status", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        self.module = module
        module.run_command = stub.run
        self.stdout = io.StringIO()
        self.stderr = io.StringIO()

    def __enter__(self):
        self._stdout, self._stderr = sys.stdout, sys.stderr
        sys.stdout, sys.stderr = self.stdout, self.stderr
        return self

    def __exit__(self, *exc):
        sys.stdout, sys.stderr = self._stdout, self._stderr
        return False

    def run(self, argv: list[str]) -> int:
        return self.module.main(argv)


def gh_ok(stdout: str) -> dict:
    return {"status": 0, "stdout": stdout, "stderr": ""}


def gh_fail(stderr: str, status: int = 1) -> dict:
    return {"status": status, "stdout": "", "stderr": stderr}


def item_list(items: list[dict], total_count: int | None = None) -> dict:
    payload = {"items": items, "totalCount": len(items) if total_count is None else total_count}
    return gh_ok(json.dumps(payload))


def existing_item(url: str = ISSUE_URL, item_id: str = "PVTI_item") -> dict:
    return {"id": item_id, "content": {"url": url, "repository": REPO}}


def item_added(item_id: str = "PVTI_new") -> dict:
    return gh_ok(json.dumps({"id": item_id}))


def arg_value(command: list[str], flag: str) -> str:
    """`--flag value` の value を取り出す。見つからなければ失敗する。"""
    assert flag in command, f"{flag} が引数にありません: {command}"
    return command[command.index(flag) + 1]


def run_with(stub: GhStub, argv: list[str]) -> tuple[int, str, str]:
    with Runner(stub) as runner:
        code = runner.run(argv)
        return code, runner.stdout.getvalue(), runner.stderr.getvalue()


def test_number_input_resolves_url_from_repository() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub(
            {
                "item-list": item_list([existing_item()]),
                "item-edit": gh_ok(""),
            }
        )
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"expected exit 0, got {code}; stderr={err!r}"
    edit = next(call for call in stub.calls if call[2] == "item-edit")
    assert arg_value(edit, "--url") == ISSUE_URL, edit
    assert arg_value(edit, "--owner") == OWNER, edit
    assert arg_value(edit, "--field") == STATUS_FIELD, edit
    assert arg_value(edit, "--value") == "In Progress", edit
    assert str(PROJECT_NUMBER) in edit, f"project number must be passed: {edit}"
    assert ISSUE_URL in out, f"stdout must record the target URL: {out!r}"
    assert "In Progress" in out, f"stdout must record the status: {out!r}"


def test_url_input_uses_same_url() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub(
            {
                "item-list": item_list([existing_item()]),
                "item-edit": gh_ok(""),
            }
        )
        run_with(stub, ["--issue", ISSUE_URL, "--status", "In Progress", "--config", str(configs.paths[0])])
    edit = next(call for call in stub.calls if call[2] == "item-edit")
    assert arg_value(edit, "--url") == ISSUE_URL, edit


def test_config_switch_changes_owner_and_project() -> None:
    other = base_config(owner="another", project=99, repository="another/project")
    other_url = f"https://github.com/another/project/issues/{ISSUE_NUMBER}"
    with ConfigDir(base_config(), other) as configs:
        stub = GhStub(
            {
                "item-list": item_list([existing_item(url=other_url)]),
                "item-edit": gh_ok(""),
            }
        )
        code, _, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[1])],
        )
    assert code == 0, f"expected exit 0, got {code}; stderr={err!r}"
    listing = next(call for call in stub.calls if call[2] == "item-list")
    assert "99" in listing, f"second config's project number must be used: {listing}"
    assert arg_value(listing, "--owner") == "another", listing
    edit = next(call for call in stub.calls if call[2] == "item-edit")
    assert arg_value(edit, "--owner") == "another", edit
    assert arg_value(edit, "--url") == other_url, edit


def test_object_project_resolves_number_by_repository() -> None:
    # 同梱設定と同じく project が repository をキーとするオブジェクトの場合も、
    # 対象 repository の番号が item-list と item-edit に渡る。
    with ConfigDir(base_config(project={REPO: PROJECT_NUMBER})) as configs:
        stub = GhStub(
            {
                "item-list": item_list([existing_item()]),
                "item-edit": gh_ok(""),
            }
        )
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"expected exit 0, got {code}; stderr={err!r}"
    listing = next(call for call in stub.calls if call[2] == "item-list")
    assert listing[3] == str(PROJECT_NUMBER), f"item-list must get the resolved number: {listing}"
    edit = next(call for call in stub.calls if call[2] == "item-edit")
    assert edit[3] == str(PROJECT_NUMBER), f"item-edit must get the resolved number: {edit}"
    assert ISSUE_URL in out, f"stdout must record the target URL: {out!r}"


def test_object_project_missing_repository_key_is_config_error_without_gh() -> None:
    # 対象 repository のキーが無いオブジェクトは、resolve_project_number が
    # gh を呼ぶ前に StatusError にし、main が理由を stdout に出して終了コード 0 で抜ける。
    with ConfigDir(base_config(project={"other/repo": 9})) as configs:
        stub = GhStub()
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"config error must exit 0, got {code}; stderr={err!r}"
    assert stub.calls == [], f"config error must not call gh: {stub.calls}"
    assert "失敗:" in out, f"stdout must record the failure: {out!r}"
    assert REPO in out, f"stdout must name the unresolved repository: {out!r}"


def test_missing_repository_is_config_error_without_gh() -> None:
    config = base_config()
    del config["repository"]
    with ConfigDir(config) as configs:
        stub = GhStub()
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"config error must exit 0, got {code}; stderr={err!r}"
    assert stub.calls == [], f"config error must not call gh: {stub.calls}"
    assert "repository" in out, f"stdout must name the missing key: {out!r}"


def test_malformed_repository_is_config_error_without_gh() -> None:
    with ConfigDir(base_config(repository="not-a-slug")) as configs:
        stub = GhStub()
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"config error must exit 0, got {code}; stderr={err!r}"
    assert stub.calls == [], f"config error must not call gh: {stub.calls}"
    assert "repository" in out, f"stdout must name the malformed key: {out!r}"


def test_missing_config_file_is_failure_recorded_on_stdout() -> None:
    code, out, err = run_with(
        GhStub(),
        ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", "/nonexistent/project-status.json"],
    )
    assert code == 0, f"missing config must exit 0, got {code}; stderr={err!r}"
    assert out.strip(), "stdout must explain the missing config"


def test_item_missing_adds_then_edits() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub(
            {
                "item-list": item_list([]),
                "item-add": item_added(),
                "item-edit": gh_ok(""),
            }
        )
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"expected exit 0, got {code}; stderr={err!r}"
    order = stub.subcommands()
    assert order.index("item-add") < order.index("item-edit"), f"must add before edit: {order}"
    add = next(call for call in stub.calls if call[2] == "item-add")
    assert arg_value(add, "--url") == ISSUE_URL, add
    edit = next(call for call in stub.calls if call[2] == "item-edit")
    assert arg_value(edit, "--url") == ISSUE_URL, edit


def test_item_add_failure_skips_edit_and_records() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub(
            {
                "item-list": item_list([]),
                "item-add": gh_fail("error: could not add item"),
            }
        )
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"expected exit 0, got {code}; stderr={err!r}"
    assert "item-add" in stub.subcommands(), "must try to add the missing item"
    assert "item-edit" not in stub.subcommands(), f"must not edit after add failure: {stub.calls}"
    assert "未実施" in out, f"stdout must record the skipped change: {out!r}"


def test_truncated_list_is_unprocessed_without_add() -> None:
    # totalCount が取得件数より大きい＝一覧が打ち切られている。項目の無いことを
    # 確定できないので、追加も変更もしない。
    with ConfigDir(base_config()) as configs:
        stub = GhStub({"item-list": item_list([], total_count=500)})
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"expected exit 0, got {code}; stderr={err!r}"
    assert "item-add" not in stub.subcommands(), f"must not add on truncated list: {stub.calls}"
    assert "item-edit" not in stub.subcommands(), f"must not edit on truncated list: {stub.calls}"
    assert "未実施" in out, f"stdout must record the skipped change: {out!r}"


def test_pull_url_is_rejected_without_gh() -> None:
    pull_url = f"https://github.com/{REPO}/pull/{ISSUE_NUMBER}"
    with ConfigDir(base_config()) as configs:
        stub = GhStub()
        code, out, err = run_with(
            stub,
            ["--issue", pull_url, "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"pull URL must exit 0, got {code}; stderr={err!r}"
    assert stub.calls == [], f"pull URL must not call gh: {stub.calls}"
    assert out.strip(), "stdout must record that the pull URL is unsupported"


def test_other_repository_url_is_rejected_without_gh() -> None:
    foreign = f"https://github.com/other/repo/issues/{ISSUE_NUMBER}"
    with ConfigDir(base_config()) as configs:
        stub = GhStub()
        code, out, err = run_with(
            stub,
            ["--issue", foreign, "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"foreign URL must exit 0, got {code}; stderr={err!r}"
    assert stub.calls == [], f"foreign URL must not call gh: {stub.calls}"
    assert out.strip(), "stdout must record that the foreign issue is unsupported"


def test_status_alias_is_applied() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub(
            {
                "item-list": item_list([existing_item()]),
                "item-edit": gh_ok(""),
            }
        )
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "in review", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"expected exit 0, got {code}; stderr={err!r}"
    edit = next(call for call in stub.calls if call[2] == "item-edit")
    assert arg_value(edit, "--value") == "In Review", edit
    assert "In Review" in out, f"stdout must record the resolved status name: {out!r}"


def test_unregistered_status_is_config_error_without_gh() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub()
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "Todo", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"unregistered status must exit 0, got {code}; stderr={err!r}"
    assert stub.calls == [], f"unregistered status must not call gh: {stub.calls}"
    assert "statusNames" in out, f"stdout must name the missing key source: {out!r}"
    assert "Todo" in out, f"stdout must quote the unregistered value: {out!r}"


def test_item_edit_failure_is_recorded_with_zero_exit() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub(
            {
                "item-list": item_list([existing_item()]),
                "item-edit": gh_fail("error: invalid option value"),
            }
        )
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"edit failure must exit 0, got {code}; stderr={err!r}"
    assert "失敗" in out, f"stdout must record the failure: {out!r}"
    assert "invalid option value" in out, f"stdout must include the gh reason: {out!r}"


def test_item_list_failure_is_recorded_with_zero_exit() -> None:
    with ConfigDir(base_config()) as configs:
        stub = GhStub({"item-list": gh_fail("error: HTTP 401")})
        code, out, err = run_with(
            stub,
            ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
        )
    assert code == 0, f"list failure must exit 0, got {code}; stderr={err!r}"
    assert "item-add" not in stub.subcommands(), f"must not add after list failure: {stub.calls}"
    assert "失敗" in out, f"stdout must record the failure: {out!r}"


def test_unexpected_response_shape_is_recorded_with_source() -> None:
    # item-list の items が list でない場合、item-add の応答が dict でない場合は、
    # 発生元（item-list / item-add）を名指ししたメッセージで stdout に記録し、
    # 汎用の「想定外の応答です」に置き換えない。
    cases = {
        "items not list": (
            {"item-list": gh_ok(json.dumps({"items": {}}))},
            "item-list",
            ["item-list"],
        ),
        "item-list not object": (
            {"item-list": gh_ok(json.dumps([1, 2, 3]))},
            "item-list",
            ["item-list"],
        ),
        "item-add not object": (
            {
                "item-list": item_list([]),
                "item-add": gh_ok(json.dumps([1, 2, 3])),
            },
            "item-add",
            ["item-list", "item-add"],
        ),
    }
    for label, (responses, expected_source, expected_calls) in cases.items():
        with ConfigDir(base_config()) as configs:
            stub = GhStub(responses)
            code, out, err = run_with(
                stub,
                ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])],
            )
        assert code == 0, f"{label}: must exit 0, got {code}; stderr={err!r}"
        assert stub.subcommands() == expected_calls, f"{label}: calls {stub.subcommands()}"
        assert expected_source in out, f"{label}: stdout must name the source {expected_source}: {out!r}"
        assert "想定外の応答です" not in out, f"{label}: must not collapse into the generic message: {out!r}"


def test_timeout_is_recorded_with_zero_exit() -> None:
    class TimeoutStub(GhStub):
        def run(self, command, **kwargs):
            raise subprocess.TimeoutExpired(cmd=list(command), timeout=kwargs.get("timeout"))

    with ConfigDir(base_config()) as configs:
        stub = TimeoutStub()
        # `run_command` は実装の `_run` を使い、gh の起動（subprocess.run）だけを
        # タイムアウトさせる。これで `_run` の TimeoutExpired → StatusError 変換を
        # 実際の経路で通す。
        spec = importlib.util.spec_from_file_location("project_status", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        original_run = module.subprocess.run
        module.subprocess.run = stub.run
        out = io.StringIO()
        saved = sys.stdout
        sys.stdout = out
        try:
            code = module.main(
                ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])]
            )
        finally:
            sys.stdout = saved
            module.subprocess.run = original_run
    assert code == 0, f"timeout must exit 0, got {code}"
    assert out.getvalue().strip(), "stdout must record the timeout"


def test_missing_executable_is_recorded_with_zero_exit() -> None:
    class MissingStub(GhStub):
        def run(self, command, **kwargs):
            raise FileNotFoundError(2, "No such file or directory", list(command)[0])

    with ConfigDir(base_config()) as configs:
        stub = MissingStub()
        # TimeoutExpired のテストと同じく、gh の起動（subprocess.run）だけを
        # FileNotFoundError にして、`_run` の OSError → StatusError 変換を実経路で通す。
        spec = importlib.util.spec_from_file_location("project_status", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        original_run = module.subprocess.run
        module.subprocess.run = stub.run
        out = io.StringIO()
        err = io.StringIO()
        saved_out, saved_err = sys.stdout, sys.stderr
        sys.stdout, sys.stderr = out, err
        try:
            code = module.main(
                ["--issue", str(ISSUE_NUMBER), "--status", "In Progress", "--config", str(configs.paths[0])]
            )
        finally:
            sys.stdout, sys.stderr = saved_out, saved_err
            module.subprocess.run = original_run
    assert code == 0, f"missing executable must exit 0, got {code}"
    recorded = out.getvalue()
    assert "失敗" in recorded, f"stdout must record the failure: {recorded!r}"
    assert "gh project item-list" in recorded, f"stdout must name the command: {recorded!r}"
    assert "Traceback" not in err.getvalue(), f"stderr must not contain a traceback: {err.getvalue()!r}"


def main() -> int:
    tests = [
        test_number_input_resolves_url_from_repository,
        test_url_input_uses_same_url,
        test_config_switch_changes_owner_and_project,
        test_object_project_resolves_number_by_repository,
        test_object_project_missing_repository_key_is_config_error_without_gh,
        test_missing_repository_is_config_error_without_gh,
        test_malformed_repository_is_config_error_without_gh,
        test_missing_config_file_is_failure_recorded_on_stdout,
        test_item_missing_adds_then_edits,
        test_item_add_failure_skips_edit_and_records,
        test_truncated_list_is_unprocessed_without_add,
        test_pull_url_is_rejected_without_gh,
        test_other_repository_url_is_rejected_without_gh,
        test_status_alias_is_applied,
        test_unregistered_status_is_config_error_without_gh,
        test_item_edit_failure_is_recorded_with_zero_exit,
        test_item_list_failure_is_recorded_with_zero_exit,
        test_unexpected_response_shape_is_recorded_with_source,
        test_timeout_is_recorded_with_zero_exit,
        test_missing_executable_is_recorded_with_zero_exit,
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
