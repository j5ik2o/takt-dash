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
  6. 層ごとの evaluate が、層名付きで PASS/FAIL を返す。層ごとの通知はこの単体の
     契約であり、make の停止（最初の未達で終わるか）は wiring テストで確かめる。
  7. 判定失敗の表示は標準エラーへ出す（stdout だけを見る呼出元でも層と FAIL が分かる）。
     成功時は標準出力だけに出す。
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
    """payload を一時ファイルへ書き、評価結果と評価中に出す標準出力・標準エラーを返す。

    成功と失敗で出力先が変わるため、stdout と stderr を別々に捕捉する。単純に結合すると
    出力先の誤りを隠してしまう。
    """
    with tempfile.TemporaryDirectory(prefix="coverage-threshold-test-") as directory:
        path = Path(directory) / "coverage.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = coverage_threshold.evaluate_file(path, layer="repo-gate")
    return result, stdout.getvalue(), stderr.getvalue()


def test_at_80_percent_passes() -> None:
    result, output, error = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 8000, 10000)))
    assert result["ok"] is True, result
    assert result["covered"] == 8000 and result["total"] == 10000, result
    assert result["percent"] == 80.0, result
    assert "repo-gate" in output and "PASS" in output, f"層名と結果を表示していません: {output!r}"
    assert "80.00%" in output, f"実率を表示していません: {output!r}"
    assert error == "", f"成功時の表示が標準エラーへ漏れています: {error!r}"


def test_below_80_percent_fails() -> None:
    result, output, error = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 7999, 10000)))
    assert result["ok"] is False, result
    assert result["percent"] == 79.99, result
    detail = f"標準出力={output!r} 標準エラー={error!r}"
    assert "FAIL" in output + error, f"未達を表示していません: {detail}"
    assert "repo-gate" in output + error, f"失敗した層を表示していません: {detail}"


def test_just_above_80_percent_passes() -> None:
    result, _, _ = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 8001, 10000)))
    assert result["ok"] is True, result


def test_rounded_up_display_below_80_percent_fails() -> None:
    # 7998/9998 は 79.996% で、coverage.py の表示は 80.00 になる。実際の JSON は
    # 整数の covered_lines/num_statements を持つ一方、percent_covered は丸めた表示率
    # でもある。丸め済みの表示率（80.0）を判定に使い、整数カウントを見ない実装は
    # この入力で実率未達を誤って通す。整数の covered_lines/num_statements は 7998/9998
    # のまま、percent_covered だけを 80.0 にしておく（実際にあり得る丸め結果）。
    payload = coverage_payload(("tools/repo-gate/repo_gate.py", 7998, 9998))
    payload["totals"]["percent_covered"] = 80.0
    # 入力の集計率は丸め済みの表示率（80.0）で、整数は実率未達（7998/9998）のまま。
    assert payload["totals"]["percent_covered"] == 80.0, payload["totals"]
    assert payload["totals"]["covered_lines"] == 7998 and payload["totals"]["num_statements"] == 9998, payload["totals"]
    result, output, error = evaluate(payload)
    assert result["ok"] is False, f"表示 80.0 の実率未達を通してしまいました: {result!r}"
    assert "repo-gate" in output + error and "FAIL" in output + error, (
        f"実率未達の層と FAIL を表示していません: 標準出力={output!r} 標準エラー={error!r}"
    )


def test_line_rate_decides_even_when_function_rate_passes() -> None:
    # 関数率（branches）を合格にしておいても、行率が未達なら失敗させる。
    payload = coverage_payload(
        ("tools/repo-gate/repo_gate.py", 7999, 10000), function_covered=1, function_total=1
    )
    result, _, _ = evaluate(payload)
    assert result["ok"] is False, "行率ではなく関数率で判定しています"


def test_zero_total_is_not_a_pass() -> None:
    # 計測データが空のときに合格扱いにすると、計測が壊れても気づけない。
    result, output, error = evaluate(coverage_payload())
    assert result["ok"] is False, result
    assert result["total"] == 0, result
    combined = (output + error).lower()
    assert "計測" in output + error or "total" in combined, f"判定できない理由がありません: {output + error!r}"


def test_reports_each_layer_verdict_by_name() -> None:
    # 層ごとの evaluate が層名付きで PASS/FAIL を返すことを確かめる。これは層ごとの
    # 判定と表示の単体契約であり、複数層が同時に未達のときに make が全層を一括通知する
    # ことは契約ではない。make が最初の未達で停止し、その失敗層が分かることは
    # coverage_wiring_test.py の実 make 配線で確かめる。
    cases = [
        ("backend", coverage_payload(("backend/src/lib.rs", 7999, 10000)), False),
        ("cli", coverage_payload(("frontend/cli/src/main.rs", 8000, 10000)), True),
        ("web", coverage_payload(("frontend/web/app/page.tsx", 7594, 9493)), False),
        ("repo-gate", coverage_payload(("tools/repo-gate/repo_gate.py", 1078, 1348)), False),
    ]
    for layer, payload, expected_ok in cases:
        result, line = coverage_threshold.evaluate(payload, layer)
        assert result["layer"] == layer, result
        assert result["ok"] is expected_ok, f"{layer}: {result!r}"
        verdict = "PASS" if expected_ok else "FAIL"
        assert line.startswith(f"{layer}: {verdict}"), f"{layer}: 層名付きの判定を表示していません: {line!r}"


def test_failure_is_reported_on_stderr_for_stdout_only_callers() -> None:
    # repo-gate の make 失敗報告は、呼び出した make の標準エラーだけを使う
    # （tools/repo-gate/gate_makefile.py の check_machine_gate）。判定失敗の層名と FAIL が
    # 標準出力だけに出ていると、この経路では失敗層が分からなくなる。失敗時は標準エラーへ
    # 出し、標準出力だけを見る呼出元でも層と FAIL が分かるようにする。
    result, output, error = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 1078, 10000)))
    assert result["ok"] is False, result
    assert "repo-gate" in error and "FAIL" in error, f"失敗層と FAIL が標準エラーにありません: {error!r}"


def test_pass_output_is_kept_on_stdout() -> None:
    # 成功時の表示はこれまでどおり標準出力へ出し、標準エラーを汚さない。
    result, output, error = evaluate(coverage_payload(("tools/repo-gate/repo_gate.py", 8000, 10000)))
    assert result["ok"] is True, result
    assert "repo-gate" in output and "PASS" in output, f"成功表示が標準出力にありません: {output!r}"
    assert error == "", f"成功時に標準エラーへ出力しています: {error!r}"


def main() -> int:
    tests = [
        test_at_80_percent_passes,
        test_below_80_percent_fails,
        test_just_above_80_percent_passes,
        test_rounded_up_display_below_80_percent_fails,
        test_line_rate_decides_even_when_function_rate_passes,
        test_zero_total_is_not_a_pass,
        test_reports_each_layer_verdict_by_name,
        test_failure_is_reported_on_stderr_for_stdout_only_callers,
        test_pass_output_is_kept_on_stdout,
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
