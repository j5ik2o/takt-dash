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
                       し、失敗は再試行または修正へ回し、ABORT へは遷移しない。段階の検出は
                       実行位置にある publish.py の呼び出しだけを数え、echo の引数・引用符内の
                       文字列・コメントは実行とみなさない。成功規則は when(true) の無条件規則
                       より前に置く（規則は先に一致したものが選ばれるため）。公開ステップを
                       置かない場合は machine_gate の成功が aislop_gate へ直結する。status
                       だけを実行するステップは公開とみなさない。
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
GATE_TARGETS = ["fmt", "clippy", "test", "typecheck", "lint", "proto_check"]
# machine_gate の system_input の as 名と、Makefile のターゲット名の対応。
# proto-check は Makefile のターゲット名にハイフンを含むため、式から参照しやすい
# as 名（proto_check）と分けて対応づける。
GATE_MAKE_TARGETS = {"proto_check": "proto-check"}


def gate_make_target(name):
    return GATE_MAKE_TARGETS.get(name, name)


# machine_gate の各検査が実行するコマンドの正規形。
# 制御演算子で検査の実行を飛ばす変異（例: `true || make fmt`）を、単純コマンドの
# 解析だけで受理しないよう、コマンド全文を照合する。検査のコマンドを変える場合は
# ここも更新する（ゲートの検査そのものを変える変更としてレビュー対象にする）。
MACHINE_GATE_COMMANDS = {
    "fmt": "bash -o pipefail -c 'make fmt 2>&1 | tail -c 60000'",
    "clippy": "bash -o pipefail -c 'make clippy 2>&1 | tail -c 60000'",
    "test": (
        "bash -c 'if [ ! -d frontend/web/node_modules ]; then "
        'bash -o pipefail -c "npm --prefix frontend/web ci 2>&1 | tail -c 4000" '
        '|| exit $?; fi; bash -o pipefail -c "make test 2>&1 | python3 tools/test-summary/test_summary.py --max-bytes 56000"\''
    ),
    "typecheck": "bash -o pipefail -c 'make typecheck 2>&1 | tail -c 60000'",
    "lint": "bash -o pipefail -c 'make lint 2>&1 | tail -c 60000'",
    "proto_check": "bash -o pipefail -c 'make proto-check 2>&1 | tail -c 60000'",
}
AISLOP_CHECK_COMMAND = "scripts/aislop-check-bootstrap.mjs check"
AISLOP_CHECK_INVOCATION = ["node", "scripts/aislop-check-bootstrap.mjs", "check"]
AISLOP_FAILURE_NOTE = (
    "|| true・; true・パイプ・後続のコマンドなどでチェックの失敗を潰す記述は、"
    "実行とみなしません"
)
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
# shell_tokens が演算子のトークンとして切り出す文字。
SHELL_OPERATOR_CHARS = ";&|\n()"


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

    missing = [name for name in GATE_TARGETS if gate_make_target(name) not in gate]
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
    """コマンド文字列が aislop チェックを起動し、その失敗を呼び出し元へ返すかを判定する。

    コメントを除去してクォートを保ったままトークンへ分け、チェックの起動がコマンド
    全体の最後の単純コマンドになっている場合だけ真を返す。直前はコマンドの先頭・&&・
    ; ・改行に限り、直後は引数とリダイレクション、末尾の ; と改行に限る。このため、
    `|| true`・`; true`・次の行のコマンドで終了状態を上書きする形、`| cat` などの
    パイプ、`&` によるバックグラウンド実行、`!` による否定、`npm ci || check` のように
    チェックが飛ばされる形は起動とみなさない。シェルの -e などの実行時設定には頼らず、
    失敗が返ることを記述だけから確かめられる形に限る保守的な判定である。echo の引数・
    引用符内の文字列・コメントは単純コマンドの先頭にならないため、実行とみなさない。
    """
    try:
        tokens = shell_tokens(command)
    except ValueError:
        return False
    width = len(AISLOP_CHECK_INVOCATION)
    return any(
        tokens[index : index + width] == AISLOP_CHECK_INVOCATION
        and starts_command_list_item(tokens[:index])
        and ends_command_list(tokens[index + width :])
        for index in range(len(tokens))
    )


def is_shell_operator(token):
    """トークンがシェルの演算子かを判定する。引用符やエスケープで作られたものは演算子ではない。"""
    return (
        bool(token)
        and not getattr(token, "quoted", False)
        and all(char in SHELL_OPERATOR_CHARS for char in token)
    )


