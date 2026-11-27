#!/usr/bin/env python3
"""prod_log.py の Web のログの読み方を、一時ディレクトリのログで確かめる。

使い方:
    python3 tools/prod-check/prod_log_test.py

設計: docs/design/issue-409.md。確かめる観点:
  1. 時刻の無い Web のログでは、log_excerpts の出力が main `7f986d3` と 1 文字も違わない
     （backend のログがあってもなくても）。
  2. split_stamp は行頭の時刻（ミリ秒・マイクロ秒・小数なし）と空白 1 つだけを外す。
     時刻だけの行は本文が空。形の崩れた時刻は外さない。読めない日付は外して時刻を None。
  3. 時刻だけ違う同じエラーは 1 組になり、別のエラーも枠に入る。表示は backend と同じ形。
  4. 時刻の数字（.500Z）はエラーの判定に入らない。
  5. backend の起動の時刻以上の最初の行から切り出す。時刻が戻っても後ろの行は落とさない。
     切り出せる行が無ければ、最後の Ready からの切り出しに戻る。
  6. 性質 P1: 本文が同じなら、時刻の付け替え・付け足し・外しで結果が変わらない。
  7. 性質 P2: web_error_groups は設計の規則を書き下した参照の実装と一致する。

本物の本番のログは読まない。すべて一時ディレクトリで完結する。
"""
import os
import random
import re
import signal
import sys
import tempfile
import traceback
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import prod_log  # noqa: E402

TEST_TIMEOUT_SECONDS = 30
PROPERTY_SEED = 409
PROPERTY_CASES = 400

BACKEND_LOG = """\
2026-11-26T22:48:01.736884Z  INFO takt_dash_backend: gRPC サーバーを起動します addr=0.0.0.0:50051
2026-11-26T22:48:31.100000Z  INFO takt_dash_backend: 定期読み取りの1サイクルを終えました。 projects_read=20 projects_failed=0 events_recorded=3
2026-11-26T22:49:00.200000Z  WARN takt_dash_backend: 道具の呼び出しが時間切れになりました tool=list_tasks elapsed_ms=5000
2026-11-26T22:49:30.300000Z  WARN takt_dash_backend: 道具の呼び出しが時間切れになりました tool=list_goals elapsed_ms=5001
2026-11-26T22:50:00.400000Z ERROR takt_dash_backend: 読み取りに失敗しました project=alpha
"""

UNSTAMPED_WEB_LOG = """\
   ▲ Next.js 15.5.0
 ✓ Ready in 1200ms
Error: fetch failed
    at async GET (/app/route.js:10:5)
Error: fetch failed
 ⚠ ERR_STREAM_PREMATURE_CLOSE
GET /api/state 500
 ✓ Ready in 900ms
Error: fetch failed
TypeError: Cannot read properties of undefined (reading 'x')
GET /api/state 500
GET /api/state 500
[taktDash][timing] render ms=12
"""

# main 7f986d3 の log_excerpts の出力（上の 2 つのログから）。
MAIN_BACKEND_PART = [
    "--- backend の直近のサイクル集計（直近 5 件）",
    "2026-11-27 07:48:31 JST INFO takt_dash_backend: 定期読み取りの1サイクルを終えました。"
    " projects_read=20 projects_failed=0 events_recorded=3",
    "--- backend の WARN と ERROR（同じ文言をまとめ）",
    "WARN takt_dash_backend: 道具の呼び出しが時間切れになりました tool=… elapsed_ms=…"
    "（2 件、最初 2026-11-27 07:49:00 JST、最後 2026-11-27 07:49:30 JST）",
    "ERROR takt_dash_backend: 読み取りに失敗しました project=…"
    "（1 件、最初 2026-11-27 07:50:00 JST、最後 2026-11-27 07:50:00 JST）",
    "--- backend の起動",
    "2026-11-27 07:48:01 JST INFO takt_dash_backend: gRPC サーバーを起動します addr=0.0.0.0:50051",
]
MAIN_WEB_PART = [
    "--- Web のエラー（最後の Ready 以降、同じ行をまとめ）",
    "Error: fetch failed",
    "TypeError: Cannot read properties of undefined (reading 'x')",
    "GET /api/state 500（2 件）",
]
READY_HEADER = MAIN_WEB_PART[0]
TIME_HEADER = "--- Web のエラー（backend の起動以降、同じ本文をまとめ）"

