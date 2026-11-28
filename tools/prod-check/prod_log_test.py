#!/usr/bin/env python3
"""prod_log.py の Web のログの読み方を、一時ディレクトリのログで確かめる。

使い方:
    python3 tools/prod-check/prod_log_test.py

設計: docs/design/issue-409.md、docs/design/issue-421.md。確かめる観点:
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
  8. #421: 500 は HTTP の状態の形（行頭・空白・`(`・`[`・`"`・`'` の直後、または
     status=・code= の形）のときだけエラー。500ms・1500ms・5000・500.81・
     duration_ms=500 などの数の形はエラーにしない。error・Error・ERR_ は今までどおり。
  9. #421: 性質 P3（数の形だけの行はエラーにしない）と P4（HTTP の 500 の形を含む行は
     必ずエラー）。残る誤検知（`{"duration_ms": 500}` など）は設計が範囲外と決めているので
     固定しない。
  10. #421: 参照の実装（P2）も新しい規則に合わせる。
  11. #445: 参照の実装は本番の正規表現を写さず、規則の言葉から文字の比べで書く。参照が設計の
     例どおりに判定することを固定の例で確かめ、P2 は 500 の前後に境界の文字を置いた行も作る。

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


# 4b. #421: 500 は HTTP の状態の形のときだけエラー

# 設計 docs/design/issue-421.md の「当たる例」。
HTTP_500_BODIES = [
    "GET /api/state 500",
    "500 Internal Server Error",
    "HTTP/1.1 500",
    "HTTP/1.1 500:",
    "500:",
    "status=500",
    "statusCode=500",
    "http_status=500",
    "status_code=500",
    "status: 500",
    "STATUS=500",
    "Status: 500",
    "code=500",
    '{"status":500}',
    '"status": "500"',
    "(500)",
    "[500]",
    "(500",
    "500)",
    "500]",
    "500}",
    '500"',
    "500'",
    "500,",
    "500;",
    "500, …",
    "... status 500.",
    "500.",
    "500.foo",
    "\t500",
    "500",
    "500 and 500",
    "status:500",
    "Code=500",
    '"code": 500',
    "'status'=500",
    'status"=500',
    'status" =500',
    "status = 500",
    "500\t",
    "500.あ",
    "500. ",
]

# 設計の「当たらない例」と、そこから規則どおりに導ける数の形。
NON_HTTP_500_BODIES = [
    "500ms",
    "1500ms",
    "5000ms",
    "5000",
    "5001",
    "1500",
    "500.81",
    "0.500s",
    "duration_ms=500",
    "duration_ms=500.81",
    "ms=500",
    "MS=500",
    "1,500",
    "x500y",
    "500件",
    "500500",
    "a500",
    "500a",
    "500=500",
    "status=5000",
    "status=500ms",
    "status = 5000",
    '{"duration_ms":500}',
    '{"count":500}',
    '{"ms":500}',
    " ✓ Ready in 500ms",
    "Ready in 1500ms",
    "[taktDash][timing] render ms=500",
    "render duration_ms=500.81",
    "2026-11-27T00:00:00.500Z",
    "{500}",
    "=500",
    ":500",
    ",500",
    "_500",
    "500(",
    "500[",
    "500{",
    "500=",
    "500_",
    "500.5",
    "status500",
    "status==500",
    "status:=500",
    'status""=500',
    'status "=500',
    "code:5000",
    "status=500.81",
]


def is_web_error(body):
    """本文 1 つの行が、Web のエラーとして組に入るか。"""
    return bool(prod_log.web_error_groups([body])[1])


def test_http_500_shapes_are_errors():
    for body in HTTP_500_BODIES:
        assert is_web_error(body), body


def test_500_lookalikes_are_not_errors():
    for body in NON_HTTP_500_BODIES:
        assert not is_web_error(body), body


def test_error_words_are_still_errors():
    for body in [
        "error", "Error", "ERR_X", "an error occurred",
        "Error: fetch failed", "http_error=1",
    ]:
        assert is_web_error(body), body


def test_unstamped_web_log_keeps_http_500_output():
    assert "GET /api/state 500（2 件）" in excerpts(UNSTAMPED_WEB_LOG)


def test_timing_500_lines_after_ready_are_not_errors():
    text = (
        " ✓ Ready in 500ms\n"
        " ✓ Ready in 1500ms\n"
        "[taktDash][timing] render ms=500\n"
        "[taktDash][timing] render duration_ms=500.81\n"
        "GET /api/state 500\n"
        "GET /api/state 500\n"
        "Error: fetch failed\n"
    )
    assert web_part(excerpts(text)) == [
        READY_HEADER,
        "GET /api/state 500（2 件）",
        "Error: fetch failed",
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

# #421 の規則（docs/design/issue-421.md の「規則」）を、本番の正規表現を写さずに文字の比べで
# 書き下す。空白は str.isspace、数字は str.isdecimal（正規表現の \s・\d と同じ集合）。
# 大文字小文字を問わない比べは、1 文字ずつの casefold（ſ も s と同じに扱う。Unicode の全文字で
# re の大文字小文字を問わない照合と一致することを確かめた）。
REF_ERROR_WORDS = ("error", "Error", "ERR_")
REF_BEFORE_500 = "([\"'"
REF_AFTER_500 = ")]}\"',:;"
REF_STATUS_KEYS = ("status", "code")


def ref_skip_space_back(body, j):
    while j > 0 and body[j - 1].isspace():
        j -= 1
    return j


def ref_status_key_ends_at(body, i):
    """body[:i] が「status か code（大文字小文字は問わない）、任意の引用符 1 つ、空白、
    : か = を 1 つ、空白」で終わるか。キーの前に区切りは要らない（http_status・zipcode も当たる）。"""
    j = ref_skip_space_back(body, i)
    if j == 0 or body[j - 1] not in ":=":
        return False
    j = ref_skip_space_back(body, j - 1)
    if j > 0 and body[j - 1] in "\"'":
        j -= 1
    return any(
        j >= len(k) and all(c.casefold() == kc for c, kc in zip(body[j - len(k):j], k))
        for k in REF_STATUS_KEYS
    )


def ref_500_before_ok(body, i):
    if i == 0 or body[i - 1].isspace() or body[i - 1] in REF_BEFORE_500:
        return True
    return ref_status_key_ends_at(body, i)


def ref_500_after_ok(body, k):
    if k == len(body) or body[k].isspace() or body[k] in REF_AFTER_500:
        return True
    return body[k] == "." and not (k + 1 < len(body) and body[k + 1].isdecimal())


def ref_is_error(body):
    if any(w in body for w in REF_ERROR_WORDS):
        return True
    return any(
        body.startswith("500", i) and ref_500_before_ok(body, i) and ref_500_after_ok(body, i + 3)
        for i in range(len(body))
    )


# 設計の「当たる例」「当たらない例」と、範囲外として残す誤検知（当たる側）。
DESIGN_HIT_EXAMPLES = [
    "GET /api/state 500", "500 Internal Server Error", "HTTP/1.1 500", "HTTP/1.1 500:", "500:",
    "status=500", "statusCode=500", "http_status=500", "status_code=500", "status: 500", "(500)",
    "[500]", '{"status":500}', "... status 500.", "500.", "500, …", "500",
    '{"duration_ms": 500}', '{"zipcode":500}', '{"barcode":500}', '{"decode":500}', '{"count":"500"}',
    # 大文字小文字を問わない形の Unicode の例（ſ は s の大文字小文字の違いとして扱う）。
    "ſtatus=500", "STATUſ=500",
]
DESIGN_MISS_EXAMPLES = [
    '{"duration_ms":500}', '{"count":500}', '{"ms":500}', "500ms", "1500ms", "5000", "5001",
    "500.81", "0.500s", "duration_ms=500", "ms=500", "1,500", "x500y", "500件",
]


def test_reference_follows_design_examples():
    for body in DESIGN_HIT_EXAMPLES + HTTP_500_BODIES:
        assert ref_is_error(body), body
    for body in DESIGN_MISS_EXAMPLES + NON_HTTP_500_BODIES:
        assert not ref_is_error(body), body

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
        if not ref_is_error(body):
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


# P2 用。500 の前後に、規則の境界の両側の文字と status・code の形を置く。
P2_HEADS = [
    "", "a", "x", "_", "日", "1", "0.", "{", "GET /api/state ", "HTTP/1.1 ", "render ",
    "status", "Status", "STATUS", "code", "Code", "statusCode", "http_status", "zipcode",
    "stat", "ode", "ſtatus", "ßtatus", "duration_ms", "ms", "count", '{"status', '{"duration_ms', "'code",
]
P2_QUOTES = ["", "", '"', "'", '""', " "]
P2_SEPS = ["", "", ":", "=", "==", ":=", ",", "{"]
P2_BEFORE = ["", "", " ", "  ", "\t", "(", "[", '"', "'", "{", ":", "=", ",", ".", "a", "0", "日"]
P2_AFTER = [
    "", " ", "\t", ")", "]", "}", '"', "'", ",", ":", ";", ".", ".81", ".5", ".x", "..", ". ",
    "(", "[", "{", "=", "_", "ms", " ms", "0", "1", "500", "a", "件", "s",
]


def random_500_body(rng):
    head = rng.choice(P2_HEADS) + rng.choice(P2_QUOTES) + rng.choice(P2_SEPS) + rng.choice(P2_BEFORE)
    return head + "500" + rng.choice(P2_AFTER) + rng.choice(["", "", random_body(rng)])


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
        lines = [
            render(rng, random_500_body(rng) if rng.random() < 0.5 else random_body(rng))
            for _ in range(rng.randint(0, 25))
        ]
        since = random_since(rng, lines)
        got = prod_log.web_error_groups(lines, since)
        want = ref_groups(lines, since)
        assert got == want, "case %d since=%r\n%r\n%r\n%r" % (case, since, lines, got, want)


# #421 の性質 P3・P4。埋め草は、数字・=・r・R を使わない（エラーの語と HTTP の
# 500 の形を偶然に作らないため。性質の定義域で、除外ではない）。
P3_FILLER = (
    " abcdefghijklmnopqstuvwxyzABCDEFGHIJKLMNOPQSTUVWXYZ"
    "あいうえおかきくけこさしすせそ✓▲.-:;[](){}'\"\t"
)
P4_FILLER = (
    "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
    " あいうえお日本語✓▲.-:;[](){}<>\"'\\/|=,_\t"
)


def p3_number_shape(rng):
    """数の形を 1 つ作る。単位や小数点以下が付く形では n に 500 を含めてよい。"""
    kind = rng.randrange(8)
    if kind == 7:
        # 裸の数だけは 500 を除く（1500・5000 のように 500 の前後に数字が続く形は含む）。
        n = rng.choice([0, 1, 12, 999, 1000, 1200, 1500, 5000, 5001, rng.randint(0, 10**6)])
        if n == 500:
            n = 1500
        return str(n)
    n = rng.choice([500, 0, 1, 12, 999, 1000, 1200, 1500, 5000, 5001, rng.randint(0, 10**6)])
    d = rng.choice([0, 1, 5, 81, 999, rng.randint(0, 999)])
    if kind == 0:
        return "%dms" % n
    if kind == 1:
        return "%d.%dms" % (n, d)
    if kind == 2:
        return "in %dms" % n
    if kind == 3:
        return "%d.%ds" % (n, d)
    if kind == 4:
        return "duration_ms=%d" % n
    if kind == 5:
        return "duration_ms=%d.%d" % (n, d)
    return "ms=%d" % n


def p3_line(rng):
    head = "".join(rng.choice(P3_FILLER) for _ in range(rng.randint(0, 12)))
    tail = "".join(rng.choice(P3_FILLER) for _ in range(rng.randint(0, 12)))
    return head + p3_number_shape(rng) + tail


def p4_line(rng):
    head = "".join(rng.choice(P4_FILLER) for _ in range(rng.randint(0, 15)))
    tail = "".join(rng.choice(P4_FILLER) for _ in range(rng.randint(0, 15)))
    return head + " " + rng.choice(HTTP_500_BODIES) + " " + tail


def test_property_p3_number_shapes_never_error():
    rng = random.Random(PROPERTY_SEED + 3)
    for case in range(PROPERTY_CASES):
        lines = [p3_line(rng) for _ in range(rng.randint(1, 20))]
        got = prod_log.web_error_groups(lines)
        assert got == (False, []), "case %d\n%r\n%r" % (case, lines, got)


def test_property_p4_http_500_always_error():
    rng = random.Random(PROPERTY_SEED + 4)
    for case in range(PROPERTY_CASES):
        line = p4_line(rng)
        got = prod_log.web_error_groups([line])
        assert [g[0] for g in got[1]] == [line], "case %d\n%r\n%r" % (case, line, got)


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
        test_http_500_shapes_are_errors,
        test_500_lookalikes_are_not_errors,
        test_error_words_are_still_errors,
        test_unstamped_web_log_keeps_http_500_output,
        test_timing_500_lines_after_ready_are_not_errors,
        test_cut_starts_at_first_line_at_or_after_backend_start,
        test_line_at_exactly_backend_start_is_included,
        test_clock_going_back_after_cut_keeps_lines,
        test_no_line_in_window_falls_back_to_last_ready,
        test_unparseable_stamp_does_not_start_cut,
        test_web_error_groups_since_none_uses_ready,
        test_web_error_groups_first_and_last_follow_line_order,
        test_property_p1_stamps_do_not_change_grouping,
        test_reference_follows_design_examples,
        test_property_p2_matches_reference_rules,
        test_property_p3_number_shapes_never_error,
        test_property_p4_http_500_always_error,
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