def starts_command_list_item(preceding):
    """直前のトークン列が、終了状態をそのまま返す位置で次のコマンドを始めるかを判定する。

    改行だけのトークンを読み飛ばした手前が、先頭・&&・; なら真とする。改行を挟んだ
    手前が普通の語なら、改行がコマンドの区切りなので真とする。|| と | と & と ( は、
    `||` の直後の改行（行継続）を含めて偽とする。改行を挟まずに語が続く場合は、
    その語の引数（echo の引数や ! の否定対象）なので偽とする。
    """
    index = len(preceding)
    while index > 0 and preceding[index - 1].strip("\n") == "":
        index -= 1
    if index == 0:
        return True
    previous = preceding[index - 1]
    if is_shell_operator(previous):
        return previous.strip("\n") in ("&&", ";")
    return index < len(preceding)


def ends_command_list(following):
    """起動の後ろのトークン列が、引数・リダイレクション・末尾の ; と改行だけかを判定する。"""
    for index, token in enumerate(following):
        if not is_shell_operator(token):
            continue
        if token.strip(";\n") == "":
            return all(rest.strip(";\n") == "" for rest in following[index + 1 :])
        if token == "&" and is_redirection_ampersand(following, index):
            continue
        return False
    return True


def is_redirection_ampersand(tokens, index):
    """`2>&1` や `&>file` の & を、バックグラウンド実行ではなくリダイレクションと判定する。

    `&` の直後に空白を挟まず `>` が続く場合だけリダイレクションとする。
    `& > file` は、バックグラウンド実行の後にリダイレクションだけの
    コマンドが続く形で、チェックの失敗は返らない。
    """
    before = tokens[index - 1] if index > 0 else ""
    after = tokens[index + 1] if index + 1 < len(tokens) else ""
    if before and not getattr(before, "quoted", False) and REDIRECTION.fullmatch(before):
        return True
    if not after.startswith(">"):
        return False
    return not getattr(after, "space_before", False)


def runs_aislop_gate(step):
    """mode が system のステップで、aislop の関門を実際に起動するかを判定する。"""
    if step.get("mode") != "system":
        return False
    return any(
        invokes_aislop_check(str(item.get("command", "")))
        for item in step.get("system_inputs") or []
        if isinstance(item, dict) and item.get("type") == "command"
    )


class ShellToken(str):
    """シェルのトークン。文字列としての比較はそのまま使える。

    quoted: 引用符の中かバックスラッシュの直後で作られた文字を含む場合に真。
    演算子に見える文字列でも、引用されたものは演算子ではない。
    space_before: 直前に空白（スペース・タブ・復帰）があった場合に真。
    `&>` と `& >` の区別に使う。
    heredoc_operator: 語の先頭（数字の fd 指定の後）に、引用されていない
    `<<` がある場合に真。shell_tokens が設定し、それ以外は偽。
    """

    heredoc_operator = False

    def __new__(cls, text, *, quoted=False, space_before=False):
        token = super().__new__(cls, text)
        token.quoted = quoted
        token.space_before = space_before
        return token


def parse_heredoc_operator(text, start):
    """text[start] から始まる <<区切り語 を解析し、(区切り語, タブ除去, 終端位置) を返す。

    start は最初の `<` の位置。`<<-` の場合はタブ除去を真とする。区切り語は
    空白と演算子の手前までで、引用符とエスケープは取り除いて読む。区切り語が
    無い場合は None を返す。
    """
    index = start + 2
    strip_tabs = False
    if index < len(text) and text[index] == "-":
        strip_tabs = True
        index += 1
    while index < len(text) and text[index] in " \t":
        index += 1
    chars = []
    while index < len(text):
        char = text[index]
        if char in " \t\r\n;&|()<>":
            break
        if char in ("'", '"'):
            quote = char
            index += 1
            while index < len(text) and text[index] != quote:
                if quote == '"' and text[index] == "\\" and index + 1 < len(text):
                    index += 1
                chars.append(text[index])
                index += 1
            if index >= len(text):
                return None
            index += 1
            continue
        if char == "\\" and index + 1 < len(text):
            chars.append(text[index + 1])
            index += 2
            continue
        chars.append(char)
        index += 1
    delimiter = "".join(chars)
    if not delimiter:
        return None
    return delimiter, strip_tabs, index


def skip_heredoc_bodies(text, index, pending):
    """ヒアドキュメントの本文の行を読み飛ばし、本文の直後の位置を返す。

    pending の (区切り語, タブ除去) の順に、各本文を区切り語だけの行まで
    読み飛ばす。区切り語の行自体も読み飛ばす。
    """
    queue = list(pending)
    while queue and index < len(text):
        end = text.find("\n", index)
        if end == -1:
            line = text[index:]
            index = len(text)
        else:
            line = text[index:end]
            index = end + 1
        delimiter, strip_tabs = queue[0]
        candidate = line[:-1] if line.endswith("\r") else line
        if strip_tabs:
            candidate = candidate.lstrip("\t")
        if candidate == delimiter:
            queue.pop(0)
    return index