# backend の起動は 2026-11-26T22:48:01.736884Z（JST 07:48:01）。
STARTED = datetime(2026, 11, 26, 22, 48, 1, 736884, tzinfo=timezone.utc)


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


def excerpts(web_text, backend_text=None):
    with tempfile.TemporaryDirectory() as root:
        data = os.path.join(root, "data")
        os.makedirs(data)
        if backend_text is not None:
            with open(os.path.join(data, "backend-launchd.err.log"), "w") as f:
                f.write(backend_text)
        if web_text is not None:
            with open(os.path.join(data, "web-launchd.err.log"), "w") as f:
                f.write(web_text)
        return prod_log.log_excerpts(root, [])


def web_part(out):
    for i, line in enumerate(out):
        if line.startswith("--- Web のエラー"):
            return out[i:]
    return []


def utc(text):
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


# 1. 時刻の無いログは今と同じ


def test_unstamped_web_log_with_backend_is_unchanged():
    assert excerpts(UNSTAMPED_WEB_LOG, BACKEND_LOG) == MAIN_BACKEND_PART + MAIN_WEB_PART


def test_unstamped_web_log_without_backend_is_unchanged():
    assert excerpts(UNSTAMPED_WEB_LOG) == MAIN_WEB_PART


def test_backend_part_is_unchanged_by_stamped_web_log():
    stamped = "".join("2026-11-26T22:50:00.500Z " + l + "\n" for l in UNSTAMPED_WEB_LOG.splitlines())
    out = excerpts(stamped, BACKEND_LOG)
    assert out[: len(MAIN_BACKEND_PART)] == MAIN_BACKEND_PART, out


# 2. split_stamp


def test_split_stamp_milliseconds():
    assert prod_log.split_stamp("2026-11-27T00:00:00.500Z Error: x") == (
        utc("2026-11-27T00:00:00.500Z"), "Error: x")


def test_split_stamp_microseconds():
    assert prod_log.split_stamp("2026-11-26T22:48:01.736884Z ERROR y") == (
        utc("2026-11-26T22:48:01.736884Z"), "ERROR y")


def test_split_stamp_without_fraction():
    assert prod_log.split_stamp("2026-11-27T00:00:00Z ok") == (utc("2026-11-27T00:00:00Z"), "ok")


def test_split_stamp_removes_only_one_space():
    assert prod_log.split_stamp("2026-11-27T00:00:00.500Z  ✓ Ready in 1s") == (
        utc("2026-11-27T00:00:00.500Z"), " ✓ Ready in 1s")


def test_split_stamp_stamp_only_line_has_empty_body():
    assert prod_log.split_stamp("2026-11-27T00:00:00.500Z") == (utc("2026-11-27T00:00:00.500Z"), "")


def test_split_stamp_stamp_and_space_has_empty_body():
    assert prod_log.split_stamp("2026-11-27T00:00:00.500Z ") == (utc("2026-11-27T00:00:00.500Z"), "")


def test_split_stamp_removes_only_the_first_stamp():
    assert prod_log.split_stamp("2026-11-27T00:00:00.500Z 2026-11-27T00:00:01.000Z x") == (
        utc("2026-11-27T00:00:00.500Z"), "2026-11-27T00:00:01.000Z x")


def test_split_stamp_leaves_unstamped_line():
    for line in ["", "Error: x", "   ▲ Next.js 15.5.0", "GET /api/state 500"]:
        assert prod_log.split_stamp(line) == (None, line), line


def test_split_stamp_leaves_malformed_stamp():
    for line in [
        "2026-11-27T00:00:00.Z Error",
        "2026-11-27 00:00:00.500Z Error",
        "2026-11-27T00:00:00.500 Error",
        "2026-11-27T00:00:00.500ZError",
        "2026-11-27T0:00:00.500Z Error",
        " 2026-11-27T00:00:00.500Z Error",
    ]:
        assert prod_log.split_stamp(line) == (None, line), line


def test_split_stamp_unparseable_date_removes_stamp_without_time():
    assert prod_log.split_stamp("2026-13-45T99:99:99.500Z Compiled") == (None, "Compiled")


# 3. 時刻だけ違う同じエラーは 1 組


