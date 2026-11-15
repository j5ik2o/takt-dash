#!/usr/bin/env python3
"""repo_gate.py の判定を、一時ディレクトリの小さなリポジトリで確かめる。

使い方:
    python3 tools/repo-gate/repo_gate_test.py

各テストは、利用者の実環境の設定に依存しないよう、リポジトリのルートを一時ディレクトリ
へ作り、一時 HOME へ隔離してから repo_gate を import する。関門が起動する外部コマンド
（cargo metadata と make done）は、テスト専用の応答へ差し替え、本物の cargo や npm や
make は呼ばない。

次の観点を確かめる。
  1. 正しいリポジトリはすべての検査に通る。
  2. ワークスペースのメンバー、crate の種類、frontend/web の scripts、proto/ の欠落を、
     それぞれ別の違反として報告する。
  3. make done が検査を省略する、clippy を -- -D warnings なしで呼ぶ、依存の導入手順を
     持たない、検査が失敗する、のそれぞれで失敗する。
  4. develop の COMPLETE が機械ゲートへ遷移しない、修正ステップが make done の出力を
     参照しない、修正から機械ゲートへ戻らない、のそれぞれで失敗する。
  5. CI が make done を呼ばない、nightly と rustfmt をセットアップしない、Node を
     セットアップしない、のそれぞれで失敗する。
  6. .gitignore に生成物の除外が無い、rust-toolchain.toml のチャネルが stable でない、
     のそれぞれで失敗する。
"""
import importlib.util
import io
import json
import os
import shutil
import signal
import sys
import tempfile
import traceback
from pathlib import Path

GATE = Path(__file__).resolve().parent / "repo_gate.py"

# make や npm を誤って本物のまま呼ぶ実装の欠陥で、テスト自体が固まらないよう、
# 1 テストの上限を置く。
TEST_TIMEOUT_SECONDS = 120


class TestTimeout(Exception):
    pass


def _on_timeout(signum, frame):
    raise TestTimeout()


class Sandbox:
    """検査の対象になる小さなリポジトリと、その外側に置く一時 HOME を持つ。"""

    def __init__(self) -> None:
        self.base = Path(tempfile.mkdtemp(prefix="repo-gate-test-"))
        self.root = self.base / "repo"
        self.home = self.base / "home"
        self.root.mkdir()
        self.home.mkdir()
        self.original_environment = dict(os.environ)
        self.module = None

    def __enter__(self):
        os.environ["HOME"] = str(self.home)
        os.environ["USERPROFILE"] = str(self.home)
        self.module = load_gate()
        return self

    def __exit__(self, *exc):
        os.environ.clear()
        os.environ.update(self.original_environment)
        shutil.rmtree(self.base, ignore_errors=True)
        return False

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def run(self, argv) -> tuple:
        stdout, stderr = io.StringIO(), io.StringIO()
        saved = sys.stdout, sys.stderr
        sys.stdout, sys.stderr = stdout, stderr
        try:
            code = self.module.main(["--root", str(self.root), *argv])
        finally:
            sys.stdout, sys.stderr = saved
        return code, stdout.getvalue(), stderr.getvalue()


