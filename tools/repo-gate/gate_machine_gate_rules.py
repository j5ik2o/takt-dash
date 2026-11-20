"""ワークフローの machine_gate ステップが6検査を実行し、失敗を検査ごとの修正へ回すかを検査する。"""
import re

from gate_common import GATE_TARGETS, GateError, gate_make_target
from gate_shell import invoked_make_targets
from gate_workflow_steps import check_fix_step_transitions, instruction_text, rules_of

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
MACHINE_GATE_SUCCESS_TERM = re.compile(
    r"context\.machine_gate\.([A-Za-z0-9_]+)\.exit_code\s*==\s*0"
)
MACHINE_GATE_FAILURE_CONDITION = re.compile(
    r"when\(\s*context\.machine_gate\.([A-Za-z0-9_]+)\.exit_code\s*!=\s*0\s*\)"
)
CONTEXT_REFERENCE = re.compile(r"\{context:([^}]+)\}")


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