def test_same_error_with_different_stamps_is_one_group():
    lines = [" ✓ Ready in 1s"]
    lines += ["Error: fetch failed"] * 12
    lines += ["TypeError: x is undefined"]
    base = utc("2026-11-27T00:00:00.000Z")
    stamped = "".join(
        (base + timedelta(seconds=i)).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z " + l + "\n"
        for i, l in enumerate(lines))
    assert web_part(excerpts(stamped)) == [
        READY_HEADER,
        "Error: fetch failed（12 件、最初 2026-11-27 09:00:01 JST、最後 2026-11-27 09:00:12 JST）",
        "TypeError: x is undefined（1 件、最初 2026-11-27 09:00:13 JST、最後 2026-11-27 09:00:13 JST）",
    ]


def test_eleven_distinct_errors_with_repeats_fill_ten_slots_by_body():
    lines = []
    for i in range(11):
        for s in range(3):
            lines.append("2026-11-27T00:%02d:%02d.000Z Error: e%d" % (i, s, i))
    out = web_part(excerpts("\n".join(lines) + "\n"))
    assert out[0] == READY_HEADER
    assert [l.split("（")[0] for l in out[1:]] == ["Error: e%d" % i for i in range(10)], out
    assert all("（3 件、" in l for l in out[1:]), out


def test_group_mixing_stamped_and_unstamped_lines_counts_both():
    text = "Error: x\n2026-11-27T00:00:05.000Z Error: x\nError: x\n"
    assert web_part(excerpts(text)) == [
        READY_HEADER,
        "Error: x（3 件、最初 2026-11-27 09:00:05 JST、最後 2026-11-27 09:00:05 JST）",
    ]


# 4. 時刻の数字はエラーにならない


def test_stamp_digits_do_not_make_error():
    text = (
        "2026-11-27T00:00:00.500Z  ✓ Compiled /api/state in 120ms\n"
        "2026-11-27T00:00:01.500Z [taktDash][timing] render ms=12\n"
        "2026-11-27T00:05:00.500Z\n"
        "2026-11-27T00:05:00.500Z \n"
    )
    assert web_part(excerpts(text)) == []
    assert excerpts(text) == []


def test_ready_inside_stamp_is_impossible_and_body_ready_is_used():
    text = (
        "2026-11-27T00:00:00.000Z Error: before\n"
        "2026-11-27T00:00:01.000Z  ✓ Ready in 1s\n"
        "2026-11-27T00:00:02.000Z Error: after\n"
    )
    assert web_part(excerpts(text)) == [
        READY_HEADER,
        "Error: after（1 件、最初 2026-11-27 09:00:02 JST、最後 2026-11-27 09:00:02 JST）",
    ]


# 5. 時刻での切り出し（backend の起動 = 2026-11-26T22:48:01.736884Z）


def test_cut_starts_at_first_line_at_or_after_backend_start():
    text = (
        "2026-11-26T22:40:00.000Z  ✓ Ready in 1s\n"
        "2026-11-26T22:48:01.000Z Error: old backend\n"
        "2026-11-26T22:48:02.000Z Error: before ready\n"
        "2026-11-26T22:48:03.000Z  ✓ Ready in 1s\n"
        "2026-11-26T22:48:04.000Z Error: after ready\n"
    )
    assert web_part(excerpts(text, BACKEND_LOG)) == [
        TIME_HEADER,
        "Error: before ready（1 件、最初 2026-11-27 07:48:02 JST、最後 2026-11-27 07:48:02 JST）",
        "Error: after ready（1 件、最初 2026-11-27 07:48:04 JST、最後 2026-11-27 07:48:04 JST）",
    ]


def test_line_at_exactly_backend_start_is_included():
    text = "2026-11-26T22:48:01.736884Z Error: same instant\n"
    assert web_part(excerpts(text, BACKEND_LOG)) == [
        TIME_HEADER,
        "Error: same instant（1 件、最初 2026-11-27 07:48:01 JST、最後 2026-11-27 07:48:01 JST）",
    ]