def load_gate():
    spec = importlib.util.spec_from_file_location("repo_gate", GATE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CARGO_WORKSPACE = '[workspace]\nresolver = "2"\nmembers = ["backend", "frontend/cli"]\n'

BACKEND_MANIFEST = (
    '[package]\nname = "takt-dash-backend"\nversion = "0.1.0"\nedition = "2021"\n'
)

CLI_MANIFEST = '[package]\nname = "takt-dash-cli"\nversion = "0.1.0"\nedition = "2021"\n'

WEB_MANIFEST = json.dumps(
    {
        "name": "web",
        "private": True,
        "scripts": {
            "dev": "next dev",
            "build": "next build",
            "start": "next start",
            "lint": "eslint",
            "typecheck": "next typegen && tsc --noEmit",
            "test": "node --test smoke.test.mjs",
        },
        "dependencies": {"next": "16.0.0", "react": "19.0.0", "react-dom": "19.0.0"},
        "devDependencies": {"typescript": "5.0.0", "eslint": "9.0.0"},
    },
    indent=2,
)

MAKEFILE = """\
.PHONY: fmt clippy test typecheck lint done

fmt:
	cargo +nightly fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace
	npm --prefix frontend/web run test

typecheck:
	npm --prefix frontend/web run typecheck

lint:
	npm --prefix frontend/web run lint

done:
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	$(MAKE) fmt
	$(MAKE) clippy
	$(MAKE) test
	$(MAKE) typecheck
	$(MAKE) lint
"""

CI = """\
name: CI
on:
  pull_request:
jobs:
  done:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
      - uses: dtolnay/rust-toolchain@nightly
        with:
          components: rustfmt
      - uses: actions/setup-node@v4
        with:
          node-version: 22
      - run: make done
      - run: npm ci
      - run: node scripts/aislop-check-bootstrap.mjs check
"""

WORKFLOW = """\
name: taktdash-change
max_steps: 70
initial_step: develop
steps:
  - name: develop
    description: development-core を呼ぶ
    kind: workflow_call
    call: development-core
    rules:
      - condition: COMPLETE
        next: machine_gate
      - condition: ABORT
        next: ABORT
  - name: machine_gate
    description: make done を実行する
    mode: system
    system_inputs:
      - type: command
        as: result
        command: make done
        timeout_ms: 1800000
    rules:
      - condition: when(context.machine_gate.result.exit_code == 0)
        next: aislop_gate
      - condition: when(true)
        next: fix_machine_gate
  - name: aislop_gate
    description: aislop で警告以上の指摘を検査する
    mode: system
    system_inputs:
      - type: command
        as: result
        command: npm ci && node scripts/aislop-check-bootstrap.mjs check
        timeout_ms: 900000
    rules:
      - condition: when(context.aislop_gate.result.exit_code == 0)
        next: COMPLETE
      - condition: when(true)
        next: fix_aislop_gate
  - name: fix_machine_gate
    description: make done の失敗を修正する
    capabilities: edit
    edit: true
    instruction:
      - fix-supervisor
      - |
        {context:machine_gate.result.stdout}
        {context:machine_gate.result.stderr}
    rules:
      - condition: make done の失敗を修正して検証した
        next: machine_gate
      - condition: 修正できない
        next: ABORT
  - name: fix_aislop_gate
    description: aislop の警告以上の指摘を修正する
    capabilities: edit
    edit: true
    instruction:
      - fix-supervisor
      - |
        {context:aislop_gate.result.stdout}
        {context:aislop_gate.result.stderr}
    rules:
      - condition: aislop の指摘を修正して検証した
        next: machine_gate
      - condition: 修正できない
        next: ABORT
"""

GITIGNORE = """\
/target
**/*.rs.bk
node_modules/
.next/
next-env.d.ts
*.sqlite
"""

TOOLCHAIN = '[toolchain]\nchannel = "stable"\n'

# 関門のコマンドは外部へ何も出力しない小さな脚本に置き換える。make done の成否は
# rest に置いた検査の記録から決め、env.toml の fail_targets に並べた検査だけ失敗させる。


def repository(sandbox, makefile=MAKEFILE, ci=CI, workflow=WORKFLOW, gitignore=GITIGNORE, toolchain=TOOLCHAIN):
    sandbox.write("Cargo.toml", CARGO_WORKSPACE)
    sandbox.write("backend/Cargo.toml", BACKEND_MANIFEST)
    sandbox.write("backend/src/lib.rs", "")
    sandbox.write("frontend/cli/Cargo.toml", CLI_MANIFEST)
    sandbox.write("frontend/cli/src/main.rs", "fn main() {}\n")
    sandbox.write("frontend/web/package.json", WEB_MANIFEST)
    sandbox.write("proto/README.md", "protobuf\n")
    if makefile is not None:
        sandbox.write("Makefile", makefile)
    if ci is not None:
        sandbox.write(".github/workflows/ci.yml", ci)
    if workflow is not None:
        sandbox.write(".takt/workflows/taktdash-change.yaml", workflow)
    if gitignore is not None:
        sandbox.write(".gitignore", gitignore)
    if toolchain is not None:
        sandbox.write("rust-toolchain.toml", toolchain)


def stub_external_tools(sandbox, make_failure=False):
    """cargo と npm と make を、記録だけする応答へ差し替える。"""
    record = sandbox.base / "calls.log"

    def fake_run(command, cwd, timeout_seconds):
        with record.open("a", encoding="utf-8") as handle:
            handle.write(" ".join(command) + "\n")
        if command[:1] == ["cargo"] and command[1:2] == ["metadata"]:
            return metadata_response(command, sandbox)
        if command == ["make", "done"]:
            if make_failure:
                return {"status": 2, "stdout": "", "stderr": "make: *** [done] Error 1"}
            return {"status": 0, "stdout": "", "stderr": ""}
        return {"status": 0, "stdout": "", "stderr": ""}

    sandbox.module._run = fake_run
    return record


def metadata_response(command, sandbox):
    sandbox_manifest = sandbox.root / "Cargo.toml"
    if sandbox.module.tomllib is None or not sandbox_manifest.exists():
        return {"status": 101, "stdout": "", "stderr": "Cargo.toml がありません"}
    manifest = sandbox.module.tomllib.loads(sandbox_manifest.read_text(encoding="utf-8"))
    members = (manifest.get("workspace") or {}).get("members") or []
    packages = []
    for member in members:
        member_manifest = sandbox.root / member / "Cargo.toml"
        if not member_manifest.exists():
            return {
                "status": 101,
                "stdout": "",
                "stderr": f"{member_manifest} がありません",
            }
        data = sandbox.module.tomllib.loads(member_manifest.read_text(encoding="utf-8"))
        name = data["package"]["name"]
        kinds = []
        sources = []
        if (sandbox.root / member / "src/lib.rs").exists():
            kinds.append("lib")
            sources.append("src/lib.rs")
        if (sandbox.root / member / "src/main.rs").exists():
            kinds.append("bin")
            sources.append("src/main.rs")
        packages.append(
            {
                "name": name,
                "id": f"path+file://{sandbox.root}/{member}#{name}",
                "manifest_path": str(member_manifest),
                "targets": [{"kind": kinds, "src_path": str(sandbox.root / member / source)} for source in sources],
            }
        )
    payload = {
        "packages": packages,
        "workspace_members": [package["id"] for package in packages],
    }
    return {"status": 0, "stdout": json.dumps(payload), "stderr": ""}


def called_commands(record):
    if not record.exists():
        return []
    return [line.strip() for line in record.read_text(encoding="utf-8").splitlines() if line.strip()]


def assert_failure(sandbox, label, name, snippet):
    code, out, err = sandbox.run([])
    assert code != 0, f"{label}: 違反があるのに成功しました。stdout={out!r}"
    lines = [line for line in err.splitlines() if line.startswith(f"{name}:")]
    assert len(lines) == 1, f"{label}: {name} の違反が 1 行で報告されません: {err!r}"
    assert snippet in lines[0], f"{label}: 違反の理由に {snippet!r} が含まれません: {lines[0]!r}"


def assert_success(sandbox, label):
    code, out, err = sandbox.run([])
    assert code == 0, f"{label}: 成功するはずのリポジトリが失敗しました: {err!r}"
    payload = json.loads(out)
    names = {check["name"] for check in payload["checks"]}
    assert names == {
        "workspace-layout",
        "machine-gate",
        "ci-runs-make-done",
        "workflow-machine-gate",
        "gitignore-rules",
        "stable-toolchain",
    }, f"{label}: 検査の内訳が違います: {names}"
    return payload


def test_correct_repository_passes() -> None:
    with Sandbox() as sandbox:
        repository(sandbox)
        stub_external_tools(sandbox)
        assert_success(sandbox, "correct")


def test_workspace_members_must_be_backend_and_cli() -> None:
    for label, cargo_workspace in {
        "proto を含む": '[workspace]\nresolver = "2"\nmembers = ["backend", "frontend/cli", "proto"]\n',
        "frontend/web を含む": '[workspace]\nresolver = "2"\nmembers = ["backend", "frontend/cli", "frontend/web"]\n',
        "cli が無い": '[workspace]\nresolver = "2"\nmembers = ["backend"]\n',
        "workspace が無い": '[package]\nname = "root"\nversion = "0.1.0"\nedition = "2021"\n',
    }.items():
        with Sandbox() as sandbox:
            repository(sandbox)
            sandbox.write("Cargo.toml", cargo_workspace)
            stub_external_tools(sandbox)
            code, out, err = sandbox.run([])
            assert code != 0, f"workspace members ({label}): 違反があるのに成功しました"
            assert err.strip().startswith("workspace-layout:"), f"workspace members ({label}): {err!r}"


def test_backend_must_have_lib_and_cli_must_have_bin() -> None:
    with Sandbox() as sandbox:
        repository(sandbox)
        (sandbox.root / "backend/src/lib.rs").unlink()
        sandbox.write("backend/src/main.rs", "fn main() {}\n")
        stub_external_tools(sandbox)
        assert_failure(sandbox, "backend が lib を持たない", "workspace-layout", "lib")

    with Sandbox() as sandbox:
        repository(sandbox)
        (sandbox.root / "frontend/cli/src/main.rs").unlink()
        sandbox.write("frontend/cli/src/lib.rs", "")
        stub_external_tools(sandbox)
        assert_failure(sandbox, "frontend/cli が bin を持たない", "workspace-layout", "bin")


def test_web_manifest_requires_scripts_and_next() -> None:
    cases = {
        "typecheck が無い": {"scripts": {"lint": "eslint", "test": "node --test smoke.test.mjs"}},
        "lint が無い": {"scripts": {"typecheck": "tsc --noEmit", "test": "node --test smoke.test.mjs"}},
        "test が無い": {"scripts": {"typecheck": "tsc --noEmit", "lint": "eslint"}},
        "next が無い": {
            "scripts": {"typecheck": "tsc --noEmit", "lint": "eslint", "test": "node --test smoke.test.mjs"},
            "dependencies": {"react": "19.0.0"},
        },
    }
    for label, overrides in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox)
            manifest = json.loads(WEB_MANIFEST)
            manifest.update(overrides)
            sandbox.write("frontend/web/package.json", json.dumps(manifest))
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workspace-layout", "frontend/web")


