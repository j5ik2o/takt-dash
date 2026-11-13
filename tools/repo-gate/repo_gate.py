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
  workflow-machine-gate taktdash-change に make done を実行する mode が system の
                       ステップがあり、develop の後ろに置かれ、make done の出力を修正
                       ステップへ渡し、成功で完了・失敗で修正へ回る遷移を持つ。
  gitignore-rules      .gitignore が Rust と Node とローカルデータの生成物を除外する。
  stable-toolchain     rust-toolchain.toml のチャネルが stable のままである。

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
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.11 未満は対象外
    tomllib = None

WORKSPACE_MEMBERS = ["backend", "frontend/cli"]
GATE_TARGETS = ["fmt", "clippy", "test", "typecheck", "lint"]


class GateError(Exception):
    """検査を実行できない理由、または満たせない条件を表す。"""


def _run(command, cwd, timeout_seconds):
    try:
        proc = subprocess.run(
            command,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
            env=_child_environment(),
            check=False,
        )
    except FileNotFoundError as error:
        return {"status": None, "stdout": "", "stderr": f"{command[0]} が見つかりません: {error}"}
    except subprocess.TimeoutExpired:
        return {
            "status": None,
            "stdout": "",
            "stderr": f"{' '.join(command)} が {timeout_seconds} 秒以内に終わりませんでした",
        }
    return {"status": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}


def _child_environment():
    env = dict(os.environ)
    # TAKT の command ステップは HOME を渡さない。HOME が無いと cargo と npm が
    # キャッシュや設定を読めないため、ログインユーザーのホームを補う。
    if "HOME" not in env:
        env["HOME"] = str(Path.home())
    return env


def read_text(path):
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError as error:
        raise GateError(f"{path} がありません: {error}")


def read_toml(path):
    if tomllib is None:
        raise GateError("tomllib を読み込めません。Python 3.11 以上で実行してください")
    try:
        return tomllib.loads(read_text(path))
    except tomllib.TOMLDecodeError as error:
        raise GateError(f"{path} を TOML として読めません: {error}")


def parse_makefile(path):
    """レシピをターゲット名からコマンド行の一覧へ変換する。

    継続行は前の行へ連結する。レシピは行頭のタブで始まる行だけを拾う。コマンド行の
    先頭の @ と - は、実行結果の表示と失敗の無視を制御する make の記法であるため外す。
    """
    targets = {}
    current = None
    pending = None
    for raw_line in read_text(path).splitlines():
        if pending is not None:
            if raw_line.startswith("\t"):
                pending += " " + raw_line.strip()
                continue
            _store_recipe(targets, current, pending)
            pending = None
        if raw_line.startswith("\t"):
            _store_recipe(targets, current, raw_line.strip())
            continue
        if not raw_line or raw_line.lstrip().startswith("#") or raw_line.startswith(" "):
            continue
        header = raw_line.split("#", 1)[0].strip()
        if ":" not in header or "=" in header.split(":", 1)[0]:
            continue
        name, _, rest = header.partition(":")
        names = name.split()
        if not names or any(token in header for token in ("%.", "${", "$(")):
            continue
        current = names[0]
        targets.setdefault(current, [])
        if rest.rstrip().endswith("\\"):
            pending = ""
            _store_recipe(targets, current, pending)
            current = names[0]
    if pending is not None:
        _store_recipe(targets, current, pending)
    return targets


def _store_recipe(targets, current, line):
    if current is None or not line:
        return
    command = line.lstrip("@-")
    targets[current].append(command)


def invoked_target(command, targets):
    """レシピの行が呼び出しているターゲット名を返す。呼び出していなければ None を返す。"""
    if "$(MAKE)" not in command and "make" not in command.split():
        return None
    try:
        tokens = shlex.split(command)
    except ValueError:
        tokens = command.split()
    for token in tokens:
        if token in targets:
            return token
    return None


def gate_commands(targets):
    """done から到達できる関門コマンドを、ターゲット名をキーに集める。

    done が呼び出しているターゲットだけを対象にする。Makefile に定義があるだけで呼び
    出していないターゲットを、実行される検査とみなさない。
    """
    if "done" not in targets:
        raise GateError("Makefile に done ターゲットがありません")
    found = {}
    for command in targets["done"]:
        nested = invoked_target(command, targets)
        if nested is None or nested == "done" or not targets.get(nested):
            continue
        found[nested] = targets[nested]
    return found


