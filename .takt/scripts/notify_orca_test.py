#!/usr/bin/env python3
"""notify_orca.py を、偽の orca と偽の run ディレクトリで実行して確かめる。

使い方:
    python3 .takt/scripts/notify_orca_test.py

確かめる観点:
  1. 指示文の `orca: bot=<名前> item=<番号>` の行から宛先と項目番号を決め、orca へ
     --from takt-workflow --to bot:<名前> --type status --subject --body-file --item を渡す。本文には
     プロジェクト・ゴール ID・タスク・workKey・issue・PR・結果・実行 ID・run のディレクトリ・
     項目番号が入る。
  2. 指示文に行が無いときは、対応表の `<ゴール ID>:<タスク名>` から宛先・項目・issue・PR を決める。
  3. どちらにも無いときは takt-dash中将宛てにし、--item を付けず、項目番号を「不明」と書く。
     形の崩れた `orca:` 行は使わない。
  4. 中断の通知は、通知の工程を除いた直前の工程を理由に書き、直前の応答はパスだけを書いて
     中身を貼らない。
  5. orca が失敗・時間切れ・存在しない・引数が足りないときも終了コード 0 で終わり、
     run のディレクトリの notify-orca.log に 1 行残す。orca の標準エラーは本文にも記録にも貼らず、
     notify-orca.stderr に分けて残す。時間切れでは orca の子プロセスも止める。
  6. TAKT と同じく HOME などを渡さない最小の環境で動く。
  7. taktdash-change では、COMPLETE と ABORT へ直接進むのは通知の工程（notify_completed /
     notify_aborted）だけで、ほかの工程の終わりはすべて通知を通る。

実の orca は呼ばない。すべて一時ディレクトリで完結する。
"""
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "notify_orca.py"
SLUG = "20261124-100000-sample-task-abcdef"
GOAL = "9a930940-7d84-47a2-b7cc-0377f0640094"
TEST_TIMEOUT_SECONDS = 30

FAKE_ORCA = """#!/bin/sh
out="{out}"
: > "$out.args"
prev=""
for arg in "$@"; do
  printf '%s\\n' "$arg" >> "$out.args"
  if [ "$prev" = "--body-file" ]; then cat "$arg" > "$out.body"; fi
  prev="$arg"
done
{tail}
"""


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


class Workspace:
    """偽のプロジェクト、作業用クローン、run ディレクトリ、偽の orca を持つ一時ディレクトリ。"""

    def __init__(self, order, orca_tail="exit 0", steps=("mark_in_progress", "develop")):
        self.base = Path(tempfile.mkdtemp(prefix="notify-orca-test-")).resolve()
        self.project = self.base / "takt-dash"
        self.clone = self.base / "takt-worktrees" / "20261124T1000-sample-task"
        self.clone.mkdir(parents=True)
        self.run = self.clone / ".takt" / "runs" / SLUG
        context = self.run / "context"
        (context / "task").mkdir(parents=True)
        (context / "task" / "order.md").write_text(order, encoding="utf-8")
        commands = context / "system-commands" / "uuid"
        commands.mkdir(parents=True)
        self.response = commands / "response.txt"
        self.response.write_text("SECRET-RESPONSE-CONTENT\n", encoding="utf-8")
        executions = [{"workflow": "taktdash-change", "step": step, "count": 1} for step in steps]
        (self.run / "meta.json").write_text(
            json.dumps({
                "runSlug": SLUG,
                "workflow": "taktdash-change",
                "status": "running",
                "stepExecutions": executions,
            }),
            encoding="utf-8",
        )
        tasks = self.project / ".takt" / "tasks.yaml"
        tasks.parent.mkdir(parents=True)
        tasks.write_text(
            "tasks:\n"
            f"  - goal_id: {GOAL}\n"
            "    goal_work_key: sample-work-key\n"
            "    name: sample-task\n"
            "    status: running\n"
            f"    run_slug: {SLUG}\n"
            "    execution_id: 11111111-2222-3333-4444-555555555555\n"
            f"    worktree_path: {self.clone}\n"
            "    workflow: taktdash-change\n",
            encoding="utf-8",
        )
        self.map = self.base / "takt-orca-map.json"
        self.out = self.base / "orca-call"
        self.orca = self.base / "fake-orca"
        self.orca.write_text(FAKE_ORCA.format(out=self.out, tail=orca_tail), encoding="utf-8")
        self.orca.chmod(0o755)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        shutil.rmtree(self.base, ignore_errors=True)
        return False

    def run_script(self, *args, orca=None, timeout="5"):
        env = {
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "TMPDIR": str(self.base),
            "TAKT_PREVIOUS_RESPONSE_FILE": str(self.response),
        }
        command = [sys.executable, str(SCRIPT), *args,
                   "--orca", str(orca or self.orca), "--map", str(self.map), "--timeout", timeout]
        started = time.monotonic()
        completed = subprocess.run(
            command, cwd=self.clone, env=env, capture_output=True, text=True, timeout=25
        )
        return completed, time.monotonic() - started

    def args(self):
        return self.out.with_suffix(".args").read_text(encoding="utf-8").splitlines()

    def body(self):
        return self.out.with_suffix(".body").read_text(encoding="utf-8")

    def log(self):
        return (self.run / "notify-orca.log").read_text(encoding="utf-8")