def strip_heredoc_bodies(command):
    """ヒアドキュメントの本文を、コマンド文字列から取り除く。

    引用符とエスケープとコメントを考慮して走査し、引用符の外にある
    <<区切り語 だけをヒアドキュメントの開始として扱う。開始を含む行の
    改行の直後から、区切り語だけの行まで（区切り語の行を含む）を除去し、
    演算子と区切り語もコマンド行から取り除く。本文はシェルの構文として
    解釈されないため、本文に含まれる引用符や # が、後段のコメント除去や
    トークン化へ影響することはない。引用符内の `<<`（例: echo "<<EOF"）や
    コメント内の `<<` は開始として扱わない。<<<（ヒアストリング）は本文を
    持たないため対象にしない。<< が語の途中にある場合（例: a<<EOF）は、
    トークン化側の扱いに合わせて開始として扱わない。数字だけの語の直後に
    ある場合は、fd 指定付きのリダイレクトとして扱う。
    """
    text = str(command)
    result = []
    pending = []
    index = 0
    quote = None
    in_comment = False

    def operator_position():
        """結果の末尾から見て、<< がリダイレクトの開始位置にあるかを返す。

        末尾の語が空（行頭・空白・演算子の直後）か、数字だけ（fd 指定）の
        ときに開始位置とする。あわせて、その数字の並びの長さを返す。
        """
        digits = 0
        for char in reversed(result):
            if char in " \t\r\n;&|()":
                break
            if not char.isdigit():
                return None
            digits += 1
        return digits

    while index < len(text):
        char = text[index]
        if in_comment:
            result.append(char)
            index += 1
            if char == "\n":
                in_comment = False
                if pending:
                    index = skip_heredoc_bodies(text, index, pending)
                    pending = []
            continue
        if quote == "'":
            result.append(char)
            if char == "'":
                quote = None
            index += 1
            continue
        if quote == '"':
            result.append(char)
            if char == "\\" and index + 1 < len(text):
                result.append(text[index + 1])
                index += 2
                continue
            if char == '"':
                quote = None
            index += 1
            continue
        if char == "\\" and index + 1 < len(text):
            result.append(char)
            result.append(text[index + 1])
            index += 2
            continue
        if char in ("'", '"'):
            quote = char
            result.append(char)
            index += 1
            continue
        if char == "#":
            in_comment = True
            result.append(char)
            index += 1
            continue
        if char == "\n":
            result.append(char)
            index += 1
            if pending:
                index = skip_heredoc_bodies(text, index, pending)
                pending = []
            continue
        if char == "<" and text[index + 1 : index + 2] == "<" and text[index + 2 : index + 3] != "<":
            digits = operator_position()
            parsed = parse_heredoc_operator(text, index) if digits is not None else None
            if parsed is not None:
                delimiter, strip_tabs, end = parsed
                pending.append((delimiter, strip_tabs))
                if digits:
                    del result[len(result) - digits :]
                index = end
                continue
            result.append(char)
            index += 1
            continue
        result.append(char)
        index += 1
    return "".join(result)


def shell_tokens(command):
    """シェルのコマンド文字列を、クォートを保ったままトークンへ分ける。

    各トークンは ShellToken で、引用符やエスケープで作られた文字を含むか
    （quoted）と、直前に空白があったか（space_before）を保持する。演算子
    の判定はこの情報を使うため、引用符内の `&&` や `;`、エスケープされた
    `;` は演算子にならない。`&>` と `& >` も区別できる。閉じていない
    引用符がある場合は、shlex と同じく ValueError を上げる。

    トークン化の前に、ヒアドキュメントの本文と行継続（バックスラッシュ＋
    改行）を取り除く。本文はシェルの構文として解釈されないため、本文の
    引用符や改行がトークンを壊すことはない。行継続は入力から取り除かれる
    ため、継続行の字下げが余分な引数になることもない。
    """
    text = strip_shell_comments(strip_heredoc_bodies(command))
    tokens = []
    index = 0
    pending_space = False

    def flush_word(chars, quoted, space_before, flags):
        if chars or quoted:
            token = ShellToken("".join(chars), quoted=quoted, space_before=space_before)
            token.heredoc_operator = starts_with_heredoc_operator(chars, flags)
            tokens.append(token)

    while index < len(text):
        char = text[index]
        if char in " \t\r":
            pending_space = True
            index += 1
            continue
        if char in SHELL_OPERATOR_CHARS:
            if char in "&|;":
                end = index + 1
                while end < len(text) and text[end] == char:
                    end += 1
                tokens.append(ShellToken(text[index:end], space_before=pending_space))
                index = end
            else:
                tokens.append(ShellToken(char, space_before=pending_space))
                index += 1
            pending_space = False
            continue
        word_space = pending_space
        pending_space = False
        chars = []
        flags = []
        quoted = False
        while index < len(text):
            char = text[index]
            if char in " \t\r" or char in SHELL_OPERATOR_CHARS:
                break
            if char == "\\" and index + 1 < len(text):
                if text[index + 1] == "\n":
                    index += 2
                    continue
                chars.append(text[index + 1])
                flags.append(True)
                quoted = True
                index += 2
                continue
            if char in ("'", '"'):
                quote = char
                quoted = True
                index += 1
                while index < len(text) and text[index] != quote:
                    if (
                        quote == '"'
                        and text[index] == "\\"
                        and index + 1 < len(text)
                        and text[index + 1] == "\n"
                    ):
                        index += 2
                        continue
                    if quote == '"' and text[index] == "\\" and index + 1 < len(text):
                        chars.append(text[index + 1])
                        flags.append(True)
                        index += 2
                        continue
                    chars.append(text[index])
                    flags.append(True)
                    index += 1
                if index >= len(text):
                    raise ValueError("No closing quotation")
                index += 1
                continue
            chars.append(char)
            flags.append(False)
            index += 1
        flush_word(chars, quoted, word_space, flags)
    return tokens


