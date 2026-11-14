#!/usr/bin/env python3
"""aislop の機械ゲート（TAKT の system step と CI から固定コマンドで呼ばれる）。

aislop を実行し、次のいずれかに当たるとき失敗する。

  - 受け入れ一覧に無い warning 以上の指摘がある
  - 受け入れ一覧のエントリに登録理由（reason）が無い
  - 受け入れ一覧に、もう出なくなったエントリ（stale）が残っている
  - 受け入れ一覧が未設定、または読めない

終了コード:
  0  合格。stdout に短い要約を出し、stderr に判定根拠と未検査範囲を出す
  1  要件違反（受け入れ一覧に無い warning 以上、reason 欠落、stale、一覧未設定）
  2  判定不能（aislop 不在・期待版と不一致・JSON 不正・scoreable が false）

aislop の版は期待版（既定 0.20.1）に固定する。版が違えば判定が変わり得るため、
合格ではなく判定不能として止める。

aislop は `ci --format json` を使う。この JSON に `baseline` が無い場合は、
`ci.baseline` が未設定であり、aislop 自身は warning があっても exit 0 になる。
要件は「受け入れ一覧に無い warning 以上で落ちる」であるため、その状態を合格にせず、
一覧未設定として失敗させる（fail-closed）。

TAKT は command に PATH/SHELL/TMPDIR などの限られた環境しか渡さない。HOME が無いと
npx と npm がキャッシュを読めないため、子プロセスでは HOME が無ければログインユーザーの
ホームを補う。
"""
import argparse
import json
import os
import pwd
import shutil
import subprocess
import sys
from pathlib import Path

DEFAULT_VERSION = "0.20.1"
BASELINE_PATH = ".aislop/ci-baseline.json"
FAILING_SEVERITIES = {"error", "warning"}
ENTRY_REQUIRED_FIELDS = ("rule", "file", "reason")
AISLOP_TIMEOUT_SECONDS = 240


class GateError(Exception):
    """判定できない理由を表す。"""


def _child_environment():
    env = dict(os.environ)
    # TAKT の command 環境には HOME が無い。npx と npm はキャッシュと設定を
    # ログインユーザーのホームに置くため、復元しないと常に解決に失敗する。
    if "HOME" not in env:
        env["HOME"] = pwd.getpwuid(os.getuid()).pw_dir
    env["AISLOP_NO_TELEMETRY"] = "1"
    env["AISLOP_NO_UPDATE_NOTIFIER"] = "1"
    env["AISLOP_NO_HISTORY"] = "1"
    return env


def _run(command, cwd, timeout_seconds):
    try:
        proc = subprocess.run(
            command,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
            env=_child_environment(),
            check=False,
        )
    except FileNotFoundError as error:
        return {"status": None, "stdout": "", "stderr": f"{command[0]} が見つかりません: {error}"}
    except subprocess.TimeoutExpired:
        return {
            "status": None,
            "stdout": "",
            "stderr": f"{' '.join(command)} が {timeout_seconds} 秒以内に終わりませんでした",
        }
    return {"status": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}


def resolve_aislop(expected_version):
    """aislop の実行コマンドを解決する。

    環境変数 AISLOP があればその実体を使う。無ければ PATH 上の aislop を使う。
    PATH に無ければ、版を固定した npx へ落ちる。
    """
    override = os.environ.get("AISLOP")
    if override:
        return [override]
    found = shutil.which("aislop")
    if found:
        return [found]
    return ["npx", "--yes", f"aislop@{expected_version}"]


def read_aislop_version(command, root):
    """aislop の版を取り出す。取り出せなければ GateError にする。"""
    result = _run([*command, "--version"], root, AISLOP_TIMEOUT_SECONDS)
    if result["status"] != 0:
        reason = (result["stderr"] or result["stdout"]).strip()
        raise GateError(f"aislop を起動できません: {reason or 'バージョンを取得できませんでした'}")
    version = result["stdout"].strip().splitlines()[0].strip() if result["stdout"].strip() else ""
    if not version:
        raise GateError("aislop のバージョン出力が空です")
    return version


def run_aislop_ci(command, root):
    """`aislop ci --format json` を実行し、stdout の JSON を返す。"""
    result = _run([*command, "ci", "--format", "json", "."], root, AISLOP_TIMEOUT_SECONDS)
    if result["status"] is None:
        raise GateError(f"aislop ci を実行できません: {result['stderr'].strip()}")
    try:
        return json.loads(result["stdout"])
    except ValueError as error:
        raise GateError(f"aislop ci の出力が JSON ではありません: {error}")


def collect_missing_tools(payload):
    """未検査になったエンジンと不足ツールを集める。"""
    missing = {}
    for name, engine in (payload.get("engines") or {}).items():
        tools = engine.get("missingTools") if isinstance(engine, dict) else None
        if tools:
            missing[name] = sorted(str(tool) for tool in tools)
    return missing


def read_baseline_entries(root):
    """受け入れ一覧を読み、entries 配列を返す。

    一覧が無ければ None を返す。`ci.baseline` 未設定と同じく、合格にしない。
    """
    path = root / BASELINE_PATH
    if not path.exists():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except ValueError as error:
        raise GateError(f"{BASELINE_PATH} を JSON として読めません: {error}")
    entries = payload.get("entries") if isinstance(payload, dict) else None
    if not isinstance(entries, list):
        raise GateError(f"{BASELINE_PATH} に entries 配列がありません")
    return entries