def compound_commands(recipe):
    """レシピの 1 行を、&& と ; と || で区切った単純コマンドへ分ける。"""
    simple = []
    for line in recipe:
        for part in re.split(r"&&|\|\||;", line):
            tokens = [token for token in part.strip().split() if token]
            if tokens:
                simple.append(tokens)
    return simple


def install_commands(recipe):
    """レシピから、ノードの依存の導入（npm ci など）を行う単純コマンドだけを返す。"""
    return [
        tokens
        for tokens in compound_commands(recipe)
        if any(token in ("npm", "yarn", "pnpm") for token in tokens)
        and any(token in ("ci", "install") for token in tokens)
    ]


def commands_for(recipes, name):
    """指定したコマンド名を持つ単純コマンドの一覧を返す（{MAKE} 経由の再帰をたどる）。"""
    matched = []
    for target, recipe in recipes.items():
        for tokens in compound_commands(recipe):
            if tokens[0] == name:
                matched.append(tokens)
    return matched


def is_install_guard(recipe):
    """ノードの依存を、無いときだけ導入する行かを判定する。

    インストールのコマンド（npm ci など）と、node_modules の有無を見る検査が同じ
    レシピの行にあるときだけ、条件付きの導入とみなす。
    """
    joined = " ".join(recipe)
    has_check = "node_modules" in joined and any(token in joined for token in (" -d ", " -f ", " -e ", "test "))
    return has_check and bool(install_commands(recipe))


def check_workspace_layout(root):
    cargo_toml = root / "Cargo.toml"
    manifest = read_toml(cargo_toml)
    workspace = manifest.get("workspace")
    if not isinstance(workspace, dict):
        raise GateError("Cargo.toml に [workspace] がありません")
    members = workspace.get("members")
    if not isinstance(members, list):
        raise GateError("Cargo.toml の [workspace] に members がありません")
    if sorted(members) != sorted(WORKSPACE_MEMBERS):
        raise GateError(
            f"Cargo.toml の members が {WORKSPACE_MEMBERS} ではありません: {members!r}"
        )

    metadata = _run(["cargo", "metadata", "--no-deps", "--format-version", "1"], root, 60)
    if metadata["status"] != 0:
        raise GateError(f"cargo metadata が失敗しました: {metadata['stderr'].strip()}")
    try:
        payload = json.loads(metadata["stdout"])
    except ValueError as error:
        raise GateError(f"cargo metadata の出力が JSON ではありません: {error}")
    member_ids = set(payload["workspace_members"])
    kinds = {}
    paths = {}
    for package in payload["packages"]:
        if package["id"] not in member_ids:
            continue
        target_kinds = []
        for target in package["targets"]:
            target_kinds.extend(target["kind"])
        kinds[Path(package["manifest_path"]).parent.resolve().relative_to(root).as_posix()] = sorted(
            set(target_kinds)
        )
        paths[package["name"]] = package["manifest_path"]
    for member in WORKSPACE_MEMBERS:
        if member not in kinds:
            raise GateError(f"ワークスペースのメンバー {member} が cargo metadata にありません")
    if "lib" not in kinds["backend"]:
        raise GateError(f"backend が lib を持ちません: {kinds['backend']!r}")
    if "bin" not in kinds["frontend/cli"]:
        raise GateError(f"frontend/cli が bin を持ちません: {kinds['frontend/cli']!r}")

    for member in ("proto", "frontend/web"):
        if member in kinds:
            raise GateError(f"{member} がワークスペースのメンバーに含まれています")

    web_manifest = read_json(root / "frontend/web/package.json")
    dependencies = {
        **(web_manifest.get("dependencies") or {}),
        **(web_manifest.get("devDependencies") or {}),
    }
    if "next" not in dependencies:
        raise GateError("frontend/web の package.json に next への依存がありません")
    if "typescript" not in dependencies:
        raise GateError("frontend/web の package.json に typescript への依存がありません")

    proto_dir = root / "proto"
    if not proto_dir.is_dir():
        raise GateError("proto/ がありません")

    web_scripts = web_manifest.get("scripts") or {}
    for script in ("typecheck", "lint", "test"):
        if not isinstance(web_scripts.get(script), str) or not web_scripts[script].strip():
            raise GateError(f"frontend/web の package.json に {script} の script がありません")

    return {
        "members": members,
        "kinds": kinds,
        "packages": paths,
        "scripts": sorted(web_scripts),
    }


def read_json(path):
    try:
        return json.loads(read_text(path))
    except ValueError as error:
        raise GateError(f"{path} を JSON として読めません: {error}")


