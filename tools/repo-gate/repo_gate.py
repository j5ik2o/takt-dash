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
                       fmt・clippy・test・typecheck・lint の5検査を system_inputs の
                       command として実行し、各検査の make ターゲットを呼ぶ。5検査の
                       exit_code == 0 の同時成立で aislop_gate へ進み、検査ごとの
                       失敗は対応する修正ステップへ回して machine_gate へ戻る。
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
AISLOP_CHECK_COMMAND = "scripts/aislop-check-bootstrap.mjs check"
AISLOP_CHECK_INVOCATION = ["node", "scripts/aislop-check-bootstrap.mjs", "check"]
GATE_EXIT_CODE_REFERENCE = re.compile(r"context\.([A-Za-z0-9_]+)\.result\.exit_code")
GATE_SUCCESS_CONDITION = re.compile(
    r"when\(\s*context\.([A-Za-z0-9_]+)\.result\.exit_code\s*==\s*0\s*\)"
)
MACHINE_GATE_SUCCESS_TERM = re.compile(
    r"context\.machine_gate\.([A-Za-z0-9_]+)\.exit_code\s*==\s*0"
)
MACHINE_GATE_FAILURE_CONDITION = re.compile(
    r"when\(\s*context\.machine_gate\.([A-Za-z0-9_]+)\.exit_code\s*!=\s*0\s*\)"
)
CONTEXT_REFERENCE = re.compile(r"\{context:([^}]+)\}")
SHELL_SEPARATORS = {";", "&", "|", "&&", "||", "(", ")", "\n"}
SHELL_KEYWORDS = {
    "if",
    "then",
    "else",
    "elif",
    "fi",
    "do",
    "done",
    "while",
    "until",
    "!",
    "{",
    "}",
}
REDIRECTION = re.compile(r"^\d*[<>]")


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


def strip_shell_comments(command):
    """シェルのコメントを、クォートとバックスラッシュを考慮して除去する。

    未クォート・未エスケープの # から行末までを除去する。行の区切りを保つため改行は
    残す。単一引用符の内側ではバックスラッシュと # をそのまま扱い、二重引用符の
    内側ではバックスラッシュで次の 1 文字をエスケープする。shlex の comments=True と
    同じく、語の途中にある未クォートの # もコメントの開始とみなす。
    """
    result = []
    quote = None
    escaped = False
    in_comment = False
    for char in str(command):
        if in_comment:
            if char == "\n":
                in_comment = False
                result.append(char)
            continue
        if escaped:
            result.append(char)
            escaped = False
            continue
        if quote == "'":
            result.append(char)
            if char == "'":
                quote = None
            continue
        if quote == '"':
            if char == "\\":
                escaped = True
            else:
                quote = None if char == '"' else quote
            result.append(char)
            continue
        if char == "\\":
            escaped = True
            result.append(char)
            continue
        if char in ("'", '"'):
            quote = char
            result.append(char)
            continue
        if char == "#":
            in_comment = True
            continue
        result.append(char)
    return "".join(result)


def invokes_aislop_check(command):
    """コマンド文字列が aislop チェックを実際に起動するかを判定する。

    先にシェルのコメントを除去してから、&& / || / ; / | / 改行による単純コマンドへの
    分割を行い、各単純コマンドの先頭が aislop チェックの起動になっているかを調べる。
    コメントを除去してから分割することで、コメント内の演算子の後ろを独立した単純
    コマンドとして解釈しない。echo の引数・引用符内の文字列・コメントは単純コマンドの
    先頭にならないため、表示だけの文字列を実行とみなさない。
    """
    stripped = strip_shell_comments(command)
    for simple in re.split(r"&&|\|\||[;|\n]", stripped):
        try:
            tokens = shlex.split(simple, comments=False)
        except ValueError:
            continue
        if tokens[: len(AISLOP_CHECK_INVOCATION)] == AISLOP_CHECK_INVOCATION:
            return True
    return False


