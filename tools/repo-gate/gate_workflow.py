"""taktdash-change の develop → machine_gate → 公開 → aislop_gate の配線を検査する。"""
from gate_aislop import AISLOP_FAILURE_NOTE, runs_aislop_gate
from gate_common import GATE_TARGETS, GateError, read_workflow
from gate_machine_gate_rules import check_machine_gate_five_checks
from gate_publish import (
    check_publish_retry_rule,
    check_publish_success_rule,
    publish_candidates,
    publish_failure_step,
    publish_step,
)
from gate_step_rules import check_machine_gate_step
from gate_workflow_steps import rules_of, transition, workflow_steps


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
