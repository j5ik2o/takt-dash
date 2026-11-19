"""シェルのコマンド文字列を、クォートとコメントを考慮してトークンと単純コマンドへ分ける。"""
import re

SHELL_SEPARATORS = {";", "&", "|", "&&", "||", "(", ")", "\n"}
SHELL_KEYWORDS = {
    "if",
    "then",
    "else",
    "elif",
    "fi",
    "do",
    "done",
    "while",
    "until",
    "!",
    "{",
    "}",
}
REDIRECTION = re.compile(r"^\d*[<>]")
# shell_tokens が演算子のトークンとして切り出す文字。
SHELL_OPERATOR_CHARS = ";&|\n()"


def strip_shell_comments(command):
    """シェルのコメントを、クォートとバックスラッシュを考慮して除去する。

    未クォート・未エスケープで語頭にある # から行末までを除去する。行の区切りを保つため改行は
    残す。単一引用符の内側ではバックスラッシュと # をそのまま扱い、二重引用符の
    内側ではバックスラッシュで次の 1 文字をエスケープする。語中の # は保持する。
    """
    result = []
    quote = None
    escaped = False
    in_comment = False
    word_start = True
    for char in str(command):
        if in_comment:
            if char == "\n":
                in_comment = False
                word_start = True
                result.append(char)
            continue
        if escaped:
            result.append(char)
            escaped = False
            if char != "\n":
                word_start = False
            continue
        if quote == "'":
            result.append(char)
            if char == "'":
                quote = None
            continue
        if quote == '"':
            if char == "\\":
                escaped = True
            else:
                quote = None if char == '"' else quote
            result.append(char)
            continue
        if char == "\\":
            escaped = True
            result.append(char)
            continue
        if char in ("'", '"'):
            quote = char
            word_start = False
            result.append(char)
            continue
        if char == "#" and word_start:
            in_comment = True
            continue
        result.append(char)
        word_start = char in " \t\r\n;&|()<>"
    return "".join(result)


def is_shell_operator(token):
    """トークンがシェルの演算子かを判定する。引用符やエスケープで作られたものは演算子ではない。"""
    return (
        bool(token)
        and not getattr(token, "quoted", False)
        and all(char in SHELL_OPERATOR_CHARS for char in token)
    )


class ShellToken(str):
    """シェルのトークン。文字列としての比較はそのまま使える。

    quoted: 引用符の中かバックスラッシュの直後で作られた文字を含む場合に真。
    演算子に見える文字列でも、引用されたものは演算子ではない。
    quoted_mask: 各文字が引用符の中かバックスラッシュの直後で作られたかの並び。
    部分文字列が未引用の文字だけでできているかを判定するために使う。
    space_before: 直前に空白（スペース・タブ・復帰）があった場合に真。
    `&>` と `& >` の区別に使う。
    """

    def __new__(cls, text, *, quoted=False, space_before=False, quoted_mask=None):
        token = super().__new__(cls, text)
        token.quoted = quoted
        token.space_before = space_before
        token.quoted_mask = (
            tuple(quoted_mask) if quoted_mask is not None else tuple(False for _ in text)
        )
        return token