def test_proto_directory_is_required() -> None:
    with Sandbox() as sandbox:
        repository(sandbox)
        shutil.rmtree(sandbox.root / "proto")
        stub_external_tools(sandbox)
        assert_failure(sandbox, "proto/ が無い", "workspace-layout", "proto/")


def test_make_done_must_run_every_check() -> None:
    for target in ("fmt", "clippy", "test", "typecheck", "lint"):
        with Sandbox() as sandbox:
            makefile = "\n".join(
                line for line in MAKEFILE.splitlines() if line.strip() != f"$(MAKE) {target}"
            )
            repository(sandbox, makefile=makefile + "\n")
            stub_external_tools(sandbox)
            assert_failure(sandbox, f"done が {target} を実行しない", "machine-gate", target)


def test_make_done_must_fail_when_a_check_fails() -> None:
    with Sandbox() as sandbox:
        repository(sandbox)
        stub_external_tools(sandbox, make_failure=True)
        assert_failure(sandbox, "検査が失敗する", "machine-gate", "make done が成功しませんでした")


def test_clippy_must_reject_warnings() -> None:
    with Sandbox() as sandbox:
        repository(
            sandbox,
            makefile=MAKEFILE.replace(
                "cargo clippy --workspace --all-targets -- -D warnings",
                "cargo clippy --workspace --all-targets",
            ),
        )
        stub_external_tools(sandbox)
        assert_failure(sandbox, "clippy が警告を失敗にしない", "machine-gate", "-- -D warnings")