def runs_aislop_gate(step):
    """mode が system のステップで、aislop の関門を実際に起動するかを判定する。"""
    if step.get("mode") != "system":
        return False
    return any(
        invokes_aislop_check(str(item.get("command", "")))
        for item in step.get("system_inputs") or []
        if isinstance(item, dict) and item.get("type") == "command"
    )


def shell_tokens(command):
    """シェルのコマンド文字列を、クォートを保ったままトークンへ分ける。"""
    lexer = shlex.shlex(
        strip_shell_comments(command),
        posix=True,
        punctuation_chars=";&|\n()",
    )
    lexer.whitespace_split = True
    lexer.commenters = ""
    lexer.whitespace = " \t\r"
    return list(lexer)


def simple_commands(tokens):
    """トークン列を、&& / || / ; / | / 改行などの区切りで単純コマンドへ分ける。"""
    current = []
    for token in tokens:
        if token in SHELL_SEPARATORS:
            if current:
                yield current
            current = []
            continue
        current.append(token)
    if current:
        yield current


def invoked_make_targets(command, depth=0):
    """コマンド文字列が実際に起動する make のターゲットを集める。

    bash -c や sh -c の内側のスクリプトへ再帰し、if/then などの制御語を除いた
    単純コマンドの先頭が make のときだけ、フラグと変数代入を除いたターゲットを
    拾う。echo の引数や引用符内の文字列、コメントは起動として扱わない。
    """
    found = set()
    if depth > 5:
        return found
    try:
        tokens = shell_tokens(command)
    except ValueError:
        return found
    for tokens_of_command in simple_commands(tokens):
        while tokens_of_command and tokens_of_command[0] in SHELL_KEYWORDS:
            tokens_of_command = tokens_of_command[1:]
        if not tokens_of_command:
            continue
        head = tokens_of_command[0]
        if head in ("bash", "sh") and "-c" in tokens_of_command:
            index = tokens_of_command.index("-c")
            if index + 1 < len(tokens_of_command):
                found |= invoked_make_targets(tokens_of_command[index + 1], depth + 1)
            continue
        if head != "make":
            continue
        for token in tokens_of_command[1:]:
            if REDIRECTION.match(token):
                break
            if token.startswith("-"):
                continue
            if "=" in token and token.split("=", 1)[0].isidentifier():
                continue
            found.add(token)
    return found


def machine_gate_success_checks(condition):
    """成功条件が5検査の exit_code == 0 の同時成立なら、その検査名を返す。

    when(...) 全体を && で区切った各節が、machine_gate の検査の exit_code == 0 に
    完全一致する場合だけ受理する。|| や単一キーの result、別ゲートの参照、重複した
    検査は、空のリストとして拒否する。
    """
    text = str(condition or "").strip()
    if "||" in text:
        return []
    outer = re.fullmatch(r"when\((.*)\)", text, re.DOTALL)
    if outer is None:
        return []
    checks = []
    for term in outer.group(1).split("&&"):
        match = MACHINE_GATE_SUCCESS_TERM.fullmatch(term.strip())
        if match is None:
            return []
        checks.append(match.group(1))
    return checks


def instruction_text(step):
    instruction = step.get("instruction")
    if isinstance(instruction, list):
        return "\n".join(str(item) for item in instruction)
    return str(instruction or "")


