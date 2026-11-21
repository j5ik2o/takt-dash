"""書き込み境界の検査で使う、接続先とは別ホストの Origin を組み立てる補助。

受信側（`frontend/web/app/api/decisions/_shared.ts`）は Origin を `new URL()` で
解析し、解析できない値もホスト不一致も 403 にする。解析できない値ではホスト
比較に到達しないため、IPv4 に接頭辞を付ける形は検査にならない。
"""
from urllib.parse import urlsplit

# 別オリジンに使う試験用ホスト名。`.invalid` は RFC 2606 の予約名で実在しない。
FOREIGN_ORIGIN_HOST = "takt-dash-prod-check.invalid"


def foreign_origin(base_url):
    """URL として解析でき、接続先とは別のホストを持つ Origin を作る。

    解析に失敗する値を返さないよう、ホスト名の定数にはスキームを含めない。
    予約名を使い、スキームとポートだけ接続先から引き継ぐ。
    """
    parts = urlsplit(base_url)
    if (parts.hostname or "").lower() == FOREIGN_ORIGIN_HOST:
        raise ValueError("接続先が試験用ホスト名と同じため別オリジンを作れません: {}".format(base_url))
    port = ":{}".format(parts.port) if parts.port else ""
    return "{}://{}{}".format(parts.scheme or "http", FOREIGN_ORIGIN_HOST, port)