def test_done_must_install_node_dependencies_when_missing() -> None:
    makefile = """\
.PHONY: fmt clippy test typecheck lint done

fmt:
	cargo +nightly fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace
	npm --prefix frontend/web run test

typecheck:
	npm --prefix frontend/web run typecheck

lint:
	npm --prefix frontend/web run lint

done:
	$(MAKE) fmt
	$(MAKE) clippy
	$(MAKE) test
	$(MAKE) typecheck
	$(MAKE) lint
"""
    with Sandbox() as sandbox:
        repository(sandbox, makefile=makefile)
        stub_external_tools(sandbox)
        assert_failure(sandbox, "依存の導入が無い", "machine-gate", "node_modules")


def test_workflow_gate_wiring() -> None:
    cases = {
        "develop が機械ゲートへ遷移しない": (
            WORKFLOW.replace("      - condition: COMPLETE\n        next: machine_gate", "      - condition: COMPLETE\n        next: COMPLETE"),
            "develop の COMPLETE",
        ),
        "make done の成功で aislop ステップへ渡さない": (
            WORKFLOW.replace("        next: aislop_gate\n      - condition: when(true)\n        next: fix_machine_gate", "        next: COMPLETE\n      - condition: when(true)\n        next: fix_machine_gate"),
            "aislop",
        ),
        "成功で完了しない": (
            WORKFLOW.replace("next: COMPLETE\n      - condition: when(true)\n        next: fix_aislop_gate", "next: fix_aislop_gate\n      - condition: when(true)\n        next: fix_aislop_gate"),
            "COMPLETE",
        ),
        "修正が make done の出力を参照しない": (
            WORKFLOW.replace("        {context:machine_gate.result.stdout}\n        {context:machine_gate.result.stderr}\n", "        make done の出力を確認してください\n"),
            "instruction",
        ),
        "修正から機械ゲートへ戻らない": (
            WORKFLOW.replace(
                "        {context:machine_gate.result.stderr}\n    rules:\n      - condition: make done の失敗を修正して検証した\n        next: machine_gate",
                "        {context:machine_gate.result.stderr}\n    rules:\n      - condition: make done の失敗を修正して検証した\n        next: COMPLETE",
            ),
            "machine_gate",
        ),
        "機械ゲートが system でない": (
            WORKFLOW.replace("    mode: system\n    system_inputs:", "    capabilities: readonly\n    system_inputs:", 1),
            "mode: system",
        ),
    }
    for label, (workflow, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_workflow_requires_aislop_gate_step() -> None:
    """aislop の機械ゲートが無い、順序が違う、失敗時に戻らない、のそれぞれで失敗する。"""
    without_step = WORKFLOW.replace(
        """  - name: aislop_gate
    description: aislop で警告以上の指摘を検査する
    mode: system
    system_inputs:
      - type: command
        as: result
        command: npm ci && node scripts/aislop-check-bootstrap.mjs check
        timeout_ms: 900000
    rules:
      - condition: when(context.aislop_gate.result.exit_code == 0)
        next: COMPLETE
      - condition: when(true)
        next: fix_aislop_gate
""",
        "",
    ).replace(
        "        next: aislop_gate\n      - condition: when(true)\n        next: fix_machine_gate",
        "        next: COMPLETE\n      - condition: when(true)\n        next: fix_machine_gate",
    )
    cases = {
        "aislop ステップが無い": (without_step, "aislop"),
        "aislop ステップが make done より前にある": (
            WORKFLOW.replace(
                "      - condition: COMPLETE\n        next: machine_gate",
                "      - condition: COMPLETE\n        next: aislop_gate",
            ).replace(
                "        next: aislop_gate\n      - condition: when(true)\n        next: fix_machine_gate",
                "        next: COMPLETE\n      - condition: when(true)\n        next: fix_machine_gate",
            ),
            "aislop",
        ),
        "aislop の失敗で修正へ遷移しない": (
            WORKFLOW.replace("        next: fix_aislop_gate", "        next: ABORT"),
            "fix_aislop_gate",
        ),
        "aislop の修正から machine_gate へ戻らない": (
            WORKFLOW.replace(
                "      - condition: aislop の指摘を修正して検証した\n        next: machine_gate",
                "      - condition: aislop の指摘を修正して検証した\n        next: aislop_gate",
            ),
            "machine_gate",
        ),
    }
    for label, (workflow, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_aislop_fix_must_reenter_machine_gate() -> None:
    """aislop の修正は、make done をやり直す機械ゲートへ戻らなければならない。

    修正後の成果物に make done を再実行せず COMPLETE へ到達できる遷移と、
    aislop だけを再実行して COMPLETE へ到達できる遷移を拒否する。
    """
    cases = {
        "aislop の修正が COMPLETE へ直接遷移する": (
            WORKFLOW.replace(
                "      - condition: aislop の指摘を修正して検証した\n        next: machine_gate",
                "      - condition: aislop の指摘を修正して検証した\n        next: COMPLETE",
            ),
            "machine_gate",
        ),
        "aislop の修正が aislop へ直接戻る": (
            WORKFLOW.replace(
                "      - condition: aislop の指摘を修正して検証した\n        next: machine_gate",
                "      - condition: aislop の指摘を修正して検証した\n        next: aislop_gate",
            ),
            "machine_gate",
        ),
    }
    for label, (workflow, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_aislop_gate_command_detection() -> None:
    """aislop チェックを実際に起動するコマンドだけを aislop ゲートと判定する。

    echo・引用符内の文字列・コメントは実行扱いせず、mode が system でないステップも
    対象にしない。
    """
    with Sandbox() as sandbox:
        module = sandbox.module

        def step(command, mode="system"):
            return {
                "name": "aislop_gate",
                "mode": mode,
                "system_inputs": [{"type": "command", "as": "result", "command": command}],
            }

        assert module.runs_aislop_gate(step("node scripts/aislop-check-bootstrap.mjs check")), "単独の起動を許可する"
        assert module.runs_aislop_gate(step("npm ci && node scripts/aislop-check-bootstrap.mjs check")), "前段付きの起動を許可する"
        assert module.runs_aislop_gate(
            step("echo 準備 # コメント\nnode scripts/aislop-check-bootstrap.mjs check")
        ), "コメント行の次の実起動を許可する"
        assert not module.runs_aislop_gate(step("echo scripts/aislop-check-bootstrap.mjs check")), "echo を実行扱いしない"
        assert not module.runs_aislop_gate(step('echo "node scripts/aislop-check-bootstrap.mjs check"')), "引用符内を実行扱いしない"
        assert not module.runs_aislop_gate(step("# node scripts/aislop-check-bootstrap.mjs check")), "コメントを実行扱いしない"
        assert not module.runs_aislop_gate(
            step("echo skipped # && node scripts/aislop-check-bootstrap.mjs check")
        ), "コメント内の && の後ろを実行扱いしない"
        assert not module.runs_aislop_gate(
            step("# 実行例: npm ci || node scripts/aislop-check-bootstrap.mjs check")
        ), "コメント行内の || の後ろを実行扱いしない"
        assert not module.runs_aislop_gate(
            step("echo skipped # ; node scripts/aislop-check-bootstrap.mjs check")
        ), "コメント内の ; の後ろを実行扱いしない"
        assert not module.runs_aislop_gate(
            step("echo skipped # | node scripts/aislop-check-bootstrap.mjs check")
        ), "コメント内の | の後ろを実行扱いしない"
        assert module.runs_aislop_gate(
            step('echo "参照: # ではない" # コメント\nnode scripts/aislop-check-bootstrap.mjs check')
        ), "引用符内の # をコメント開始とみなさない"
        assert not module.runs_aislop_gate(
            step("node scripts/aislop-check-bootstrap.mjs check", mode="agent")
        ), "mode が system でないステップを対象にしない"


def test_aislop_gate_command_must_invoke_the_check() -> None:
    """echo やコメント内の文字列を aislop ゲートとして扱わない。"""
    cases = {
        "echo で文字列を表示するだけ": "        command: 'echo scripts/aislop-check-bootstrap.mjs check'",
        "コメント内の演算子の後ろにだけ書く": (
            "        command: 'echo skipped # && node scripts/aislop-check-bootstrap.mjs check'"
        ),
        "コメント行にだけ書く": "        command: '# node scripts/aislop-check-bootstrap.mjs check'",
    }
    for label, replacement in cases.items():
        with Sandbox() as sandbox:
            workflow = WORKFLOW.replace(
                "        command: npm ci && node scripts/aislop-check-bootstrap.mjs check",
                replacement,
            )
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", "aislop-gate を実行する")


def test_ci_aislop_gate_command_must_invoke_the_check() -> None:
    """CI でも、aislop チェックを実際に起動しないコマンドを呼び出しとみなさない。"""
    cases = {
        "echo で文字列を表示するだけ": "      - run: echo scripts/aislop-check-bootstrap.mjs check\n",
        "引用符の中にだけコマンドを書く": '      - run: echo "node scripts/aislop-check-bootstrap.mjs check"\n',
        "コメント内の演算子の後ろにだけ書く": (
            "      - run: echo skipped # && node scripts/aislop-check-bootstrap.mjs check\n"
        ),
        "コメント行にだけ書く": "      - run: '# node scripts/aislop-check-bootstrap.mjs check'\n",
    }
    for label, run in cases.items():
        with Sandbox() as sandbox:
            ci = CI.replace("      - run: node scripts/aislop-check-bootstrap.mjs check\n", run)
            repository(sandbox, ci=ci)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "ci-runs-make-done", "aislop のゲート")


def test_ci_aislop_gate_accepts_multiline_run_with_comment() -> None:
    """コメント行を含む複数行の run でも、実際に起動していれば受理する。"""
    multi_line = (
        "      - run: |\n"
        "          # 依存を導入する\n"
        "          npm ci\n"
        "          node scripts/aislop-check-bootstrap.mjs check\n"
    )
    with Sandbox() as sandbox:
        ci = CI.replace("      - run: node scripts/aislop-check-bootstrap.mjs check\n", multi_line)
        repository(sandbox, ci=ci)
        stub_external_tools(sandbox)
        assert_success(sandbox, "コメント行を含む複数行の run")


def test_gate_success_condition_must_reference_its_own_gate() -> None:
    """各ゲートの成功条件は、別ゲートではなくそのゲート自身の exit_code だけを見る。"""
    success = "      - condition: when(context.aislop_gate.result.exit_code == 0)"
    cases = {
        "aislop_gate が machine_gate の exit_code を成功条件にする": (
            WORKFLOW.replace(
                "      - condition: when(context.aislop_gate.result.exit_code == 0)",
                "      - condition: when(context.machine_gate.result.exit_code == 0)",
            ),
            "aislop_gate",
        ),
        "machine_gate が aislop_gate の exit_code を成功条件にする": (
            WORKFLOW.replace(
                "      - condition: when(context.machine_gate.result.exit_code == 0)",
                "      - condition: when(context.aislop_gate.result.exit_code == 0)",
            ),
            "machine_gate",
        ),
        "aislop_gate が別ゲートの成功を AND で併記する": (
            WORKFLOW.replace(
                success,
                "      - condition: when(context.aislop_gate.result.exit_code != 0 "
                "&& context.machine_gate.result.exit_code == 0)",
            ),
            "aislop_gate",
        ),
        "aislop_gate が別ゲートの成功を OR で併記する": (
            WORKFLOW.replace(
                success,
                "      - condition: when(context.aislop_gate.result.exit_code == 0 "
                "|| context.machine_gate.result.exit_code == 0)",
            ),
            "aislop_gate",
        ),
        "aislop_gate が自ゲートの否定とリテラル真を併記する": (
            WORKFLOW.replace(
                success,
                "      - condition: when(context.aislop_gate.result.exit_code != 0 || 0 == 0)",
            ),
            "aislop_gate",
        ),
    }
    # 成功条件の規則そのものが正規形でも、別の規則が別ゲートの exit_code == 0 を見ると、
    # 規則は先に一致したものが選ばれるため、そのゲートの成功で次へ進めてしまう。
    aislop_success_rule = (
        "      - condition: when(context.aislop_gate.result.exit_code == 0)\n"
        "        next: COMPLETE\n"
    )
    machine_success_rule = (
        "      - condition: when(context.machine_gate.result.exit_code == 0)\n"
        "        next: aislop_gate\n"
    )
    other_gate_rules = {
        "aislop_gate": (
            aislop_success_rule,
            "      - condition: when(context.machine_gate.result.exit_code == 0)\n"
            "        next: COMPLETE\n",
            "別ゲート machine_gate",
        ),
        "machine_gate": (
            machine_success_rule,
            "      - condition: when(context.aislop_gate.result.exit_code == 0)\n"
            "        next: aislop_gate\n",
            "別ゲート aislop_gate",
        ),
    }
    for gate, (success_rule, extra_rule, snippet) in other_gate_rules.items():
        for position, replaced in (
            ("成功条件の規則より前", extra_rule + success_rule),
            ("成功条件の規則より後", success_rule + extra_rule),
        ):
            cases[f"{gate} の別規則が{position}に別ゲートの成功を見る"] = (
                WORKFLOW.replace(success_rule, replaced),
                snippet,
            )
    for label, (workflow, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_ci_must_run_make_done_with_toolchains() -> None:
    cases = {
        "make done を呼ばない": (CI.replace("      - run: make done", "      - run: cargo test"), "make done"),
        "nightly と rustfmt が無い": (
            CI.replace("      - uses: dtolnay/rust-toolchain@nightly\n        with:\n          components: rustfmt\n", ""),
            "nightly",
        ),
        "stable が無い": (
            CI.replace("      - uses: dtolnay/rust-toolchain@stable\n        with:\n          components: clippy\n", ""),
            "stable",
        ),
        "Node が無い": (
            CI.replace("      - uses: actions/setup-node@v4\n        with:\n          node-version: 22\n", ""),
            "Node",
        ),
    }
    for label, (ci, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, ci=ci)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "ci-runs-make-done", snippet)

    with Sandbox() as sandbox:
        repository(sandbox, ci=None)
        stub_external_tools(sandbox)
        assert_failure(sandbox, "CI の定義が無い", "ci-runs-make-done", ".github/workflows/")


def test_ci_must_call_aislop_gate() -> None:
    """CI から aislop のゲートを呼ぶ。チェックアウトの前に呼ぶ構成も失敗にする。"""
    aislop_run = "      - run: node scripts/aislop-check-bootstrap.mjs check\n"
    cases = {
        "aislop ゲートを呼ばない": (
            CI.replace(aislop_run, ""),
            "aislop",
        ),
        "チェックアウトの前に aislop を呼ぶ": (
            CI.replace(aislop_run, "").replace(
                "      - uses: actions/checkout@v5\n",
                aislop_run + "      - uses: actions/checkout@v5\n",
            ),
            "aislop",
        ),
    }
    for label, (ci, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, ci=ci)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "ci-runs-make-done", snippet)


def test_ci_checkout_order_is_per_job() -> None:
    """チェックアウト順はジョブ単位で判定する。

    先のジョブの checkout が後続ジョブへ累積してはならない。aislop を呼ぶジョブが
    自身の checkout を持たない場合は、他ジョブに checkout があっても失敗にする。
    併せて、問題のない複数ジョブ構成と、make done を持たないジョブの混在が
    判定を変えないことも確かめる。
    """
    aislop_only_job = (
        "  extra:\n"
        "    runs-on: ubuntu-latest\n"
        "    steps:\n"
        "      - run: node scripts/aislop-check-bootstrap.mjs check\n"
    )
    cases = {
        "aislop を呼ぶジョブに checkout が無い": (
            # done ジョブが checkout と make done を持ち、extra ジョブが aislop のみを持つ。
            CI.rstrip("\n") + "\n" + aislop_only_job,
            "チェックアウトより前に呼んでいます",
        ),
        "aislop を呼ぶ別ジョブに自身の checkout がある": (
            CI.rstrip("\n")
            + "\n"
            + "  extra:\n"
            + "    runs-on: ubuntu-latest\n"
            + "    steps:\n"
            + "      - uses: actions/checkout@v5\n"
            + "      - run: node scripts/aislop-check-bootstrap.mjs check\n",
            None,
        ),
        "make done を持たないジョブが混在しても結果が変わらない": (
            CI.rstrip("\n")
            + "\n"
            + "  unrelated:\n"
            + "    runs-on: ubuntu-latest\n"
            + "    steps:\n"
            + "      - run: echo ok\n",
            None,
        ),
    }
    for label, (ci, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, ci=ci)
            stub_external_tools(sandbox)
            if snippet is None:
                code, out, err = sandbox.run([])
                assert code == 0, f"{label}: 成功するはずが失敗しました: {err!r}"
            else:
                assert_failure(sandbox, label, "ci-runs-make-done", snippet)


def test_gitignore_and_toolchain_rules() -> None:
    with Sandbox() as sandbox:
        repository(sandbox, gitignore="*.log\n")
        stub_external_tools(sandbox)
        assert_failure(sandbox, "生成物の除外が無い", "gitignore-rules", "node_modules")

    with Sandbox() as sandbox:
        repository(sandbox, toolchain='[toolchain]\nchannel = "nightly"\n')
        stub_external_tools(sandbox)
        assert_failure(sandbox, "nightly チャネル", "stable-toolchain", "stable")


def test_skip_make_omits_the_command_run() -> None:
    with Sandbox() as sandbox:
        repository(sandbox)
        record = stub_external_tools(sandbox)
        code, out, err = sandbox.run(["--skip-make"])
        assert code == 0, f"skip-make: {err!r}"
        payload = json.loads(out)
        detail = {check["name"]: check for check in payload["checks"]}["machine-gate"]
        assert detail["detail"]["make_done"] == "skipped", detail
        assert ["make", "done"] not in [line.split() for line in called_commands(record)], called_commands(record)


def test_install_guard_detection() -> None:
    with Sandbox() as sandbox:
        module = sandbox.module
        guarded = ["test -d frontend/web/node_modules || npm --prefix frontend/web ci"]
        assert module.is_install_guard(guarded)
        assert module.install_commands(guarded) == [["npm", "--prefix", "frontend/web", "ci"]]
        assert not module.is_install_guard(["npm --prefix frontend/web run lint"])
        assert not module.is_install_guard(["test -d frontend/web/node_modules"])


def test_makefile_recipe_parsing() -> None:
    with Sandbox() as sandbox:
        module = sandbox.module
        sandbox.write(
            "Makefile",
            "fmt:\n\tcargo +nightly fmt --all -- --check\n\ndone:\n\t$(MAKE) fmt\n\ttest -d frontend/web/node_modules || npm ci\n",
        )
        targets = module.parse_makefile(sandbox.root / "Makefile")
        assert targets["fmt"] == ["cargo +nightly fmt --all -- --check"], targets
        assert targets["done"][0] == "$(MAKE) fmt", targets


def main() -> int:
    tests = [
        test_correct_repository_passes,
        test_workspace_members_must_be_backend_and_cli,
        test_backend_must_have_lib_and_cli_must_have_bin,
        test_web_manifest_requires_scripts_and_next,
        test_proto_directory_is_required,
        test_make_done_must_run_every_check,
        test_make_done_must_fail_when_a_check_fails,
        test_clippy_must_reject_warnings,
        test_done_must_install_node_dependencies_when_missing,
        test_workflow_gate_wiring,
        test_workflow_requires_aislop_gate_step,
        test_aislop_fix_must_reenter_machine_gate,
        test_aislop_gate_command_detection,
        test_aislop_gate_command_must_invoke_the_check,
        test_gate_success_condition_must_reference_its_own_gate,
        test_ci_must_run_make_done_with_toolchains,
        test_ci_must_call_aislop_gate,
        test_ci_aislop_gate_command_must_invoke_the_check,
        test_ci_aislop_gate_accepts_multiline_run_with_comment,
        test_ci_checkout_order_is_per_job,
        test_gitignore_and_toolchain_rules,
        test_skip_make_omits_the_command_run,
        test_install_guard_detection,
        test_makefile_recipe_parsing,
    ]
    failures = 0
    for test in tests:
        signal.signal(signal.SIGALRM, _on_timeout)
        signal.alarm(TEST_TIMEOUT_SECONDS)
        try:
            test()
        except AssertionError as error:
            failures += 1
            print(f"FAIL {test.__name__}: {error}")
        except TestTimeout:
            failures += 1
            print(f"FAIL {test.__name__}: did not finish within {TEST_TIMEOUT_SECONDS}s")
        except Exception:  # 実装漏れや脚本の穴は、失敗として記録する
            failures += 1
            print(f"FAIL {test.__name__}:")
            traceback.print_exc()
        else:
            print(f"ok   {test.__name__}")
        finally:
            signal.alarm(0)
    print(f"{len(tests) - failures} passed, {failures} failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
