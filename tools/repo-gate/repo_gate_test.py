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
  4. machine_gate が mode: system で、proto-check・fmt・clippy・test・typecheck・lint の
     6つの system_input を過不足なく持ち、各 command が対応する make ターゲットを起動する。
     検査の欠落・重複・別ターゲットの起動は失敗させる。検査を実行させない制御演算子
     （例: `true || make fmt`）を含む command も、正規形との照合で失敗させる。
  5. machine_gate の成功条件が6つの exit_code == 0 の同時成立で次のステップへ進み、
     失敗条件が検査ごとの修正ステップへ分岐する。公開ステップを置く場合は公開へ、
     置かない場合は aislop_gate へ進む。1検査の抜け、別の遷移先、単一キーの条件は
     失敗させる。
  6. 各修正ステップが当該検査の stdout と stderr だけを参照し、machine_gate へ戻る。
     別検査の出力参照・戻り遷移の欠落・別の遷移先は失敗させる。戻り遷移を残したまま
     COMPLETE などへ抜ける迂回規則を足した場合も失敗させる。
  7. aislop_gate の既存の契約（成功で COMPLETE、失敗で修正へ、修正は machine_gate へ
     戻る）と develop → machine_gate → aislop_gate の順序を維持する。aislop の起動は、
     チェックの失敗が呼び出し元へ返る形だけを認め、`|| true`・`; true`・パイプなどで
     終了状態を潰す command や run は失敗させる。aislop_gate の規則は順序と遷移先も
     検査し、成功条件以外の規則や修正ステップから COMPLETE へ進む構成は失敗させる。
  8. 公開ステップ（publish.py）を置く場合の配線を確かめる。公開ステップが 4 段階
     （commit/push/pr/status）を実行しない、公開の成功条件が commit/push/pr の
     exit_code を含まない、|| で結合されている、複数の rule に分かれている、status の
     exit_code を含む、公開の成功が aislop ステップへ渡らない、公開の失敗が修正ステップ
     （fix_publish）へ遷移しない、公開の修正が公開の出力を参照しない、修正から公開へ
     戻らない、修正が ABORT へ遷移する、のそれぞれで失敗する。status だけを実行する
     ステップは公開とみなさない。
  9. CI が make done を呼ばない、nightly と rustfmt をセットアップしない、Node を
     セットアップしない、のそれぞれで失敗する。
  10. .gitignore に生成物の除外が無い、rust-toolchain.toml のチャネルが stable でない、
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
.PHONY: fmt clippy test typecheck lint proto-check done

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

proto-check:
	$(MAKE) proto-gen
	git diff --exit-code -- frontend/web/src/proto

done:
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	$(MAKE) proto-check
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

DEVELOP_STEP = """\
  - name: develop
    description: development-core を呼ぶ
    kind: workflow_call
    call: development-core
    rules:
      - condition: COMPLETE
        next: machine_gate
      - condition: ABORT
        next: ABORT
"""

FMT_INPUT = """\
      - type: command
        as: fmt
        command: "bash -o pipefail -c 'make fmt 2>&1 | tail -c 60000'"
        timeout_ms: 1800000
"""

CLIPPY_INPUT = """\
      - type: command
        as: clippy
        command: "bash -o pipefail -c 'make clippy 2>&1 | tail -c 60000'"
        timeout_ms: 1800000
"""

TEST_INPUT = """\
      - type: command
        as: test
        command: >-
          bash -c 'if [ ! -d frontend/web/node_modules ]; then bash -o pipefail -c "npm --prefix frontend/web ci 2>&1 | tail -c 4000" || exit $?; fi; bash -o pipefail -c "make test 2>&1 | python3 tools/test-summary/test_summary.py --max-bytes 56000"'
        timeout_ms: 1800000
"""

TYPECHECK_INPUT = """\
      - type: command
        as: typecheck
        command: "bash -o pipefail -c 'make typecheck 2>&1 | tail -c 60000'"
        timeout_ms: 1800000
"""

LINT_INPUT = """\
      - type: command
        as: lint
        command: "bash -o pipefail -c 'make lint 2>&1 | tail -c 60000'"
        timeout_ms: 1800000
"""

PROTO_CHECK_INPUT = """\
      - type: command
        as: proto_check
        command: "bash -o pipefail -c 'make proto-check 2>&1 | tail -c 60000'"
        timeout_ms: 1800000
"""

EXTRA_INPUT = """\
      - type: command
        as: coverage
        command: "bash -o pipefail -c 'make coverage 2>&1 | tail -c 60000'"
        timeout_ms: 1800000
"""

FMT_FAILURE_RULE = """\
      - condition: when(context.machine_gate.fmt.exit_code != 0)
        next: fix_machine_gate_fmt
"""

CLIPPY_FAILURE_RULE = """\
      - condition: when(context.machine_gate.clippy.exit_code != 0)
        next: fix_machine_gate_clippy
"""

LINT_FAILURE_RULE = """\
      - condition: when(context.machine_gate.lint.exit_code != 0)
        next: fix_machine_gate_lint
"""

PROTO_CHECK_FAILURE_RULE = """\
      - condition: when(context.machine_gate.proto_check.exit_code != 0)
        next: fix_machine_gate_proto_check
"""

MACHINE_GATE_FAILURE_RULES = """\
      - condition: when(context.machine_gate.fmt.exit_code != 0)
        next: fix_machine_gate_fmt
      - condition: when(context.machine_gate.clippy.exit_code != 0)
        next: fix_machine_gate_clippy
      - condition: when(context.machine_gate.test.exit_code != 0)
        next: fix_machine_gate_test
      - condition: when(context.machine_gate.typecheck.exit_code != 0)
        next: fix_machine_gate_typecheck
      - condition: when(context.machine_gate.lint.exit_code != 0)
        next: fix_machine_gate_lint
      - condition: when(context.machine_gate.proto_check.exit_code != 0)
        next: fix_machine_gate_proto_check
"""

MACHINE_GATE_SUCCESS_RULE = """\
      - condition: when(context.machine_gate.proto_check.exit_code == 0 && context.machine_gate.fmt.exit_code == 0 && context.machine_gate.clippy.exit_code == 0 && context.machine_gate.test.exit_code == 0 && context.machine_gate.typecheck.exit_code == 0 && context.machine_gate.lint.exit_code == 0)
        next: aislop_gate
"""

MACHINE_GATE_ABORT_RULE = """\
      - condition: when(true)
        next: ABORT
"""

MACHINE_GATE_STEP = (
    "  - name: machine_gate\n"
    "    description: proto-check・fmt・clippy・test・typecheck・lint を検査ごとに実行する\n"
    "    mode: system\n"
    "    system_inputs:\n"
    + FMT_INPUT
    + CLIPPY_INPUT
    + TEST_INPUT
    + TYPECHECK_INPUT
    + LINT_INPUT
    + PROTO_CHECK_INPUT
    + "    rules:\n"
    + MACHINE_GATE_FAILURE_RULES
    + MACHINE_GATE_SUCCESS_RULE
    + MACHINE_GATE_ABORT_RULE
)

AISLOP_GATE_STEP = """\
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
"""