def test_clock_going_back_after_cut_keeps_lines():
    text = (
        "2026-11-26T22:47:00.000Z Error: excluded\n"
        "2026-11-26T22:49:00.000Z Error: first\n"
        "2026-11-26T22:30:00.000Z Error: clock went back\n"
        "Error: unstamped after cut\n"
    )
    assert web_part(excerpts(text, BACKEND_LOG)) == [
        TIME_HEADER,
        "Error: first（1 件、最初 2026-11-27 07:49:00 JST、最後 2026-11-27 07:49:00 JST）",
        "Error: clock went back（1 件、最初 2026-11-27 07:30:00 JST、最後 2026-11-27 07:30:00 JST）",
        "Error: unstamped after cut",
    ]


def test_no_line_in_window_falls_back_to_last_ready():
    # ラッパーを外して戻したあと: 時刻付きの行はすべて起動より前、尾は時刻が無い。
    text = (
        "2026-11-26T22:00:00.000Z  ✓ Ready in 1s\n"
        "2026-11-26T22:01:00.000Z Error: stamped old\n"
        " ✓ Ready in 900ms\n"
        "Error: unstamped tail\n"
    )
    assert web_part(excerpts(text, BACKEND_LOG)) == [READY_HEADER, "Error: unstamped tail"]


def test_unparseable_stamp_does_not_start_cut():
    text = (
        " ✓ Ready in 1s\n"
        "2026-13-45T99:99:99.500Z Error: bad date\n"
    )
    assert web_part(excerpts(text, BACKEND_LOG)) == [READY_HEADER, "Error: bad date"]


def test_web_error_groups_since_none_uses_ready():
    lines = ["Error: a", "2026-11-27T00:00:00.000Z  ✓ Ready", "2026-11-27T00:00:01.000Z Error: b"]
    assert prod_log.web_error_groups(lines) == (
        False, [("Error: b", 1, utc("2026-11-27T00:00:01.000Z"), utc("2026-11-27T00:00:01.000Z"))])


def test_web_error_groups_first_and_last_follow_line_order():
    lines = [
        "2026-11-27T00:00:09.000Z Error: a",
        "2026-11-27T00:00:01.000Z Error: a",
        "Error: a",
    ]
    since = utc("2026-11-27T00:00:00.000Z")
    assert prod_log.web_error_groups(lines, since) == (
        True, [("Error: a", 3, utc("2026-11-27T00:00:09.000Z"), utc("2026-11-27T00:00:01.000Z"))])


# 6, 7. 性質（random の種つき）

REF_STAMP_RE = re.compile(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z)(?: |$)")
REF_ERROR_RE = re.compile(r"error|Error|ERR_|500")
PIECES = [
    "Error: fetch failed", "error", "ERR_X", "GET /api/state 500", " ✓ Ready in 1s", "Ready",
    "   ▲ Next.js 15.5.0", "", " ", "  ", "ok", "読み取りに失敗しました", "x500y", "readY",
    "2026-11-27T00:00:00.500Z", "2026-11-27T00:00:00.500Z body", "2026-11-27T00:00:00.500Z ",
    "2026-13-45T99:99:99.500Z x", "2026-11-27T00:00:00.Z x", "2026-11-27T00:00:00Z", "\t", "Z", "T",
]
ALPHABET = "aErRoyd5 0Z T:-.✓\t日"


def ref_split(line):
    m = REF_STAMP_RE.match(line)
    if not m:
        return None, line
    try:
        t = datetime.fromisoformat(m.group(1).replace("Z", "+00:00"))
    except ValueError:
        t = None
    return t, line[m.end():]


def ref_groups(lines, since):
    parsed = [ref_split(l) for l in lines]
    start = None
    if since is not None:
        start = next((i for i, (t, _) in enumerate(parsed) if t is not None and t >= since), None)
    by_time = start is not None
    if not by_time:
        start = max((i for i, (_, b) in enumerate(parsed) if "Ready" in b), default=0)
    groups = {}
    for t, body in parsed[start:]:
        if not REF_ERROR_RE.search(body):
            continue
        g = groups.setdefault(body, [0, None, None])
        g[0] += 1
        if t is not None:
            if g[1] is None:
                g[1] = t
            g[2] = t
    return by_time, [(b, c, f, l) for b, (c, f, l) in groups.items()]


def random_body(rng):
    parts = []
    for _ in range(rng.randint(1, 3)):
        if rng.random() < 0.6:
            parts.append(rng.choice(PIECES))
        else:
            parts.append("".join(rng.choice(ALPHABET) for _ in range(rng.randint(0, 10))))
    return "".join(parts).replace("\n", "")


