#!/usr/bin/env python3
"""通常 make done が coverage の4層80%判定を、実際に実行して失敗へ伝播することを確かめる。

使い方:
    python3 tools/coverage/coverage_wiring_test.py

観測するのは振る舞いだけである。出荷される Makefile を隔離した一時ディレクトリへ
コピーし、実 make で done を起動する。backend/cli の cargo-llvm-cov と web の c8 は、
計測器の CLI 契約（`--fail-under-lines N`、`--check-coverage --lines N`）を実際に解釈して
合否を返す代用品へ差し替える。repo-gate 層は coverage JSON を生成し、実
`coverage_threshold.py` に渡して判定させる。レシピの文字列や一致件数ではなく、次の
実行結果を観測する。

  1. 全層が80%以上のとき、backend・cli・web・repo-gate が実行され、done が終了0。
  2. 各層だけを80%未満にすると、その層の実行結果が未達になり、done が非0。
  3. 計測結果が合格でも、coverage 呼び出しが終了状態を潰されていれば未達が done へ
     伝わらない（配線の前提確認）。
  4. done から coverage が除かれていれば、層の計測が実行されない。
  5. 動作を保つ表記変更（`$(MAKE) coverage` を `$(MAKE) --no-print-directory coverage`
     にする等）では、合否が変わらない。
"""
import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SHIPPED_MAKEFILE = REPO_ROOT / "Makefile"
REAL_THRESHOLD = REPO_ROOT / "tools/coverage/coverage_threshold.py"

CARGO_TOML = '[workspace]\nresolver = "2"\nmembers = ["backend", "frontend/cli"]\n'

# 計測器の代用品。呼び出し引数を記録し、計測器の CLI 契約に沿って合否を返す。
# どの層かはパッケージ名・c8 の指定から判定し、閾値は引数の指定値を使う。
# しきい値ちょうどは合格、下回れば不合格とする（実 coverage_threshold.py と同じ整数比較）。
STUB = """#!/bin/sh
echo "$0 $@" >> "$STUB_CALLS"
case "$*" in
  *"--version"*) exit 0 ;;
  *"component add"*) exit 0 ;;
  *"npm --prefix frontend/web ci"*) exit 0 ;;
esac
layer=""
covered=""
total=""
threshold=""
case "$*" in
  *takt-dash-backend*)
    layer=backend
    covered="${COVERED_BACKEND:-8000}" ;;
  *takt-dash-cli*)
    layer=cli
    covered="${COVERED_CLI:-8000}" ;;
  *c8@12.0.0*)
    layer=web
    covered="${COVERED_WEB:-8000}" ;;
esac
if [ -n "$layer" ]; then
  total=10000
  # cargo-llvm-cov は --fail-under-lines N、c8 は --check-coverage と --lines N の
  # 両方が揃って初めて閾値判定を行う。実計測器の契約に合わせ、Web は有効化指定が
  # ない場合に閾値判定をしない。
  set -- $*
  prev=""
  check_coverage=0
  threshold=""
  for arg in "$@"; do
    case "$prev" in
      --fail-under-lines|--lines) threshold="$arg" ;;
    esac
    case "$arg" in
      --check-coverage) check_coverage=1 ;;
    esac
    prev="$arg"
  done
  if [ "$layer" = web ] && [ "$check_coverage" -ne 1 ]; then
    # --check-coverage がない c8 は判定せず成功する（実計測器と同じ）。
    exit 0
  fi
  if [ -z "$threshold" ]; then
    echo "stub: $layer の呼び出しに閾値の指定がありません" >&2
    exit 2
  fi
  # covered * 100 >= total * threshold を整数で比べる。
  lhs=$((covered * 100))
  rhs=$((total * threshold))
  if [ "$lhs" -lt "$rhs" ]; then
    echo "stub: $layer の行カバレッジが${threshold}%未満です" >&2
    exit 1
  fi
fi
exit 0
"""