def fix_machine_gate_step(name, check, failure, alias=None):
    key = alias or check
    return (
        f"  - name: {name}\n"
        f"    description: make {check} の失敗出力を確認して修正し、機械ゲートへ戻る\n"
        "    capabilities: edit\n"
        "    edit: true\n"
        "    instruction:\n"
        "      - fix-supervisor\n"
        "      - |\n"
        f"        make {check} が失敗しました。次の出力の{failure}を修正してください。\n"
        f"        --- make {check} の stdout ---\n"
        f"        {{context:machine_gate.{key}.stdout}}\n"
        f"        --- make {check} の stderr ---\n"
        f"        {{context:machine_gate.{key}.stderr}}\n"
        "    rules:\n"
        f"      - condition: make {check} の失敗を修正して検証した\n"
        "        next: machine_gate\n"
        "      - condition: 裏付けられた原因を安全に修正または検証できない\n"
        "        next: ABORT\n"
    )


FIX_MACHINE_GATE_FMT_STEP = fix_machine_gate_step("fix_machine_gate_fmt", "fmt", "整形の失敗")
FIX_MACHINE_GATE_CLIPPY_STEP = fix_machine_gate_step(
    "fix_machine_gate_clippy", "clippy", "clippy の警告とエラー"
)
FIX_MACHINE_GATE_TEST_STEP = fix_machine_gate_step(
    "fix_machine_gate_test", "test", "テストの失敗"
)
FIX_MACHINE_GATE_TYPECHECK_STEP = fix_machine_gate_step(
    "fix_machine_gate_typecheck", "typecheck", "型検査の失敗"
)
FIX_MACHINE_GATE_LINT_STEP = fix_machine_gate_step("fix_machine_gate_lint", "lint", "lint の指摘")
FIX_MACHINE_GATE_PROTO_CHECK_STEP = fix_machine_gate_step(
    "fix_machine_gate_proto_check",
    "proto-check",
    "生成物と proto の不一致",
    alias="proto_check",
)
FIX_MACHINE_GATE_STEPS = [
    FIX_MACHINE_GATE_FMT_STEP,
    FIX_MACHINE_GATE_CLIPPY_STEP,
    FIX_MACHINE_GATE_TEST_STEP,
    FIX_MACHINE_GATE_TYPECHECK_STEP,
    FIX_MACHINE_GATE_LINT_STEP,
    FIX_MACHINE_GATE_PROTO_CHECK_STEP,
]

FIX_AISLOP_GATE_STEP = """\
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

WORKFLOW_HEADER = """\
name: taktdash-change
max_steps: 70
initial_step: develop
steps:
"""

WORKFLOW = (
    WORKFLOW_HEADER
    + DEVELOP_STEP
    + MACHINE_GATE_STEP
    + AISLOP_GATE_STEP
    + "".join(FIX_MACHINE_GATE_STEPS)
    + FIX_AISLOP_GATE_STEP
)


def workflow_with(*steps):
    return WORKFLOW_HEADER + "".join(steps)


# 公開ステップを置く場合の形。置くかどうかは任意で、置く場合は公開の段階・成功条件・
# 失敗時の修正が検査される。
PUBLISH_STEPS = """\
  - name: create_pr
    description: コミット・push・PR 作成・Status 更新を段階実行する
    mode: system
    system_inputs:
      - type: command
        as: commit
        command: |
          python3 tools/project-status/publish.py commit
      - type: command
        as: push
        command: |
          python3 tools/project-status/publish.py push
      - type: command
        as: pr
        command: |
          python3 tools/project-status/publish.py pr
      - type: command
        as: status
        command: |
          python3 tools/project-status/publish.py status --status "In Review"
    rules:
      - condition: when(context.create_pr.commit.exit_code == 0 && context.create_pr.push.exit_code == 0 && context.create_pr.pr.exit_code == 0)
        next: aislop_gate
      - condition: when(true)
        next: fix_publish
  - name: fix_publish
    description: 公開の失敗を修正する
    capabilities: edit
    edit: true
    pass_previous_response: false
    instruction:
      - fix-supervisor
      - |
        {context:create_pr.commit.stdout}
        {context:create_pr.push.stderr}
        {context:create_pr.pr.stdout}
        {context:create_pr.status.stderr}
    rules:
      - condition: 公開の失敗を修正して再試行する
        next: create_pr
      - condition: コードを修正する
        next: machine_gate