def random_stamp(rng):
    kind = rng.random()
    if kind < 0.05:
        return "2026-13-45T99:99:99.%03dZ" % rng.randint(0, 999)
    t = datetime(2026, 11, 26, tzinfo=timezone.utc) + timedelta(
        microseconds=rng.randint(0, 2 * 86400 * 10**6))
    if kind < 0.45:
        return t.strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"
    if kind < 0.85:
        return t.strftime("%Y-%m-%dT%H:%M:%S.%fZ")
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def render(rng, body):
    """本文 body を持つ行を 1 つ作る。時刻なしは、行が時刻で始まらない本文のときだけ同じ本文になる。"""
    if REF_STAMP_RE.match(body) is None and rng.random() < 0.4:
        return body
    return random_stamp(rng) + " " + body


def random_since(rng, lines):
    kind = rng.random()
    if kind < 0.2:
        return None
    stamped = [t for t, _ in map(ref_split, lines) if t is not None]
    if stamped and kind < 0.5:
        return rng.choice(stamped)
    return datetime(2026, 11, 26, tzinfo=timezone.utc) + timedelta(
        microseconds=rng.randint(0, 2 * 86400 * 10**6))


def bodies_and_counts(result):
    by_time, groups = result
    return by_time, [(g[0], g[1]) for g in groups]


def test_property_p1_stamps_do_not_change_grouping():
    rng = random.Random(PROPERTY_SEED)
    for case in range(PROPERTY_CASES):
        bodies = [random_body(rng) for _ in range(rng.randint(0, 25))]
        a = [render(rng, b) for b in bodies]
        b = [render(rng, b) for b in bodies]
        got_a = bodies_and_counts(prod_log.web_error_groups(a))
        got_b = bodies_and_counts(prod_log.web_error_groups(b))
        assert got_a == got_b, "case %d\n%r\n%r\n%r\n%r" % (case, a, b, got_a, got_b)
        assert got_a == bodies_and_counts(ref_groups(a, None)), "case %d %r" % (case, a)


def test_property_p2_matches_reference_rules():
    rng = random.Random(PROPERTY_SEED + 1)
    for case in range(PROPERTY_CASES):
        lines = [render(rng, random_body(rng)) for _ in range(rng.randint(0, 25))]
        since = random_since(rng, lines)
        got = prod_log.web_error_groups(lines, since)
        want = ref_groups(lines, since)
        assert got == want, "case %d since=%r\n%r\n%r\n%r" % (case, since, lines, got, want)


def main() -> int:
    tests = [
        test_unstamped_web_log_with_backend_is_unchanged,
        test_unstamped_web_log_without_backend_is_unchanged,
        test_backend_part_is_unchanged_by_stamped_web_log,
        test_split_stamp_milliseconds,
        test_split_stamp_microseconds,
        test_split_stamp_without_fraction,
        test_split_stamp_removes_only_one_space,
        test_split_stamp_stamp_only_line_has_empty_body,
        test_split_stamp_stamp_and_space_has_empty_body,
        test_split_stamp_removes_only_the_first_stamp,
        test_split_stamp_leaves_unstamped_line,
        test_split_stamp_leaves_malformed_stamp,
        test_split_stamp_unparseable_date_removes_stamp_without_time,
        test_same_error_with_different_stamps_is_one_group,
        test_eleven_distinct_errors_with_repeats_fill_ten_slots_by_body,
        test_group_mixing_stamped_and_unstamped_lines_counts_both,
        test_stamp_digits_do_not_make_error,
        test_ready_inside_stamp_is_impossible_and_body_ready_is_used,
        test_cut_starts_at_first_line_at_or_after_backend_start,
        test_line_at_exactly_backend_start_is_included,
        test_clock_going_back_after_cut_keeps_lines,
        test_no_line_in_window_falls_back_to_last_ready,
        test_unparseable_stamp_does_not_start_cut,
        test_web_error_groups_since_none_uses_ready,
        test_web_error_groups_first_and_last_follow_line_order,
        test_property_p1_stamps_do_not_change_grouping,
        test_property_p2_matches_reference_rules,
    ]
    signal.signal(signal.SIGALRM, _on_timeout)
    failures = 0
    for test in tests:
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
