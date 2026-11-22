#!/usr/bin/env python3
"""coverage.py の JSON を読み、行カバレッジが実率 80% 以上かを整数比で判定する。

使い方:
    python3 tools/coverage/coverage_threshold.py --json target/coverage/repo-gate.json --layer repo-gate

coverage.py の `coverage json` は percent_covered を少数第 2 位へ丸めて出力する。
7998/9998（実率 79.996%）でも表示は 80.00 になるため、表示値を比べると 80% 未満を
通してしまう。ここでは covered_lines と num_statements の整数から
`covered * 100 >= total * 80` で判定し、表示の丸めに依存しない。

終了コード:
  0  すべての層が実率 80% 以上
  1  1 層以上が実率 80% 未満、または計測データが無く判定できない
  2  JSON を読めない、または入力が不正
"""
import argparse
import json
import sys

THRESHOLD_PERCENT = 80


def evaluate(payload, layer):
    """coverage.py の JSON から層の行率を評価し、判定結果と表示行を返す。"""
    totals = payload.get("totals") or {}
    covered = totals.get("covered_lines")
    total = totals.get("num_statements")
    if not isinstance(covered, int) or not isinstance(total, int) or total <= 0:
        reason = (
            f"行カバレッジを判定できません: total={total!r}（計測データがありません）"
        )
        line = f"{layer}: FAIL {reason}"
        return {"layer": layer, "ok": False, "covered": covered, "total": total, "percent": None}, line
    # 表示丸めを避け、整数比で 80% 以上かを厳密に比べる。
    ok = covered * 100 >= total * THRESHOLD_PERCENT
    percent = covered * 100.0 / total
    verdict = "PASS" if ok else "FAIL"
    line = f"{layer}: {verdict} {covered}/{total} ({percent:.2f}%) 行カバレッジ"
    return {"layer": layer, "ok": ok, "covered": covered, "total": total, "percent": percent}, line


def evaluate_file(path, layer):
    """JSON ファイルを読み、1 層を評価する。

    判定結果は成功時は標準出力、失敗時は標準エラーへ出す。下流の repo-gate の失敗報告
    （tools/repo-gate/gate_makefile.py の check_machine_gate）は make done の標準エラー
    だけを使うため、失敗時に標準出力だけへ出すと失敗層が失われる。
    """
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        print(f"{layer}: coverage JSON を読めません: {error}", file=sys.stderr)
        raise SystemExit(2)
    result, line = evaluate(payload, layer)
    print(line, file=sys.stdout if result["ok"] else sys.stderr)
    return result


def main(argv):
    parser = argparse.ArgumentParser(description="coverage.py の JSON から行率 80% を判定する")
    parser.add_argument("--json", required=True, help="coverage json が出力した JSON のパス")
    parser.add_argument("--layer", required=True, help="判定結果に表示する層名")
    args = parser.parse_args(argv)

    from pathlib import Path

    result = evaluate_file(Path(args.json), args.layer)
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