"""

PUBLISH_WORKFLOW = WORKFLOW.replace(
    MACHINE_GATE_SUCCESS_RULE,
    MACHINE_GATE_SUCCESS_RULE.replace("next: aislop_gate", "next: create_pr"),
) + PUBLISH_STEPS

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


def assert_gate_rejects(sandbox, label, name="workflow-machine-gate"):
    """ほかの検査が通る前提で、対象の検査だけが違反として報告されることを確かめる。"""
    code, out, err = sandbox.run([])
    assert code != 0, f"{label}: 違反があるのに成功しました。stdout={out!r}"
    lines = [line for line in err.splitlines() if line.startswith(f"{name}:")]
    assert len(lines) == 1, f"{label}: {name} の違反が 1 行で報告されません: {err!r}"


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
    checks = (
        ("proto-check", "proto_check"),
        ("fmt", "fmt"),
        ("clippy", "clippy"),
        ("test", "test"),
        ("typecheck", "typecheck"),
        ("lint", "lint"),
    )
    for target, reported in checks:
        with Sandbox() as sandbox:
            makefile = "\n".join(
                line for line in MAKEFILE.splitlines() if line.strip() != f"$(MAKE) {target}"
            )
            repository(sandbox, makefile=makefile + "\n")
            stub_external_tools(sandbox)
            assert_failure(sandbox, f"done が {target} を実行しない", "machine-gate", reported)


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
.PHONY: fmt clippy test typecheck lint proto-check done

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

proto-check:
	$(MAKE) proto-gen
	git diff --exit-code -- frontend/web/src/proto

done:
	$(MAKE) proto-check
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


def test_machine_gate_develop_complete_and_mode() -> None:
    success_condition = (
        "context.create_pr.commit.exit_code == 0 && "
        "context.create_pr.push.exit_code == 0 && "
        "context.create_pr.pr.exit_code == 0"
    )
    cases = {
        "develop が機械ゲートへ遷移しない": (
            workflow_with(
                DEVELOP_STEP.replace("        next: machine_gate", "        next: COMPLETE"),
                MACHINE_GATE_STEP,
                AISLOP_GATE_STEP,
                *FIX_MACHINE_GATE_STEPS,
                FIX_AISLOP_GATE_STEP,
            ),
            "develop の COMPLETE",
        ),
        "機械ゲートが system でない": (
            workflow_with(
                DEVELOP_STEP,
                MACHINE_GATE_STEP.replace("    mode: system\n", "    capabilities: readonly\n", 1),
                AISLOP_GATE_STEP,
                *FIX_MACHINE_GATE_STEPS,
                FIX_AISLOP_GATE_STEP,
            ),
            "mode: system",
        ),
        "機械ゲートのステップが無い": (
            workflow_with(DEVELOP_STEP, AISLOP_GATE_STEP, *FIX_MACHINE_GATE_STEPS, FIX_AISLOP_GATE_STEP),
            "machine_gate",
        ),
        "機械ゲートの成功が aislop へ遷移しない": (
            WORKFLOW.replace("        next: aislop_gate\n", "        next: COMPLETE\n", 1),
            "aislop_gate",
        ),
        "公開ステップを置く場合に機械ゲートの成功が公開へ遷移しない": (
            PUBLISH_WORKFLOW.replace(
                "        next: create_pr\n      - condition: when(true)\n        next: ABORT",
                "        next: COMPLETE\n      - condition: when(true)\n        next: ABORT",
            ),
            "create_pr",
        ),
        "公開の成功が aislop ステップへ渡さない": (
            PUBLISH_WORKFLOW.replace(
                "        next: aislop_gate\n      - condition: when(true)\n        next: fix_publish",
                "        next: COMPLETE\n      - condition: when(true)\n        next: fix_publish",
            ),
            "aislop",
        ),
        "公開に push の入力が無い": (
            PUBLISH_WORKFLOW.replace(
                "      - type: command\n        as: push\n        command: |\n          python3 tools/project-status/publish.py push\n",
                "",
            ),
            "push",
        ),
        "公開に commit の入力が無い": (
            PUBLISH_WORKFLOW.replace(
                "      - type: command\n        as: commit\n        command: |\n          python3 tools/project-status/publish.py commit\n",
                "",
            ),
            "commit",
        ),
        "公開に status の入力が無い": (
            PUBLISH_WORKFLOW.replace(
                "      - type: command\n        as: status\n        command: |\n          python3 tools/project-status/publish.py status --status \"In Review\"\n",
                "",
            ),
            "status",
        ),
        "公開の成功条件に push の exit_code が無い": (
            PUBLISH_WORKFLOW.replace(
                success_condition,
                "context.create_pr.commit.exit_code == 0 && context.create_pr.pr.exit_code == 0",
            ),
            "push",
        ),
        "公開の成功条件に pr の exit_code が無い": (
            PUBLISH_WORKFLOW.replace(
                success_condition,
                "context.create_pr.commit.exit_code == 0 && context.create_pr.push.exit_code == 0",
            ),
            "pr",
        ),
        "公開の成功条件に status の exit_code を含む": (
            PUBLISH_WORKFLOW.replace(
                "&& context.create_pr.pr.exit_code == 0)",
                "&& context.create_pr.pr.exit_code == 0 && context.create_pr.status.exit_code == 0)",
            ),
            "status",
        ),
        "公開の成功条件が || で結合されている": (
            PUBLISH_WORKFLOW.replace(
                success_condition,
                "context.create_pr.commit.exit_code == 0 || context.create_pr.push.exit_code == 0 || context.create_pr.pr.exit_code == 0",
            ),
            "||",
        ),
        "公開の完了条件が複数の rule に分かれている": (
            PUBLISH_WORKFLOW.replace(
                "      - condition: when(" + success_condition + ")\n        next: aislop_gate\n",
                "      - condition: when(context.create_pr.commit.exit_code == 0)\n        next: aislop_gate\n"
                "      - condition: when(context.create_pr.push.exit_code == 0 && context.create_pr.pr.exit_code == 0)\n"
                "        next: aislop_gate\n",
            ),
            "1件",
        ),
        "公開の失敗が修正ステップへ遷移しない": (
            PUBLISH_WORKFLOW.replace("next: fix_publish", "next: COMPLETE"),
            "fix_publish",
        ),
        "公開の修正が公開の出力を参照しない": (
            PUBLISH_WORKFLOW.replace(
                "        {context:create_pr.commit.stdout}\n        {context:create_pr.push.stderr}\n        {context:create_pr.pr.stdout}\n        {context:create_pr.status.stderr}\n",
                "        公開の出力を確認してください\n",
            ),
            "instruction",
        ),
        "公開の修正から公開へ戻らない": (
            PUBLISH_WORKFLOW.replace(
                "next: create_pr\n      - condition: コードを修正する",
                "next: COMPLETE\n      - condition: コードを修正する",
            ),
            "create_pr",
        ),
        "公開の修正が ABORT へ遷移する": (
            PUBLISH_WORKFLOW.replace(
                "      - condition: コードを修正する\n        next: machine_gate",
                "      - condition: コードを修正する\n        next: ABORT",
            ),
            "ABORT",
        ),
        "公開ステップが system でない": (
            PUBLISH_WORKFLOW.replace(
                "  - name: create_pr\n    description: コミット・push・PR 作成・Status 更新を段階実行する\n    mode: system",
                "  - name: create_pr\n    description: コミット・push・PR 作成・Status 更新を段階実行する\n    capabilities: readonly",
            ),
            "mode: system",
        ),
    }
    for label, (workflow, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_machine_gate_requires_six_system_inputs() -> None:
    cases = {
        "lint の system_input が無い": (
            MACHINE_GATE_STEP.replace(LINT_INPUT, ""),
            "system_inputs",
        ),
        "proto_check の system_input が無い": (
            MACHINE_GATE_STEP.replace(PROTO_CHECK_INPUT, ""),
            "system_inputs",
        ),
        "7 個目の system_input がある": (
            MACHINE_GATE_STEP.replace(LINT_INPUT, LINT_INPUT + EXTRA_INPUT),
            "system_inputs",
        ),
        "同じ as の system_input が重複する": (
            MACHINE_GATE_STEP.replace(LINT_INPUT, FMT_INPUT),
            "system_inputs",
        ),
        "as が検査名と一致しない": (
            MACHINE_GATE_STEP.replace(LINT_INPUT, LINT_INPUT.replace("as: lint", "as: linting")),
            "system_inputs",
        ),
        "system_input の type が command でない": (
            MACHINE_GATE_STEP.replace(LINT_INPUT, LINT_INPUT.replace("type: command", "type: file")),
            "command",
        ),
    }
    for label, (machine_step, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    machine_step,
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_machine_gate_commands_invoke_their_make_targets() -> None:
    fmt_command = 'command: "bash -o pipefail -c \'make fmt 2>&1 | tail -c 60000\'"'
    cases = {
        "fmt が別の検査を呼ぶ": (
            MACHINE_GATE_STEP.replace(FMT_INPUT, FMT_INPUT.replace("make fmt 2>&1", "make lint 2>&1")),
            "make fmt を呼んでいません",
        ),
        "fmt が echo で表示するだけ": (
            MACHINE_GATE_STEP.replace(
                FMT_INPUT,
                FMT_INPUT.replace(fmt_command, 'command: "echo make fmt"'),
            ),
            "make fmt を呼んでいません",
        ),
        "fmt が make done を呼ぶ": (
            MACHINE_GATE_STEP.replace(
                FMT_INPUT,
                FMT_INPUT.replace(fmt_command, 'command: "make done"'),
            ),
            "make fmt を呼んでいません",
        ),
        "fmt が別の検査も一緒に呼ぶ": (
            MACHINE_GATE_STEP.replace(
                FMT_INPUT,
                FMT_INPUT.replace(
                    "make fmt 2>&1 | tail -c 60000",
                    "make fmt && make lint 2>&1 | tail -c 60000",
                ),
            ),
            "lint",
        ),
        "clippy が make clippy を呼ばない": (
            MACHINE_GATE_STEP.replace(
                CLIPPY_INPUT,
                CLIPPY_INPUT.replace("make clippy 2>&1", "make fmt 2>&1"),
            ),
            "make clippy を呼んでいません",
        ),
        "proto_check が make proto-check を呼ばない": (
            MACHINE_GATE_STEP.replace(
                PROTO_CHECK_INPUT,
                PROTO_CHECK_INPUT.replace("make proto-check 2>&1", "make fmt 2>&1"),
            ),
            "make proto-check を呼んでいません",
        ),
        "proto_check が別の検査も一緒に呼ぶ": (
            MACHINE_GATE_STEP.replace(
                PROTO_CHECK_INPUT,
                PROTO_CHECK_INPUT.replace(
                    "make proto-check 2>&1 | tail -c 60000",
                    "make proto-check && make lint 2>&1 | tail -c 60000",
                ),
            ),
            "lint",
        ),
        "fmt が制御演算子で検査を実行させない": (
            MACHINE_GATE_STEP.replace(
                FMT_INPUT,
                FMT_INPUT.replace("make fmt 2>&1 | tail -c 60000", "true || make fmt"),
            ),
            "正規形",
        ),
        "test が制御演算子で make test を実行させない": (
            MACHINE_GATE_STEP.replace(
                TEST_INPUT,
                TEST_INPUT.replace(
                    'bash -o pipefail -c "make test 2>&1 | python3 tools/test-summary/test_summary.py --max-bytes 56000"',
                    'bash -o pipefail -c "true || make test 2>&1 | python3 tools/test-summary/test_summary.py --max-bytes 56000"',
                ),
            ),
            "正規形",
        ),
    }
    for label, (machine_step, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    machine_step,
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_machine_gate_success_requires_all_six_exit_codes() -> None:
    four_check_success = """\
      - condition: when(context.machine_gate.fmt.exit_code == 0 && context.machine_gate.clippy.exit_code == 0 && context.machine_gate.test.exit_code == 0 && context.machine_gate.typecheck.exit_code == 0)
        next: aislop_gate