def starts_with_heredoc_operator(chars, flags):
    """語の先頭（数字の fd 指定の後）に、引用されていない << があるかを返す。

    `<<'EOF'` のように区切り語だけが引用されている場合は演算子として扱う。
    `"<<EOF"` のように << 自体が引用符の中にある場合は、ヒアドキュメントの
    開始ではない。<<<（ヒアストリング）は本文を持たないため対象にしない。
    """
    index = 0
    while index < len(chars) and chars[index].isdigit() and not flags[index]:
        index += 1
    return (
        index + 1 < len(chars)
        and chars[index] == "<"
        and not flags[index]
        and chars[index + 1] == "<"
        and not flags[index + 1]
        and (index + 2 >= len(chars) or chars[index + 2] != "<")
    )


def heredoc_delimiter(token):
    """トークンがヒアドキュメントの開始（<<区切り語）なら、その区切り語を返す。

    区切り語が次のトークンに分かれている形式（<< だけのトークン）では空文字列を
    返す。ヒアドキュメントでない場合は None を返す。<<<（ヒアストリング）は
    本文を持たないため対象にしない。`<<` 自体が引用符の中にあるトークン
    （例: echo の引数 "<<EOF"）は、引用されていない `<<` で始まるトークン
    ではないため対象にしない。
    """
    if not getattr(token, "heredoc_operator", False):
        return None
    text = str(token)
    index = 0
    while index < len(text) and text[index].isdigit():
        index += 1
    rest = text[index:]
    if not rest.startswith("<<") or rest.startswith("<<<"):
        return None
    delimiter = rest[2:]
    if delimiter.startswith("-"):
        delimiter = delimiter[1:]
    return delimiter