def option(args, name):
    assert name in args, f"{name} が orca の引数にありません: {args}"
    return args[args.index(name) + 1]


def test_order_line_routes_to_bot_and_item() -> None:
    order = "issue #93 の修正。\nPR https://github.com/j5ik2o/takt-dash/pull/341\norca: bot=副元帥 item=149\n"
    with Workspace(order) as ws:
        completed, _ = ws.run_script("--result", "completed")
        assert completed.returncode == 0, completed
        args = ws.args()
        assert args[:2] == ["orchestration", "send"], args
        assert option(args, "--from") == "takt-workflow"
        assert option(args, "--to") == "bot:副元帥"
        assert option(args, "--type") == "status"
        assert option(args, "--subject") == "TAKT 完了: sample-task"
        assert option(args, "--item") == "149"
        assert "--body-file" in args
        body = ws.body()
        for expected in (
            f"プロジェクト: {ws.project}",
            f"ゴール ID: {GOAL}",
            "タスク: sample-task（workKey: sample-work-key）",
            "issue: 93",
            "PR: https://github.com/j5ik2o/takt-dash/pull/341",
            "結果: 完了",
            "実行 ID: 11111111-2222-3333-4444-555555555555",
            f"run のディレクトリ: {ws.run}",
            "Orca の項目番号: 149（宛先の決め方: 指示文）",
        ):
            assert expected in body, f"本文に {expected!r} がありません:\n{body}"
        assert "理由:" not in body
        assert "送信した: bot:副元帥 item=149" in ws.log()
        assert len(completed.stdout.splitlines()) == 1, completed.stdout


def test_map_routes_when_order_has_no_line() -> None:
    with Workspace("指示文に宛先の行は無い。\n") as ws:
        ws.map.write_text(json.dumps({
            f"{GOAL}:sample-task": {
                "item": 134, "bot": "takt-dash中将", "issues": [93, 94],
                "pr": "https://github.com/j5ik2o/takt-dash/pull/341",
            },
        }), encoding="utf-8")
        completed, _ = ws.run_script("--result", "completed")
        assert completed.returncode == 0
        args = ws.args()
        assert option(args, "--to") == "bot:takt-dash中将"
        assert option(args, "--item") == "134"
        body = ws.body()
        assert "issue: #93, #94" in body, body
        assert "PR: https://github.com/j5ik2o/takt-dash/pull/341" in body, body
        assert "Orca の項目番号: 134（宛先の決め方: 対応表）" in body, body


def test_default_route_without_line_or_map() -> None:
    with Workspace("orca: bot=副元帥 item=abc\norca: bot=\"x y\" item=3\n") as ws:
        completed, _ = ws.run_script("--result", "completed")
        assert completed.returncode == 0
        args = ws.args()
        assert option(args, "--to") == "bot:takt-dash中将"
        assert "--item" not in args, args
        assert "Orca の項目番号: 不明（宛先の決め方: 既定）" in ws.body()


