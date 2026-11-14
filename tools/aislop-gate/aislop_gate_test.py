#!/usr/bin/env python3
"""aislop_gate.py の判定を、固定の応答で確かめる。

使い方:
    python3 tools/aislop-gate/aislop_gate_test.py

aislop 本体と子プロセスの起動は差し替え、本物の aislop やネットワークは使わない。
ゲートが読むのは aislop の `ci --format json` の出力（実測した形）であるため、
固定応答はその形をそのまま写す。

次の観点を確かめる。
  1. 未登録の warning 以上が 1 件でもあれば exit 1、info だけなら exit 0。
  2. 受け入れ一覧（baseline）の status が ok でない（missing/invalid）なら exit 1。
  3. baseline が設定されていない（JSON に baseline が無い）場合は合格にせず exit 1。
  4. stale エントリがあれば exit 1。
  5. 受け入れ一覧のエントリに reason（登録理由）が無ければ exit 1。
  6. aislop の版が期待版と違えば判定不能として exit 2。
  7. aislop の JSON が読めない・scoreable が false・起動できなければ exit 2。
  8. 未検査の範囲（ruff・cargo audit などの不足ツール）を stderr に出す。
  9. 合格・不合格・判定不能のいずれでも、判定根拠を score と件数で stderr に出す。
"""
import importlib.util
import io
import json
import os
import shutil
import signal
import sys
import tempfile
import traceback
from pathlib import Path

GATE = Path(__file__).resolve().parent / "aislop_gate.py"

# 差し替え漏れの実装が本物の aislop を起動し続けることがないよう、1 テストの上限を置く。
TEST_TIMEOUT_SECONDS = 30


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


EXPECTED_VERSION = "0.20.1"

# aislop `ci --format json` の実測した形のうち、ゲートが読む部分だけを持つ。
def ci_output(score=98, scoreable=True, diagnostics=None, baseline=None, missing_tools=None):
    diagnostics = diagnostics if diagnostics is not None else []
    engines = {
        "format": {"issues": 0, "skipped": False, "missingTools": missing_tools or []},
        "lint": {"issues": 0, "skipped": False, "missingTools": missing_tools or []},
        "code-quality": {"issues": 0, "skipped": False, "missingTools": None},
        "ai-slop": {"issues": len(diagnostics), "skipped": False, "missingTools": None},
        "security": {"issues": 0, "skipped": False, "missingTools": ["cargo audit"]},
    }
    payload = {
        "schemaVersion": "1",
        "cliVersion": EXPECTED_VERSION,
        "version": EXPECTED_VERSION,
        "score": score,
        "label": "Healthy",
        "scoreable": scoreable,
        "coverage": {"supportedFiles": 63, "unsupportedFiles": 0, "scoreable": scoreable},
        "engines": engines,
        "diagnostics": diagnostics,
        "findingAssessment": {"rows": [], "byKind": {}, "byConfidence": {}},
        "summary": {"errors": 0, "warnings": len(diagnostics), "fixable": 0, "files": 63, "elapsed": "1ms"},
    }
    if baseline is not None:
        payload["baseline"] = baseline
    return payload


def diagnostic(rule, severity, file_path, baselined=False, line=1):
    entry = {
        "filePath": file_path,
        "engine": "ai-slop",
        "rule": rule,
        "severity": severity,
        "message": f"{rule} の指摘",
        "line": line,
        "column": 0,
        "fixable": False,
    }
    if baselined:
        entry["baselined"] = True
    return entry


OK_BASELINE = {"path": ".aislop/ci-baseline.json", "status": "ok", "accepted": 1, "new": 0, "stale": 0}


class Sandbox:
    """ゲートの対象リポジトリと、その外側に置く一時 HOME を持つ。"""

    def __init__(self) -> None:
        self.base = Path(tempfile.mkdtemp(prefix="aislop-gate-test-"))
        self.root = self.base / "repo"
        self.home = self.base / "home"
        self.root.mkdir()
        self.home.mkdir()
        self.original_environment = dict(os.environ)
        self.module = None

    def __enter__(self):
        os.environ["HOME"] = str(self.home)
        os.environ.pop("AISLOP", None)
        self.module = load_gate()
        return self

    def __exit__(self, *exc):
        os.environ.clear()
        os.environ.update(self.original_environment)
        shutil.rmtree(self.base, ignore_errors=True)
        return False

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def write_baseline(self, entries):
        """受け入れ一覧を置く。entries は baseline の entries 配列そのもの。"""
        return self.write(
            ".aislop/ci-baseline.json",
            json.dumps({"version": 1, "entries": entries}, ensure_ascii=False, indent=2) + "\n",
        )

    def run(self, argv=None) -> tuple:
        stdout, stderr = io.StringIO(), io.StringIO()
        saved = sys.stdout, sys.stderr
        sys.stdout, sys.stderr = stdout, stderr
        try:
            code = self.module.main(["--root", str(self.root), *(argv or [])])
        finally:
            sys.stdout, sys.stderr = saved
        return code, stdout.getvalue(), stderr.getvalue()


