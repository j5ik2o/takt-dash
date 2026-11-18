#!/usr/bin/env python3
"""test_summary.py の出力を、合成した make test の出力で確かめる。

使い方:
    python3 tools/test-summary/test_summary_test.py

test_summary.py は make test の出力を stdin から受け取り、修正役へ渡す出力を
stdout へ出す。--max-bytes で出力全体の上限を受け取る。次の形を確かめる。
  1. 失敗を抽出しない入力  出力は入力の末尾 max_bytes バイトと同じバイト列
  2. 失敗を抽出した入力    出力は「=== make test の失敗の要約」で始まる行から始まり、
                            要約の後に「=== make test の出力の末尾」で始まる行と、
                            入力の末尾が続く。出力全体は max_bytes バイト以下
  3. 終了コード            入力の内容によらず 0。pipefail で make test の終了コードが残る
"""
import signal
import subprocess
import sys
import traceback
from pathlib import Path

TOOL = Path(__file__).resolve().parent / "test_summary.py"
MAX_BYTES = 56000
SUMMARY_HEADER = "=== make test の失敗の要約"
TAIL_HEADER = "=== make test の出力の末尾"

TEST_TIMEOUT_SECONDS = 30


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


def run_tool(data: bytes, max_bytes: int = MAX_BYTES) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(TOOL), "--max-bytes", str(max_bytes)],
        input=data,
        capture_output=True,
        timeout=TEST_TIMEOUT_SECONDS,
    )


def run_ok(data: bytes, max_bytes: int = MAX_BYTES) -> bytes:
    result = run_tool(data, max_bytes)
    assert result.returncode == 0, (result.returncode, result.stderr[-2000:])
    assert result.stderr == b"", result.stderr[-2000:]
    return result.stdout


def split_summary(output: bytes) -> tuple:
    """出力を要約と末尾に分ける。UTF-8 として正しく復号できることも確かめる。"""
    text = output.decode("utf-8")
    lines = text.split("\n")
    assert lines[0].startswith(SUMMARY_HEADER), lines[0][:200]
    tail_index = next(
        (i for i, line in enumerate(lines) if line.startswith(TAIL_HEADER)), None
    )
    assert tail_index is not None, "末尾の見出しの行が無い"
    return "\n".join(lines[1:tail_index]), "\n".join(lines[tail_index + 1 :])


def cargo_ok_block(binary: str, count: int) -> str:
    lines = [
        f"     Running tests/{binary}.rs (target/debug/deps/{binary}-0123456789abcdef)",
        "",
        f"running {count} tests",
    ]
    lines += [f"test {binary}::case_{i:05d}_passes_without_any_problem ... ok" for i in range(count)]
    lines += [
        "",
        f"test result: ok. {count} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s",
        "",
    ]
    return "\n".join(lines) + "\n"


def cargo_ok_output(min_bytes: int) -> str:
    blocks = []
    size = 0
    index = 0
    while size < min_bytes:
        block = cargo_ok_block(f"ok_binary_{index}", 200)
        blocks.append(block)
        size += len(block.encode("utf-8"))
        index += 1
    return "".join(blocks)


def cargo_failed_block(failures: list, passed: int = 1) -> str:
    """failures は (テスト名, 表明のメッセージ, 行番号) の列。"""
    lines = [
        "     Running unittests src/lib.rs (target/debug/deps/backend-0123456789abcdef)",
        "",
        f"running {passed + len(failures)} tests",
        "test tests::the_test_harness_runs ... ok",
    ]
    lines += [f"test {name} ... FAILED" for name, _, _ in failures]
    lines += ["", "failures:", ""]
    for name, message, line in failures:
        lines += [
            f"---- {name} stdout ----",
            "",
            f"thread '{name}' (12345) panicked at src/lib.rs:{line}:5:",
            f"assertion `left == right` failed: {message}",
            "  left: 1",
            " right: 2",
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
            "",
        ]
    lines += ["", "failures:"]
    lines += [f"    {name}" for name, _, _ in failures]
    lines += [
        "",
        f"test result: FAILED. {passed} passed; {len(failures)} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s",
        "",
        "error: test failed, to rerun pass `-p backend --lib`",
    ]
    return "\n".join(lines) + "\n"