def test_aborted_reports_previous_step_without_response_body() -> None:
    steps = ("mark_in_progress", "develop", "machine_gate", "notify_aborted")
    with Workspace("orca: bot=副元帥 item=149\n", steps=steps) as ws:
        completed, _ = ws.run_script("--result", "aborted")
        assert completed.returncode == 0
        assert option(ws.args(), "--subject") == "TAKT 中断: sample-task"
        body = ws.body()
        assert "結果: 中断" in body
        assert f"理由: 工程 taktdash-change/machine_gate のあと中断で終わった（直前の応答: {ws.response}）" in body, body
        assert "SECRET-RESPONSE-CONTENT" not in body
        assert "SECRET-RESPONSE-CONTENT" not in ws.log()


def test_orca_failure_still_exits_zero() -> None:
    with Workspace("orca: bot=副元帥 item=149\n", orca_tail="echo boom >&2; exit 3") as ws:
        completed, _ = ws.run_script("--result", "completed")
        assert completed.returncode == 0
        assert f"orca が終了コード 3 で失敗した（標準エラー: {ws.run / 'notify-orca.stderr'}）" in ws.log()
        assert "boom" not in ws.log() and "boom" not in completed.stdout
        assert (ws.run / "notify-orca.stderr").read_text(encoding="utf-8") == "boom\n"


def test_orca_timeout_is_cut_and_child_stopped() -> None:
    tail = f'sleep 30 & echo $! > "{{pidfile}}"; wait'
    with Workspace("orca: bot=副元帥 item=149\n") as ws:
        pidfile = ws.base / "child.pid"
        ws.orca.write_text(
            FAKE_ORCA.format(out=ws.out, tail=tail.format(pidfile=pidfile)), encoding="utf-8"
        )
        completed, elapsed = ws.run_script("--result", "completed", timeout="1")
        assert completed.returncode == 0
        assert elapsed < 10, f"時間切れで打ち切られていません: {elapsed:.1f}s"
        assert "orca が 1 秒で終わらず打ち切った" in ws.log()
        child = int(pidfile.read_text().strip())
        time.sleep(0.2)
        try:
            os.kill(child, 0)
        except ProcessLookupError:
            pass
        else:
            os.kill(child, signal.SIGKILL)
            raise AssertionError("orca の子プロセスが残っています")


def test_missing_orca_still_exits_zero() -> None:
    with Workspace("orca: bot=副元帥 item=149\n") as ws:
        completed, _ = ws.run_script("--result", "completed", orca=ws.base / "no-such-orca")
        assert completed.returncode == 0
        assert "orca を起動できない" in ws.log()


def test_missing_result_argument_still_exits_zero() -> None:
    with Workspace("orca: bot=副元帥 item=149\n") as ws:
        completed, _ = ws.run_script()
        assert completed.returncode == 0
        assert not ws.out.with_suffix(".args").exists(), "引数不足なのに orca を呼びました"
        assert "引数が不正" in (ws.clone / ".takt" / "notify-orca.log").read_text(encoding="utf-8")


WORKFLOW = SCRIPT.parent.parent / "workflows" / "taktdash-change.yaml"


def test_taktdash_change_ends_only_through_notify_steps() -> None:
    lines = WORKFLOW.read_text(encoding="utf-8").splitlines()
    step = None
    direct = []
    for line in lines:
        stripped = line.strip()
        if line.startswith("  - name: "):
            step = stripped.removeprefix("- name: ")
        if stripped in ("next: COMPLETE", "next: ABORT"):
            direct.append((step, stripped.removeprefix("next: ")))
    assert direct == [("notify_completed", "COMPLETE"), ("notify_aborted", "ABORT")], direct
    for name, result in (("notify_completed", "completed"), ("notify_aborted", "aborted")):
        assert f"command: python3 .takt/scripts/notify_orca.py --result {result}" in "\n".join(lines), name


def main() -> int:
    tests = [
        test_order_line_routes_to_bot_and_item,
        test_map_routes_when_order_has_no_line,
        test_default_route_without_line_or_map,
        test_aborted_reports_previous_step_without_response_body,
        test_orca_failure_still_exits_zero,
        test_orca_timeout_is_cut_and_child_stopped,
        test_missing_orca_still_exits_zero,
        test_missing_result_argument_still_exits_zero,
        test_taktdash_change_ends_only_through_notify_steps,
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