def check_machine_gate(root, timeout_seconds):
    makefile = root / "Makefile"
    targets = parse_makefile(makefile)
    gate = gate_commands(targets)

    missing = [name for name in GATE_TARGETS if name not in gate]
    if missing:
        raise GateError(f"Makefile の done が実行しない検査があります: {missing}")

    cargo_runs = commands_for(gate, "cargo")
    clippy_line = " ".join(targets.get("clippy", []))
    if not any("+nightly" in tokens and "fmt" in tokens for tokens in cargo_runs):
        raise GateError("fmt に cargo +nightly fmt の実行がありません")
    if "clippy" not in clippy_line or "--all-targets" not in clippy_line:
        raise GateError("clippy に cargo clippy --all-targets（--workspace）の実行がありません")
    if "-- -D warnings" not in clippy_line:
        raise GateError("clippy に -- -D warnings がありません。警告が残っても成功してしまいます")
    if not any("test" in tokens and "--workspace" in tokens for tokens in cargo_runs):
        raise GateError("test に cargo test --workspace の実行がありません")

    npm_runs = commands_for(gate, "npm")
    if not any("run" in tokens and "test" in tokens for tokens in npm_runs):
        raise GateError("test に npm の test script の実行がありません")
    if not any("run" in tokens and "typecheck" in tokens for tokens in npm_runs):
        raise GateError("typecheck に npm の typecheck script の実行がありません")
    if not any("run" in tokens and "lint" in tokens for tokens in npm_runs):
        raise GateError("lint に npm の lint script の実行がありません")

    if not is_install_guard(targets["done"]):
        raise GateError(
            "done に、node_modules が無いときにノードの依存を導入する手順がありません"
        )

    result = _run(["make", "done"], root, timeout_seconds)
    if result["status"] != 0:
        raise GateError(
            f"make done が成功しませんでした（終了コード {result['status']}）: "
            f"{result['stderr'].strip()[-2000:]}"
        )
    return {
        "targets": sorted(gate),
        "install_guard": install_commands(targets["done"]),
        "make_done": result["status"],
    }


def read_workflow(path):
    """ワークフローの YAML を読み込む。PyYAML が必要である。"""
    try:
        import yaml
    except ModuleNotFoundError as error:
        raise GateError(f"PyYAML を読み込めません: {error}")
    try:
        return yaml.safe_load(read_text(path))
    except yaml.YAMLError as error:
        raise GateError(f"{path} を YAML として読めません: {error}")


def workflow_steps(workflow):
    steps = workflow.get("steps")
    if not isinstance(steps, list):
        raise GateError("taktdash-change に steps がありません")
    named = {}
    for step in steps:
        if isinstance(step, dict) and isinstance(step.get("name"), str):
            named[step["name"]] = step
    return steps, named


def rules_of(step):
    rules = step.get("rules")
    if not isinstance(rules, list):
        raise GateError(f"ステップ {step.get('name')} に rules がありません")
    return [rule for rule in rules if isinstance(rule, dict)]


def runs_make_done(step):
    """mode が system のステップで、make done を固定コマンドとして実行するかを判定する。"""
    if step.get("mode") != "system":
        return False
    for item in step.get("system_inputs") or []:
        if not isinstance(item, dict) or item.get("type") != "command":
            continue
        tokens = [token for token in str(item.get("command", "")).split() if token]
        if tokens[:2] == ["make", "done"]:
            return True
    return False


def transition(rules, predicate):
    for rule in rules:
        if predicate(rule):
            return rule.get("next")
    return None


