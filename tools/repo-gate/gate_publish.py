"""公開ステップ（publish.py）の段階、成功条件、失敗時の修正ステップを検査する。"""
import re

from gate_common import GateError
from gate_workflow_steps import rules_of, transition

PUBLISH_STAGES = ("commit", "push", "pr", "status")
PUBLISH_SUCCESS_STAGES = ("commit", "push", "pr")


# 公開コマンドの正規形。公開ステップのコマンドは、この形に一致する1行だけを
# 受理する。段階の後ろには publish.py 自身のオプションだけを置ける。
# オプションの値は、シェルの制御演算子や置換を含まないリテラルだけを受理する。
# `--status x;true` のように値へ後続のコマンドを付けた入力は、公開コマンド
# 1つだけという正規形の定義に反するため受理しない。
PUBLISH_COMMAND_FORM = (
    'python3 tools/project-status/publish.py <段階> [--status "<名前>"] [--require-published]'
)
CANONICAL_PUBLISH_COMMAND = re.compile(
    r"^python3 tools/project-status/publish\.py (commit|push|pr|status)"
    r"((?: --status (?:\"[^\"]+\"|'[^']+'|\S+)| --require-published)*)$"
)
STATUS_OPTION_VALUE = re.compile(r"--status (\"[^\"]+\"|'[^']+'|\S+)")
# 引用符で囲まれていない値に含まれると、後続のコマンド・パイプ・
# リダイレクトとして解釈される文字。
UNQUOTED_VALUE_OPERATORS = ";&|<>"


def status_value_is_literal(raw):
    """--status の値が、シェルの制御演算子や置換を含まないリテラルかを返す。

    引用符で囲まれた値でも、コマンド置換（$(...) とバッククォート）は
    実行されるため受理しない。引用符で囲まれていない値は、制御演算子や
    コマンド置換を含むものを受理しない。
    """
    if raw[:1] in ("\"", "'"):
        inner = raw[1:-1]
        return "$(" not in inner and "`" not in inner
    if "$(" in raw or "`" in raw:
        return False
    return not any(operator in raw for operator in UNQUOTED_VALUE_OPERATORS)


def canonical_publish_stage(command):
    """コマンド文字列が公開コマンドの正規形なら、その段階名を返す。

    正規形は `python3 tools/project-status/publish.py <段階>` の1行であり、
    段階の後ろには publish.py 自身のオプション（--status と
    --require-published）だけを置ける。--status の値は、シェルの制御演算子や
    置換を含まないリテラルでなければならない。正規形でなければ None を返す。
    """
    text = str(command).strip()
    if "\n" in text:
        return None
    match = CANONICAL_PUBLISH_COMMAND.match(text)
    if match is None:
        return None
    for raw in STATUS_OPTION_VALUE.findall(match.group(2)):
        if not status_value_is_literal(raw):
            return None
    return match.group(1)


def publish_stages(step):
    """公開ステップが実行する publish.py の段階を、{段階名: as 名} で返す。

    段階の検出は正規形の照合で行う。コマンド入力に publish.py への言及が
    あるのに正規形に一致しない場合は、正規形を示して GateError にする。
    echo の引数・コメント・引用符内の文字列・ラッパー越しの起動など、記法を
    問わずすべてここで落ちる。publish.py への言及が無いコマンドは公開と
    無関係であり、そのまま通す。
    """
    stages = {}
    for item in step.get("system_inputs") or []:
        if not isinstance(item, dict) or item.get("type") != "command":
            continue
        command = str(item.get("command", ""))
        if "publish.py" not in command:
            continue
        stage = canonical_publish_stage(command)
        if stage is None:
            raise GateError(
                f"公開コマンドが正規形ではありません。正規形は {PUBLISH_COMMAND_FORM} の1行です"
                f"（段階は commit / push / pr / status）: {command!r}"
            )
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
