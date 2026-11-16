"""ワークフローのステップと規則を読み取る補助と、修正ステップの遷移先を限る検査。"""
from gate_common import GateError


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


def instruction_text(step):
    instruction = step.get("instruction")
    if isinstance(instruction, list):
        return "\n".join(str(item) for item in instruction)
    return str(instruction or "")


def check_fix_step_transitions(fix_name, fix_rules, return_to):
    """修正ステップの遷移先が、戻り先（return_to）か ABORT だけであることを検査する。"""
    allowed = {return_to, "ABORT"}
    unexpected = sorted({str(rule.get("next")) for rule in fix_rules if rule.get("next") not in allowed})
    if unexpected:
        raise GateError(
            f"{fix_name} に許可されない遷移先があります: {unexpected!r}。"
            f"修正ステップの遷移先は {return_to} か ABORT に限ります"
        )


def transition(rules, predicate):
    for rule in rules:
        if predicate(rule):
            return rule.get("next")
    return None
