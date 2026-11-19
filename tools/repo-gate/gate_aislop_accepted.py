"""aislop の受け入れ一覧（scripts/aislop-accepted.json）の各エントリを検査する。

受け入れは窓口が承認した一時的な措置で、解消を追跡する issue が閉じたらエントリを
削除する。そのため各エントリの reason に解消 issue の URL を求め、各 finding に
受け入れ時点の行数の上限（maxLines）を求めて、対象ファイルの実測行数と比べる。
missingToolsAllowed と engines は検査しない。
"""
import re
from pathlib import PurePath

from gate_common import GateError, read_json

ACCEPTED_PATH = "scripts/aislop-accepted.json"
ISSUE_LINK = re.compile(r"https://github\.com/j5ik2o/takt-dash/issues/[1-9][0-9]*(?![0-9A-Za-z_/])")
ISSUE_LINK_FORM = "https://github.com/j5ik2o/takt-dash/issues/<番号>"
OPERATION_NOTE = "受け入れは窓口の承認が要る一時的な措置で、解消 issue が閉じたらエントリを削除する"


def check_aislop_accepted(root):
    data = read_json(root / ACCEPTED_PATH)
    entries = data.get("accepted") if isinstance(data, dict) else None
    if not isinstance(entries, list):
        raise GateError(f"{ACCEPTED_PATH} の accepted が配列ではありません")
    findings = 0
    for index, entry in enumerate(entries, start=1):
        findings += _check_entry(root, index, entry)
    return {"entries": len(entries), "findings": findings}


def _check_entry(root, index, entry):
    label = f"{ACCEPTED_PATH} の accepted[{index}]"
    if not isinstance(entry, dict):
        raise GateError(f"{label} がオブジェクトではありません")
    reason = entry.get("reason")
    if not isinstance(reason, str) or ISSUE_LINK.search(reason) is None:
        raise GateError(f"{label} の reason に解消 issue の URL（{ISSUE_LINK_FORM}）がありません（{OPERATION_NOTE}）")
    findings = entry.get("findings")
    if not isinstance(findings, list) or not findings:
        raise GateError(f"{label} の findings が空で、受け入れ時点の行数の上限（maxLines）がありません")
    for position, finding in enumerate(findings, start=1):
        _check_finding(root, f"{label}.findings[{position}]", finding)
    return len(findings)


def _check_finding(root, label, finding):
    if not isinstance(finding, dict):
        raise GateError(f"{label} がオブジェクトではありません")
    file = finding.get("file")
    if not isinstance(file, str) or not file:
        raise GateError(f"{label} の file がありません")
    limit = finding.get("maxLines")
    if not isinstance(limit, int) or isinstance(limit, bool) or limit < 1:
        raise GateError(f"{label}（{file}）に受け入れ時点の行数の上限 maxLines（1 以上の整数）がありません: {limit!r}")
    lines = _count_lines(root, label, file)
    if lines > limit:
        raise GateError(f"{label} の {file} が {lines} 行で、受け入れ時点の上限 maxLines {limit} 行を超えています")


def _count_lines(root, label, file):
    base = root.resolve()
    path = (base / file).resolve()
    if PurePath(file).is_absolute() or base not in path.parents or not path.is_file():
        raise GateError(f"{label} の {file} がリポジトリの中のファイルとしてありません")
    data = path.read_bytes()
    return data.count(b"\n") + (1 if data and not data.endswith(b"\n") else 0)
