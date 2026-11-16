"""公開ステップ（publish.py）の段階、成功条件、失敗時の修正ステップを検査する。"""
from gate_common import GateError
from gate_workflow_steps import rules_of, transition

PUBLISH_STAGES = ("commit", "push", "pr", "status")
PUBLISH_SUCCESS_STAGES = ("commit", "push", "pr")


def publish_stages(step):
    """公開ステップが実行する publish.py の段階を、{段階名: as 名} で返す。"""
    stages = {}
    for item in step.get("system_inputs") or []:
        if not isinstance(item, dict) or item.get("type") != "command":
            continue
        tokens = [token for token in str(item.get("command", "")).split() if token]
        for index, token in enumerate(tokens):
            if not token.endswith("publish.py") or index + 1 >= len(tokens):
                continue
            stage = tokens[index + 1]
            if stage in PUBLISH_STAGES:
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
    """公開ステップの成功条件が aislop ゲートへ進む形になっていることを確かめる。"""
    success_rules = [rule for rule in publish_rules if rule.get("condition") != "when(true)"]
    if len(success_rules) != 1:
        raise GateError(
            f"公開ステップ {publish_name} で成功を表す rule が1件ではありません: {len(success_rules)} 件"
        )
    success_rule = success_rules[0]
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
