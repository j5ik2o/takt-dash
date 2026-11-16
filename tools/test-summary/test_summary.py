#!/usr/bin/env python3
"""make test の出力から失敗の要約を抽出し、上限のバイト数に収めて出す。

使い方:
    make test 2>&1 | python3 tools/test-summary/test_summary.py --max-bytes 56000

taktdash-change ワークフローの machine_gate の test 検査が、修正役へ渡す出力を作る。
従来の `tail -c` だけでは、先頭側の失敗の詳細が上限で切られることがあった（issue #133）。

出力:
  - 失敗を抽出しなかった場合は、入力の末尾 max_bytes バイトをそのまま出す（`tail -c` と同じ）。
  - 失敗を抽出した場合は、先頭に「=== make test の失敗の要約」の見出しと要約を置き、
    その後に「=== make test の出力の末尾」の見出しと、残りの予算に収まる入力の末尾を置く。
    要約は失敗したテストの名前、集計の行（cargo の `test result: FAILED`、npm の `ℹ` の行）、
    表明の本文の順に入れ、予算に入らない分は省略したことを行で示す。
  - 出力全体は max_bytes バイト以下で、UTF-8 の文字の途中では切らない。

抽出の対象:
  - cargo: 2 つ目の `failures:` の直後に 4 字字下げで並ぶテスト名、`---- <名前> stdout ----` の
    節の本文（`panicked at` の行から。`note: run with` の行は除く）、`test result: FAILED.` の行、
    失敗したバイナリの `Running` の行、`error: test failed` の行。
  - npm（node の test runner の spec 出力）: `✖ failing tests:` の節の各項目の `test at` の行、
    `✖ <名前>` の行、エラー本文（スタックの `at ...` の行の手前まで）と、`ℹ tests/pass/fail/cancelled` の行。

終了コード:
  0  処理できた（入力の内容によらない。make test の終了コードは呼び出し側の pipefail で伝える）
  非 0  引数の誤りや予期しない例外
"""
import argparse
import re
import sys

SUMMARY_HEADER = "=== make test の失敗の要約"
TAIL_HEADER = "=== make test の出力の末尾"
# 要約に使える予算は、上限のうち次の割合まで。残りを入力の末尾に充てる。
SUMMARY_SHARE_NUMERATOR = 2
SUMMARY_SHARE_DENOMINATOR = 5
# 表明の本文 1 件あたりの上限。1 件の巨大な本文がほかの失敗を押し出さないようにする。
BODY_MAX_BYTES = 2000

NPM_COUNT_LINE = re.compile(r"^ℹ (tests|pass|fail|cancelled) \d+$")
NPM_STACK_LINE = re.compile(r"^\s+at ")


class Failures:
    def __init__(self):
        self.names = []
        self.result_lines = []
        self.bodies = []

    def found(self):
        return bool(self.names or self.result_lines or self.bodies)


def byte_len(text):
    return len(text.encode("utf-8"))


def cut_head(text, max_bytes):
    """text の先頭から max_bytes バイト以内を、文字の途中で切らずに返す。"""
    data = text.encode("utf-8")
    if len(data) <= max_bytes:
        return text
    return data[:max_bytes].decode("utf-8", errors="ignore")


def cut_tail(data, max_bytes):
    """data の末尾 max_bytes バイト以内を、UTF-8 の継続バイトから始めずに返す。"""
    if max_bytes <= 0:
        return b""
    if len(data) <= max_bytes:
        return data
    tail = data[-max_bytes:]
    start = 0
    while start < len(tail) and start < 4 and (tail[start] & 0xC0) == 0x80:
        start += 1
    return tail[start:]


def cargo_stdout_body(lines):
    """`---- <名前> stdout ----` の節の行から、表明の本文を作る。"""
    start = next((i for i, line in enumerate(lines) if " panicked at " in line), 0)
    kept = [line for line in lines[start:] if not line.startswith("note: run with")]
    return "\n".join(kept).strip("\n")


