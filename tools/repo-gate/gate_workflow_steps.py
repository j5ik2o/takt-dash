"""ワークフローのステップと規則を読み取る補助と、修正ステップの遷移先を限る検査。"""
import re

from gate_common import GateError


TERMINALS = ("COMPLETE", "ABORT")
NOTIFY_SCRIPT = ".takt/scripts/notify_orca.py"


def notify_terminal(step):
    """終わりを Orca へ知らせるだけの工程なら、その後に進む終端（COMPLETE / ABORT）を返す。

    mode: system で、system_inputs が notify_orca.py を呼ぶ command 1 つだけ、rules が
    when(true) から COMPLETE か ABORT への 1 つだけの工程を通知の工程とみなす。
    """
    if not isinstance(step, dict) or step.get("mode", step.get("kind")) != "system":
        return None
    inputs = step.get("system_inputs")
    rules = step.get("rules")
    if not (isinstance(inputs, list) and len(inputs) == 1 and isinstance(rules, list) and len(rules) == 1):
        return None
    command = inputs[0].get("command") if isinstance(inputs[0], dict) else None
    rule = rules[0] if isinstance(rules[0], dict) else {}
    if not (isinstance(command, str) and NOTIFY_SCRIPT in command):
        return None
    if not re.fullmatch(r"when\(\s*true\s*\)", str(rule.get("condition") or "").strip()):
        return None
    return rule.get("next") if rule.get("next") in TERMINALS else None


def _through_notify(steps):
    """通知の工程を取り除き、そこへの遷移をその先の終端へ置き換えた steps を返す。

    通知の工程は判定を変えずに終端へ進むため、配線の検査では終端と同じに扱う。
    """
    terminals = {}
    for step in steps:
        terminal = notify_terminal(step)
        if terminal is not None and isinstance(step.get("name"), str):
            terminals[step["name"]] = terminal
    if not terminals:
        return steps
    resolved = []
    for step in steps:
        if not isinstance(step, dict) or step.get("name") in terminals:
            if not isinstance(step, dict):
                resolved.append(step)
            continue
        rules = step.get("rules")
        if isinstance(rules, list):
            step = dict(step)
            step["rules"] = [
                {**rule, "next": terminals[rule["next"]]}
                if isinstance(rule, dict) and rule.get("next") in terminals else rule
                for rule in rules
            ]
        resolved.append(step)
    return resolved


def workflow_steps(workflow):
    steps = workflow.get("steps")
    if not isinstance(steps, list):
        raise GateError("taktdash-change に steps がありません")
    steps = _through_notify(steps)
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
    for rule in fix_rules:
        if rule.get("next") == return_to:
            break
        if re.fullmatch(r"when\(\s*true\s*\)", str(rule.get("condition") or "").strip()):
            raise GateError(f"{fix_name} の無条件規則が {return_to} への戻りを遮っています")


def transition(rules, predicate):
    for rule in rules:
        if predicate(rule):
            return rule.get("next")
    return None