def simple_commands(tokens):
    """トークン列を、&& / || / ; / | / 改行などの区切りで単純コマンドへ分ける。

    ヒアドキュメントの本文はコマンドとして実行されないため、コマンド解析の
    対象から外す。本文は通常、shell_tokens の段階で取り除かれている。ここでの
    読み飛ばしは、トークン列に開始のトークンが残っている場合の備えである。
    本文は、ヒアドキュメントを含むコマンド行の改行の直後から、区切り語だけの
    行までである。
    """
    current = []
    pending_delimiters = []
    body_delimiters = []
    body_line = []
    expecting_delimiter = False
    for token in tokens:
        if body_delimiters:
            if token == "\n" and is_shell_operator(token):
                if len(body_line) == 1 and body_line[0] == body_delimiters[0]:
                    body_delimiters.pop(0)
                body_line = []
            else:
                body_line.append(token)
            continue
        if token in SHELL_SEPARATORS and is_shell_operator(token):
            if current:
                yield current
            current = []
            if token == "\n" and pending_delimiters:
                body_delimiters = pending_delimiters
                pending_delimiters = []
                body_line = []
            continue
        if expecting_delimiter:
            pending_delimiters.append(str(token))
            expecting_delimiter = False
            current.append(token)
            continue
        delimiter = heredoc_delimiter(token)
        if delimiter is not None:
            if delimiter == "":
                expecting_delimiter = True
            else:
                pending_delimiters.append(delimiter)
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
    """成功条件が6検査の exit_code == 0 の同時成立なら、その検査名を返す。

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
    """machine_gate が mode: system で6検査を command として実行するかを検査する。"""
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
            f"machine_gate の system_inputs の as が6検査と一致しません: missing={missing}, extra={extra}"
        )
    for name in GATE_TARGETS:
        item = by_name[name]
        if item.get("type") != "command":
            raise GateError(
                f"machine_gate の {name} の system_input が type: command ではありません: "
                f"{item.get('type')!r}"
            )
        targets = invoked_make_targets(str(item.get("command", "")))
        make_target = gate_make_target(name)
        if make_target not in targets:
            raise GateError(
                f"{name} の command が make {make_target} を呼んでいません: {item.get('command')!r}"
            )
        other_targets = {gate_make_target(other) for other in GATE_TARGETS if other != name}
        others = sorted(targets & other_targets)
        if others:
            raise GateError(
                f"{name} の command が別の検査 {', '.join(others)} を呼んでいます: "
                f"{item.get('command')!r}"
            )
        expected = MACHINE_GATE_COMMANDS[name]
        command = str(item.get("command", "")).strip()
        if command != expected:
            raise GateError(
                f"{name} の command が検査を実行する正規形ではありません: {command!r} "
                f"（期待する形: {expected!r}）"
            )


def check_machine_gate_success_rule(gate_rules, next_name):
    """成功規則が6検査の exit_code == 0 の同時成立で next_name へ進むかを検査する。"""
    success_rules = [rule for rule in gate_rules if rule.get("next") == next_name]
    if len(success_rules) != 1:
        raise GateError(
            f"machine_gate に {next_name} へ進む成功規則が 1 つだけありません: {len(success_rules)}"
        )
    success_checks = machine_gate_success_checks(success_rules[0].get("condition"))
    if sorted(success_checks) != sorted(GATE_TARGETS):
        raise GateError(
            "machine_gate の成功条件が6検査"
            "（proto_check・fmt・clippy・test・typecheck・lint）の "
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


def check_fix_step_transitions(fix_name, fix_rules, return_to):
    """修正ステップの遷移先が、戻り先（return_to）か ABORT だけであることを検査する。"""
    allowed = {return_to, "ABORT"}
    unexpected = sorted({str(rule.get("next")) for rule in fix_rules if rule.get("next") not in allowed})
    if unexpected:
        raise GateError(
            f"{fix_name} に許可されない遷移先があります: {unexpected!r}。"
            f"修正ステップの遷移先は {return_to} か ABORT に限ります"
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
        fix_rules = rules_of(fix)
        check_fix_step_transitions(fix_name, fix_rules, "machine_gate")
        returns = [rule for rule in fix_rules if rule.get("next") == "machine_gate"]
        if len(returns) != 1:
            raise GateError(
                f"{fix_name} から machine_gate へ戻る遷移が 1 つだけありません: {len(returns)}"
            )


def check_machine_gate_transitions(gate_rules, next_name):
    """machine_gate の遷移先が既知のものだけで、ABORT が最後の when(true) かを検査する。"""
    allowed = {f"fix_machine_gate_{name}" for name in GATE_TARGETS} | {next_name, "ABORT"}
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


def check_machine_gate_five_checks(named, next_name):
    """machine_gate が6検査を実行し、検査ごとに失敗を振り分けるかを検査する。

    6検査の成功は next_name へ進む。公開ステップを置く場合はそのステップ名、
    置かない場合は aislop_gate を渡す。
    """
    gate = named["machine_gate"]
    check_machine_gate_inputs(gate)
    gate_rules = rules_of(gate)
    check_machine_gate_success_rule(gate_rules, next_name)
    check_machine_gate_failure_rules(gate_rules)
    check_machine_gate_fix_steps(named)
    check_machine_gate_transitions(gate_rules, next_name)
    return [f"fix_machine_gate_{name}" for name in GATE_TARGETS]


def transition(rules, predicate):
    for rule in rules:
        if predicate(rule):
            return rule.get("next")
    return None


PUBLISH_STAGES = ("commit", "push", "pr", "status")
PUBLISH_SUCCESS_STAGES = ("commit", "push", "pr")


ENV_ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")


def strip_command_prefix(tokens_of_command):
    """単純コマンドの先頭にある制御語と環境変数代入を読み飛ばす。

    `FOO=1 python3 ...` の代入はコマンド名ではなく、その後ろの実行ファイルが
    起動する。代入だけのコマンドは空の列になる。
    """
    while tokens_of_command and (
        tokens_of_command[0] in SHELL_KEYWORDS or ENV_ASSIGNMENT.match(tokens_of_command[0])
    ):
        tokens_of_command = tokens_of_command[1:]
    return tokens_of_command


def shell_command_string(arguments):
    """bash / sh の引数列から、-c で実行されるスクリプトを返す。

    -n（読み込むが実行しない）が付いている場合、そのスクリプトは実行されない
    ため None を返す。-c が無い場合（スクリプトファイルの実行など）も None を
    返す。-o のように引数を取るオプションは、その引数も読み飛ばす。
    """
    noexec = False
    index = 0
    while index < len(arguments):
        token = arguments[index]
        if token == "--":
            return None
        if token.startswith("+") and len(token) > 1:
            index += 1
            continue
        if not token.startswith("-") or token == "-":
            return None
        if token.startswith("--"):
            index += 1
            continue
        step = 1
        for position, option in enumerate(token[1:]):
            if option == "n":
                noexec = True
            elif option == "c":
                if noexec or index + 1 >= len(arguments):
                    return None
                return arguments[index + 1]
            elif option == "o":
                if position == len(token) - 2:
                    step = 2
                break
        index += step
    return None


def python_script_arguments(arguments):
    """python / python3 の引数列から、実行するスクリプトとその引数を返す。

    インタプリタのオプション（-u など。引数を取る -W / -X はその引数も）を
    読み飛ばす。-c や -m はスクリプトファイルの実行ではないため、その場合は
    None を返す。--help や --version のようにスクリプトを実行しない場合と、
    スクリプトが無い場合も None を返す。
    """
    index = 0
    while index < len(arguments):
        token = arguments[index]
        if token == "--":
            index += 1
            break
        if not token.startswith("-") or token == "-":
            break
        if token.startswith("--"):
            if token[2:].split("=", 1)[0] in ("help", "version"):
                return None
            index += 1
            continue
        step = 1
        for position, option in enumerate(token[1:]):
            if option in ("c", "m"):
                return None
            if option in ("W", "X"):
                if position == len(token) - 2:
                    step = 2
                break
        index += step
    if index >= len(arguments):
        return None
    return arguments[index:]


def invoked_publish_stages(command, depth=0):
    """コマンド文字列が実際に起動する publish.py の段階を集める。

    bash -c や sh -c の内側のスクリプトへ再帰し、単純コマンドの先頭にある
    publish.py の呼び出し（直接実行、または python / python3 の引数としての
    実行）だけを拾う。コマンド先頭の環境変数代入と python のオプションは
    読み飛ばす。-n 付きのシェルと、python の -c / -m は実行として扱わない。
    echo の引数や引用符内の文字列、コメント、ヒアドキュメントの本文は、
    コマンド文字列の任意の位置に publish.py と段階名が並んでいても起動として
    扱わない。
    """
    found = set()
    if depth > 5:
        return found
    try:
        tokens = shell_tokens(command)
    except ValueError:
        return found
    for tokens_of_command in simple_commands(tokens):
        tokens_of_command = strip_command_prefix(tokens_of_command)
        if not tokens_of_command:
            continue
        head = tokens_of_command[0]
        if head in ("bash", "sh"):
            script = shell_command_string(tokens_of_command[1:])
            if script is not None:
                found |= invoked_publish_stages(script, depth + 1)
            continue
        arguments = tokens_of_command[1:]
        if not head.endswith("publish.py"):
            if head.rsplit("/", 1)[-1] not in ("python", "python3"):
                continue
            script_arguments = python_script_arguments(arguments)
            if script_arguments is None or not script_arguments[0].endswith("publish.py"):
                continue
            arguments = script_arguments[1:]
        if arguments and arguments[0] in PUBLISH_STAGES:
            found.add(arguments[0])
    return found


def publish_stages(step):
    """公開ステップが実行する publish.py の段階を、{段階名: as 名} で返す。"""
    stages = {}
    for item in step.get("system_inputs") or []:
        if not isinstance(item, dict) or item.get("type") != "command":
            continue
        for stage in invoked_publish_stages(str(item.get("command", ""))):
            stages.setdefault(stage, item.get("as"))
    return stages


def publish_candidates(steps):
    """公開（commit / push / pr）を実行する mode: system のステップを返す。

    status だけを実行するステップ（開始時の In Progress 更新など）は公開ステップと
    みなさない。公開ステップを置くかどうかは任意である。
    """
    return [
        step
        for step in steps
        if isinstance(step, dict)
        and any(stage in publish_stages(step) for stage in PUBLISH_SUCCESS_STAGES)
    ]


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
    成功と判定できてしまうため受け付けない。規則の順序と各規則の遷移先も検査し、
    失敗時の修正ステップは return_to へ戻る。
    """
    gate_rules = rules_of(named[name])
    check_gate_rule_references(gate_rules, name)
    succeeds, failure_next = check_gate_rule_order(gate_rules, name)
    check_gate_fix_step(named, name, failure_next, return_to)
    return succeeds, failure_next