def extract_cargo(lines, failures):
    running = None
    section = None
    for index, line in enumerate(lines):
        if line.startswith("     Running "):
            running = line
        if section is not None and (line.startswith("---- ") or line == "failures:"):
            failures.bodies.append(cargo_stdout_body(section))
            section = None
        if line.startswith("---- ") and line.endswith(" stdout ----"):
            section = []
            continue
        if section is not None:
            section.append(line)
            continue
        if line == "failures:":
            for name_line in lines[index + 1 :]:
                if not name_line.startswith("    ") or not name_line.strip():
                    break
                failures.names.append(name_line.strip())
        elif line.startswith("test result: FAILED."):
            if running is not None:
                failures.result_lines.append(running.strip())
            failures.result_lines.append(line)
        elif line.startswith("error: test failed"):
            failures.result_lines.append(line)
    if section is not None:
        failures.bodies.append(cargo_stdout_body(section))


def extract_npm(lines, failures):
    try:
        start = lines.index("✖ failing tests:")
    except ValueError:
        start = None
    counts = [line for line in lines if NPM_COUNT_LINE.match(line)]
    has_failed_count = any(
        line.startswith("ℹ fail ") and line != "ℹ fail 0" for line in counts
    )
    if start is None and not has_failed_count:
        return
    failures.result_lines.extend(counts)
    if start is None:
        return
    # 項目は `test at` の行で始まる。`test at` の行を持たない項目は `✖ <名前>` の行で始まる。
    item = None
    in_stack = False
    for line in lines[start + 1 :]:
        is_name = line.startswith("✖ ")
        follows_location = item is not None and len(item) == 1 and item[0].startswith("test at ")
        if is_name:
            failures.names.append(line)
        if line.startswith("test at ") or (is_name and not follows_location):
            if item:
                failures.bodies.append("\n".join(item).strip("\n"))
            item = [line]
            in_stack = False
        elif item is None:
            continue
        elif NPM_STACK_LINE.match(line):
            in_stack = True
        elif not in_stack:
            item.append(line)
    if item:
        failures.bodies.append("\n".join(item).strip("\n"))


def build_summary(failures, raw_size, budget):
    """要約の行を優先順に予算へ入れる。入らない行は省略の行で示す。"""
    lines = [f"{SUMMARY_HEADER}（生の出力 {raw_size} バイトから抽出） ==="]
    used = byte_len(lines[0]) + 1
    # 省略の行のために予約する。
    reserve = 200

    def add_group(title, items, unit):
        nonlocal used
        if not items:
            return
        lines.append(title)
        used += byte_len(title) + 1
        for position, item in enumerate(items):
            size = byte_len(item) + 1
            if used + size > budget - reserve:
                note = f"（以降の{unit} {len(items) - position} 件を省略）"
                lines.append(note)
                used += byte_len(note) + 1
                return
            lines.append(item)
            used += size

    add_group("失敗したテスト:", [f"    {name}" for name in failures.names], "テスト名")
    add_group("集計:", failures.result_lines, "集計の行")
    bodies = []
    for body in failures.bodies:
        clipped = cut_head(body, BODY_MAX_BYTES)
        if clipped != body:
            omitted = byte_len(body) - byte_len(clipped)
            clipped += f"\n（この表明の残り {omitted} バイトを省略）"
        bodies.append(clipped)
    add_group("表明:", bodies, "表明")
    return ("\n".join(lines) + "\n").encode("utf-8")


def summarize(data, max_bytes):
    text = data.decode("utf-8", errors="replace")
    lines = text.split("\n")
    failures = Failures()
    extract_cargo(lines, failures)
    extract_npm(lines, failures)
    if not failures.found():
        return data[-max_bytes:] if len(data) > max_bytes else data
    budget = max_bytes * SUMMARY_SHARE_NUMERATOR // SUMMARY_SHARE_DENOMINATOR
    summary = build_summary(failures, len(data), budget)
    tail_template = TAIL_HEADER + "（生の出力 {} バイトのうち最後の {} バイト） ===\n"
    reserved_header = byte_len(tail_template.format(len(data), max_bytes))
    tail = cut_tail(data, max_bytes - len(summary) - reserved_header)
    header = tail_template.format(len(data), len(tail)).encode("utf-8")
    return summary + header + tail


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--max-bytes", type=int, required=True)
    args = parser.parse_args()
    if args.max_bytes <= 0:
        parser.error("--max-bytes は正の整数で指定する")
    data = sys.stdin.buffer.read()
    sys.stdout.buffer.write(summarize(data, args.max_bytes))
    sys.stdout.buffer.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
