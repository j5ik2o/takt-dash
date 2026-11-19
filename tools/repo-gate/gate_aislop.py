"""コマンド文字列とステップが aislop チェックを起動し、その失敗を返すかを判定する。"""
from gate_shell import REDIRECTION, SHELL_KEYWORDS, is_shell_operator, shell_tokens, simple_commands

AISLOP_CHECK_COMMAND = "scripts/aislop-check-bootstrap.mjs check"
AISLOP_CHECK_INVOCATION = ["node", "scripts/aislop-check-bootstrap.mjs", "check"]
AISLOP_FAILURE_NOTE = (
    "|| true・; true・パイプ・後続のコマンドなどでチェックの失敗を潰す記述は、"
    "実行とみなしません"
)


def invokes_aislop_check(command):
    """コマンド文字列が aislop チェックを起動し、その失敗を呼び出し元へ返すかを判定する。

    コメントを除去してクォートを保ったままトークンへ分け、チェックの起動がコマンド
    全体の最後の単純コマンドになっている場合だけ真を返す。直前はコマンドの先頭・&&・
    ; ・改行に限り、直後は引数とリダイレクション、末尾の ; と改行に限る。このため、
    `|| true`・`; true`・次の行のコマンドで終了状態を上書きする形、`| cat` などの
    パイプ、`&` によるバックグラウンド実行、`!` による否定、`npm ci || check` のように
    チェックが飛ばされる形は起動とみなさない。シェルの -e などの実行時設定には頼らず、
    失敗が返ることを記述だけから確かめられる形に限る保守的な判定である。echo の引数・
    引用符内の文字列・コメントは単純コマンドの先頭にならないため、実行とみなさない。
    """
    try:
        tokens = shell_tokens(command)
    except ValueError:
        return False
    width = len(AISLOP_CHECK_INVOCATION)
    return any(
        tokens[index : index + width] == AISLOP_CHECK_INVOCATION
        and starts_command_list_item(tokens[:index])
        and ends_command_list(tokens[index + width :])
        for index in range(len(tokens))
    )


def starts_command_list_item(preceding):
    """直前のトークン列が、終了状態をそのまま返す位置で次のコマンドを始めるかを判定する。

    改行だけのトークンを読み飛ばした手前が、先頭・&&・; なら真とする。改行を挟んだ
    手前が普通の語なら、改行がコマンドの区切りなので真とする。|| と | と & と ( は、
    `||` の直後の改行（行継続）を含めて偽とする。改行を挟まずに語が続く場合は、
    その語の引数（echo の引数や ! の否定対象）なので偽とする。先行する各単純コマンドも
    検査し、終了・評価・シェル設定の変更や、解析対象外の制御構文は受理しない。
    """
    index = len(preceding)
    while (
        index > 0
        and is_shell_operator(preceding[index - 1])
        and preceding[index - 1].strip("\n") == ""
    ):
        index -= 1
    if index == 0:
        return True
    previous = preceding[index - 1]
    if is_shell_operator(previous):
        if previous not in ("&&", ";"):
            return False
    elif index == len(preceding):
        return False
    # 終了・評価・シェル設定を変える組み込みや複合構文は、後段への到達を保証できない。
    for token in preceding:
        if is_shell_operator(token) and token not in ("&&", ";", "\n"):
            return False
    unsafe_heads = SHELL_KEYWORDS | {
        "exit", "exec", "return", "eval", ".", "source", "trap", "set",
        "alias", "unalias", "command", "builtin", "break", "continue", "logout",
        "enable", "shopt", "time",
    }
    for command in simple_commands(preceding):
        head = command[0]
        # 引用状態は文字単位では残らないため、語頭の禁止名に続く <・> は保守的に扱う。
        command_name = head.partition("<")[0].partition(">")[0]
        if (
            command_name in unsafe_heads
            or any(char in head for char in "=$`\n")
            or REDIRECTION.match(head)
        ):
            return False
    return True


def ends_command_list(following):
    """起動の後ろのトークン列が、引数・リダイレクション・末尾の ; と改行だけかを判定する。"""
    for index, token in enumerate(following):
        if not is_shell_operator(token):
            continue
        if token.strip(";\n") == "":
            return all(rest.strip(";\n") == "" for rest in following[index + 1 :])
        if token == "&" and is_redirection_ampersand(following, index):
            continue
        return False
    return True


def is_redirection_ampersand(tokens, index):
    """`2>&1` や `&>file` の & を、バックグラウンド実行ではなくリダイレクションと判定する。

    `&` の直後に空白を挟まず `>` が続く場合だけリダイレクションとする。
    `& > file` は、バックグラウンド実行の後にリダイレクションだけの
    コマンドが続く形で、チェックの失敗は返らない。
    """
    before = tokens[index - 1] if index > 0 else ""
    after = tokens[index + 1] if index + 1 < len(tokens) else ""
    if before and not getattr(before, "quoted", False) and REDIRECTION.fullmatch(before):
        return True
    if not after.startswith(">"):
        return False
    return not getattr(after, "space_before", False)


def runs_aislop_gate(step):
    """mode が system のステップで、aislop の関門を実際に起動するかを判定する。"""
    if step.get("mode") != "system":
        return False
    results = [
        item for item in step.get("system_inputs") or []
        if isinstance(item, dict) and item.get("as") == "result"
    ]
    return (
        len(results) == 1
        and results[0].get("type") == "command"
        and invokes_aislop_check(str(results[0].get("command", "")))
    )
