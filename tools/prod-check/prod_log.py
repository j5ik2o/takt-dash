#!/usr/bin/env python3
"""prod-check のログ読み取り部分。

backend のログ（UTC）を読み、起動時刻、読み取りサイクルの集計、
失敗時の抜粋を作る。時刻は JST へ直して返す。
"""

import os
import re
from datetime import datetime, timedelta, timezone

JST = timezone(timedelta(hours=9))
ANSI = re.compile(r"\x1b\[[0-9;]*m")
CYCLE_RE = re.compile(
    r"定期読み取りの1サイクルを終えました。\s+"
    r"cycle_ms=(\d+)\s+projects=(\d+)\s+projects_read=(\d+)\s+projects_failed=(\d+)\s+"
    r"events_recorded=(\d+)\s+tool_calls=(\d+)\s+tool_failures=(\d+)\s+tool_timeouts=(\d+)"
)
TOOL_TIMEOUT_RE = re.compile(r"(\w+)\(calls=\d+, failures=\d+, timeouts=(\d+)")


def fmt_jst(dt):
    return dt.strftime("%Y-%m-%d %H:%M:%S JST")


def parse_iso(text):
    try:
        return datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return None


def read_backend_log(deploy_dir):
    path = os.path.join(deploy_dir, "data", "backend-launchd.err.log")
    if not os.path.exists(path):
        return None, path
    with open(path, errors="replace") as f:
        return ANSI.sub("", f.read()), path


def backend_started_at(log_text):
    started = None
    for line in log_text.splitlines():
        if "gRPC サーバーを起動します" in line:
            m = re.match(r"(\S+)", line)
            if m:
                dt = parse_iso(m.group(1))
                if dt:
                    started = dt
    return started


def cycles_since_start(log_text):
    lines = log_text.splitlines()
    start_index = 0
    for i, line in enumerate(lines):
        if "gRPC サーバーを起動します" in line:
            start_index = i
    cycles = []
    for line in lines[start_index:]:
        m = CYCLE_RE.search(line)
        if m:
            cycles.append({
                "cycle_ms": int(m.group(1)),
                "projects_failed": int(m.group(4)),
                "tool_calls": int(m.group(6)),
                "tool_timeouts": int(m.group(8)),
                "tool_timeouts_by_name": {
                    name: int(n) for name, n in TOOL_TIMEOUT_RE.findall(line) if int(n) > 0
                },
                "line": line,
            })
    return cycles


def trim_tools(line):
    m = re.search(r"tools=(.*)$", line)
    if not m:
        return line
    kept = [t for t in m.group(1).split("), ") if t and "timeouts=0" not in t]
    head = line[: m.start()]
    return head + "tools=" + "), ".join(kept) + (")" if kept else "")


def jst_stamp(line):
    """行頭の UTC の ISO 時刻を JST の表記へ直す。"""
    m = re.match(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z)\s+(.*)$", line)
    if not m:
        return line
    dt = parse_iso(m.group(1))
    if dt is None:
        return line
    return fmt_jst(dt.astimezone(JST)) + " " + m.group(2)


def log_excerpts(deploy_dir, cycles):
    """失敗時のログ抜粋を作る。backend は時刻を JST へ直し、制御文字を落とす。"""
    out = []
    log_text, _ = read_backend_log(deploy_dir)
    if log_text is not None:
        lines = log_text.splitlines()
        recent_cycles = [l for l in lines if "定期読み取りの1サイクルを終えました" in l][-5:]
        out.append("--- backend の直近のサイクル集計（直近 5 件）")
        for line in recent_cycles:
            out.append(jst_stamp(trim_tools(line)))
        grouped = {}
        for line in lines:
            if " WARN " not in line and " ERROR " not in line:
                continue
            if "会話の行をスキップしました" in line or "unary RPC を処理しました" in line:
                continue
            key = re.sub(r"^\S+\s+", "", line)
            key = re.sub(r"=\S+", "=…", key)[:160]
            stamp = line.split(" ")[0]
            first, last, count = grouped.get(key, (stamp, stamp, 0))
            grouped[key] = (first, stamp, count + 1)
        if grouped:
            out.append("--- backend の WARN と ERROR（同じ文言をまとめ）")
            for key, (first, last, count) in list(grouped.items())[:10]:
                out.append("%s（%d 件、最初 %s、最後 %s）" % (
                    key, count, jst_stamp(first + " x").rsplit(" x", 1)[0],
                    jst_stamp(last + " x").rsplit(" x", 1)[0]))
        starts = [l for l in lines if "gRPC サーバーを起動します" in l][-2:]
        if starts:
            out.append("--- backend の起動")
            out.extend(jst_stamp(l) for l in starts)
    web_path = os.path.join(deploy_dir, "data", "web-launchd.err.log")
    if os.path.exists(web_path):
        with open(web_path, errors="replace") as f:
            web_lines = ANSI.sub("", f.read()).splitlines()
        ready = max((i for i, l in enumerate(web_lines) if "Ready" in l), default=0)
        counts = {}
        for l in web_lines[ready:]:
            if re.search(r"error|Error|ERR_|500", l):
                counts[l] = counts.get(l, 0) + 1
        if counts:
            out.append("--- Web のエラー（最後の Ready 以降、同じ行をまとめ）")
            for line, count in list(counts.items())[:10]:
                out.append("%s（%d 件）" % (line, count) if count > 1 else line)
    return out[:40]
