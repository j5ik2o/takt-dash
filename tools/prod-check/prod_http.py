#!/usr/bin/env python3
"""prod-check の HTTP 読み取り部分。

通常の要求と、状態配信（SSE）の最初のイベントの読み取りを行う。
"""

import json
import urllib.error
import urllib.request


def http(method, url, timeout=15, headers=None, data=None):
    req = urllib.request.Request(url, method=method, headers=headers or {}, data=data)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as res:
            return res.status, res.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:
        raise ConnectionError(str(e))


def fetch_state(base_url, timeout=20):
    """状態配信の最初のイベントを読み、JSON として返す。"""
    req = urllib.request.Request(base_url + "/api/state", method="GET")
    with urllib.request.urlopen(req, timeout=timeout) as res:
        # read(65536) は指定した大きさまで待ち続けることがあるため、届いた
        # 分だけを返す read1 で読み、最初のイベントの区切りで止める。
        read_chunk = getattr(res, "read1", None) or (lambda n: res.read(n))
        buf = b""
        while b"\n\n" not in buf and len(buf) < 4_000_000:
            chunk = read_chunk(65536)
            if not chunk:
                break
            buf += chunk
    for line in buf.decode("utf-8", errors="replace").splitlines():
        if line.startswith("data: "):
            return json.loads(line[len("data: "):])
    raise ValueError("状態配信に data 行がありません")