def check_machine_gate_inputs(gate):
    """machine_gate が mode: system で5検査を command として実行するかを検査する。"""
    if gate.get("mode") != "system":
        raise GateError(f"machine_gate は mode: system でなければなりません: {gate.get('mode')!r}")

    inputs = gate.get("system_inputs")
    if not isinstance(inputs, list):
        raise GateError("machine_gate に system_inputs がありません")
    by_name = {}
    for item in inputs:
        if not isinstance(item, dict):
            raise GateError(f"machine_gate の system_inputs に辞書でない要素があります: {item!r}")
        alias = item.get("as")
        if not isinstance(alias, str):
            raise GateError(
                f"machine_gate の system_inputs の as が文字列ではありません: {alias!r}"
            )
        if alias in by_name:
            raise GateError(f"machine_gate の system_inputs に同じ as が重複しています: {alias!r}")
        by_name[alias] = item
    missing = [name for name in GATE_TARGETS if name not in by_name]
    extra = sorted(name for name in by_name if name not in GATE_TARGETS)
    if missing or extra:
        raise GateError(
            f"machine_gate の system_inputs の as が5検査と一致しません: missing={missing}, extra={extra}"
        )
    for name in GATE_TARGETS:
        item = by_name[name]
        if item.get("type") != "command":
            raise GateError(
                f"machine_gate の {name} の system_input が type: command ではありません: "
                f"{item.get('type')!r}"
            )
        targets = invoked_make_targets(str(item.get("command", "")))
        if name not in targets:
            raise GateError(f"{name} の command が make {name} を呼んでいません: {item.get('command')!r}")
        others = sorted(targets & (set(GATE_TARGETS) - {name}))
        if others:
            raise GateError(
                f"{name} の command が別の検査 {', '.join(others)} を呼んでいます: "
                f"{item.get('command')!r}"
            )


def check_machine_gate_success_rule(gate_rules, aislop_name):
    """成功規則が5検査の exit_code == 0 の同時成立で aislop_gate へ進むかを検査する。"""
    success_rules = [rule for rule in gate_rules if rule.get("next") == aislop_name]
    if len(success_rules) != 1:
        raise GateError(
            f"machine_gate に {aislop_name} へ進む成功規則が 1 つだけありません: {len(success_rules)}"
        )
    success_checks = machine_gate_success_checks(success_rules[0].get("condition"))
    if sorted(success_checks) != sorted(GATE_TARGETS):
        raise GateError(
            "machine_gate の成功条件が5検査（fmt・clippy・test・typecheck・lint）の "
            f"exit_code == 0 の同時成立になっていません: {success_rules[0].get('condition')!r}"
        )


def check_machine_gate_failure_rules(gate_rules):
    """失敗規則が検査ごとに対応する修正ステップへ1件ずつ分岐するかを検査する。"""
    for name in GATE_TARGETS:
        fix_name = f"fix_machine_gate_{name}"
        failure_rules = [rule for rule in gate_rules if rule.get("next") == fix_name]
        if len(failure_rules) != 1:
            raise GateError(
                f"machine_gate に {name} の失敗（exit_code != 0）で {fix_name} へ進む規則が "
                f"1 つだけありません: {len(failure_rules)}"
            )
        condition = str(failure_rules[0].get("condition") or "").strip()
        match = MACHINE_GATE_FAILURE_CONDITION.fullmatch(condition)
        if match is None or match.group(1) != name:
            raise GateError(
                f"machine_gate の {name} の失敗条件が "
                f"when(context.machine_gate.{name}.exit_code != 0) ではありません: {condition!r}"
            )


def check_machine_gate_fix_steps(named):
    """各修正ステップが当該検査の出力だけを参照し、machine_gate へ戻るかを検査する。"""
    for name in GATE_TARGETS:
        fix_name = f"fix_machine_gate_{name}"
        fix = named.get(fix_name)
        if fix is None:
            raise GateError(f"machine_gate の {name} の修正ステップ {fix_name} がありません")
        references = set(CONTEXT_REFERENCE.findall(instruction_text(fix)))
        expected = {f"machine_gate.{name}.stdout", f"machine_gate.{name}.stderr"}
        if references != expected:
            raise GateError(
                f"{fix_name} の instruction が参照する出力が {name} の stdout と stderr だけでは"
                f"ありません: {sorted(references)}"
            )
        if not any(rule.get("next") == "machine_gate" for rule in rules_of(fix)):
            raise GateError(f"{fix_name} から machine_gate へ戻る遷移がありません")


