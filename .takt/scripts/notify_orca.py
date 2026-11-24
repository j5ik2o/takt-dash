#!/usr/bin/env python3
"""TAKT の実行の終わりを、Orca の担当ボットへ `orca orchestration send` で知らせる。

使い方（ワークフローの system step の command から呼ぶ）:
    python3 .takt/scripts/notify_orca.py --result completed
    python3 .takt/scripts/notify_orca.py --result aborted

結果の値:
  completed  ワークフローが COMPLETE へ進む直前（taktdash-change の notify_completed）
  aborted / failed / question  いまはワークフローから呼ばない。ABORT の前に通知の工程を置くと
             TAKT がタスクの failure.step と再開位置をその工程として保存し、再試行が修正を
             せずに ABORT へ戻るため。TAKT 本体に、再開位置を変えずに終了時に外部コマンドを
             呼ぶ出口ができたときに使う（理由は直前に終わった工程名で示す）。

宛先の決め方（先に見つかったものを使う）。宛先はその TAKT の仕事を管理しているボットにする:
  1. タスクの指示文（order.md）の 1 行 `orca: bot=<ボット名> item=<番号>`。投入したボットが
     自分の名前と項目番号を書く。
  2. 設定の map が指す対応表の `<ゴール ID>:<タスク名>` または `<ゴール ID>:<workKey>` のエントリ
  3. 設定の default_bot（そのプロジェクトを担当するボット）、項目番号「不明」
  どれも無ければ送らずに記録だけ残す。

設定（プロジェクトごとに 1 か所）:
  .takt/notify-orca.json に {"default_bot": "<担当ボット>", "map": "<対応表のパス>"} を置く。
  map は省略でき、~ はホームに展開する。作業用クローン（cwd）の .takt にあればそれを、
  無ければ tasks.yaml のあるプロジェクトの .takt を読む。ほかのプロジェクトへ写すときは
  この設定だけを変える。

実行の情報の読み方:
  TAKT は command の環境を PATH・SHELL・TMPDIR などと TAKT_PREVIOUS_RESPONSE_FILE だけに
  絞る。応答ファイルは <run>/context/system-commands/<uuid>/response.txt にあるため、
  その祖先を run のディレクトリとし、meta.json と context/task/order.md を読む。
  ゴール ID・workKey・タスク名は .takt/tasks.yaml の該当エントリ（task_context で解決）から読む。

壊さない:
  どんな失敗でも終了コード 0 で終わる。orca の呼び出しは --timeout 秒（既定 10）で打ち切る。
  結果は 1 行だけを run のディレクトリの notify-orca.log（run が分からなければ
  cwd/.takt/notify-orca.log）に追記し、標準出力にも 1 行だけ出す。応答やログの中身は
  本文にも記録にも貼らず、パスだけを書く。orca の標準エラーは同じ場所の notify-orca.stderr に
  先頭 4 KiB だけ残す（原因を調べるため。本文と記録には貼らない）。
"""
from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
PROJECT_STATUS = HERE.parent.parent / "tools" / "project-status"

DEFAULT_ORCA = "/Applications/Orca.app/Contents/Resources/bin/orca"
CONFIG_NAME = "notify-orca.json"
# Orca の端末の外（TAKT の command）から送るため、送信元の名前を --from で明示する。
# 無いと orca は no_active_sender_terminal で送らない。
DEFAULT_SENDER = "takt-workflow"
UNKNOWN_ITEM = "不明"
LOG_NAME = "notify-orca.log"
STDERR_NAME = "notify-orca.stderr"
STDERR_LIMIT = 4096
RESULT_LABELS = {
    "completed": "完了",
    "failed": "失敗",
    "aborted": "中断",
    "question": "質問待ち",
}
ORCA_LINE = re.compile(r"^\s*orca:\s*bot=(\S+)\s+item=([1-9][0-9]*)\s*$", re.MULTILINE)
BOT_NAME = re.compile(r"^[^\s\"'`$\\]{1,64}$")
PR_URL = re.compile(r"https://github\.com/[^/\s]+/[^/\s]+/pull/\d+")
NOTIFY_STEP_PREFIX = "notify_"