RUN_PY = """#!/usr/bin/env python3
import json
import os
import subprocess
import sys

args = sys.argv[1:]
with open(os.environ["STUB_CALLS"], "a", encoding="utf-8") as handle:
    handle.write("python " + " ".join(args) + "\\n")

if args[:2] == ["-m", "pip"]:
    sys.exit(0)
if args and args[0].endswith("coverage_threshold.py"):
    # 実閾値補助へ委譲する。JSON の読込みと整数判定を実際に通す。
    sys.exit(subprocess.run([sys.executable] + args).returncode)
if args[:2] == ["-m", "coverage"]:
    action = args[2]
    if action == "run":
        sys.exit(int(os.environ.get("STUB_REPO_GATE_RUN_EXIT", "0")))
    if action == "json":
        output = args[args.index("-o") + 1] if "-o" in args else None
        if output:
            covered = int(os.environ.get("COVERED_REPO_GATE", "8000"))
            total = 10000
            with open(output, "w", encoding="utf-8") as handle:
                json.dump({"totals": {"covered_lines": covered, "num_statements": total}}, handle)
        sys.exit(0)
    sys.exit(0)
sys.exit(0)
"""


PREREQUISITE_TARGETS = {"fmt", "clippy", "test", "typecheck", "lint", "proto-gen", "proto-check"}


def strip_non_coverage_prerequisites(makefile_text):
    """done から coverage へ至る配線だけを残し、無関係な前提検査を成功へ差し替える。

    done の recipe は `$(MAKE) test` などを経由する。本テストが観測するのは
    coverage の4層判定と、その失敗が done まで伝わることだけなので、coverage に
    到達する前段の各ターゲットを空の recipe（常に成功）にする。done 自身の
    coverage 呼び出し行と coverage の recipe はそのまま残す。
    """
    lines = makefile_text.splitlines()
    result = []
    stripping = False
    for line in lines:
        header = line.rstrip()
        if header.endswith(":") and not header.startswith("\t"):
            name = header[:-1].strip()
            stripping = name in PREREQUISITE_TARGETS
            result.append(line)
            continue
        if stripping and line.startswith("\t"):
            continue
        result.append(line)
    return "\n".join(result) + "\n"


