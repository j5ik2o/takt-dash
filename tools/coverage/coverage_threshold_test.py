#!/usr/bin/env python3
"""coverage.py の JSON を読み、行カバレッジが実率 80% 以上かを整数比で判定する補助を確かめる。

使い方:
    python3 tools/coverage/coverage_threshold_test.py

判定は表示用の百分率（少数第 2 位で丸めた値）ではなく、covered/total の整数から
`covered * 100 >= total * 80` で行う。Python の coverage.py は rounded な表示で
7998/9998 を 80.00 と出すため、表示値を比べる実装では 80% 未満を通してしまう。
次の観点を確かめる。
  1. 8000/10000（80.00）と 8001/10000 は通し、7999/10000 は失敗させる。
  2. 7998/9998（表示は 80.00、実率は 80% 未満）を失敗させる。
  3. 行率が未達で関数率が合格する例を失敗させ、行率と関数率を取り違えない。
  4. total が 0 の層は合格にも未達にもせず、判定できない理由を返す。
  5. 層名と covered/total と実率を、成功と失敗の両方で表示する。
  6. 失敗した層の一覧を、複数層が同時に未達のときも欠けなく返す。
"""
import contextlib
import io
import json
import signal
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import coverage_threshold  # noqa: E402

TEST_TIMEOUT_SECONDS = 30


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


def coverage_payload(*files, function_covered=1, function_total=1):
    """coverage.py の `coverage json` と同じ形の集計を作る。

    files は (パス, 行の covered, 行の total) の並び。
    """
    entries = {}
    line_covered = 0
    line_total = 0
    for path, covered, total in files:
        entries[path] = {
            "executed_lines": list(range(1, covered + 1)),
            "summary": {
                "covered_lines": covered,
                "num_statements": total,
                "percent_covered": (covered * 100.0 / total) if total else 0.0,
            },
        }
        line_covered += covered
        line_total += total
    return {
        "files": entries,
        "totals": {
            "covered_lines": line_covered,
            "num_statements": line_total,
            "percent_covered": (line_covered * 100.0 / line_total) if line_total else 0.0,
            "covered_branches": function_covered,
            "num_branches": function_total,
        },
    }


def evaluate(payload):
    """payload を一時ファイルへ書き、評価結果と評価中に出す標準出力を返す。"""
    with tempfile.TemporaryDirectory(prefix="coverage-threshold-test-") as directory:
        path = Path(directory) / "coverage.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            result = coverage_threshold.evaluate_file(path, layer="repo-gate")
    return result, stdout.getvalue()


def test_at_80_percent_passes() -> None:
    result, output = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 8000, 10000)))
    assert result["ok"] is True, result
    assert result["covered"] == 8000 and result["total"] == 10000, result
    assert result["percent"] == 80.0, result
    assert "repo-gate" in output and "PASS" in output, f"層名と結果を表示していません: {output!r}"
    assert "80.00%" in output, f"実率を表示していません: {output!r}"


def test_below_80_percent_fails() -> None:
    result, output = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 7999, 10000)))
    assert result["ok"] is False, result
    assert result["percent"] == 79.99, result
    assert "FAIL" in output, f"未達を表示していません: {output!r}"
    assert "repo-gate" in output, f"失敗した層を表示していません: {output!r}"


def test_just_above_80_percent_passes() -> None:
    result, _ = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 8001, 10000)))
    assert result["ok"] is True, result


def test_rounded_up_display_below_80_percent_fails() -> None:
    # 7998/9998 は 79.996% で、coverage.py の表示は 80.00 になる。表示の丸めに
    # 依存せず整数比で判定できていなければ、この入力は誤って通ってしまう。
    payload = coverage_payload(("tools/repo-gate/repo_gate.py", 7998, 9998))
    assert round(payload["totals"]["percent_covered"], 2) == 80.0, payload["totals"]
    result, _ = evaluate(payload)
    assert result["ok"] is False, "表示 80.00 の実率未達を通してしまいました"


def test_line_rate_decides_even_when_function_rate_passes() -> None:
    # 関数率（branches）を合格にしておいても、行率が未達なら失敗させる。
    payload = coverage_payload(
        ("tools/repo-gate/repo_gate.py", 7999, 10000), function_covered=1, function_total=1
    )
    result, _ = evaluate(payload)
    assert result["ok"] is False, "行率ではなく関数率で判定しています"


def test_zero_total_is_not_a_pass() -> None:
    # 計測データが空のときに合格扱いにすると、計測が壊れても気づけない。
    result, output = evaluate(coverage_payload())
    assert result["ok"] is False, result
    assert result["total"] == 0, result
    assert "計測" in output or "total" in output.lower(), f"判定できない理由がありません: {output!r}"


def test_reports_every_failed_layer() -> None:
    # 層ごとに evaluate が層名付きで PASS/FAIL を返す。複数層が同時に未達でも、
    # 各層の名前が失敗として識別できる。
    payloads = {
        "backend": coverage_payload(("backend/src/lib.rs", 7999, 10000)),
        "cli": coverage_payload(("frontend/cli/src/main.rs", 8000, 10000)),
        "web": coverage_payload(("frontend/web/app/page.tsx", 7594, 9493)),
        "repo-gate": coverage_payload(("tools/repo-gate/repo_gate.py", 1078, 1348)),
    }
    results = [coverage_threshold.evaluate(payload, layer) for layer, payload in payloads.items()]
    failed = [result["layer"] for result, _ in results if not result["ok"]]
    assert failed == ["backend", "web", "repo-gate"], failed


def main() -> int:
    tests = [
        test_at_80_percent_passes,
        test_below_80_percent_fails,
        test_just_above_80_percent_passes,
        test_rounded_up_display_below_80_percent_fails,
        test_line_rate_decides_even_when_function_rate_passes,
        test_zero_total_is_not_a_pass,
        test_reports_every_failed_layer,
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
        except Exception:  # 実装漏れや脚本の穴は、失敗として記録する
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