def compile_log() -> str:
    return "   Compiling backend v0.1.0 (/work/backend)\n    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.00s\n"


def npm_header() -> str:
    return "\n> web@0.1.0 test\n> node --test --test-reporter=spec\n\n"


def npm_failing_item(file_line: str, name: str, message: str, stack_lines: int = 3) -> str:
    lines = [
        f"test at {file_line}",
        f"✖ {name} (0.39ms)",
        f"  AssertionError [ERR_ASSERTION]: {message}",
        "",
        "  1 !== 2",
        "",
    ]
    lines += [
        f"      at TestContext.<anonymous> (file:///work/frontend/web/{file_line}:{i})"
        for i in range(stack_lines)
    ]
    lines += [
        "    {",
        "      generatedMessage: false,",
        "      code: 'ERR_ASSERTION',",
        "      actual: 1,",
        "      expected: 2,",
        "      operator: 'strictEqual'",
        "    }",
        "",
    ]
    return "\n".join(lines) + "\n"


def npm_counts(tests: int, passed: int, failed: int) -> str:
    return (
        f"ℹ tests {tests}\nℹ suites 0\nℹ pass {passed}\nℹ fail {failed}\n"
        "ℹ cancelled 0\nℹ skipped 0\nℹ todo 0\nℹ duration_ms 120.5\n"
    )


# --- 失敗が無い入力 ---------------------------------------------------------


def test_success_output_is_byte_identical_tail() -> None:
    data = (compile_log() + cargo_ok_output(70000)).encode("utf-8")
    assert len(data) > MAX_BYTES + 10000
    output = run_ok(data)
    assert output == data[-MAX_BYTES:], (len(output), output[:200])


def test_small_success_output_is_passed_through() -> None:
    data = (compile_log() + cargo_ok_block("small", 3)).encode("utf-8")
    assert run_ok(data) == data


def test_failed_word_in_passing_names_is_not_a_failure() -> None:
    body = cargo_ok_output(60000) + (
        "     Running unittests src/lib.rs (target/debug/deps/backend-0123456789abcdef)\n\n"
        "running 3 tests\n"
        "test tests::shows_FAILED_badge ... ok\n"
        "test tests::reports_failed_state ... ok\n"
        "failures: 0 件の表示を確かめる\n"
        "failures:\n"
        "test tests::counts_failures ... ok\n\n"
        "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"
    )
    data = body.encode("utf-8")
    output = run_ok(data)
    assert SUMMARY_HEADER.encode("utf-8") not in output
    assert output == data[-MAX_BYTES:]


def test_npm_running_lines_and_warnings_are_not_a_failure() -> None:
    body = cargo_ok_output(60000) + npm_header() + (
        "(node:1) ExperimentalWarning: Transform Types is an experimental feature and might change at any time\n"
        "(Use `node --trace-warnings ...` to show where the warning was created)\n"
        "✔ ok one (0.3ms)\n"
        "✖ flaky display label (1ms)\n"
        "(node:1) [MODULE_TYPELESS_PACKAGE_JSON] Warning: Module type of file:///x.ts is not specified\n"
    ) + npm_counts(2, 2, 0)
    data = body.encode("utf-8")
    output = run_ok(data)
    assert SUMMARY_HEADER.encode("utf-8") not in output
    assert output == data[-MAX_BYTES:]


# --- cargo の失敗 -----------------------------------------------------------