def check_gate_rule_order(gate_rules, name):
    """ゲートの規則が、成功規則 1 件と最後の when(true) 1 件を持ち、ほかは修正へ進むかを検査する。

    規則は順に評価されるため、成功規則があるだけでは、成功条件以外の規則から COMPLETE
    などへ進めてしまう。成功規則以外のすべての規則の遷移先を、最後の when(true) と同じ
    修正ステップに限る。戻り値は (成功規則の遷移先, 修正ステップ名)。
    """
    successes = [rule for rule in gate_rules if is_gate_success_condition(rule.get("condition"), name)]
    if not successes:
        raise GateError(f"{name} に成功（context.{name}.result.exit_code == 0）の遷移がありません")
    if len(successes) != 1:
        raise GateError(f"{name} の成功規則が 1 つだけではありません: {len(successes)}")
    fallbacks = [index for index, rule in enumerate(gate_rules) if rule.get("condition") == "when(true)"]
    if not fallbacks:
        raise GateError(f"{name} に失敗時の遷移（when(true)）がありません")
    if fallbacks != [len(gate_rules) - 1]:
        raise GateError(f"{name} の when(true) が規則列の最後に 1 つだけではありません")
    failure_next = gate_rules[-1].get("next")
    if failure_next == "ABORT":
        raise GateError(
            f"{name} の失敗が修正ステップではなく ABORT へ遷移しています。"
            f"失敗時は修正ステップ（例: fix_{name}）へ遷移してください"
        )
    for rule in gate_rules:
        if rule is successes[0]:
            continue
        if rule.get("next") == "COMPLETE":
            raise GateError(
                f"{name} の成功条件以外の規則が COMPLETE へ遷移しています: {rule.get('condition')!r}"
            )
        if rule.get("next") != failure_next:
            raise GateError(
                f"{name} の成功条件以外の規則の遷移先が修正ステップ {failure_next} ではありません: "
                f"{rule.get('condition')!r} -> {rule.get('next')!r}"
            )
    return successes[0].get("next"), failure_next


