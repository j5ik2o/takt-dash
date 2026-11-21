#!/usr/bin/env python3
"""taktDash の本番を検証する。

使い方:
    python3 tools/prod-check/prod_check.py [--base-url http://127.0.0.1:3000]
    make prod-check BASE_URL=http://10.0.1.143:3000

不具合が修正されたかどうかを、このスクリプトの合否で判定する。検査は
次のとおり。

  - 版の一致: 画面の frontend と backend のコミットが一致し、状態の
    buildInfo とも一致し、稼働中のコミットが展開先の HEAD とも一致
    すること。buildInfo が無ければ確認できずにする。
  - main の展開: 展開先の HEAD が origin/main を含むこと。版が一致して
    いても、修正が未展開なら不合格にする。origin の取得に失敗したら、
    古い参照では判定せず確認できずにする。
  - データの鮮度: 状態配信の readAt とプロジェクトごとの lastReadAt が
    読み取り間隔の 2 倍以内で、切断や遅延のプロジェクトが無いこと。
    最終読み取り時刻が無いプロジェクトは、まだ読めていないとして
    不合格にする。
  - 読み取りサイクル: backend の起動後のサイクルを集計し、成功率と、
    起動・初期化の失敗（道具の呼び出しが 0 件の失敗サイクル）と道具ごと
    の時間切れを分けて数えること。計測の項目が無いサイクルの行（main の
    backend が出す形）も数え、その場合は道具の内訳は不明として扱う。
  - ページ: 主要ページが 200 で、エラーの文言が無いこと。
  - 書き込みの境界: 空の本文の POST が、同じオリジンで 400（入力検証）、
    別のオリジンで 403、text/plain で 415 になること。空の本文は route の
    入力検証で止まり backend へ届かないため、本番の状態は変わらない。
  - 設定の縮退（警告のみ）: プロジェクト数と読み取り間隔が期待値と違う
    ことを警告として出す。合否には影響しない。

失敗した検査があれば、backend と Web のログから対応する抜粋を報告へ
含める。全文は出力先のフォルダへ書き、報告には場所だけを載せる。

終了コード: 0 = 合格、1 = 不合格、2 = 確認できず（合格でも不合格でも
ない検査がある）。make 経由では GNU make がレシピの失敗を一律に 2 へ
変えるため、終了コードで判定するジョブはこのスクリプトを直接実行する
こと。
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from prod_http import fetch_state, http  # noqa: E402
from prod_log import (  # noqa: E402
    JST,
    backend_started_at,
    cycles_since_start,
    fmt_jst,
    log_excerpts,
    parse_iso,
    read_backend_log,
)

ERROR_WORDS = ("backend に接続できません", "応答なし", "コミットが不一致です")
WRITE_PATHS = (
    "/api/decisions/answer",
    "/api/decisions/merge",
    "/api/decisions/goal-registration/approve",
    "/api/decisions/goal-registration/reject",
)

PASS, FAIL, UNKNOWN, WARN = "OK", "NG", "UNKNOWN", "WARN"


class Result:
    def __init__(self, name, status, detail):
        self.name = name
        self.status = status
        self.detail = detail


def now_jst():
    return datetime.now(JST)


def check_versions(base_url, head=""):
    name = "版の一致"
    try:
        _, body = http("GET", base_url + "/")
        html = body.decode("utf-8", errors="replace")
    except ConnectionError as e:
        return Result(name, UNKNOWN, "トップページへ接続できません: %s" % e)
    m_front = re.search(r"frontend\W{0,24}?([0-9a-f]{7,40})\b", html)
    m_back = re.search(r"backend\W{0,24}?([0-9a-f]{7,40})\b", html)
    if not m_front or not m_back:
        return Result(name, UNKNOWN, "サイドバーのコミット表示が見つかりません")
    front, back = m_front.group(1), m_back.group(1)
    try:
        state = fetch_state(base_url)
        build = (state.get("buildInfo") or {}).get("commit", "")
    except Exception as e:
        return Result(name, UNKNOWN, "状態配信を読めません: %s" % e)
    if front != back:
        return Result(name, FAIL, "frontend %s と backend %s が一致しません" % (front, back))
    if not build:
        return Result(name, UNKNOWN,
                      "状態の buildInfo にコミットがありません。三者の一致を確認できません")
    if build != back:
        return Result(name, FAIL, "backend %s と状態の buildInfo %s が一致しません" % (back, build))
    if head and not (head.startswith(back) or back.startswith(head)):
        return Result(name, FAIL,
                      "稼働中の backend %s が展開先の HEAD %s と違います。再起動が要ります" % (back, head))
    return Result(name, PASS, "frontend と backend はともに %s" % back)


def check_main_deployed(deploy_dir):
    name = "main の展開"
    def git(*args):
        return subprocess.run(
            ["git", "-C", deploy_dir] + list(args),
            capture_output=True, text=True, timeout=60,
        )
    try:
        fetched = git("fetch", "origin", "-q")
        if fetched.returncode != 0:
            return Result(
                name, UNKNOWN,
                "origin の取得に失敗したため、古い参照では判定しません: %s"
                % fetched.stderr.strip()[:200],
            ), ""
        head = git("rev-parse", "--short", "HEAD").stdout.strip()
        main = git("rev-parse", "--short", "origin/main").stdout.strip()
        r = git("merge-base", "--is-ancestor", "origin/main", "HEAD")
    except Exception as e:
        return Result(name, UNKNOWN, "git で確認できません: %s" % e), ""
    if r.returncode != 0:
        return Result(
            name, FAIL,
            "展開先 %s は origin/main %s を含みません。修正が未展開です" % (head, main),
        ), head
    return Result(name, PASS, "展開先 %s は origin/main %s を含みます" % (head, main)), head


def check_freshness(base_url, interval):
    name = "データの鮮度"
    try:
        state = fetch_state(base_url)
    except Exception as e:
        return Result(name, UNKNOWN, "状態配信を読めません: %s" % e), None
    limit = interval * 2 + 30
    problems = []
    read_at = parse_iso(state.get("readAt", ""))
    if read_at is None:
        problems.append("readAt がありません")
    else:
        age = (datetime.now(timezone.utc) - read_at).total_seconds()
        if age > limit:
            problems.append("readAt が %d 秒前です（上限 %d 秒）" % (age, limit))
    for project in state.get("projects", []):
        last = parse_iso(project.get("lastReadAt", ""))
        state_name = project.get("connectionState", "") or ""
        # proto の JSON では false や未設定の項目が省略されるため、connected が
        # 無いこと自体は切断とみなさない。状態の名前は CONNECTION_STATE_
        # UNRESPONSIVE が実際の値で、UNAVAILABLE では検出できない。
        bad_state = any(k in state_name for k in ("UNRESPONSIVE", "UNAVAILABLE", "DELAYED"))
        if project.get("connected") is False or bad_state:
            problems.append("%s は %s です" % (project.get("name"), state_name or "未接続"))
        elif last is None:
            problems.append("%s の最終読み取り時刻がありません（まだ読めていません）"
                            % project.get("name"))
        else:
            age = (datetime.now(timezone.utc) - last).total_seconds()
            if age > limit:
                problems.append("%s の最終読み取りが %d 秒前です" % (project.get("name"), age))
    if problems:
        return Result(name, FAIL, "。".join(problems)), state
    return Result(name, PASS, "全プロジェクトの読み取りが %d 秒以内に収まっています" % limit), state


def check_cycles(deploy_dir):
    name = "読み取りサイクル"
    log_text, _ = read_backend_log(deploy_dir)
    if log_text is None:
        return Result(name, UNKNOWN, "backend のログがありません"), None
    cycles = cycles_since_start(log_text)
    if len(cycles) < 3:
        return Result(name, UNKNOWN, "起動後のサイクルが %d 件しかありません" % len(cycles)), cycles
    total = len(cycles)
    succeeded = sum(1 for c in cycles if c["projects_failed"] == 0)
    spawn_failed = sum(1 for c in cycles if c["projects_failed"] > 0 and c["tool_calls"] == 0)
    timeouts = sum(c["tool_timeouts"] or 0 for c in cycles)
    unmeasured = sum(1 for c in cycles if c["tool_calls"] is None)
    rate = succeeded * 100 // total
    detail = "起動後の %d サイクル中 %d 件が成功（%d%%）。起動・初期化の失敗 %d 件、道具の時間切れ %d 件" % (
        total, succeeded, rate, spawn_failed, timeouts)
    if unmeasured:
        detail += "。道具の内訳が無いサイクル %d 件を含みます" % unmeasured
    if spawn_failed > 0:
        return Result(name, FAIL, detail + "。起動・初期化の失敗が残っています"), cycles
    if rate < 80:
        return Result(name, FAIL, detail), cycles
    return Result(name, PASS, detail), cycles


def check_pages(base_url, state, main_checkout):
    name = "ページ"
    pages = [("概要", "/"), ("プロジェクト", "/projects"), ("判断待ち", "/decisions"),
             ("アクティビティ", "/activity")]
    if state:
        projects = state.get("projects", [])
        goals = state.get("goals", [])
        if projects:
            first = projects[0].get("name", "")
            pages.append(("会話", "/conversations?project=" + first))
        names = [p.get("name", "") for p in projects]
        if "takt-dash" in names:
            task_name = first_task_name(main_checkout)
            if task_name:
                pages.append(("タスク詳細", "/tasks/takt-dash?task=" + task_name))
        if goals:
            pages.append(("ゴール詳細", "/goals/" + goals[0].get("goalId", "")))
    runs_dir = os.path.join(main_checkout, ".takt", "runs")
    if os.path.isdir(runs_dir):
        slugs = sorted(os.listdir(runs_dir))
        if slugs:
            pages.append(("run 詳細", "/runs/" + slugs[-1]))
    problems = []
    checked = []
    for label, path in pages:
        try:
            status, body = http("GET", base_url + path, timeout=30)
        except ConnectionError as e:
            if problems:
                return Result(name, FAIL, "。".join(problems)
                              + "。その後の %s へ接続できませんでした（%s）" % (label, e))
            return Result(name, UNKNOWN, "%s へ接続できません（%s）" % (label, e))
        text = body.decode("utf-8", errors="replace")
        if status != 200:
            problems.append("%s が %d を返しました" % (label, status))
            continue
        hit = [w for w in ERROR_WORDS if w in text]
        if hit:
            problems.append("%s にエラーの文言「%s」があります" % (label, hit[0]))
            continue
        checked.append(label)
    if problems:
        return Result(name, FAIL, "。".join(problems))
    return Result(name, PASS, "%s の %d ページが 200 でエラーの文言がありません" % ("、".join(checked), len(checked)))


def first_task_name(main_checkout):
    path = os.path.join(main_checkout, ".takt", "tasks.yaml")
    try:
        with open(path, errors="replace") as f:
            for line in f:
                m = re.match(r"\s+name:\s*(\S+)\s*$", line)
                if m:
                    return m.group(1)
    except OSError:
        return None
    return None


def check_write_boundary(base_url):
    name = "書き込みの境界"
    origin = base_url
    problems = []
    for path in WRITE_PATHS:
        try:
            status, _ = http("POST", base_url + path, timeout=10,
                             headers={"Origin": origin, "Content-Type": "application/json"},
                             data=b"{}")
            if status == 403:
                problems.append("%s が同じオリジンで 403 です（オリジン検査の不具合）" % path)
            elif status != 400:
                problems.append("%s が同じオリジンで %s です（期待は 400）" % (path, status))
            status, _ = http("POST", base_url + path, timeout=10,
                             headers={"Origin": "https://example.invalid",
                                      "Content-Type": "application/json"},
                             data=b"{}")
            if status != 403:
                problems.append("%s が別オリジンで %s です（期待は 403）" % (path, status))
            status, _ = http("POST", base_url + path, timeout=10,
                             headers={"Origin": origin, "Content-Type": "text/plain"},
                             data=b"{}")
            if status != 415:
                problems.append("%s が text/plain で %s です（期待は 415）" % (path, status))
        except ConnectionError as e:
            if problems:
                return Result(name, FAIL, "。".join(problems)
                              + "。その後の書き込み経路へ接続できませんでした: %s" % e)
            return Result(name, UNKNOWN, "書き込み経路へ接続できません: %s" % e)
    if problems:
        return Result(name, FAIL, "。".join(problems))
    return Result(name, PASS, "4 経路とも同じオリジンで 400、別オリジンで 403、text/plain で 415 です")


def check_config(deploy_dir, expect_projects, expect_interval):
    """設定の縮退を警告として返し、あわせて実際の読み取り間隔を返す。"""
    name = "設定の縮退"
    path = os.path.join(deploy_dir, "taktdash.config.json")
    try:
        with open(path) as f:
            config = json.load(f)
    except Exception as e:
        return Result(name, UNKNOWN, "設定ファイルを読めません: %s" % e), None
    count = len(config.get("projects", []))
    interval = config.get("intervalSeconds")
    notes = []
    if count != expect_projects:
        notes.append("プロジェクトが %d 件です（期待は %d 件）" % (count, expect_projects))
    if interval != expect_interval:
        notes.append("読み取り間隔が %s 秒です（期待は %d 秒）" % (interval, expect_interval))
    if notes:
        return Result(name, WARN, "。".join(notes)), interval
    return Result(name, PASS, "プロジェクト %d 件、間隔 %d 秒です" % (count, expect_interval)), interval


def check_e2e(repo_root, base_url):
    name = "e2e（本番 smoke）"
    e2e_dir = os.path.join(repo_root, "e2e")
    if not os.path.isdir(os.path.join(e2e_dir, "node_modules")):
        return Result(name, UNKNOWN, "e2e の依存が未導入です（npm --prefix e2e ci が必要）")
    env = dict(os.environ)
    env["TAKTDASH_PROD_BASE_URL"] = base_url
    try:
        r = subprocess.run(
            ["npm", "--prefix", "e2e", "run", "test:prod"],
            cwd=repo_root, env=env, capture_output=True, text=True, timeout=600,
        )
    except Exception as e:
        return Result(name, UNKNOWN, "e2e を実行できません: %s" % e)
    if r.returncode != 0:
        tail = "\n".join((r.stdout + r.stderr).splitlines()[-5:])
        return Result(name, FAIL, "prod smoke が失敗しました。\n%s" % tail)
    return Result(name, PASS, "prod smoke が通過しました")


def main():
    parser = argparse.ArgumentParser(description="taktDash の本番を検証する")
    parser.add_argument("--base-url", default="http://127.0.0.1:3000")
    parser.add_argument("--deploy-dir", default="/Users/j5ik2o/local/takt-dash-app")
    parser.add_argument("--main-checkout",
                        default="/Users/j5ik2o/Sources/j5ik2o.github.com/j5ik2o/takt-dash")
    parser.add_argument("--expect-projects", type=int, default=20)
    parser.add_argument("--expect-interval", type=int, default=30)
    parser.add_argument("--with-e2e", action="store_true")
    parser.add_argument("--out-dir", default=None)
    args = parser.parse_args()

    base_url = args.base_url.rstrip("/")
    out_dir = args.out_dir or "/tmp/prod-check-" + now_jst().strftime("%Y%m%d-%H%M%S")
    os.makedirs(out_dir, exist_ok=True)
    repo_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

    log_text, _ = read_backend_log(args.deploy_dir)
    started = backend_started_at(log_text) if log_text else None
    try:
        load = "、".join("%.2f" % v for v in os.getloadavg())
    except OSError:
        load = "不明"

    results = []
    deployed, head = check_main_deployed(args.deploy_dir)
    results.append(deployed)
    results.append(check_versions(base_url, head))
    config_result, config_interval = check_config(
        args.deploy_dir, args.expect_projects, args.expect_interval)
    interval = config_interval if config_interval is not None else args.expect_interval
    freshness, state = check_freshness(base_url, interval)
    results.append(freshness)
    cycles_result, cycles = check_cycles(args.deploy_dir)
    results.append(cycles_result)
    results.append(check_pages(base_url, state, args.main_checkout))
    results.append(check_write_boundary(base_url))
    if args.with_e2e:
        if any(r.status == FAIL for r in results):
            results.append(Result("e2e（本番 smoke）", UNKNOWN,
                                  "軽い検査で不合格があるため、Chrome を起動しませんでした"))
        else:
            results.append(check_e2e(repo_root, base_url))

    print("taktDash 本番検証 %s" % fmt_jst(now_jst()))
    print("接続先: %s、展開先: %s（HEAD %s）" % (base_url, args.deploy_dir, head or "不明"))
    print("load average: %s、backend の起動: %s" % (
        load, fmt_jst(started.astimezone(JST)) if started else "不明"))
    failed = False
    unknown = False
    for r in results:
        print("[%s] %s: %s" % (r.status, r.name, r.detail))
    if config_result.status != PASS:
        print("[%s] %s: %s" % (config_result.status, config_result.name, config_result.detail))
    for r in results:
        if r.status == FAIL:
            failed = True
        elif r.status == UNKNOWN:
            unknown = True
    if failed:
        excerpts = log_excerpts(args.deploy_dir, cycles)
        if excerpts:
            print("")
            print("ログの抜粋:")
            for line in excerpts:
                print("  " + line)
        for src, dst in (("data/backend-launchd.err.log", "backend-launchd.err.log"),
                         ("data/web-launchd.err.log", "web-launchd.err.log")):
            src_path = os.path.join(args.deploy_dir, src)
            if os.path.exists(src_path):
                shutil.copy(src_path, os.path.join(out_dir, dst))
        print("ログの全文: %s" % out_dir)
    if failed:
        return 1
    if unknown:
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
