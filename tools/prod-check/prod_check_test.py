#!/usr/bin/env python3
"""prod_check.py の書き込み境界の検査で使う別オリジンの生成を確かめる。

使い方:
    python3 tools/prod-check/prod_check_test.py

`foreign_origin` は、書き込みの境界の検査で「通ってはならないオリジン」を作る。
受信側（`frontend/web/app/api/decisions/_shared.ts`）は Origin を `new URL()` で
解析し、解析できない値・ホストが一致しない値を 403 にする。解析に失敗する値でも
403 になるが、それはホスト比較に到達していないため、検査が求める「別ホストを
拒否する」確認にはならない。

そこで、生成した Origin が次の2点を満たすことを確かめる。

  1. Node の `new URL()` で解析できる（受信側と同じ URL 解釈）
  2. 正規化後のホストが接続先と異なる

接続先は文書にある IPv4 3例（`tools/prod-check/README.md`）と、名前付きホストを
使う。接続先が IPv4 で `other-` を前置する旧方式では、いずれも解析に失敗する。
"""
import json
import subprocess
import sys
import traceback
from pathlib import Path
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parent))

from prod_check import foreign_origin  # noqa: E402

# 受信側と同じ URL 解釈で、生成値を解析できるかと正規化後のホストを Node で確かめる。
# Node は Web の実行環境であり、`new URL()` の挙動が受信側と一致する。
NODE_SCRIPT = """
const origin = process.argv[1];
try {
  const url = new URL(origin);
  process.stdout.write(JSON.stringify({ ok: true, hostname: url.hostname, port: url.port, protocol: url.protocol }));
} catch (error) {
  process.stdout.write(JSON.stringify({ ok: false, code: error.code || "ERR_INVALID_URL" }));
}
"""


def node_parse(origin):
    """Node の `new URL()` で Origin を解析した結果を返す。"""
    result = subprocess.run(
        ["node", "-e", NODE_SCRIPT, origin],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def assert_valid_foreign_origin(base_url):
    """生成した Origin が解析でき、ホストが接続先と異なることを確かめる。"""
    origin = foreign_origin(base_url)
    parsed = node_parse(origin)
    assert parsed["ok"], f"{base_url} の生成値 {origin} が URL として解析できません: {parsed}"
    target = node_parse(base_url)
    assert parsed["hostname"] != target["hostname"], (
        f"{base_url} の生成値 {origin} のホストが接続先と同じです"
    )
    return origin, parsed


def test_documented_ipv4_lan_target() -> None:
    origin, parsed = assert_valid_foreign_origin("http://10.0.1.143:3000")
    assert parsed["protocol"] == "http:", parsed
    assert parsed["port"] == "3000", parsed
    assert origin != "http://10.0.1.143:3000"


def test_documented_ipv4_tailscale_target() -> None:
    origin, parsed = assert_valid_foreign_origin("http://100.106.166.1:3000")
    assert parsed["protocol"] == "http:", parsed
    assert parsed["port"] == "3000", parsed


def test_documented_ipv4_local_target() -> None:
    origin, parsed = assert_valid_foreign_origin("http://127.0.0.1:3000")
    assert parsed["protocol"] == "http:", parsed
    assert parsed["port"] == "3000", parsed


def test_named_host_without_port_keeps_scheme_and_no_port() -> None:
    origin, parsed = assert_valid_foreign_origin("https://dash.example.com")
    assert parsed["protocol"] == "https:", parsed
    assert parsed["port"] == "", parsed


def test_origin_never_equals_the_target() -> None:
    for base_url in (
        "http://10.0.1.143:3000",
        "http://100.106.166.1:3000",
        "http://127.0.0.1:3000",
        "https://dash.example.com",
    ):
        assert foreign_origin(base_url) != base_url


def test_reserved_host_is_used_and_never_resolves_to_the_target() -> None:
    # `.invalid` は RFC 2606 の予約名で、実在のホスト名と衝突しない。
    origin = foreign_origin("http://10.0.1.143:3000")
    hostname = urlsplit(origin).hostname
    assert hostname is not None and hostname.endswith(".invalid"), origin


def test_target_equal_to_reserved_host_is_refused() -> None:
    """接続先が試験用ホスト名と同じ場合は、同一オリジンを黙って返さない。"""
    from prod_check import FOREIGN_ORIGIN_HOST

    try:
        foreign_origin("http://" + FOREIGN_ORIGIN_HOST + ":3000")
    except ValueError:
        return
    raise AssertionError("接続先が試験用ホスト名と同じ場合に例外を出していません")


def test_old_prefix_style_would_fail_parsing() -> None:
    """旧方式（IPv4 へ other- を前置）が解析に失敗することを固定する。

    この失敗は、生成値が受信側の URL 解釈で解析できず、ホスト比較より前に 403 と
    なることを示す。`foreign_origin` がこの形を返さないことを、解析成功で確かめる。
    """
    for base_url in ("http://10.0.1.143:3000", "http://100.106.166.1:3000", "http://127.0.0.1:3000"):
        parts = urlsplit(base_url)
        old = "http://other-{}{}".format(parts.hostname, ":{}".format(parts.port) if parts.port else "")
        assert not node_parse(old)["ok"], f"旧方式の {old} が解析できてしまいました"
        assert node_parse(foreign_origin(base_url))["ok"], f"{base_url} の現行の生成値が解析できません"


def main() -> int:
    tests = [
        test_documented_ipv4_lan_target,
        test_documented_ipv4_tailscale_target,
        test_documented_ipv4_local_target,
        test_named_host_without_port_keeps_scheme_and_no_port,
        test_origin_never_equals_the_target,
        test_reserved_host_is_used_and_never_resolves_to_the_target,
        test_target_equal_to_reserved_host_is_refused,
        test_old_prefix_style_would_fail_parsing,
    ]
    failures = 0
    for test in tests:
        try:
            test()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {test.__name__}: {error}")
        except Exception:  # 実装漏れや脚本の穴は、失敗として記録する
            failures += 1
            print(f"FAIL {test.__name__}:")
            traceback.print_exc()
        else:
            print(f"ok   {test.__name__}")
    print(f"{len(tests) - failures} passed, {failures} failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