def check_gate_fix_step(named, name, fix_name, return_to):
    """ゲートの修正ステップが、ゲートの出力を参照し、return_to か ABORT へだけ進むかを検査する。"""
    fix = named.get(fix_name)
    if fix is None:
        raise GateError(f"{name} の失敗時の遷移先 {fix_name!r} のステップがありません")
    text = instruction_text(fix)
    if f"context:{name}.result.stdout" not in text and (
        f"context:{name}.result.stderr" not in text
    ):
        raise GateError(f"{fix_name} の instruction に {name} の出力の参照がありません")
    fix_rules = rules_of(fix)
    check_fix_step_transitions(fix_name, fix_rules, return_to)
    if not any(rule.get("next") == return_to for rule in fix_rules):
        raise GateError(f"{fix_name} から {return_to} へ戻る遷移がありません")


def publish_step(steps):
    """publish.py で公開を実行する mode: system のステップと、そのステップ名を返す。"""
    publishers = publish_candidates(steps)
    if not publishers:
        raise GateError("publish.py で公開を実行する mode: system のステップがありません")
    publisher = max(publishers, key=lambda step: len(publish_stages(step)))
    publish_name = publisher["name"]
    if publisher.get("mode") != "system":
        raise GateError(f"公開ステップ {publish_name} が mode: system ではありません")
    stages = publish_stages(publisher)
    missing = [stage for stage in PUBLISH_STAGES if stage not in stages]
    if missing:
        raise GateError(
            f"公開ステップ {publish_name} に {' / '.join(missing)} の入力（publish.py <段階>）がありません"
        )
    for stage in PUBLISH_STAGES:
        if stages[stage] != stage:
            raise GateError(
                f"公開ステップ {publish_name} の {stage} 入力の as が {stage} ではありません: {stages[stage]!r}"
            )
    return publisher, publish_name


def publish_failure_step(publish_rules, publish_name, named):
    """公開ステップの失敗時の遷移と、その遷移先の修正ステップを返す。"""
    publish_failure_next = transition(publish_rules, lambda rule: rule.get("condition") == "when(true)")
    if publish_failure_next is None:
        raise GateError(f"公開ステップ {publish_name} に失敗時の遷移（when(true)）がありません")
    retry = named.get(publish_failure_next)
    if publish_failure_next == "COMPLETE" or retry is None:
        raise GateError(
            f"公開の失敗時の遷移が修正ステップ（fix_publish）へ行っていません: {publish_failure_next!r}"
        )
    return publish_failure_next, retry