def test_cargo_early_failure_survives_large_following_output() -> None:
    failed = cargo_failed_block([("tests::early_fails", "表明のメッセージ", 4)])
    data = (compile_log() + failed + cargo_ok_output(60000)).encode("utf-8")
    output = run_ok(data)
    assert len(output) <= MAX_BYTES, len(output)
    summary, tail = split_summary(output)
    assert "tests::early_fails" in summary, summary[:2000]
    assert "panicked at src/lib.rs:4:5" in summary, summary[:2000]
    assert "表明のメッセージ" in summary, summary[:2000]
    assert "test result: FAILED. 1 passed; 1 failed;" in summary, summary[:2000]
    # 末尾の切り取りも残す。入力の最後の行が出力の最後に来る。
    last_line = data.decode("utf-8").rstrip("\n").split("\n")[-1]
    assert tail.rstrip("\n").endswith(last_line), tail[-300:]


def test_first_failures_section_header_is_not_a_test_name() -> None:
    failed = cargo_failed_block([("tests::a", "a の表明", 7)])
    data = (failed + cargo_ok_output(60000)).encode("utf-8")
    summary, _ = split_summary(run_ok(data))
    stripped = [line.strip() for line in summary.split("\n")]
    assert stripped.count("tests::a") == 1, [line for line in stripped if "tests::a" in line]
    assert "---- tests::a stdout ----" not in stripped, summary[:2000]


def test_every_failed_cargo_name_is_listed() -> None:
    names = [(f"tests::fails_{i}", f"表明 {i}", 10 + i) for i in range(5)]
    data = (cargo_failed_block(names) + cargo_ok_output(60000)).encode("utf-8")
    summary, _ = split_summary(run_ok(data))
    stripped = [line.strip() for line in summary.split("\n")]
    for name, message, _ in names:
        assert name in stripped, (name, summary[:2000])
        assert message in summary, (message, summary[:2000])
    assert "test result: FAILED. 1 passed; 5 failed;" in summary


def test_huge_assertion_body_keeps_its_panicked_line() -> None:
    # 失敗したバイナリ自体の本文が上限を超える場合（Debug 表示が巨大な表明など）。
    huge = "x" * 120000
    failed = cargo_failed_block([("tests::huge_debug", huge, 9)])
    data = failed.encode("utf-8")
    output = run_ok(data)
    assert len(output) <= MAX_BYTES, len(output)
    summary, _ = split_summary(output)
    assert "panicked at src/lib.rs:9:5" in summary, summary[:500]
    assert "tests::huge_debug" in [line.strip() for line in summary.split("\n")]
    assert "test result: FAILED. 1 passed; 1 failed;" in summary


def test_many_failures_stay_within_bound_without_losing_names() -> None:
    names = [
        (f"tests::many_{i:03d}", "日本語の表明の本文" * 120, 100 + i) for i in range(200)
    ]
    tail_filler = "末尾の出力に含まれる多バイト文字" * 3000
    data = (cargo_failed_block(names) + tail_filler).encode("utf-8")
    output = run_ok(data)
    assert len(output) <= MAX_BYTES, len(output)
    summary, _ = split_summary(output)  # 文字の途中で切っていれば復号で失敗する
    stripped = [line.strip() for line in summary.split("\n")]
    for name, _, _ in names:
        assert name in stripped, name
    assert "test result: FAILED. 1 passed; 200 failed;" in summary
    assert "省略" in summary, "表明の本文を省略したことを示す行が無い"


def test_bound_is_respected_for_smaller_max_bytes() -> None:
    names = [(f"tests::small_{i}", "表明" * 50, i) for i in range(10)]
    data = (cargo_failed_block(names) + cargo_ok_output(30000)).encode("utf-8")
    output = run_ok(data, max_bytes=8000)
    assert len(output) <= 8000, len(output)
    summary, _ = split_summary(output)
    stripped = [line.strip() for line in summary.split("\n")]
    for name, _, _ in names:
        assert name in stripped, name


# --- 小さい上限（issue #184） -----------------------------------------------

def test_issue_repro_line_stays_within_bound() -> None:
    # issue #184 の再現入力。失敗の集計の行だけの入力でも上限を守る。
    data = b"test result: FAILED. 0 passed; 1 failed\n"
    for max_bytes in (1, 100, 200):
        output = run_ok(data, max_bytes=max_bytes)
        assert len(output) <= max_bytes, (max_bytes, len(output))
        output.decode("utf-8")  # 文字の途中で切れていれば例外になる
    # 100 バイトでは短い形式になり、失敗の集計の行が残る。
    output = run_ok(data, max_bytes=100)
    assert b"test result: FAILED." in output, output