def shell_tokens(command):
    """シェルのコマンド文字列を、クォートを保ったままトークンへ分ける。

    各トークンは ShellToken で、引用符やエスケープで作られた文字を含むか
    （quoted）と、直前に空白があったか（space_before）を保持する。演算子
    の判定はこの情報を使うため、引用符内の `&&` や `;`、エスケープされた
    `;` は演算子にならない。`&>` と `& >` も区別できる。閉じていない
    引用符がある場合は、shlex と同じく ValueError を上げる。各文字の引用状態は
    quoted_mask にも保持するため、引用された部分文字列と未引用の演算子を区別できる。
    """
    text = strip_shell_comments(command)
    tokens = []
    index = 0
    pending_space = False

    def flush_word(chars, quoted, space_before, quoted_mask=()):
        if chars or quoted:
            tokens.append(
                ShellToken(
                    "".join(chars),
                    quoted=quoted,
                    space_before=space_before,
                    quoted_mask=quoted_mask,
                )
            )

    while index < len(text):
        char = text[index]
        if char in " \t\r":
            pending_space = True
            index += 1
            continue
        if char in SHELL_OPERATOR_CHARS:
            if char in "&|;":
                end = index + 1
                while end < len(text) and text[end] == char:
                    end += 1
                tokens.append(ShellToken(text[index:end], space_before=pending_space))
                index = end
            else:
                tokens.append(ShellToken(char, space_before=pending_space))
                index += 1
            pending_space = False
            continue
        word_space = pending_space
        pending_space = False
        chars = []
        quoted = False
        quoted_mask = []
        while index < len(text):
            char = text[index]
            if char in " \t\r" or char in SHELL_OPERATOR_CHARS:
                break
            if char == "\\" and index + 1 < len(text):
                chars.append(text[index + 1])
                quoted_mask.append(True)
                quoted = True
                index += 2
                continue
            if char in ("'", '"'):
                quote = char
                quoted = True
                index += 1
                while index < len(text) and text[index] != quote:
                    if quote == '"' and text[index] == "\\" and index + 1 < len(text):
                        chars.append(text[index + 1])
                        quoted_mask.append(True)
                        index += 2
                        continue
                    chars.append(text[index])
                    quoted_mask.append(True)
                    index += 1
                if index >= len(text):
                    raise ValueError("No closing quotation")
                index += 1
                continue
            chars.append(char)
            quoted_mask.append(False)
            index += 1
        flush_word(chars, quoted, word_space, quoted_mask)
    return tokens


def contains_unquoted(token, substring):
    """トークンに、引用されていない文字だけでできた部分文字列が含まれるかを判定する。

    quoted_mask が無いトークンは、従来どおり部分文字列の有無だけで判定する。
    """
    text = str(token)
    mask = getattr(token, "quoted_mask", None)
    if not substring or len(substring) > len(text):
        return False
    if mask is None or len(mask) != len(text):
        return substring in text
    return any(
        text[start:start + len(substring)] == substring
        and not any(mask[start:start + len(substring)])
        for start in range(len(text) - len(substring) + 1)
    )


def simple_commands(tokens):
    """トークン列を、&& / || / ; / | / 改行などの区切りで単純コマンドへ分ける。"""
    current = []
    for token in tokens:
        if token in SHELL_SEPARATORS and is_shell_operator(token):
            if current:
                yield current
            current = []
            continue
        current.append(token)
    if current:
        yield current


def invoked_make_targets(command, depth=0):
    """コマンド文字列が実際に起動する make のターゲットを集める。

    bash -c や sh -c の内側のスクリプトへ再帰し、if/then などの制御語を除いた
    単純コマンドの先頭が make のときだけ、フラグと変数代入を除いたターゲットを
    拾う。echo の引数や引用符内の文字列、コメントは起動として扱わない。
    """
    found = set()
    if depth > 5:
        return found
    try:
        tokens = shell_tokens(command)
    except ValueError:
        return found
    for tokens_of_command in simple_commands(tokens):
        while tokens_of_command and tokens_of_command[0] in SHELL_KEYWORDS:
            tokens_of_command = tokens_of_command[1:]
        if not tokens_of_command:
            continue
        head = tokens_of_command[0]
        if head in ("bash", "sh") and "-c" in tokens_of_command:
            index = tokens_of_command.index("-c")
            if index + 1 < len(tokens_of_command):
                found |= invoked_make_targets(tokens_of_command[index + 1], depth + 1)
            continue
        if head != "make":
            continue
        for token in tokens_of_command[1:]:
            if REDIRECTION.match(token):
                break
            if token.startswith("-"):
                continue
            if "=" in token and token.split("=", 1)[0].isidentifier():
                continue
            found.add(token)
    return found