def check_publish_success_rule(publish_rules, publish_name, aislop_name):
    """公開ステップの成功条件が aislop ゲートへ進む形になっていることを確かめる。

    規則は先に一致したものから順に評価されるため、成功規則が when(true) の
    無条件規則より後にあると、公開が成功しても無条件規則が先に選ばれて
    aislop ゲートへ進まない。成功規則が無条件規則より前にあることも確かめる。
    """
    success_rules = [rule for rule in publish_rules if rule.get("condition") != "when(true)"]
    if len(success_rules) != 1:
        raise GateError(
            f"公開ステップ {publish_name} で成功を表す rule が1件ではありません: {len(success_rules)} 件"
        )
    success_rule = success_rules[0]
    fallback_index = next(
        (index for index, rule in enumerate(publish_rules) if rule.get("condition") == "when(true)"),
        None,
    )
    if fallback_index is not None and publish_rules.index(success_rule) > fallback_index:
        raise GateError(
            f"公開ステップ {publish_name} の成功規則が when(true) の無条件規則より後にあります。"
            "規則は先に一致したものが選ばれるため、公開が成功しても無条件規則が先に選ばれ、"
            f"{aislop_name} へ進みません"
        )
    publish_success_next = success_rule.get("next")
    if publish_success_next != aislop_name:
        raise GateError(
            f"公開ステップ {publish_name} の成功が {aislop_name} へ遷移していません: {publish_success_next!r}"
        )
    success_condition = str(success_rule.get("condition", "")).strip()
    if not (success_condition.startswith("when(") and success_condition.endswith(")")):
        raise GateError(
            f"公開ステップ {publish_name} の完了条件が when(...) ではありません: {success_condition!r}"
        )
    inner = success_condition[len("when(") : -1]
    if "||" in inner:
        raise GateError(
            f"公開ステップ {publish_name} の完了条件に || があります。commit / push / pr を && で結合してください"
        )
    if f"{publish_name}.status.exit_code" in inner:
        raise GateError("公開の成功条件に status の exit_code を含めています（Status の失敗は非致命です）")
    if "!=" in inner:
        raise GateError(
            f"公開ステップ {publish_name} の完了条件が exit_code == 0 ではありません: {success_condition!r}"
        )
    parts = [part.strip() for part in inner.split("&&")]
    for stage in PUBLISH_SUCCESS_STAGES:
        if f"context.{publish_name}.{stage}.exit_code == 0" not in parts:
            raise GateError(f"公開の成功条件に {stage} の exit_code がありません")
    expected = [f"context.{publish_name}.{stage}.exit_code == 0" for stage in PUBLISH_SUCCESS_STAGES]
    if sorted(parts) != sorted(expected):
        raise GateError(
            f"公開ステップ {publish_name} の完了条件は commit / push / pr の exit_code == 0 を"
            f"単一の when(...) 内で && 結合する必要があります: {success_condition!r}"
        )


def check_publish_retry_rule(retry, publish_failure_next, publish_name):
    """公開の失敗後の修正ステップが、公開ステップへ戻る形になっていることを確かめる。"""
    retry_rules = rules_of(retry)
    retry_nexts = [rule.get("next") for rule in retry_rules]
    if "ABORT" in retry_nexts:
        raise GateError(f"{publish_failure_next} の遷移に ABORT があります")
    if publish_name not in retry_nexts:
        raise GateError(f"{publish_failure_next} から {publish_name} へ戻る遷移がありません")
    retry_instruction = retry.get("instruction")
    retry_text = (
        "\n".join(retry_instruction)
        if isinstance(retry_instruction, list)
        else str(retry_instruction or "")
    )
    if f"context:{publish_name}." not in retry_text:
        raise GateError(
            f"{publish_failure_next} の instruction に公開ステップの出力の参照がありません"
        )


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
        raise GateError(
            "aislop-gate を実行する mode: system のステップがありません"
            f"（{AISLOP_FAILURE_NOTE}）"
        )
    aislop_name = aislop_steps[0]["name"]

    publishers = publish_candidates(steps)
    if publishers:
        publisher, publish_name = publish_step(steps)
    else:
        publisher, publish_name = None, None

    machine_success_next = publish_name or aislop_name
    fix_names = check_machine_gate_five_checks(named, machine_success_next)

    order = [step.get("name") for step in steps if isinstance(step, dict)]
    if order.index("develop") > order.index("machine_gate"):
        raise GateError("machine_gate が develop より前の位置にあります")
    if order.index("machine_gate") > order.index(aislop_name):
        raise GateError(f"{aislop_name} が machine_gate より前の位置にあります")
    aislop_next, aislop_fix = check_machine_gate_step(named, aislop_name, "machine_gate")
    if aislop_next != "COMPLETE":
        raise GateError(f"{aislop_name} の成功（exit_code == 0）で COMPLETE へ遷移していません: {aislop_next!r}")

    if publisher is None:
        publish_failure_next = None
    else:
        publish_rules = rules_of(publisher)
        publish_failure_next, retry = publish_failure_step(publish_rules, publish_name, named)
        check_publish_success_rule(publish_rules, publish_name, aislop_name)
        check_publish_retry_rule(retry, publish_failure_next, publish_name)

    return {
        "gates": ["machine_gate", aislop_name],
        "checks": list(GATE_TARGETS),
        "failure_steps": fix_names + [aislop_fix],
        "develop_complete": complete_next,
        "publish": publish_name,
        "publish_failure_step": publish_failure_next,
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
            problems.append(
                f"aislop のゲート（node {AISLOP_CHECK_COMMAND}）を呼んでいません"
                f"（{AISLOP_FAILURE_NOTE}）"
            )
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
