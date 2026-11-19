"""Makefile のレシピを解析し、make done が関門の検査をすべて実行するかを検査する。"""
import re
import shlex

from gate_common import GATE_TARGETS, GateError, gate_make_target, read_text


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


def check_machine_gate(root, timeout_seconds, run):
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

    result = run(["make", "done"], root, timeout_seconds)
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