def load_gate():
    spec = importlib.util.spec_from_file_location("aislop_gate", GATE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def stub_aislop(sandbox, payload=None, version=EXPECTED_VERSION, raw_stdout=None, fail_start=False):
    """aislop の起動を、version と ci の固定応答へ差し替える。"""
    calls = []

    def fake_run(command, cwd, timeout_seconds):
        calls.append(list(command))
        if fail_start:
            return {"status": None, "stdout": "", "stderr": "aislop が見つかりません"}
        if "--version" in command or "version" in command[1:2]:
            return {"status": 0, "stdout": f"{version}\n", "stderr": ""}
        if "ci" in command:
            stdout = raw_stdout if raw_stdout is not None else json.dumps(payload or {})
            return {"status": 0, "stdout": stdout, "stderr": ""}
        return {"status": 0, "stdout": "", "stderr": ""}

    sandbox.module._run = fake_run
    return calls


def assert_exit(code, expected, label, stderr=""):
    assert code == expected, f"{label}: exit code must be {expected}, got {code}; stderr={stderr!r}"


def test_new_warning_fails() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([{"rule": "ai-slop/x", "engine": "ai-slop", "file": "a.ts", "fingerprint": "f", "count": 1, "reason": "登録理由"}])
        stub_aislop(
            sandbox,
            ci_output(
                diagnostics=[diagnostic("ai-slop/double-type-assertion", "warning", "new.ts")],
                baseline={"path": ".aislop/ci-baseline.json", "status": "ok", "accepted": 1, "new": 1, "stale": 0},
            ),
        )
        code, out, err = sandbox.run()
        assert_exit(code, 1, "未登録の warning", err)
        assert "ai-slop/double-type-assertion" in err, f"未登録の warning の場所を stderr に出していません: {err!r}"


def test_new_error_fails() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([])
        stub_aislop(
            sandbox,
            ci_output(
                diagnostics=[diagnostic("lint/no-unused", "error", "b.ts")],
                baseline={"path": ".aislop/ci-baseline.json", "status": "ok", "accepted": 0, "new": 1, "stale": 0},
            ),
        )
        code, out, err = sandbox.run()
        assert_exit(code, 1, "未登録の error", err)


def test_info_only_passes() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([])
        stub_aislop(
            sandbox,
            ci_output(
                diagnostics=[diagnostic("ai-slop/todo-stub", "info", "todo.ts")],
                baseline={"path": ".aislop/ci-baseline.json", "status": "ok", "accepted": 0, "new": 0, "stale": 0},
            ),
        )
        code, out, err = sandbox.run()
        assert_exit(code, 0, "info のみ", err)


def test_all_baselined_passes() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([{"rule": "complexity/file-too-large", "engine": "code-quality", "file": "tools/repo-gate/repo_gate.py", "fingerprint": "message:abc", "count": 1, "reason": "分割は別作業"}])
        stub_aislop(
            sandbox,
            ci_output(
                diagnostics=[diagnostic("complexity/file-too-large", "warning", "tools/repo-gate/repo_gate.py", baselined=True)],
                baseline=OK_BASELINE,
            ),
        )
        code, out, err = sandbox.run()
        assert_exit(code, 0, "一覧に登録済み", err)


def test_baseline_status_not_ok_fails() -> None:
    for status in ("missing", "invalid"):
        with Sandbox() as sandbox:
            sandbox.write_baseline([])
            stub_aislop(
                sandbox,
                ci_output(
                    diagnostics=[diagnostic("ai-slop/x", "warning", "c.ts", baselined=True)],
                    baseline={"path": ".aislop/ci-baseline.json", "status": status, "accepted": 0, "new": 0, "stale": 0},
                ),
            )
            code, out, err = sandbox.run()
            assert_exit(code, 1, f"baseline status={status}", err)
            assert status in err, f"baseline の status {status!r} を stderr に出していません: {err!r}"


def test_unconfigured_baseline_fails_closed() -> None:
    # aislop は ci.baseline が未設定だと baseline の項目を出さず、warning があっても exit 0 になる。
    # ゲートはそれを合格にせず、受け入れ一覧が設定されていないものとして失敗させる。
    with Sandbox() as sandbox:
        stub_aislop(sandbox, ci_output(diagnostics=[diagnostic("ai-slop/x", "warning", "d.ts")]))
        code, out, err = sandbox.run()
        assert_exit(code, 1, "baseline 未設定", err)


def test_stale_entry_fails() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([{"rule": "ai-slop/x", "engine": "ai-slop", "file": "stale.ts", "fingerprint": "f", "count": 1, "reason": "登録理由"}])
        stub_aislop(
            sandbox,
            ci_output(
                baseline={"path": ".aislop/ci-baseline.json", "status": "ok", "accepted": 0, "new": 0, "stale": 1},
            ),
        )
        code, out, err = sandbox.run()
        assert_exit(code, 1, "stale エントリ", err)
        assert "stale" in err, f"stale を stderr に出していません: {err!r}"


def test_entry_without_reason_fails() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([{"rule": "ai-slop/x", "engine": "ai-slop", "file": "a.ts", "fingerprint": "f", "count": 1}])
        stub_aislop(
            sandbox,
            ci_output(
                diagnostics=[diagnostic("ai-slop/x", "warning", "a.ts", baselined=True)],
                baseline={"path": ".aislop/ci-baseline.json", "status": "ok", "accepted": 1, "new": 0, "stale": 0},
            ),
        )
        code, out, err = sandbox.run()
        assert_exit(code, 1, "reason の無いエントリ", err)
        assert "reason" in err, f"reason の欠落を stderr に出していません: {err!r}"


def test_version_mismatch_is_unknown() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([])
        stub_aislop(sandbox, ci_output(), version="0.20.0")
        code, out, err = sandbox.run()
        assert_exit(code, 2, "版の不一致", err)
        assert "0.20.1" in err, f"期待版を stderr に出していません: {err!r}"


def test_unreadable_json_is_unknown() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([])
        stub_aislop(sandbox, raw_stdout="<html>not json</html>")
        code, out, err = sandbox.run()
        assert_exit(code, 2, "JSON が壊れている", err)


def test_unscoreable_is_unknown() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([])
        stub_aislop(sandbox, ci_output(scoreable=False, baseline=OK_BASELINE))
        code, out, err = sandbox.run()
        assert_exit(code, 2, "scoreable が false", err)


def test_aislop_not_found_is_unknown() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([])
        stub_aislop(sandbox, fail_start=True)
        code, out, err = sandbox.run()
        assert_exit(code, 2, "aislop を起動できない", err)


def test_reports_unchecked_scope_and_counts() -> None:
    with Sandbox() as sandbox:
        sandbox.write_baseline([{"rule": "ai-slop/x", "engine": "ai-slop", "file": "a.ts", "fingerprint": "f", "count": 1, "reason": "登録理由"}])
        stub_aislop(
            sandbox,
            ci_output(
                diagnostics=[diagnostic("ai-slop/x", "warning", "a.ts", baselined=True)],
                baseline=OK_BASELINE,
                missing_tools=["ruff"],
            ),
        )
        code, out, err = sandbox.run()
        assert_exit(code, 0, "合格時", err)
        # 未検査の範囲（ruff・cargo audit）は、合格しても必ず stderr に残す。
        assert "ruff" in err, f"ruff の不足を stderr に出していません: {err!r}"
        assert "cargo audit" in err, f"cargo audit の不足を stderr に出していません: {err!r}"
        # 判定根拠（score と件数）も残す。
        assert "98" in err, f"score を stderr に出していません: {err!r}"


def test_baseline_entries_are_validated_not_only_aislop() -> None:
    """受け入れ一覧の書式（rule と file）をゲート自身が検査する。"""
    cases = {
        "rule が無い": {"engine": "ai-slop", "file": "a.ts", "fingerprint": "f", "count": 1, "reason": "r"},
        "file が無い": {"rule": "ai-slop/x", "engine": "ai-slop", "fingerprint": "f", "count": 1, "reason": "r"},
    }
    for label, entry in cases.items():
        with Sandbox() as sandbox:
            sandbox.write_baseline([entry])
            stub_aislop(sandbox, ci_output(baseline=OK_BASELINE))
            code, out, err = sandbox.run()
            assert_exit(code, 1, label, err)


def main() -> int:
    tests = [
        test_new_warning_fails,
        test_new_error_fails,
        test_info_only_passes,
        test_all_baselined_passes,
        test_baseline_status_not_ok_fails,
        test_unconfigured_baseline_fails_closed,
        test_stale_entry_fails,
        test_entry_without_reason_fails,
        test_version_mismatch_is_unknown,
        test_unreadable_json_is_unknown,
        test_unscoreable_is_unknown,
        test_aislop_not_found_is_unknown,
        test_reports_unchecked_scope_and_counts,
        test_baseline_entries_are_validated_not_only_aislop,
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
        except Exception:  # 未実装（import できない）やスタブの想定外は、失敗として記録する
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
