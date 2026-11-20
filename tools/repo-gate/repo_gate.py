#!/usr/bin/env python3
"""リポジトリの土台（配置・ワークスペース・機械ゲートの定義）を検査する関門。

使い方:
    python3 tools/repo-gate/repo_gate.py --root .

検査の内訳:
  workspace-layout     ルートの Cargo.toml がワークスペースで、メンバーが backend と
                       frontend/cli だけである。backend は lib、frontend/cli は bin を
                       持つ。frontend/web は package.json を持ち、.proto を置く
                       proto/ がある。`cargo metadata` の結果と設定ファイルから導出する。
  machine-gate         Makefile の done が fmt と clippy と test と typecheck と lint を
                       すべて実行し、node_modules が無いときに依存を導入する手順を持つ。
                       関門のコマンドを実際に走らせ、終了コードで成否を導出する。
  ci-runs-make-done    .github/workflows/ の各 CI が make done を呼び、Rust の stable と
                       nightly（rustfmt）と Node をセットアップする。
  workflow-machine-gate taktdash-change の machine_gate が mode: system で、
                       proto-check・fmt・clippy・test・typecheck・lint の6検査を
                       system_inputs の command として実行し、各検査の make ターゲットを
                       呼ぶ。6検査の exit_code == 0 の同時成立で次のステップへ進み、
                       検査ごとの失敗は対応する修正ステップへ回して machine_gate へ戻る。
                       公開ステップ（publish.py）は任意で、置く場合は commit / push / pr /
                       status の 4 段階を実行し、commit / push / pr の成功だけを完了条件に
                       し、失敗は再試行または修正へ回し、ABORT へは遷移しない。公開ステップを
                       置かない場合は machine_gate の成功が aislop_gate へ直結する。status
                       だけを実行するステップは公開とみなさない。aislop の受け入れ一覧
                       （scripts/aislop-accepted.json）の各エントリが reason に解消 issue の
                       URL を含み、各 finding が maxLines を持ち、対象ファイルの実測行数が
                       maxLines 以下であることも確かめる。
  gitignore-rules      .gitignore が Rust と Node とローカルデータの生成物を除外する。
  stable-toolchain     rust-toolchain.toml のチャネルが stable のままである。

各検査の実装は、同じディレクトリの gate_*.py に役割ごとに置く。このファイルは
引数の解析、検査の呼び出し、結果の出力だけを受け持つ。

終了コード:
  0  すべての検査に通った。stdout に検査名と結論の JSON を出す
  1  1 件以上が違反した。stderr に検査名と違反の理由と根拠を出す

根拠の扱い:
  宣言ファイルの設定は、キーと値の意味（メンバー、ターゲットの種類、scripts 名、
  コマンド文字列、遷移先）で判定する。Rust と Next.js の骨格をこの検査へ丸写ししない。
  機械ゲートの成否は、関門のコマンド自体の終了コードから導出する。
"""
import argparse
import json
import sys
from pathlib import Path

from gate_aislop import invokes_aislop_check, runs_aislop_gate
from gate_ci import check_ci
from gate_common import GateError, run_command, tomllib
from gate_frontend_style import check_frontend_style
from gate_makefile import check_machine_gate, install_commands, is_install_guard, parse_makefile
from gate_publish import publish_stages
from gate_repository import check_gitignore, check_toolchain, check_workspace_layout
from gate_workflow import check_workflow_machine_gate

# 外部コマンド（cargo metadata と make done）を実行する関数。テストはこの名前を
# 差し替えて外部コマンドを呼ばずに検査する。run_checks は呼び出しのたびにこの名前を
# 解決して各検査へ渡すため、差し替えは cargo metadata と make done の両方に効く。
_run = run_command

# 入口として残す名前と、repo_gate から参照される検査部品の名前。
__all__ = [
    "main",
    "run_checks",
    "_run",
    "tomllib",
    "invokes_aislop_check",
    "runs_aislop_gate",
    "install_commands",
    "is_install_guard",
    "parse_makefile",
    "publish_stages",
]


def run_checks(root, make_timeout_seconds, skip_make):
    checks = [
        ("workspace-layout", lambda: check_workspace_layout(root, _run)),
        ("ci-runs-make-done", lambda: check_ci(root)),
        ("workflow-machine-gate", lambda: check_workflow_machine_gate(root)),
        ("gitignore-rules", lambda: check_gitignore(root)),
        ("stable-toolchain", lambda: check_toolchain(root)),
        ("frontend-style", lambda: check_frontend_style(root, GateError)),
    ]
    if skip_make:
        checks.append(("machine-gate", lambda: {"make_done": "skipped"}))
    else:
        checks.append(("machine-gate", lambda: check_machine_gate(root, make_timeout_seconds, _run)))

    results = []
    failures = []
    for name, check in checks:
        try:
            detail = check()
        except GateError as error:
            failures.append(f"{name}: {error}")
            results.append({"name": name, "ok": False, "reason": str(error)})
        else:
            results.append({"name": name, "ok": True, "detail": detail})
    return results, failures


def main(argv):
    parser = argparse.ArgumentParser(description="リポジトリの土台を検査する関門")
    parser.add_argument("--root", default=".", help="リポジトリのルート")
    parser.add_argument(
        "--skip-make",
        action="store_true",
        help="make done の実行を省き、宣言ファイルの構造だけを検査する",
    )
    parser.add_argument(
        "--make-timeout",
        type=int,
        default=1800,
        help="make done の実行に許す秒数（既定は 1800）",
    )
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    if not root.is_dir():
        print(f"リポジトリのルートがありません: {root}", file=sys.stderr)
        return 2

    results, failures = run_checks(root, args.make_timeout, args.skip_make)
    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1
    print(json.dumps({"root": str(root), "checks": results}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