def check_workflow_machine_gate(root):
    path = root / ".takt/workflows/taktdash-change.yaml"
    workflow = read_workflow(path)
    steps, named = workflow_steps(workflow)

    if "develop" not in named:
        raise GateError("taktdash-change に develop ステップがありません")
    complete_next = transition(rules_of(named["develop"]), lambda rule: rule.get("condition") == "COMPLETE")
    if complete_next is None:
        raise GateError("develop に COMPLETE の遷移がありません")

    matching = [step for step in steps if isinstance(step, dict) and runs_make_done(step)]
    if not matching:
        raise GateError("make done を実行する mode: system のステップがありません")
    gate = matching[0]
    gate_name = gate["name"]
    if complete_next != gate_name:
        raise GateError(f"develop の COMPLETE が {gate_name} へ遷移していません: {complete_next!r}")

    order = [step.get("name") for step in steps if isinstance(step, dict)]
    if order.index("develop") > order.index(gate_name):
        raise GateError(f"{gate_name} が develop より前の位置にあります")

    gate_rules = rules_of(gate)
    exit_code_next = transition(
        gate_rules,
        lambda rule: "exit_code" in str(rule.get("condition", "")) and rule.get("next") == "COMPLETE",
    )
    if exit_code_next != "COMPLETE":
        raise GateError("機械ゲートの成功（exit_code == 0）で COMPLETE へ遷移していません")
    failure_next = transition(gate_rules, lambda rule: rule.get("condition") == "when(true)")
    if failure_next is None:
        raise GateError("機械ゲートに失敗時の遷移（when(true)）がありません")

    fix = named.get(failure_next)
    if fix is None:
        raise GateError(f"機械ゲートの失敗時の遷移先 {failure_next!r} のステップがありません")
    instruction = fix.get("instruction")
    instruction_text = "\n".join(instruction) if isinstance(instruction, list) else str(instruction or "")
    if f"context:{gate_name}.result.stdout" not in instruction_text and (
        f"context:{gate_name}.result.stderr" not in instruction_text
    ):
        raise GateError(f"{failure_next} の instruction に make done の出力の参照がありません")
    if transition(rules_of(fix), lambda rule: rule.get("next") == gate_name) != gate_name:
        raise GateError(f"{failure_next} から {gate_name} へ戻る遷移がありません")

    return {
        "gate": gate_name,
        "failure_step": failure_next,
        "develop_complete": complete_next,
        "max_steps": workflow.get("max_steps"),
    }


def check_ci(root):
    workflows_dir = root / ".github/workflows"
    files = sorted(
        path
        for path in workflows_dir.glob("*")
        if path.suffix in (".yml", ".yaml") and path.is_file()
    )
    if not files:
        raise GateError(".github/workflows/ に CI の定義がありません")

    found = []
    for path in files:
        workflow = read_workflow(path)
        runs = []
        for job in (workflow.get("jobs") or {}).values():
            for step in job.get("steps") or []:
                if isinstance(step.get("run"), str):
                    runs.append(step["run"])
        make_done = [
            line
            for run in runs
            for line in run.splitlines()
            if [token for token in line.strip().split() if token][:2] == ["make", "done"]
        ]
        if not make_done:
            continue
        text = read_text(path)
        toolchains = re.findall(r"dtolnay/rust-toolchain@(stable|nightly)", text)
        components = re.findall(r"components:\s*([^\n]+)", text)
        flattened = " ".join(components)
        node_versions = re.findall(r"node-version:\s*['\"]?([0-9.x]+)", text)
        has_node_action = "actions/setup-node" in text
        problems = []
        if "stable" not in toolchains:
            problems.append("Rust の stable をセットアップしていません")
        if "nightly" not in toolchains or "rustfmt" not in flattened:
            problems.append("Rust の nightly と rustfmt をセットアップしていません")
        if not has_node_action or not node_versions:
            problems.append("Node をセットアップしていません")
        found.append({"file": str(path.relative_to(root)), "problems": problems})

    if not found:
        raise GateError(".github/workflows/ の CI が make done を呼んでいません")
    problems = [problem for item in found for problem in item["problems"]]
    if problems:
        raise GateError("; ".join(problems))
    return {"workflows": found}


def check_gitignore(root):
    lines = [line.strip() for line in read_text(root / ".gitignore").splitlines()]
    present = {line.rstrip("/") for line in lines if line and not line.startswith("#")}
    required = ["/target", "node_modules", ".next", "next-env.d.ts", "*.sqlite"]
    missing = [pattern for pattern in required if pattern not in present]
    if missing:
        raise GateError(f".gitignore に生成物を除外する行がありません: {missing}")
    return {"required": required}


def check_toolchain(root):
    manifest = read_toml(root / "rust-toolchain.toml")
    channel = (manifest.get("toolchain") or {}).get("channel")
    if channel != "stable":
        raise GateError(f"rust-toolchain.toml の channel が stable ではありません: {channel!r}")
    return {"channel": channel}


def run_checks(root, make_timeout_seconds, skip_make):
    checks = [
        ("workspace-layout", lambda: check_workspace_layout(root)),
        ("ci-runs-make-done", lambda: check_ci(root)),
        ("workflow-machine-gate", lambda: check_workflow_machine_gate(root)),
        ("gitignore-rules", lambda: check_gitignore(root)),
        ("stable-toolchain", lambda: check_toolchain(root)),
    ]
    if skip_make:
        checks.append(("machine-gate", lambda: {"make_done": "skipped"}))
    else:
        checks.append(("machine-gate", lambda: check_machine_gate(root, make_timeout_seconds)))

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