def check_machine_gate_transitions(gate_rules, aislop_name):
    """machine_gate の遷移先が既知のものだけで、ABORT が最後の when(true) かを検査する。"""
    allowed = {f"fix_machine_gate_{name}" for name in GATE_TARGETS} | {aislop_name, "ABORT"}
    for rule in gate_rules:
        next_value = rule.get("next")
        if next_value not in allowed:
            raise GateError(f"machine_gate に許可されない遷移先があります: {next_value!r}")
    for index, rule in enumerate(gate_rules):
        if rule.get("next") != "ABORT":
            continue
        if str(rule.get("condition") or "").strip() != "when(true)":
            raise GateError(
                f"machine_gate の ABORT への遷移条件が when(true) ではありません: "
                f"{rule.get('condition')!r}"
            )
        if index != len(gate_rules) - 1:
            raise GateError("machine_gate の when(true) -> ABORT が規則列の最後にありません")


def check_machine_gate_five_checks(named, aislop_name):
    """machine_gate が5検査を実行し、検査ごとに失敗を振り分けるかを検査する。"""
    gate = named["machine_gate"]
    check_machine_gate_inputs(gate)
    gate_rules = rules_of(gate)
    check_machine_gate_success_rule(gate_rules, aislop_name)
    check_machine_gate_failure_rules(gate_rules)
    check_machine_gate_fix_steps(named)
    check_machine_gate_transitions(gate_rules, aislop_name)
    return [f"fix_machine_gate_{name}" for name in GATE_TARGETS]


def transition(rules, predicate):
    for rule in rules:
        if predicate(rule):
            return rule.get("next")
    return None


def is_gate_success_condition(condition, name):
    """成功条件が、そのゲート自身の exit_code だけを == 0 で見る正規形かを判定する。

    条件中の exit_code 参照をすべて抽出し、自ゲート以外を参照していれば成功条件と
    みなさない。AND/OR で別ゲートを併記した条件や、自ゲートを否定して別の
    `== 0` を足した条件を成功として受理しないよう、条件全体も正規形に一致させる。
    """
    text = str(condition).strip()
    references = GATE_EXIT_CODE_REFERENCE.findall(text)
    if not references or any(reference != name for reference in references):
        return False
    match = GATE_SUCCESS_CONDITION.fullmatch(text)
    return match is not None and match.group(1) == name


def check_gate_rule_references(gate_rules, name):
    """ゲートの全規則の exit_code 参照が、そのゲート自身だけを指すことを検査する。

    規則は順に評価され、先に一致した条件が遷移を決める。成功条件の規則だけを検査すると、
    別の規則に別ゲートの `exit_code == 0` を置くことで、そのゲートの成功でも次へ進めて
    しまう。規則列全体の参照を列挙し、自ゲート以外の exit_code を見る規則を拒否する。
    """
    for rule in gate_rules:
        for reference in GATE_EXIT_CODE_REFERENCE.findall(str(rule.get("condition", ""))):
            if reference != name:
                raise GateError(
                    f"{name} の規則が別ゲート {reference} の exit_code を参照しています: "
                    f"{rule.get('condition')!r}"
                )


def check_machine_gate_step(named, name, return_to):
    """機械ゲートの 1 ステップが、成功を次へ渡し、失敗を修正ステップへ回すかを検査する。

    成功条件は、そのゲート自身の結果（context:<name>.result.exit_code）を参照することを
    要求する。別ゲートの exit_code を見る条件では、そのゲートが実行されていなくても
    成功と判定できてしまうため受け付けない。失敗時の修正ステップは return_to へ戻る。
    """
    gate = named[name]
    gate_rules = rules_of(gate)
    check_gate_rule_references(gate_rules, name)
    success_reference = f"context.{name}.result.exit_code"
    succeeds = transition(
        gate_rules,
        lambda rule: is_gate_success_condition(rule.get("condition"), name),
    )
    if succeeds is None:
        raise GateError(f"{name} に成功（{success_reference} == 0）の遷移がありません")
    failure_next = transition(gate_rules, lambda rule: rule.get("condition") == "when(true)")
    if failure_next is None:
        raise GateError(f"{name} に失敗時の遷移（when(true)）がありません")
    if failure_next == "ABORT":
        raise GateError(
            f"{name} の失敗が修正ステップではなく ABORT へ遷移しています。"
            f"失敗時は修正ステップ（例: fix_{name}）へ遷移してください"
        )

    fix = named.get(failure_next)
    if fix is None:
        raise GateError(f"{name} の失敗時の遷移先 {failure_next!r} のステップがありません")
    text = instruction_text(fix)
    if f"context:{name}.result.stdout" not in text and (
        f"context:{name}.result.stderr" not in text
    ):
        raise GateError(f"{failure_next} の instruction に {name} の出力の参照がありません")
    if transition(rules_of(fix), lambda rule: rule.get("next") == return_to) != return_to:
        raise GateError(f"{failure_next} から {return_to} へ戻る遷移がありません")
    return succeeds, failure_next