def validate_baseline_entries(entries):
    """受け入れ一覧の書式を、aislop とは別にゲート自身でも検査する。

    rule と file で対象を対応付け、reason で登録理由を残す。aislop は reason を
    検査しないため、一覧の書式はこのゲートが守る。
    """
    problems = []
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            problems.append(f"entries[{index}] がオブジェクトではありません")
            continue
        missing = [field for field in ENTRY_REQUIRED_FIELDS if not str(entry.get(field) or "").strip()]
        if missing:
            problems.append(f"entries[{index}] に {', '.join(missing)} がありません")
    return problems


def new_findings(payload):
    """受け入れ一覧に無く、severity が error か warning の指摘を返す。"""
    found = []
    for diagnostic in payload.get("diagnostics") or []:
        if not isinstance(diagnostic, dict):
            continue
        if diagnostic.get("baselined") is True:
            continue
        severity = str(diagnostic.get("severity") or "")
        if severity not in FAILING_SEVERITIES:
            continue
        found.append(diagnostic)
    return found


def describe_new_finding(diagnostic):
    return "{rule} ({severity}) {file}:{line}".format(
        rule=diagnostic.get("rule"),
        severity=diagnostic.get("severity"),
        file=diagnostic.get("filePath"),
        line=diagnostic.get("line"),
    )


def evaluate(root, expected_version):
    """判定を 1 か所で行い、終了コードと stderr 行を同じ結果から作る。"""
    command = resolve_aislop(expected_version)
    version = read_aislop_version(command, root)
    if version != expected_version:
        raise GateError(
            f"aislop の版が期待版と違います: 実測 {version}、期待 {expected_version}。"
            " 判定が変わり得るため、合格にせず判定不能として止めます。"
        )

    payload = run_aislop_ci(command, root)
    if payload.get("scoreable") is not True:
        raise GateError(f"aislop のスキャンが採点可能ではありません: scoreable={payload.get('scoreable')!r}")

    summary = payload.get("baseline")

    problems = []
    if not isinstance(summary, dict):
        problems.append(
            f"受け入れ一覧 {BASELINE_PATH} が設定されていません。"
            " aislop の ci.baseline を有効にし、一覧を作成してください。"
        )
        summary = {}
    elif summary.get("status") != "ok":
        problems.append(f"受け入れ一覧の状態が ok ではありません: status={summary.get('status')!r}")

    entries = read_baseline_entries(root)
    if entries is None:
        problems.append(f"受け入れ一覧 {BASELINE_PATH} がありません")
    else:
        problems.extend(f"受け入れ一覧の書式: {problem}" for problem in validate_baseline_entries(entries))

    findings = new_findings(payload)
    if findings:
        problems.append(f"受け入れ一覧に無い warning 以上の指摘が {len(findings)} 件あります")
        problems.extend(f"  {describe_new_finding(finding)}" for finding in findings)

    stale = summary.get("stale") or 0
    if stale > 0:
        problems.append(
            f"受け入れ一覧にもう出なくなったエントリが {stale} 件残っています。"
            " aislop baseline prune で削除してください。"
        )

    report = {
        "score": payload.get("score"),
        "warnings": (payload.get("summary") or {}).get("warnings"),
        "errors": (payload.get("summary") or {}).get("errors"),
        "files": (payload.get("summary") or {}).get("files"),
        "accepted": summary.get("accepted"),
        "new": summary.get("new"),
        "stale": stale,
        "missing_tools": collect_missing_tools(payload),
    }
    return problems, report


def describe_report(report):
    tools = report["missing_tools"]
    tool_text = "なし"
    if tools:
        tool_text = "; ".join(
            f"{engine}: {', '.join(names)}" for engine, names in sorted(tools.items())
        )
    return (
        f"aislop score={report['score']} errors={report['errors']} warnings={report['warnings']} "
        f"files={report['files']} accepted={report['accepted']} new={report['new']} stale={report['stale']}\n"
        f"検査できていない範囲（不足ツール）: {tool_text}"
    )


def main(argv):
    parser = argparse.ArgumentParser(description="aislop の機械ゲート")
    parser.add_argument("--root", default=".", help="リポジトリのルート")
    parser.add_argument(
        "--version",
        default=os.environ.get("AISLOP_VERSION") or DEFAULT_VERSION,
        help=f"期待する aislop の版（既定は {DEFAULT_VERSION}）",
    )
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    if not root.is_dir():
        print(f"リポジトリのルートがありません: {root}", file=sys.stderr)
        return 2

    try:
        problems, report = evaluate(root, args.version)
    except GateError as error:
        print(f"aislop_gate: {error}", file=sys.stderr)
        return 2

    print(describe_report(report), file=sys.stderr)
    if problems:
        print("aislop_gate: 失敗", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print("aislop_gate: 合格", file=sys.stderr)
    print(
        f"aislop 合格: score={report['score']} warnings={report['warnings']} "
        f"accepted={report['accepted']} new={report['new']}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