def read_config(explicit, cwd: Path, tasks_path) -> dict:
    """notify-orca.json を読み、default_bot と map（Path か None）を返す。読めなければ空。"""
    candidates = [Path(explicit)] if explicit else [cwd / ".takt" / CONFIG_NAME]
    if not explicit and tasks_path is not None:
        candidates.append(tasks_path.parent / CONFIG_NAME)
    for path in candidates:
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict):
            continue
        bot = data.get("default_bot")
        table = data.get("map")
        return {
            "default_bot": bot if isinstance(bot, str) and BOT_NAME.fullmatch(bot) else None,
            # TAKT は HOME を渡さないが、expanduser はパスワードデータベースへ倒れる。
            "map": Path(os.path.expanduser(table)) if isinstance(table, str) and table else None,
        }
    return {"default_bot": None, "map": None}


def _load_task_context():
    if str(PROJECT_STATUS) not in sys.path:
        sys.path.insert(0, str(PROJECT_STATUS))
    import task_context

    return task_context


def find_run_dir(cwd: Path, environ) -> Path | None:
    response = (environ.get("TAKT_PREVIOUS_RESPONSE_FILE") or "").strip()
    if response:
        path = Path(response)
        for directory in path.parents:
            if directory.name == "context" and (directory.parent / "meta.json").is_file():
                return directory.parent
    runs = cwd / ".takt" / "runs"
    running = []
    for meta in sorted(runs.glob("*/meta.json")) if runs.is_dir() else []:
        try:
            if json.loads(meta.read_text(encoding="utf-8")).get("status") == "running":
                running.append(meta.parent)
        except (OSError, ValueError, AttributeError):
            continue
    return running[0] if len(running) == 1 else None