def test_tiny_max_bytes_stays_within_bound_for_cargo_failures() -> None:
    failed = cargo_failed_block([("tests::early_fails", "表明のメッセージ", 4)])
    data = (compile_log() + failed + cargo_ok_output(60000)).encode("utf-8")
    for max_bytes in (1, 100, 200):
        output = run_ok(data, max_bytes=max_bytes)
        assert len(output) <= max_bytes, (max_bytes, len(output))
        output.decode("utf-8")


def test_tiny_max_bytes_stays_within_bound_for_npm_failures() -> None:
    body = (
        npm_header()
        + "✖ bad one (0.39ms)\n"
        + npm_counts(2, 1, 1)
        + "\n✖ failing tests:\n\n"
        + npm_failing_item("test/x.test.ts:4:1", "bad one", "一致しない")
    )
    data = body.encode("utf-8")
    for max_bytes in (1, 100, 200):
        output = run_ok(data, max_bytes=max_bytes)
        assert len(output) <= max_bytes, (max_bytes, len(output))
        output.decode("utf-8")



# --- 中間の上限（issue #205） -------------------------------------------------

def test_mid_range_max_bytes_keeps_failed_names_via_short_form() -> None:
    # issue #205: 210〜820 バイトでは、省略の行のための予約のために通常の形式が
    # 失敗したテスト名を 1 件も入れられず、見出しと成功した末尾だけが出ていた。
    # 短い形式へ切り替わり、失敗したテスト名と集計の行が残ることを確かめる。
    failed = cargo_failed_block([("tests::early_fails", "表明のメッセージ", 4)])
    data = (compile_log() + failed + cargo_ok_output(60000)).encode("utf-8")
    for max_bytes in (300, 500, 820):
        output = run_ok(data, max_bytes=max_bytes)
        assert len(output) <= max_bytes, (max_bytes, len(output))
        text = output.decode("utf-8")
        assert "tests::early_fails" in text, (max_bytes, text)
        assert "test result: FAILED. 1 passed; 1 failed;" in text, (max_bytes, text)
        assert TAIL_HEADER not in text, (max_bytes, text)


def test_mid_range_lower_bound_shows_failure_result_line() -> None:
    # 範囲の下限の 210 バイトでも短い形式になり、失敗の集計の行が残る。
    # 成功した末尾だけが出て、失敗が見えなくなることはない。
    failed = cargo_failed_block([("tests::early_fails", "表明のメッセージ", 4)])
    data = (compile_log() + failed + cargo_ok_output(60000)).encode("utf-8")
    output = run_ok(data, max_bytes=210)
    assert len(output) <= 210, len(output)
    text = output.decode("utf-8")
    assert "test result: FAILED. 1 passed; 1 failed;" in text, text
    assert TAIL_HEADER not in text, text


def test_mid_range_max_bytes_keeps_npm_failed_names() -> None:
    body = (
        npm_header()
        + "✔ ok one (0.3ms)\n✖ bad one (0.39ms)\n"
        + npm_counts(2, 1, 1)
        + "\n✖ failing tests:\n\n"
        + npm_failing_item("test/x.test.ts:4:1", "bad one", "一致しない")
        + "✔ tail ok\n" * 300
    )
    data = body.encode("utf-8")
    for max_bytes in (210, 500, 820):
        output = run_ok(data, max_bytes=max_bytes)
        assert len(output) <= max_bytes, (max_bytes, len(output))
        text = output.decode("utf-8")
        assert "✖ bad one" in text, (max_bytes, text)
        assert "ℹ fail 1" in text, (max_bytes, text)


# --- npm の失敗 -------------------------------------------------------------


