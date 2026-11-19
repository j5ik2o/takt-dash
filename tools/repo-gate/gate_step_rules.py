"""単一の exit_code を見るゲートのステップ（aislop_gate など）の規則と修正ステップを検査する。"""
import re

from gate_common import GateError
from gate_workflow_steps import check_fix_step_transitions, instruction_text, rules_of

GATE_EXIT_CODE_REFERENCE = re.compile(r"context\.([A-Za-z0-9_]+)\.result\.exit_code")
GATE_SUCCESS_CONDITION = re.compile(
    r"when\(\s*context\.([A-Za-z0-9_]+)\.result\.exit_code\s*==\s*0\s*\)"
)


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
    zero_excluding = re.compile(
        rf"when\(\s*context\.{re.escape(name)}\.result\.exit_code\s*"
        r"(?:(==)\s*(-?\d+)|(!=)\s*0)\s*\)"
    )
    for rule in gate_rules:
        if rule is successes[0]:
            break
        match = zero_excluding.fullmatch(str(rule.get("condition", "")).strip())
        if match is None or (match.group(1) == "==" and int(match.group(2)) == 0):
            raise GateError(f"{name} の成功規則より前の条件が終了コード0を除外していません")
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