def check_workflow_machine_gate(root):
    path = root / ".takt/workflows/taktdash-change.yaml"
    workflow = read_workflow(path)
    steps, named = workflow_steps(workflow)

    if "develop" not in named:
        raise GateError("taktdash-change に develop ステップがありません")
    complete_next = transition(rules_of(named["develop"]), lambda rule: rule.get("condition") == "COMPLETE")
    if complete_next is None:
        raise GateError("develop の COMPLETE 遷移がありません")
    if "machine_gate" not in named:
        raise GateError("taktdash-change に machine_gate ステップがありません")
    if complete_next != "machine_gate":
        raise GateError(f"develop の COMPLETE が machine_gate へ遷移していません: {complete_next!r}")

    aislop_steps = [step for step in steps if isinstance(step, dict) and runs_aislop_gate(step)]
    if not aislop_steps:
        raise GateError("aislop-gate を実行する mode: system のステップがありません")
    aislop_name = aislop_steps[0]["name"]

    fix_names = check_machine_gate_five_checks(named, aislop_name)

    order = [step.get("name") for step in steps if isinstance(step, dict)]
    if order.index("develop") > order.index("machine_gate"):
        raise GateError("machine_gate が develop より前の位置にあります")
    if order.index("machine_gate") > order.index(aislop_name):
        raise GateError(f"{aislop_name} が machine_gate より前の位置にあります")

    aislop_next, aislop_fix = check_machine_gate_step(named, aislop_name, "machine_gate")
    if aislop_next != "COMPLETE":
        raise GateError(f"{aislop_name} の成功（exit_code == 0）で COMPLETE へ遷移していません: {aislop_next!r}")

    return {
        "gates": ["machine_gate", aislop_name],
        "checks": list(GATE_TARGETS),
        "failure_steps": fix_names + [aislop_fix],
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
        ordered_runs = []
        # チェックアウト順の不変条件はジョブ単位で判定する。ファイル全体で 1 回だけ
        # 初期化すると、先のジョブの checkout が後続ジョブへ累積し、aislop を含む
        # ジョブ自身に checkout が無くても合格してしまう。
        aislop_misordered_jobs = []
        aislop_ordered_jobs = []
        for job_id, job in (workflow.get("jobs") or {}).items():
            checkout_seen = False
            job_calls_aislop = False
            job_aislop_ordered = False
            for step in job.get("steps") or []:
                if not isinstance(step, dict):
                    continue
                uses = str(step.get("uses") or "")
                if uses.startswith("actions/checkout"):
                    checkout_seen = True
                run = step.get("run")
                if not isinstance(run, str):
                    continue
                ordered_runs.append(run)
                if not invokes_aislop_check(run):
                    continue
                job_calls_aislop = True
                if checkout_seen:
                    job_aislop_ordered = True
                else:
                    aislop_misordered_jobs.append(job_id)
            if job_calls_aislop:
                aislop_ordered_jobs.append(job_aislop_ordered)
        make_done = [
            line
            for run in ordered_runs
            for line in run.splitlines()
            if [token for token in line.strip().split() if token][:2] == ["make", "done"]
        ]
        if not make_done:
            continue
        aislop_called = bool(aislop_ordered_jobs)
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
        if not aislop_called:
            problems.append(f"aislop のゲート（node {AISLOP_CHECK_COMMAND}）を呼んでいません")
        elif aislop_misordered_jobs:
            problems.append("aislop のゲートをチェックアウトより前に呼んでいます")
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
