#!/usr/bin/env python3
"""workflow_inspect.py の判定を、偽の takt で確かめる。

使い方:
    python3 tools/workflow-inspect/workflow_inspect_test.py

`takt` は PATH から探すか `TAKT_BIN` の指定を使うため、テストは一時ディレクトリへ
実行可能な偽の takt（シェルスクリプト）を置き、`TAKT_BIN` で差し替える。本物の takt は
呼ばない。workflow の YAML も一時ディレクトリへ置くので、リポジトリの `.takt` は読まない。

次の観点を確かめる。
  1. 全件成功: 偽の takt がすべて 0 で終われば、0 を返し、各 workflow を OK と表示する。
  2. 1 件失敗: 偽の takt が 1 件だけ 1 で終われば、1 を返し、その名前と takt の出力を出す。
  3. takt が無い: PATH にも `TAKT_BIN` にも takt が無ければ、失敗にせず 0 を返し、
     検査をスキップしたことを表示する。
  4. 引数: 名前を渡すと、その名前だけを inspect する。
  5. `TAKT_BIN` の指す実行ファイルが無いときは、黙ってスキップせず 1 を返す。
"""
import contextlib
import io
import os
import signal
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import workflow_inspect  # noqa: E402

TEST_TIMEOUT_SECONDS = 30
WORKFLOW_STEMS = ["alpha", "beta", "gamma"]
FAKE_TAKT = """#!/bin/sh
# 偽の takt。`workflow inspect <name>` だけを受け、名前を記録して 0 で終わる。
# FAKE_TAKT_FAIL と同じ名前だけ、解決できない workflow_call のエラーを出して 1 で終わる。
if [ "$1" != "workflow" ] || [ "$2" != "inspect" ] || [ -z "$3" ]; then
  echo "unexpected arguments: $*" >&2
  exit 64
fi
if [ -n "$FAKE_TAKT_LOG" ]; then
  printf '%s\\n' "$3" >> "$FAKE_TAKT_LOG"
fi
if [ "$3" = "$FAKE_TAKT_FAIL" ]; then
  echo "workflow \\"$3\\" step \\"develop\\" call \\"development-core\\": workflow_call target was not found"
  exit 1
fi
echo "Workflow inspect: $3"
exit 0
"""


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


@contextlib.contextmanager
def temporary_repository():
    """`.takt/workflows/*.yaml` と偽の takt を持つ一時リポジトリを用意する。"""
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        workflows = root / ".takt" / "workflows"
        workflows.mkdir(parents=True)
        for stem in WORKFLOW_STEMS:
            (workflows / f"{stem}.yaml").write_text(
                f"name: {stem}\ndescription: {stem} の試験用ワークフロー\n",
                encoding="utf-8",
            )
        yield root


def write_fake_takt(directory):
    """実行可能な偽の takt を書いて、そのパスを返す。"""
    path = Path(directory) / "takt"
    path.write_text(FAKE_TAKT, encoding="utf-8")
    path.chmod(0o755)
    return path


@contextlib.contextmanager
def environment(**values):
    """環境変数を差し替え、抜けたら元へ戻す。None は削除を意味する。"""
    saved = {key: os.environ.get(key) for key in values}
    try:
        for key, value in values.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        yield
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def run_main(argv, root):
    """main を呼び、終了コードと標準出力と標準エラーを返す。"""
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        code = workflow_inspect.main(argv, root=root)
    return code, stdout.getvalue(), stderr.getvalue()


def test_all_success_returns_zero():
    with temporary_repository() as root, environment(
        TAKT_BIN=str(write_fake_takt(root)), FAKE_TAKT_FAIL=None, FAKE_TAKT_LOG=None
    ):
        code, stdout, stderr = run_main([], root)
    assert code == 0, (code, stdout, stderr)
    assert stderr == "", stderr
    for stem in WORKFLOW_STEMS:
        assert f"{stem}: OK" in stdout, stdout
    assert "3/3 件の workflow を inspect できました" in stdout, stdout


def test_one_failure_returns_one_and_names_it():
    with temporary_repository() as root, environment(
        TAKT_BIN=str(write_fake_takt(root)), FAKE_TAKT_FAIL="beta", FAKE_TAKT_LOG=None
    ):
        code, stdout, stderr = run_main([], root)
    assert code == 1, (code, stdout, stderr)
    assert "beta: FAIL" in stderr, stderr
    # 失敗した workflow 以外は通ったことを示す。
    assert "alpha: OK" in stdout, stdout
    assert "gamma: OK" in stdout, stdout
    assert "beta: OK" not in stdout, stdout
    # takt の出力をそのまま見せる（名指しのエラーが消えない）。
    assert "workflow_call target was not found" in stderr, stderr
    assert 'workflow "beta" step "develop" call "development-core"' in stderr, stderr
    assert "失敗: beta" in stderr, stderr


def test_missing_takt_skips_with_zero():
    with temporary_repository() as root, environment(
        TAKT_BIN=None, PATH=str(root / "empty-path"), FAKE_TAKT_FAIL=None
    ):
        (root / "empty-path").mkdir()
        code, stdout, stderr = run_main([], root)
    assert code == 0, (code, stdout, stderr)
    assert stderr == "", stderr
    assert "takt が見つかりません" in stdout, stdout
    assert "スキップ" in stdout, stdout


def test_targets_limit_the_inspection():
    with temporary_repository() as root:
        log = root / "inspected.txt"
        with environment(
            TAKT_BIN=str(write_fake_takt(root)), FAKE_TAKT_FAIL=None, FAKE_TAKT_LOG=str(log)
        ):
            code, stdout, stderr = run_main(["alpha"], root)
        inspected = log.read_text(encoding="utf-8").split()
    assert code == 0, (code, stdout, stderr)
    assert inspected == ["alpha"], inspected
    assert "1/1 件の workflow を inspect できました" in stdout, stdout


def test_missing_takt_binary_is_not_skipped_silently():
    with temporary_repository() as root, environment(TAKT_BIN=str(root / "no-takt")):
        code, stdout, stderr = run_main([], root)
    assert code == 1, (code, stdout, stderr)
    assert "TAKT_BIN" in stderr, stderr
    assert stdout == "", stdout


def main():
    tests = [
        test_all_success_returns_zero,
        test_one_failure_returns_one_and_names_it,
        test_missing_takt_skips_with_zero,
        test_targets_limit_the_inspection,
        test_missing_takt_binary_is_not_skipped_silently,
    ]
    failures = 0
    for test in tests:
        signal.signal(signal.SIGALRM, _on_timeout)
        signal.alarm(TEST_TIMEOUT_SECONDS * 2)
        try:
            test()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {test.__name__}: {str(error)[:1000]}")
        except TestTimeout:
            failures += 1
            print(f"FAIL {test.__name__}: did not finish within {TEST_TIMEOUT_SECONDS * 2}s")
        except Exception:  # 実装漏れや試験の穴は、失敗として記録する
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
