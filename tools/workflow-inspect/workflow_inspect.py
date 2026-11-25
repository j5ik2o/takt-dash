#!/usr/bin/env python3
"""解決できない workflow_call を、TAKT を流す前に見つける任意の検査。

使い方:
    python3 tools/workflow-inspect/workflow_inspect.py
    python3 tools/workflow-inspect/workflow_inspect.py taktdash-change

`.takt/workflows/*.yaml`（`.yml` も）を列挙し、`takt workflow inspect <name>` を 1 つずつ
実行する。inspect は、解決できない workflow_call を名指しでエラーにする。同じ状態でも
`takt workflow doctor` は `Workflow OK` と言うため、doctor ではこの検査の代わりにならない。

CI には入れない（CI に takt が無い）。回帰テストの代わりにもしない。TAKT を流す前に人が
確かめるための入口で、`make workflow-inspect` から呼ぶ。

引数に名前を渡すと、その名前だけを inspect する（ファイルを持たない組み込みの workflow も
指定できる）。引数が無ければ `.takt/workflows` の全件を inspect する。

takt の実体は、`TAKT_BIN` があればその実行ファイル、無ければ PATH 上の `takt` を使う。
takt が無いときは検査をスキップして終了コード 0 で終わる（手元と CI を妨げない）。

終了コード:
  0  すべての workflow を inspect できた、または takt が無くてスキップした
  1  inspect に失敗した workflow がある、または TAKT_BIN の実行ファイルが無い
  2  `.takt/workflows` に workflow の YAML が無く、inspect する対象が無い
"""
import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_DIR = Path(".takt") / "workflows"
# inspect は定義を読むだけで TAKT を流さないが、設定の解決で外のコマンドを待つことがある。
# 人が待ち続けないよう上限を置く。
INSPECT_TIMEOUT_SECONDS = 120
SKIP_MESSAGE = "takt が見つかりません。workflow の検査をスキップしました"


def workflow_names(root):
    """`.takt/workflows` の YAML から workflow 名を集める。"""
    directory = root / WORKFLOW_DIR
    paths = sorted(directory.glob("*.yaml")) + sorted(directory.glob("*.yml"))
    return [workflow_name(path) for path in paths]


def workflow_name(path):
    """YAML の先頭にある `name:` の値を返す。無ければファイル名を workflow 名とする。

    `.takt/workflows/*.yaml` は先頭行に `name:` を置く。PyYAML を使わずに読める形なので、
    標準ライブラリだけで済ませる。値の後ろのコメント（`name: taktdash-change # 説明`）は
    名前ではないため落とす。
    """
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("name:"):
            value = yaml_scalar(line.partition(":")[2])
            if value:
                return value
            break
    return path.stem


def yaml_scalar(text):
    """YAML の 1 行の値を読む。引用符を外し、行末のコメントを落とす。"""
    value = text.strip()
    if value[:1] in ("'", '"'):
        end = value.find(value[0], 1)
        if end != -1:
            return value[1:end].strip()
    return value.split(" #", 1)[0].strip()


def locate_takt():
    """takt の実体を絶対パスで返す。`TAKT_BIN` の指定を優先し、無ければ PATH から探す。

    inspect は root を子プロセスの作業ディレクトリにする。相対パスのまま渡すと、検索した
    ときと起動したときで基準が変わり、実行ファイルを見つけられない。見つけた時点で、呼び
    出したときの作業ディレクトリを基準に絶対化する。
    """
    override = os.environ.get("TAKT_BIN", "").strip()
    if override:
        path = Path(override).expanduser()
        if path.is_file():
            return str(path.absolute())
    binary = shutil.which(override or "takt")
    return None if binary is None else str(Path(binary).absolute())


def inspect(binary, name, root):
    """1 つの workflow を inspect し、名前と成否と出力を返す。"""
    command = [binary, "workflow", "inspect", name]
    try:
        proc = subprocess.run(
            command,
            cwd=str(root),
            capture_output=True,
            text=True,
            timeout=INSPECT_TIMEOUT_SECONDS,
            check=False,
        )
    except (FileNotFoundError, PermissionError) as error:
        return {"name": name, "ok": False, "output": f"{binary} を実行できません: {error}"}
    except subprocess.TimeoutExpired:
        return {
            "name": name,
            "ok": False,
            "output": f"{' '.join(command)} が {INSPECT_TIMEOUT_SECONDS} 秒以内に終わりませんでした",
        }
    output = "\n".join(part for part in (proc.stdout.strip(), proc.stderr.strip()) if part)
    if output == "":
        output = f"{' '.join(command)} は何も出力せず、終了コード {proc.returncode} で終わりました"
    return {"name": name, "ok": proc.returncode == 0, "output": output}


def run(binary, names, root):
    """workflow 名を 1 つずつ inspect し、結果を表示して終了コードを返す。"""
    results = [inspect(binary, name, root) for name in names]
    failures = [result for result in results if not result["ok"]]
    for result in results:
        if result["ok"]:
            print(f"{result['name']}: OK")
    for result in failures:
        print(f"\n{result['name']}: FAIL", file=sys.stderr)
        print(result["output"], file=sys.stderr)
    summary = f"{len(results) - len(failures)}/{len(results)} 件の workflow を inspect できました"
    if failures:
        print(
            f"{summary}。失敗: {', '.join(result['name'] for result in failures)}",
            file=sys.stderr,
        )
        return 1
    print(summary)
    return 0


def main(argv, root=None):
    parser = argparse.ArgumentParser(
        description="解決できない workflow_call を takt workflow inspect で見つける（任意の検査）"
    )
    parser.add_argument(
        "targets",
        nargs="*",
        metavar="WORKFLOW",
        help="inspect する workflow 名。省略すると .takt/workflows の全件を inspect する",
    )
    arguments = parser.parse_args(argv)
    root = PROJECT_ROOT if root is None else root

    override = os.environ.get("TAKT_BIN", "").strip()
    binary = locate_takt()
    if binary is None:
        if override:
            print(
                f"TAKT_BIN の実行ファイルが見つかりません: {override}",
                file=sys.stderr,
            )
            return 1
        print(SKIP_MESSAGE)
        return 0

    names = arguments.targets or workflow_names(root)
    if not names:
        print(
            f"{root / WORKFLOW_DIR} に workflow の YAML がありません",
            file=sys.stderr,
        )
        return 2
    return run(binary, names, root)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