def test_npm_failing_section_is_summarized_without_stack() -> None:
    body = (
        cargo_ok_output(60000)
        + npm_header()
        + "✔ ok one (0.3ms)\n✖ bad one (0.39ms)\n"
        + npm_counts(2, 1, 1)
        + "\n✖ failing tests:\n\n"
        + npm_failing_item("test/x.test.ts:4:1", "bad one", "一致しない")
    )
    data = body.encode("utf-8")
    output = run_ok(data)
    assert len(output) <= MAX_BYTES, len(output)
    summary, _ = split_summary(output)
    assert "✖ bad one" in summary, summary[:2000]
    assert "test at test/x.test.ts:4:1" in summary, summary[:2000]
    assert "AssertionError [ERR_ASSERTION]: 一致しない" in summary, summary[:2000]
    assert "ℹ fail 1" in summary, summary[:2000]
    assert "at TestContext" not in summary, summary[:2000]


def test_first_npm_failure_survives_long_later_failures() -> None:
    later = "".join(
        npm_failing_item(f"test/later.test.ts:{i}:1", f"later failure {i}", f"後続の失敗 {i}", 40)
        for i in range(60)
    )
    body = (
        npm_header()
        + npm_counts(61, 0, 61)
        + "\n✖ failing tests:\n\n"
        + npm_failing_item("test/first.test.ts:2:1", "first bad", "最初の失敗")
        + later
    )
    data = body.encode("utf-8")
    assert len(data) > MAX_BYTES
    output = run_ok(data)
    assert len(output) <= MAX_BYTES, len(output)
    summary, _ = split_summary(output)
    assert "✖ first bad" in summary, summary[:2000]
    assert "AssertionError [ERR_ASSERTION]: 最初の失敗" in summary, summary[:2000]
    assert "ℹ fail 61" in summary, summary[:2000]


# --- 終了コード -------------------------------------------------------------


def test_pipefail_keeps_make_test_exit_code() -> None:
    failed = cargo_failed_block([("tests::early_fails", "表明のメッセージ", 4)])
    script = (
        f"bash -o pipefail -c \"(cat; exit 101) | '{sys.executable}' '{TOOL}' --max-bytes {MAX_BYTES}\""
    )
    result = subprocess.run(
        ["bash", "-c", script],
        input=failed.encode("utf-8"),
        capture_output=True,
        timeout=TEST_TIMEOUT_SECONDS,
    )
    assert result.returncode == 101, (result.returncode, result.stderr[-2000:])
    assert b"tests::early_fails" in result.stdout


def test_tool_exits_zero_on_failure_input() -> None:
    failed = cargo_failed_block([("tests::early_fails", "表明のメッセージ", 4)])
    result = run_tool(failed.encode("utf-8"))
    assert result.returncode == 0, (result.returncode, result.stderr[-2000:])


def main() -> int:
    tests = [
        test_success_output_is_byte_identical_tail,
        test_small_success_output_is_passed_through,
        test_failed_word_in_passing_names_is_not_a_failure,
        test_npm_running_lines_and_warnings_are_not_a_failure,
        test_cargo_early_failure_survives_large_following_output,
        test_first_failures_section_header_is_not_a_test_name,
        test_every_failed_cargo_name_is_listed,
        test_huge_assertion_body_keeps_its_panicked_line,
        test_many_failures_stay_within_bound_without_losing_names,
        test_bound_is_respected_for_smaller_max_bytes,
        test_issue_repro_line_stays_within_bound,
        test_tiny_max_bytes_stays_within_bound_for_cargo_failures,
        test_tiny_max_bytes_stays_within_bound_for_npm_failures,
        test_mid_range_max_bytes_keeps_failed_names_via_short_form,
        test_mid_range_lower_bound_shows_failure_result_line,
        test_mid_range_max_bytes_keeps_npm_failed_names,
        test_npm_failing_section_is_summarized_without_stack,
        test_first_npm_failure_survives_long_later_failures,
        test_pipefail_keeps_make_test_exit_code,
        test_tool_exits_zero_on_failure_input,
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