"""
    five_check_success = """\
      - condition: when(context.machine_gate.fmt.exit_code == 0 && context.machine_gate.clippy.exit_code == 0 && context.machine_gate.test.exit_code == 0 && context.machine_gate.typecheck.exit_code == 0 && context.machine_gate.lint.exit_code == 0)
        next: aislop_gate
"""
    or_success = """\
      - condition: when(context.machine_gate.proto_check.exit_code == 0 || context.machine_gate.fmt.exit_code == 0 || context.machine_gate.clippy.exit_code == 0 || context.machine_gate.test.exit_code == 0 || context.machine_gate.typecheck.exit_code == 0 || context.machine_gate.lint.exit_code == 0)
        next: aislop_gate
"""
    result_success = """\
      - condition: when(context.machine_gate.result.exit_code == 0)
        next: aislop_gate
"""
    when_true_success = """\
      - condition: when(true)
        next: aislop_gate
"""
    foreign_key_success = MACHINE_GATE_SUCCESS_RULE.replace(
        "context.machine_gate.lint.exit_code == 0",
        "context.aislop_gate.result.exit_code == 0",
    )
    cases = {
        "成功条件から lint の検査が抜けている": (
            MACHINE_GATE_STEP.replace(MACHINE_GATE_SUCCESS_RULE, four_check_success),
            "6検査",
        ),
        "成功条件から proto_check の検査が抜けている": (
            MACHINE_GATE_STEP.replace(MACHINE_GATE_SUCCESS_RULE, five_check_success),
            "6検査",
        ),
        "成功の遷移先が aislop_gate でない": (
            MACHINE_GATE_STEP.replace(
                MACHINE_GATE_SUCCESS_RULE,
                MACHINE_GATE_SUCCESS_RULE.replace("next: aislop_gate", "next: COMPLETE"),
            ),
            "aislop_gate",
        ),
    }
    for label, (machine_step, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    machine_step,
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)

    rejected = {
        "成功条件が OR で結ばれている": MACHINE_GATE_STEP.replace(MACHINE_GATE_SUCCESS_RULE, or_success),
        "成功条件が単一キーの result を見る": MACHINE_GATE_STEP.replace(
            MACHINE_GATE_SUCCESS_RULE, result_success
        ),
        "成功条件が別ゲートのキーを見る": MACHINE_GATE_STEP.replace(
            MACHINE_GATE_SUCCESS_RULE, foreign_key_success
        ),
        "when(true) で aislop_gate へ進む": MACHINE_GATE_STEP.replace(
            MACHINE_GATE_SUCCESS_RULE, when_true_success
        ),
        "when(true) が成功条件より前にある": MACHINE_GATE_STEP.replace(
            MACHINE_GATE_SUCCESS_RULE + MACHINE_GATE_ABORT_RULE,
            MACHINE_GATE_ABORT_RULE + MACHINE_GATE_SUCCESS_RULE,
        ),
    }
    for label, machine_step in rejected.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    machine_step,
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_gate_rejects(sandbox, label)


def test_machine_gate_requires_single_success_rule() -> None:
    """machine_gate の aislop_gate 成功規則は、ちょうど 1 件でなければならない。

    成功条件そのものが正規形でも、別ゲートの成功で aislop_gate へ進む規則を
    前後に足すと、先に一致した規則で成功条件を迂回できる。
    """
    extra_success_rule = (
        "      - condition: when(context.aislop_gate.result.exit_code == 0)\n"
        "        next: aislop_gate\n"
    )
    cases = {
        "成功条件の規則より前に別の aislop_gate 規則がある": (
            extra_success_rule + MACHINE_GATE_SUCCESS_RULE
        ),
        "成功条件の規則より後に別の aislop_gate 規則がある": (
            MACHINE_GATE_SUCCESS_RULE + extra_success_rule
        ),
    }
    for label, replaced in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    MACHINE_GATE_STEP.replace(MACHINE_GATE_SUCCESS_RULE, replaced),
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(
                sandbox, label, "workflow-machine-gate", "成功規則が 1 つだけありません"
            )


def test_machine_gate_failure_routes_to_matching_fix_step() -> None:
    cases = {
        "fmt の失敗条件が無い": (
            MACHINE_GATE_STEP.replace(FMT_FAILURE_RULE, ""),
            "fmt",
        ),
        "clippy の失敗条件が exit_code != 0 でない": (
            MACHINE_GATE_STEP.replace(
                CLIPPY_FAILURE_RULE,
                CLIPPY_FAILURE_RULE.replace("!= 0", "== 0"),
            ),
            "clippy",
        ),
    }
    for label, (machine_step, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    machine_step,
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)

    rejected = {
        "lint の失敗が fmt の修正ステップへ遷移する": MACHINE_GATE_STEP.replace(
            LINT_FAILURE_RULE,
            LINT_FAILURE_RULE.replace("next: fix_machine_gate_lint", "next: fix_machine_gate_fmt"),
        ),
    }
    for label, machine_step in rejected.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    machine_step,
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_gate_rejects(sandbox, label)

    without_lint_fix = [step for step in FIX_MACHINE_GATE_STEPS if step is not FIX_MACHINE_GATE_LINT_STEP]
    with Sandbox() as sandbox:
        repository(
            sandbox,
            workflow=workflow_with(
                DEVELOP_STEP,
                MACHINE_GATE_STEP,
                AISLOP_GATE_STEP,
                *without_lint_fix,
                FIX_AISLOP_GATE_STEP,
            ),
        )
        stub_external_tools(sandbox)
        assert_failure(sandbox, "lint の修正ステップが無い", "workflow-machine-gate", "fix_machine_gate_lint")


def test_machine_gate_fix_steps_only_use_their_own_output() -> None:
    cases = {
        "lint の修正が fmt の出力を参照する": (
            FIX_MACHINE_GATE_LINT_STEP,
            FIX_MACHINE_GATE_LINT_STEP.replace(
                "{context:machine_gate.lint.stdout}",
                "{context:machine_gate.fmt.stdout}",
            ),
        ),
        "lint の修正が stderr を参照しない": (
            FIX_MACHINE_GATE_LINT_STEP,
            FIX_MACHINE_GATE_LINT_STEP.replace(
                "        {context:machine_gate.lint.stderr}\n", ""
            ),
        ),
        "lint の修正が aislop の出力も参照する": (
            FIX_MACHINE_GATE_LINT_STEP,
            FIX_MACHINE_GATE_LINT_STEP.replace(
                "        {context:machine_gate.lint.stderr}\n",
                "        {context:machine_gate.lint.stderr}\n        {context:aislop_gate.result.stdout}\n",
            ),
        ),
        "fmt の修正が clippy の出力を参照する": (
            FIX_MACHINE_GATE_FMT_STEP,
            FIX_MACHINE_GATE_FMT_STEP.replace(
                "{context:machine_gate.fmt.stdout}",
                "{context:machine_gate.clippy.stdout}",
            ),
        ),
    }
    for label, (original, mutated) in cases.items():
        steps = [mutated if step is original else step for step in FIX_MACHINE_GATE_STEPS]
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    MACHINE_GATE_STEP,
                    AISLOP_GATE_STEP,
                    *steps,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_gate_rejects(sandbox, label)


def test_machine_gate_fix_steps_return_to_machine_gate() -> None:
    cases = {
        "lint の修正が aislop へ戻る": FIX_MACHINE_GATE_LINT_STEP.replace(
            "        next: machine_gate", "        next: aislop_gate"
        ),
        "lint の修正が COMPLETE へ進む": FIX_MACHINE_GATE_LINT_STEP.replace(
            "        next: machine_gate", "        next: COMPLETE"
        ),
        "lint の修正に戻りの遷移が無い": FIX_MACHINE_GATE_LINT_STEP.replace(
            "        next: machine_gate\n", ""
        ),
        "lint の修正に COMPLETE への迂回規則を足す": FIX_MACHINE_GATE_LINT_STEP.replace(
            "    rules:\n",
            "    rules:\n      - condition: when(true)\n        next: COMPLETE\n",
        ),
        "lint の修正に aislop への迂回規則を足す": FIX_MACHINE_GATE_LINT_STEP.replace(
            "    rules:\n",
            "    rules:\n      - condition: when(true)\n        next: aislop_gate\n",
        ),
    }
    for label, mutated in cases.items():
        steps = [mutated if step is FIX_MACHINE_GATE_LINT_STEP else step for step in FIX_MACHINE_GATE_STEPS]
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    MACHINE_GATE_STEP,
                    AISLOP_GATE_STEP,
                    *steps,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", "machine_gate")


def test_status_only_step_is_not_a_publish_step() -> None:
    """開始時の Status 更新のように publish.py status だけを実行するステップは公開とみなさない。"""
    workflow = WORKFLOW.replace(
        "steps:\n  - name: develop",
        "steps:\n"
        "  - name: mark_in_progress\n"
        "    description: 開始時に対象 issue を In Progress にする\n"
        "    mode: system\n"
        "    system_inputs:\n"
        "      - type: command\n"
        "        as: status\n"
        "        command: |\n"
        "          python3 tools/project-status/publish.py status --status \"In Progress\"\n"
        "    rules:\n"
        "      - condition: when(true)\n"
        "        next: develop\n"
        "  - name: develop",
    )
    with Sandbox() as sandbox:
        repository(sandbox, workflow=workflow)
        stub_external_tools(sandbox)
        assert_success(sandbox, "status だけを実行する開始ステップ")


def test_publish_stage_detection() -> None:
    """publish.py の段階検出は、正規形のコマンドだけを受理する。

    正規形は `python3 tools/project-status/publish.py <段階>` の1行であり、
    段階の後ろには publish.py 自身のオプション（--status と
    --require-published）だけを置ける。publish.py への言及があるのに正規形に
    一致しないコマンドは、正規形を示す理由つきで拒否する。echo の引数・
    コメント・引用符内の文字列・ラッパー越し・複数行など、記法を問わない。
    publish.py への言及が無いコマンドは公開と無関係であり、そのまま通す。
    """
    with Sandbox() as sandbox:
        module = sandbox.module

        def step(command, as_name="commit"):
            return {
                "name": "create_pr",
                "mode": "system",
                "system_inputs": [{"type": "command", "as": as_name, "command": command}],
            }

        for stage in ("commit", "push", "pr"):
            assert module.publish_stages(
                step(f"python3 tools/project-status/publish.py {stage}", stage)
            ) == {stage: stage}, f"正規形の {stage} を拾う"
        assert module.publish_stages(
            step('python3 tools/project-status/publish.py status --status "In Review"', "status")
        ) == {"status": "status"}, "status の --status（引用符付き）を受理する"
        assert module.publish_stages(
            step("python3 tools/project-status/publish.py status --status 'In Review'", "status")
        ) == {"status": "status"}, "status の --status（単一引用符）を受理する"
        assert module.publish_stages(
            step("python3 tools/project-status/publish.py status --status InReview", "status")
        ) == {"status": "status"}, "status の --status（引用符なし）を受理する"
        assert module.publish_stages(
            step(
                'python3 tools/project-status/publish.py status --status "In Review"'
                " --require-published",
                "status",
            )
        ) == {"status": "status"}, "--require-published を受理する"
        assert module.publish_stages(
            step(
                'python3 tools/project-status/publish.py status --require-published'
                ' --status "In Review"',
                "status",
            )
        ) == {"status": "status"}, "オプションの順序は問わない"
        assert module.publish_stages(step("make done")) == {}, (
            "publish.py への言及が無いコマンドはそのまま通す"
        )
        assert module.publish_stages(
            {
                "name": "create_pr",
                "mode": "system",
                "system_inputs": [
                    {"type": "command", "as": "note", "command": "echo 公開の準備"},
                    {
                        "type": "command",
                        "as": "commit",
                        "command": "python3 tools/project-status/publish.py commit",
                    },
                ],
            }
        ) == {"commit": "commit"}, "言及の無いコマンドと正規形が混ざっても正規形だけを拾う"

        rejected = {
            "echo の引数": "echo python3 tools/project-status/publish.py commit",
            "引用符内の文字列": 'echo "python3 tools/project-status/publish.py commit"',
            "コメント": "# python3 tools/project-status/publish.py commit",
            "bash -c の内側": "bash -c 'python3 tools/project-status/publish.py commit'",
            "前段との連結": "cd frontend && python3 tools/project-status/publish.py commit",
            "python のオプション付き": "python3 -u tools/project-status/publish.py commit",
            "環境変数代入付き": "FOO=1 python3 tools/project-status/publish.py commit",
            "ラッパー越し": "env FOO=1 python3 tools/project-status/publish.py commit",
            "バージョン付きインタプリタ": "python3.11 tools/project-status/publish.py commit",
            "パス付きインタプリタ": "/usr/bin/python3 tools/project-status/publish.py commit",
            "直接実行": "tools/project-status/publish.py commit",
            "複数行": "cat <<'EOF'\nready\nEOF\npython3 tools/project-status/publish.py commit",
            "段階が無い": "python3 tools/project-status/publish.py",
            "段階が違う": "python3 tools/project-status/publish.py deploy",
            "publish.py に無いオプション": "python3 tools/project-status/publish.py commit --verbose",
            "後段との連結": "python3 tools/project-status/publish.py commit; echo done",
        }
        for label, command in rejected.items():
            try:
                module.publish_stages(step(command))
            except module.GateError as error:
                assert "正規形" in str(error), (
                    f"{label}: 拒否の理由に正規形が含まれません: {error}"
                )
            else:
                raise AssertionError(f"{label}: 正規形でないコマンドが受理されました")


def test_publish_workflow_passes() -> None:
    """公開ステップを置く正しい構成は、関門を通る。"""
    with Sandbox() as sandbox:
        repository(sandbox, workflow=PUBLISH_WORKFLOW)
        stub_external_tools(sandbox)
        assert_success(sandbox, "公開ステップを置く構成")


def test_publish_workflow_rejects_noncanonical_forms() -> None:
    """公開コマンドの正規形でない記法は、正規形を示す理由つきで拒否する。

    python のオプション・環境変数代入・ラッパー・バージョン付きインタプリタ・
    ヒアドキュメントの前置き・行継続・前段との連結は、シェルでは公開スクリプトを
    起動する有効な記法だが、正規形に一致しないため公開コマンドとして受理しない。
    """
    stage_lines = [
        "          python3 tools/project-status/publish.py commit\n",
        "          python3 tools/project-status/publish.py push\n",
        "          python3 tools/project-status/publish.py pr\n",
        "          python3 tools/project-status/publish.py status --status \"In Review\"\n",
    ]
    for line in stage_lines:
        assert line in PUBLISH_WORKFLOW, f"置換対象が見つかりません: {line!r}"
    cases = {
        "python のオプション付き": lambda line: line.replace("python3 ", "python3 -u ", 1),
        "環境変数代入付き": lambda line: line.replace(
            "          python3 ", "          PYTHONUNBUFFERED=1 python3 ", 1
        ),
        "ラッパー越し": lambda line: line.replace(
            "python3 ", "env PYTHONUNBUFFERED=1 python3 ", 1
        ),
        "バージョン付きインタプリタ": lambda line: line.replace("python3 ", "python3.11 ", 1),
        "ヒアドキュメントの前置き": lambda line: (
            "          cat <<'EOF'\n          It's a publish step\n          EOF\n" + line
        ),
        "行継続で折り返す": lambda line: line.replace(
            "python3 ", "python3 \\\n            ", 1
        ),
        "前段との連結": lambda line: line.replace("python3 ", "true && python3 ", 1),
    }
    for label, transform in cases.items():
        workflow = PUBLISH_WORKFLOW
        for line in stage_lines:
            workflow = workflow.replace(line, transform(line))
        with Sandbox() as sandbox:
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", "正規形")


def test_publish_commands_must_use_canonical_form() -> None:
    """publish.py への言及だけでは、公開ステップとして受理しない。

    echo の引数・引用符内の文字列・コメント・ヒアドキュメントの本文・構文確認
    だけのシェル・場所の表示だけを行う command -v など、publish.py に言及する
    だけのコマンドは、正規形を示す理由つきで拒否する。
    """
    stage_lines = [
        "          python3 tools/project-status/publish.py commit\n",
        "          python3 tools/project-status/publish.py push\n",
        "          python3 tools/project-status/publish.py pr\n",
        "          python3 tools/project-status/publish.py status --status \"In Review\"\n",
    ]
    for line in stage_lines:
        assert line in PUBLISH_WORKFLOW, f"置換対象が見つかりません: {line!r}"
    cases = {
        "echo で表示するだけ": lambda line: "          echo " + line.strip() + "\n",
        "引用符内の文字列で表示するだけ": lambda line: "          echo \"" + line.strip() + "\"\n",
        "コメントに書くだけ": lambda line: "          # " + line.strip() + "\n",
        "構文確認だけのシェルで表示するだけ": lambda line: "          bash -n -c '"
        + line.strip()
        + "'\n",
        "ヒアドキュメントで表示するだけ": lambda line: "          cat <<'EOF'\n"
        + line
        + "          EOF\n",
        "command -v で場所を表示するだけ": lambda line: "          command -v "
        + line.strip()
        + "\n",
        "ラッパー越しの構文確認だけのシェルで表示するだけ": lambda line: (
            "          env FOO=1 bash -n -c '" + line.strip() + "'\n"
        ),
    }
    for label, transform in cases.items():
        workflow = PUBLISH_WORKFLOW
        for line in stage_lines:
            workflow = workflow.replace(line, transform(line))
        with Sandbox() as sandbox:
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", "正規形")


def test_publish_success_rule_must_precede_unconditional_rule() -> None:
    """公開の成功規則は、when(true) の無条件規則より前に置かなければならない。

    規則は先に一致したものから順に評価されるため、無条件規則が先にあると、公開が
    成功してもそちらが選ばれて aislop_gate へ進まない。
    """
    success_rule = (
        "      - condition: when(context.create_pr.commit.exit_code == 0 && "
        "context.create_pr.push.exit_code == 0 && context.create_pr.pr.exit_code == 0)\n"
        "        next: aislop_gate\n"
    )
    fallback_rule = "      - condition: when(true)\n        next: fix_publish\n"
    assert success_rule + fallback_rule in PUBLISH_WORKFLOW, "公開ステップの規則列が見つかりません"
    workflow = PUBLISH_WORKFLOW.replace(success_rule + fallback_rule, fallback_rule + success_rule)
    with Sandbox() as sandbox:
        repository(sandbox, workflow=workflow)
        stub_external_tools(sandbox)
        assert_failure(
            sandbox, "無条件規則が成功規則より前にある", "workflow-machine-gate", "無条件規則より後"
        )


def test_workflow_requires_aislop_gate_step() -> None:
    """aislop の機械ゲートが無い、順序が違う、失敗時に戻らない、のそれぞれで失敗する。"""
    machine_success_complete = MACHINE_GATE_STEP.replace(
        MACHINE_GATE_SUCCESS_RULE,
        MACHINE_GATE_SUCCESS_RULE.replace("next: aislop_gate", "next: COMPLETE"),
    )
    without_step = workflow_with(
        DEVELOP_STEP,
        machine_success_complete,
        *FIX_MACHINE_GATE_STEPS,
        FIX_AISLOP_GATE_STEP,
    )
    reordered = workflow_with(
        DEVELOP_STEP,
        AISLOP_GATE_STEP,
        MACHINE_GATE_STEP,
        *FIX_MACHINE_GATE_STEPS,
        FIX_AISLOP_GATE_STEP,
    )
    cases = {
        "aislop ステップが無い": (without_step, "aislop"),
        "aislop ステップが machine_gate より前にある": (reordered, "aislop"),
        "aislop の失敗で修正へ遷移しない": (
            workflow_with(
                DEVELOP_STEP,
                MACHINE_GATE_STEP,
                AISLOP_GATE_STEP.replace("        next: fix_aislop_gate", "        next: ABORT"),
                *FIX_MACHINE_GATE_STEPS,
                FIX_AISLOP_GATE_STEP,
            ),
            "fix_aislop_gate",
        ),
        "aislop の修正から machine_gate へ戻らない": (
            workflow_with(
                DEVELOP_STEP,
                MACHINE_GATE_STEP,
                AISLOP_GATE_STEP,
                *FIX_MACHINE_GATE_STEPS,
                FIX_AISLOP_GATE_STEP.replace("        next: machine_gate", "        next: aislop_gate"),
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
    """aislop の修正は、machine_gate をやり直す機械ゲートへ戻らなければならない。

    修正後の成果物に machine_gate を再実行せず COMPLETE へ到達できる遷移と、
    aislop だけを再実行して COMPLETE へ到達できる遷移を拒否する。
    """
    cases = {
        "aislop の修正が COMPLETE へ直接遷移する": FIX_AISLOP_GATE_STEP.replace(
            "        next: machine_gate", "        next: COMPLETE"
        ),
        "aislop の修正が aislop へ直接戻る": FIX_AISLOP_GATE_STEP.replace(
            "        next: machine_gate", "        next: aislop_gate"
        ),
    }
    for label, fix_step in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    MACHINE_GATE_STEP,
                    AISLOP_GATE_STEP,
                    *FIX_MACHINE_GATE_STEPS,
                    fix_step,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", "machine_gate")


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
        assert not module.runs_aislop_gate(
            step("echo '&&' node scripts/aislop-check-bootstrap.mjs check")
        ), "引用符内の && の後ろを実行扱いしない"
        assert not module.runs_aislop_gate(
            step("node scripts/aislop-check-bootstrap.mjs check & > /dev/null")
        ), "バックグラウンド実行の & > を実行扱いしない"
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


AISLOP_CHECK = "node scripts/aislop-check-bootstrap.mjs check"

# チェックの失敗を呼び出し元へ返さない後続。ワークフローと CI の両方で拒否する。
AISLOP_FAILURE_SWALLOWING_SUFFIXES = {
    "末尾の || true": " || true",
    "末尾の ; true": "; true",
    "末尾のパイプ | cat": " | cat",
    "末尾の & > /dev/null": " & > /dev/null",
}


def test_aislop_check_must_return_its_failure() -> None:
    """チェックの失敗が呼び出し元へ返る構成だけを、aislop チェックの起動と判定する。

    後続のコマンドで終了状態を上書きする形、パイプで終了状態を潰す形、バックグラウンド
    実行、否定、前段の成功でチェックを飛ばす形を拒否する。前段を && でつなぐ形、
    チェックが最終行にある複数行の記述、リダイレクションは受理する。
    """
    with Sandbox() as sandbox:
        invokes = sandbox.module.invokes_aislop_check
        rejected = {
            "末尾の || true": f"{AISLOP_CHECK} || true",
            "前段付きで末尾の || true": f"npm ci && {AISLOP_CHECK} || true",
            "末尾の || exit 0": f"{AISLOP_CHECK} || exit 0",
            "末尾の ; true": f"{AISLOP_CHECK}; true",
            "前段付きで末尾の ; true": f"npm ci && {AISLOP_CHECK}; true",
            "末尾のパイプ | cat": f"{AISLOP_CHECK} | cat",
            "前段付きで末尾のパイプ | cat": f"npm ci && {AISLOP_CHECK} | cat",
            "末尾のパイプ |& cat": f"{AISLOP_CHECK} |& cat",
            "バックグラウンド実行": f"{AISLOP_CHECK} &",
            "バックグラウンド実行の & > /dev/null": f"{AISLOP_CHECK} & > /dev/null",
            "引用符内の && の後ろ": f"echo '&&' {AISLOP_CHECK}",
            "二重引用符内の && の後ろ": f'npm ci "&&" {AISLOP_CHECK}',
            "エスケープした ; の後ろ": f"echo \\; {AISLOP_CHECK}",
            "否定": f"! {AISLOP_CHECK}",
            "前段が成功するとチェックを飛ばす ||": f"npm ci || {AISLOP_CHECK}",
            "改行で続けた || の後ろ": f"npm ci ||\n{AISLOP_CHECK}",
            "次の行の true で終了状態を上書きする": f"{AISLOP_CHECK}\ntrue\n",
        }
        for label, command in rejected.items():
            assert not invokes(command), f"{label} を起動とみなしました: {command!r}"
        accepted = {
            "単独の起動": AISLOP_CHECK,
            "前段を && でつなぐ": f"npm ci && {AISLOP_CHECK}",
            "末尾の改行": f"{AISLOP_CHECK}\n",
            "チェックが最終行にある複数行": f"npm ci\n{AISLOP_CHECK}\n",
            "コメント行を含む複数行": f"# 依存を導入する\nnpm ci\n{AISLOP_CHECK}\n",
            "標準エラーの合流": f"{AISLOP_CHECK} 2>&1",
            "ファイルへのリダイレクション": f"{AISLOP_CHECK} > out.txt",
            "&>file へのリダイレクション": f"{AISLOP_CHECK} &>file",
            "&> file へのリダイレクション": f"{AISLOP_CHECK} &> file",
        }
        for label, command in accepted.items():
            assert invokes(command), f"{label} を起動とみなしませんでした: {command!r}"


def test_aislop_gate_command_must_return_the_check_failure() -> None:
    """ワークフローの aislop_gate で、チェックの失敗を潰す command を起動とみなさない。"""
    original = f"        command: npm ci && {AISLOP_CHECK}"
    for label, suffix in AISLOP_FAILURE_SWALLOWING_SUFFIXES.items():
        with Sandbox() as sandbox:
            workflow = WORKFLOW.replace(original, original + suffix)
            assert workflow != WORKFLOW, f"{label}: 置換対象が見つかりません"
            repository(sandbox, workflow=workflow)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", "aislop-gate を実行する")


def test_ci_aislop_gate_command_must_return_the_check_failure() -> None:
    """CI の run で、チェックの失敗を潰す記述を aislop の呼び出しとみなさない。

    前段を && でつなぐ記述は、引き続き呼び出しとして受理する。
    """
    original = f"      - run: {AISLOP_CHECK}\n"
    for label, suffix in AISLOP_FAILURE_SWALLOWING_SUFFIXES.items():
        with Sandbox() as sandbox:
            ci = CI.replace(original, f"      - run: {AISLOP_CHECK}{suffix}\n")
            assert ci != CI, f"{label}: 置換対象が見つかりません"
            repository(sandbox, ci=ci)
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "ci-runs-make-done", "aislop のゲート")
    with Sandbox() as sandbox:
        repository(sandbox, ci=CI.replace(original, f"      - run: npm ci && {AISLOP_CHECK}\n"))
        stub_external_tools(sandbox)
        assert_success(sandbox, "前段を && でつなぐ run")


def test_aislop_gate_rules_must_not_complete_except_on_success() -> None:
    """aislop_gate は成功条件の規則だけで COMPLETE へ進み、ほかの規則は修正ステップへ進む。

    規則は順に評価されるため、成功条件の規則があるだけでは足りない。成功条件以外の規則が
    COMPLETE や別のステップへ進む構成、when(true) が規則列の最後にない構成、修正ステップ
    から COMPLETE へ抜ける構成を拒否する。成功条件以外の規則が修正ステップへ進む構成は
    受理する。
    """
    success_rule = (
        "      - condition: when(context.aislop_gate.result.exit_code == 0)\n"
        "        next: COMPLETE\n"
    )
    fallback_rule = "      - condition: when(true)\n        next: fix_aislop_gate\n"

    def failure_rule(next_step):
        return (
            "      - condition: when(context.aislop_gate.result.exit_code == 1)\n"
            f"        next: {next_step}\n"
        )

    def aislop_rules(*rules):
        replaced = AISLOP_GATE_STEP.replace(success_rule + fallback_rule, "".join(rules))
        assert replaced != AISLOP_GATE_STEP, "aislop_gate の規則列が見つかりません"
        return replaced

    def gate_workflow(aislop_step=AISLOP_GATE_STEP, fix_step=FIX_AISLOP_GATE_STEP):
        return workflow_with(
            DEVELOP_STEP, MACHINE_GATE_STEP, aislop_step, *FIX_MACHINE_GATE_STEPS, fix_step
        )

    fix_completes = FIX_AISLOP_GATE_STEP.replace(
        "    rules:\n",
        "    rules:\n      - condition: 指摘を確認した\n        next: COMPLETE\n",
    )
    assert fix_completes != FIX_AISLOP_GATE_STEP, "fix_aislop_gate の規則列が見つかりません"
    cases = {
        "成功条件以外の規則が先頭で COMPLETE へ進む": (
            aislop_rules(failure_rule("COMPLETE"), success_rule, fallback_rule),
            "COMPLETE",
        ),
        "成功条件以外の規則が成功規則の後ろで COMPLETE へ進む": (
            aislop_rules(success_rule, failure_rule("COMPLETE"), fallback_rule),
            "COMPLETE",
        ),
        "when(true) が成功規則より前にある": (
            aislop_rules(fallback_rule, success_rule),
            "when(true)",
        ),
        "when(true) が 2 件ある": (
            aislop_rules(success_rule, fallback_rule, fallback_rule),
            "when(true)",
        ),
        "成功条件以外の規則が develop へ進む": (
            aislop_rules(failure_rule("develop"), success_rule, fallback_rule),
            "fix_aislop_gate",
        ),
        "成功条件以外の規則が ABORT へ進む": (
            aislop_rules(failure_rule("ABORT"), success_rule, fallback_rule),
            "fix_aislop_gate",
        ),
    }
    for label, (aislop_step, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(sandbox, workflow=gate_workflow(aislop_step=aislop_step))
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)

    with Sandbox() as sandbox:
        repository(sandbox, workflow=gate_workflow(fix_step=fix_completes))
        stub_external_tools(sandbox)
        assert_failure(
            sandbox,
            "fix_aislop_gate が machine_gate への戻りを残したまま COMPLETE へ進む",
            "workflow-machine-gate",
            "machine_gate か ABORT",
        )

    with Sandbox() as sandbox:
        accepted = aislop_rules(
            "      - condition: when(context.aislop_gate.result.exit_code != 0)\n"
            "        next: fix_aislop_gate\n",
            success_rule,
            fallback_rule,
        )
        repository(sandbox, workflow=gate_workflow(aislop_step=accepted))
        stub_external_tools(sandbox)
        assert_success(sandbox, "成功条件以外の規則が修正ステップへ進む")


def test_aislop_gate_success_condition_must_reference_its_own_gate() -> None:
    """aislop_gate の成功条件は、別ゲートではなく aislop_gate 自身の exit_code だけを見る。"""
    success = "      - condition: when(context.aislop_gate.result.exit_code == 0)"
    cases = {
        "aislop_gate が machine_gate の exit_code を成功条件にする": (
            AISLOP_GATE_STEP.replace(
                success,
                "      - condition: when(context.machine_gate.result.exit_code == 0)",
            ),
            "aislop_gate",
        ),
        "aislop_gate が別ゲートの成功を AND で併記する": (
            AISLOP_GATE_STEP.replace(
                success,
                "      - condition: when(context.aislop_gate.result.exit_code != 0 "
                "&& context.machine_gate.result.exit_code == 0)",
            ),
            "aislop_gate",
        ),
        "aislop_gate が別ゲートの成功を OR で併記する": (
            AISLOP_GATE_STEP.replace(
                success,
                "      - condition: when(context.aislop_gate.result.exit_code == 0 "
                "|| context.machine_gate.result.exit_code == 0)",
            ),
            "aislop_gate",
        ),
        "aislop_gate が自ゲートの否定とリテラル真を併記する": (
            AISLOP_GATE_STEP.replace(
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
    extra_rule = (
        "      - condition: when(context.machine_gate.result.exit_code == 0)\n"
        "        next: COMPLETE\n"
    )
    for position, replaced in (
        ("成功条件の規則より前", extra_rule + aislop_success_rule),
        ("成功条件の規則より後", aislop_success_rule + extra_rule),
    ):
        cases[f"aislop_gate の別規則が{position}に別ゲートの成功を見る"] = (
            AISLOP_GATE_STEP.replace(aislop_success_rule, replaced),
            "別ゲート machine_gate",
        )
    for label, (aislop_step, snippet) in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    MACHINE_GATE_STEP,
                    aislop_step,
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(sandbox, label, "workflow-machine-gate", snippet)


def test_aislop_gate_success_must_complete() -> None:
    """aislop_gate の成功は COMPLETE へ遷移しなければならない。

    成功条件は aislop_gate 自身の exit_code == 0 のまま、遷移先だけを変異させる。
    """
    success_rule = (
        "      - condition: when(context.aislop_gate.result.exit_code == 0)\n"
        "        next: COMPLETE\n"
    )
    cases = {
        "aislop の成功が修正ステップへ遷移する": success_rule.replace(
            "next: COMPLETE", "next: fix_aislop_gate"
        ),
        "aislop の成功が ABORT へ遷移する": success_rule.replace("next: COMPLETE", "next: ABORT"),
    }
    for label, replaced in cases.items():
        with Sandbox() as sandbox:
            repository(
                sandbox,
                workflow=workflow_with(
                    DEVELOP_STEP,
                    MACHINE_GATE_STEP,
                    AISLOP_GATE_STEP.replace(success_rule, replaced),
                    *FIX_MACHINE_GATE_STEPS,
                    FIX_AISLOP_GATE_STEP,
                ),
            )
            stub_external_tools(sandbox)
            assert_failure(
                sandbox, label, "workflow-machine-gate", "COMPLETE へ遷移していません"
            )


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
        test_machine_gate_develop_complete_and_mode,
        test_status_only_step_is_not_a_publish_step,
        test_publish_stage_detection,
        test_publish_workflow_passes,
        test_publish_workflow_rejects_noncanonical_forms,
        test_publish_commands_must_use_canonical_form,
        test_publish_success_rule_must_precede_unconditional_rule,
        test_machine_gate_requires_six_system_inputs,
        test_machine_gate_commands_invoke_their_make_targets,
        test_machine_gate_success_requires_all_six_exit_codes,
        test_machine_gate_requires_single_success_rule,
        test_machine_gate_failure_routes_to_matching_fix_step,
        test_machine_gate_fix_steps_only_use_their_own_output,
        test_machine_gate_fix_steps_return_to_machine_gate,
        test_workflow_requires_aislop_gate_step,
        test_aislop_fix_must_reenter_machine_gate,
        test_aislop_gate_command_detection,
        test_aislop_gate_command_must_invoke_the_check,
        test_aislop_check_must_return_its_failure,
        test_aislop_gate_command_must_return_the_check_failure,
        test_aislop_gate_rules_must_not_complete_except_on_success,
        test_aislop_gate_success_condition_must_reference_its_own_gate,
        test_aislop_gate_success_must_complete,
        test_ci_must_run_make_done_with_toolchains,
        test_ci_must_call_aislop_gate,
        test_ci_aislop_gate_command_must_invoke_the_check,
        test_ci_aislop_gate_command_must_return_the_check_failure,
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