class Workspace:
    """出荷 Makefile と計測器の代用品を持つ一時リポジトリ。"""

    def __init__(self, makefile_text=None):
        self.base = Path(tempfile.mkdtemp(prefix="coverage-wiring-"))
        self.root = self.base / "repo"
        self.root.mkdir()
        self.bin = self.base / "bin"
        self.bin.mkdir()
        self.venv = self.base / "venv"
        (self.venv / "bin").mkdir(parents=True)
        self.calls = self.base / "calls.log"
        self.makefile_text = makefile_text

    def __enter__(self):
        (self.root / "Cargo.toml").write_text(CARGO_TOML, encoding="utf-8")
        for relative in ("backend/Cargo.toml", "frontend/cli/Cargo.toml"):
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text('[package]\nname = "x"\nversion = "0.1.0"\n', encoding="utf-8")
        web = self.root / "frontend/web"
        web.mkdir(parents=True)
        (web / "package.json").write_text(
            json.dumps({"name": "web", "version": "0.0.0", "scripts": {"test": "true"}}),
            encoding="utf-8",
        )
        (self.root / "proto").mkdir()
        (self.root / "proto/README.md").write_text("proto\n", encoding="utf-8")

        shipped = self.makefile_text or SHIPPED_MAKEFILE.read_text(encoding="utf-8")
        (self.root / "Makefile").write_text(
            strip_non_coverage_prerequisites(shipped).replace("target/coverage/venv", str(self.venv)),
            encoding="utf-8",
        )

        for name in ("cargo", "rustup", "npm", "npx"):
            self._write_executable(self.bin / name, STUB)
        self._write_executable(self.venv / "bin/python", RUN_PY)
        self._write_executable(self.venv / "bin/pip", STUB)
        # 実閾値補助をそのまま使う。JSON の生成から判定までを実際に通す。
        threshold_dest = self.root / "tools/coverage/coverage_threshold.py"
        threshold_dest.parent.mkdir(parents=True, exist_ok=True)
        threshold_dest.write_text(REAL_THRESHOLD.read_text(encoding="utf-8"), encoding="utf-8")
        return self

    def _write_executable(self, path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    def __exit__(self, *exc):
        shutil.rmtree(self.base, ignore_errors=True)
        return False

    def write_makefile(self, text):
        (self.root / "Makefile").write_text(
            strip_non_coverage_prerequisites(text).replace("target/coverage/venv", str(self.venv)),
            encoding="utf-8",
        )

    def run_done(self, env_overrides, target="done"):
        environment = {
            "PATH": str(self.bin) + os.pathsep + os.environ.get("PATH", ""),
            "STUB_CALLS": str(self.calls),
            "HOME": str(self.base / "home"),
            "PYTHONPATH": "",
            "PYTHONHOME": "",
        }
        environment.update(env_overrides)
        if self.calls.exists():
            self.calls.unlink()
        return subprocess.run(
            ["make", target],
            cwd=self.root,
            env=environment,
            capture_output=True,
            text=True,
            timeout=120,
        )

    def recorded_calls(self):
        if not self.calls.exists():
            return []
        return [line for line in self.calls.read_text(encoding="utf-8").splitlines() if line]


def good_env():
    return {
        "COVERED_BACKEND": "8000",
        "COVERED_CLI": "8000",
        "COVERED_WEB": "8000",
        "COVERED_REPO_GATE": "8000",
    }


def layer_calls(calls):
    result = {"backend": [], "cli": [], "web": [], "repo-gate-run": [], "repo-gate-threshold": []}
    for call in calls:
        if "takt-dash-backend" in call:
            result["backend"].append(call)
        elif "takt-dash-cli" in call:
            result["cli"].append(call)
        elif "c8@12.0.0" in call:
            result["web"].append(call)
        elif " coverage run " in call:
            result["repo-gate-run"].append(call)
        elif "coverage_threshold.py" in call:
            result["repo-gate-threshold"].append(call)
    return result


def test_all_layers_reached_and_done_passes():
    with Workspace() as workspace:
        result = workspace.run_done(good_env())
        calls = layer_calls(workspace.recorded_calls())
        assert result.returncode == 0, f"全層合格でも done が失敗しました: {result.stderr[-2000:]}"
        for layer in ("backend", "cli", "web", "repo-gate-run", "repo-gate-threshold"):
            assert calls[layer], f"{layer} が実行されませんでした: {workspace.recorded_calls()!r}"


def _assert_single_layer_below_80_reaches_done(env_key, layer):
    env = dict(good_env(), **{env_key: "7999"})
    with Workspace() as workspace:
        result = workspace.run_done(env)
        calls = layer_calls(workspace.recorded_calls())
        assert result.returncode != 0, f"{layer} の未達が done に伝わりません: {workspace.recorded_calls()!r}"
        assert calls[layer], f"{layer} が実行されませんでした"


def test_backend_below_80_reaches_done():
    _assert_single_layer_below_80_reaches_done("COVERED_BACKEND", "backend")


def test_cli_below_80_reaches_done():
    _assert_single_layer_below_80_reaches_done("COVERED_CLI", "cli")


def test_web_below_80_reaches_done():
    _assert_single_layer_below_80_reaches_done("COVERED_WEB", "web")


def test_repo_gate_below_80_reaches_done():
    with Workspace() as workspace:
        result = workspace.run_done(dict(good_env(), COVERED_REPO_GATE="7999"))
        assert result.returncode != 0, "repo-gate の未達が done に伝わりません"
        assert layer_calls(workspace.recorded_calls())["repo-gate-threshold"], (
            "repo-gate の判定が実行されませんでした"
        )


def test_at_exactly_80_percent_passes():
    # 80.00% ちょうどは合格（実率80%以上）。四層すべて 8000/10000。
    with Workspace() as workspace:
        result = workspace.run_done(good_env())
        assert result.returncode == 0, f"80.00%ちょうどが不合格になりました: {result.stderr[-1000:]}"


def test_swallowed_coverage_failure_does_not_reach_done():
    # coverage の失敗を || true で潰すと、未達が done に伝わらない。配線確認の前提として、
    # この状態で done が成功することを観測する。
    with Workspace() as workspace:
        text = (workspace.root / "Makefile").read_text(encoding="utf-8")
        patched = re.sub(
            r"(\$\(MAKE\)[^\n]*\bcoverage\b)(?!\s*\|\|)",
            r"\1 || true",
            text,
            count=1,
        )
        assert patched != text, "coverage 呼び出し行がありません"
        workspace.write_makefile(patched)
        result = workspace.run_done(dict(good_env(), COVERED_REPO_GATE="7999"))
        assert result.returncode == 0, (
            f"coverage の失敗を || true で潰したのに done が失敗しました: {result.stderr[-1000:]}"
        )


def test_done_without_coverage_runs_no_layer():
    # done から coverage 呼び出しを除くと、層の計測が実行されない。
    with Workspace() as workspace:
        text = (workspace.root / "Makefile").read_text(encoding="utf-8")
        lines = [
            line for line in text.splitlines() if not re.search(r"\$\(MAKE\)[^\n]*\bcoverage\b", line)
        ]
        assert lines != text.splitlines(), "coverage 呼び出し行がありません"
        workspace.write_makefile("\n".join(lines) + "\n")
        result = workspace.run_done(good_env())
        calls = layer_calls(workspace.recorded_calls())
        assert result.returncode == 0, f"前提が崩れています（done が失敗）: {result.stderr[-1000:]}"
        assert not calls["backend"] and not calls["cli"] and not calls["web"], (
            f"coverage を外した done が層の計測を実行しています: {workspace.recorded_calls()!r}"
        )


def test_preserving_make_flag_spelling_does_not_change_result():
    # 動作を変えない Makefile の書換（coverage 呼び出し行へ --no-print-directory を足す等）で
    # 合否が変わらないことを観測する。特定の表記に一致しない場合も、動作を保つ別の書換で
    # 同じ合否になることを確認し、特定の字面だけを合否条件にしない。
    with Workspace() as workspace:
        text = (workspace.root / "Makefile").read_text(encoding="utf-8")

        # 1) coverage 呼び出し行へ --no-print-directory を足す。既に付いている等で一致しない
        #    場合は、2) 行末へコメントを足す同等の書換で代替する。どちらも動作を変えない。
        variants = [
            re.sub(r"\$\(MAKE\)(?!\s+--no-print-directory)(\s+coverage)\b", r"$(MAKE) --no-print-directory\1", text),
            re.sub(r"(\$\(MAKE\)[^\n]*\bcoverage\b)(\s*)$", r"\1 # noop\2", text, count=1, flags=re.MULTILINE),
        ]

        applied = next((v for v in variants if v != text), None)
        assert applied is not None, "coverage 呼び出し行を動作を保って書き換えられません"
        workspace.write_makefile(applied)

        result = workspace.run_done(good_env())
        assert result.returncode == 0, (
            f"動作を変えない表記変更で done が失敗しました（表記依存の疑い）: {result.stderr[-1000:]}"
        )
        calls = layer_calls(workspace.recorded_calls())
        for layer in ("backend", "cli", "web", "repo-gate-threshold"):
            assert calls[layer], f"表記変更後に {layer} が実行されませんでした"


def test_repo_gate_real_threshold_reads_generated_json():
    # repo-gate 層が生成 JSON を実閾値補助へ渡して判定することを確かめる。
    # 未達では実補助が FAIL 表示で非0を返し、done が非0になる。
    with Workspace() as workspace:
        result = workspace.run_done(dict(good_env(), COVERED_REPO_GATE="1078"))
        # 1078/10000=10.78% は未達。実補助が FAIL を出力して非0で終わる。
        combined = result.stdout + result.stderr
        assert result.returncode != 0, "repo-gate 未達が done に伝わりません"
        assert "repo-gate" in combined and "FAIL" in combined, (
            f"実閾値補助の FAIL 表示がありません: {combined[-2000:]}"
        )


def main() -> int:
    tests = [
        test_all_layers_reached_and_done_passes,
        test_backend_below_80_reaches_done,
        test_cli_below_80_reaches_done,
        test_web_below_80_reaches_done,
        test_repo_gate_below_80_reaches_done,
        test_at_exactly_80_percent_passes,
        test_swallowed_coverage_failure_does_not_reach_done,
        test_done_without_coverage_runs_no_layer,
        test_preserving_make_flag_spelling_does_not_change_result,
        test_repo_gate_real_threshold_reads_generated_json,
    ]
    failures = 0
    for test in tests:
        try:
            test()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {test.__name__}: {error}")
        except Exception:  # 実装漏れや脚本の穴は、失敗として記録する
            import traceback

            failures += 1
            print(f"FAIL {test.__name__}:")
            traceback.print_exc()
        else:
            print(f"ok   {test.__name__}")
    print(f"{len(tests) - failures} passed, {failures} failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