def read_meta(run_dir: Path | None) -> dict:
    if run_dir is None:
        return {}
    try:
        data = json.loads((run_dir / "meta.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def _step_label(entry: dict) -> str | None:
    step = entry.get("step")
    if not isinstance(step, str) or step.startswith(NOTIFY_STEP_PREFIX):
        return None
    workflow = entry.get("workflow")
    return f"{workflow}/{step}" if isinstance(workflow, str) else step


def last_work_step(run_dir: Path | None, meta: dict) -> str | None:
    """直前に終わった工程（通知の工程を除く）を「ワークフロー名/工程名」で返す。

    実行順は run の logs/*.jsonl の step_complete から読む。meta.json の stepExecutions は
    工程ごとの回数を初出順に並べたもので、同じ工程を繰り返すと順序が実行順と食い違うため、
    ログが読めないときだけ使う。
    """
    if run_dir is not None:
        last = None
        for log in sorted((run_dir / "logs").glob("*.jsonl")):
            try:
                lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
            except OSError:
                continue
            for line in lines:
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                if isinstance(record, dict) and record.get("type") == "step_complete":
                    last = _step_label(record) or last
        if last is not None:
            return last
    executions = meta.get("stepExecutions")
    if not isinstance(executions, list):
        return None
    for entry in reversed(executions):
        label = _step_label(entry) if isinstance(entry, dict) else None
        if label is not None:
            return label
    return None


def read_order(run_dir: Path | None, cwd: Path, environ, task_context) -> tuple[str, Path | None]:
    candidates = []
    if run_dir is not None:
        candidates.append(run_dir / "context" / "task" / "order.md")
    for path in candidates:
        if path.is_file():
            return path.read_text(encoding="utf-8", errors="replace"), path
    if task_context is not None:
        try:
            found = Path(task_context.resolve_order(cwd, environ))
            return found.read_text(encoding="utf-8", errors="replace"), found
        except (task_context.ContextError, OSError):
            return "", None
    return "", None


def find_entry(cwd: Path, run_slug, task_context) -> tuple[dict, Path | None]:
    if task_context is None:
        return {}, None
    try:
        entry, tasks_path = task_context.find_task_entry(cwd, run_slug)
        return entry, tasks_path
    except Exception:
        return {}, None


def route_from_order(order: str):
    match = ORCA_LINE.search(order)
    if match is None or not BOT_NAME.fullmatch(match.group(1)):
        return None
    return match.group(1), match.group(2), "指示文"


def route_from_map(map_path: Path, goal_id: str, refs):
    if not goal_id:
        return None
    try:
        table = json.loads(map_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(table, dict):
        return None
    for ref in refs:
        route = table.get(f"{goal_id}:{ref}") if ref else None
        if not isinstance(route, dict):
            continue
        bot, item = route.get("bot"), route.get("item")
        if isinstance(bot, str) and BOT_NAME.fullmatch(bot) and type(item) is int and item > 0:
            return bot, str(item), "対応表", route
    return None


def build_message(args, cwd: Path, environ) -> dict:
    try:
        task_context = _load_task_context()
    except Exception:
        task_context = None
    run_dir = find_run_dir(cwd, environ)
    meta = read_meta(run_dir)
    run_slug = meta.get("runSlug") if isinstance(meta.get("runSlug"), str) else None
    entry, tasks_path = find_entry(cwd, run_slug, task_context)
    order, order_path = read_order(run_dir, cwd, environ, task_context)

    goal_id = entry.get("goal_id", "")
    work_key = entry.get("goal_work_key", "")
    task_name = entry.get("name", "")
    config = read_config(args.config, cwd, tasks_path)
    issue = None
    if task_context is not None and order:
        try:
            # 宛先の行（item=<番号>）を issue と取り違えないよう、その行を除いて読む。
            issue = task_context.extract_issue_reference(ORCA_LINE.sub("", order))
        except Exception:
            issue = None
    prs = sorted(set(PR_URL.findall(order)))

    route = route_from_order(order)
    map_route = None
    if route is None and config["map"] is not None:
        found = route_from_map(config["map"], goal_id, (task_name, work_key))
        if found is not None:
            route, map_route = found[:3], found[3]
    if route is None and config["default_bot"] is not None:
        route = (config["default_bot"], UNKNOWN_ITEM, "既定")
    bot, item, route_source = route if route is not None else (None, UNKNOWN_ITEM, "なし")
    if map_route is not None:
        if not issue and map_route.get("issues"):
            issue = ", ".join(f"#{number}" for number in map_route["issues"])
        if map_route.get("pr"):
            prs = sorted(set(prs) | {map_route["pr"]})

    label = RESULT_LABELS[args.result]
    project = tasks_path.parent.parent if tasks_path is not None else cwd
    task_label = task_name or (run_slug or "不明")
    lines = [
        f"TAKT の実行が終わりました: {label}",
        f"- プロジェクト: {project}",
        f"- ゴール ID: {goal_id or 'なし'}",
        f"- タスク: {task_name or '不明'}（workKey: {work_key or 'なし'}）",
        f"- issue: {issue or 'なし'}",
        f"- PR: {', '.join(prs) if prs else 'なし'}",
        f"- 結果: {label}",
    ]
    if args.result != "completed":
        step = last_work_step(run_dir, meta)
        response = (environ.get("TAKT_PREVIOUS_RESPONSE_FILE") or "").strip()
        reason = f"工程 {step or '不明'} のあと{label}で終わった"
        if response:
            reason += f"（直前の応答: {response}）"
        lines.append(f"- 理由: {reason}")
    lines += [
        f"- ワークフロー: {meta.get('workflow') or entry.get('workflow') or '不明'}",
        f"- 実行 ID: {entry.get('execution_id') or '不明'}（run: {run_slug or '不明'}）",
        f"- run のディレクトリ: {run_dir or '不明'}",
        f"- 指示文: {order_path or '不明'}",
        f"- Orca の項目番号: {item}（宛先の決め方: {route_source}）",
    ]
    return {
        "bot": bot,
        "item": item,
        "subject": f"TAKT {label}: {task_label}",
        "body": "\n".join(lines) + "\n",
        "log_dir": run_dir if run_dir is not None else cwd / ".takt",
    }


def send(orca: str, message: dict, timeout: float, sender: str = DEFAULT_SENDER) -> str:
    """orca を呼び、結果の 1 行の要約を返す。例外は投げない。

    orca の標準エラーは log_dir の notify-orca.stderr に残し（先頭 4 KiB まで）、要約にはパスだけを書く。
    """
    body_path = None
    stderr_path = message["log_dir"] / STDERR_NAME
    try:
        with tempfile.NamedTemporaryFile(
            "w", encoding="utf-8", prefix="notify-orca-", suffix=".md", delete=False
        ) as handle:
            handle.write(message["body"])
            body_path = handle.name
        command = [
            orca, "orchestration", "send",
            "--from", sender,
            "--to", f"bot:{message['bot']}",
            "--type", "status",
            "--subject", message["subject"],
            "--body-file", body_path,
        ]
        if message["item"] != UNKNOWN_ITEM:
            command += ["--item", message["item"]]
        stderr_path.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryFile() as stderr:
            try:
                process = subprocess.Popen(
                    command,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=stderr,
                    start_new_session=True,
                )
            except OSError as error:
                return f"orca を起動できない（{type(error).__name__}）"
            try:
                code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                code = None
            stderr.seek(0)
            stderr_path.write_bytes(stderr.read(STDERR_LIMIT))
        if code is None:
            return f"orca が {timeout:g} 秒で終わらず打ち切った（標準エラー: {stderr_path}）"
        if code != 0:
            return f"orca が終了コード {code} で失敗した（標準エラー: {stderr_path}）"
        return "送信した"
    except Exception as error:
        return f"送信の準備に失敗した（{type(error).__name__}）"
    finally:
        if body_path is not None:
            Path(body_path).unlink(missing_ok=True)


def record(log_dir: Path, line: str) -> str:
    """1 行を記録し、標準出力へ出す 1 行を返す。記録できなければその旨を足す。"""
    try:
        log_dir.mkdir(parents=True, exist_ok=True)
        with (log_dir / LOG_NAME).open("a", encoding="utf-8") as handle:
            handle.write(line + "\n")
    except OSError as error:
        return f"{line}（記録に失敗した: {type(error).__name__}）"
    return line


def main(argv=None, environ=None, cwd=None) -> int:
    environ = os.environ if environ is None else environ
    cwd = Path.cwd() if cwd is None else Path(cwd)
    log_dir = cwd / ".takt"
    try:
        parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
        parser.add_argument("--result", required=True, choices=sorted(RESULT_LABELS))
        parser.add_argument("--orca", default=DEFAULT_ORCA)
        parser.add_argument("--config", help=f"既定は .takt/{CONFIG_NAME}")
        parser.add_argument("--timeout", type=float, default=10.0)
        parser.add_argument("--sender", default=DEFAULT_SENDER)
        try:
            args = parser.parse_args(argv)
        except SystemExit:
            line = f"{_now()} notify-orca: 引数が不正なので送らなかった"
            print(record(log_dir, line))
            return 0
        message = build_message(args, cwd, environ)
        log_dir = message["log_dir"]
        if message["bot"] is None:
            outcome = f"宛先が指示文・対応表・設定（{CONFIG_NAME} の default_bot）のどれにも無いので送らなかった"
        else:
            outcome = send(args.orca, message, args.timeout, args.sender)
        line = (
            f"{_now()} notify-orca: {outcome}: bot:{message['bot']} item={message['item']} "
            f"件名「{message['subject']}」"
        )
    except BaseException as error:  # 通知の失敗でワークフローの判定を変えない
        line = f"{_now()} notify-orca: 想定外の失敗（{type(error).__name__}）"
    print(record(log_dir, line))
    return 0


def _now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


if __name__ == "__main__":
    sys.exit(main())
