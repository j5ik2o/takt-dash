"""frontend/web の CSS とコンポーネント（.tsx）にある色の直書きと列数の固定を検出する。

repo_gate.py の frontend-style 検査が使う。判定の基準と除外の規則は docs/layout.md の
「機械の検査」の節に書いてあり、ここを変えるときは文書も合わせて直す。
"""
import os
import re
from pathlib import Path

WEB_DIR = "frontend/web"
# どの深さにあっても走査しないディレクトリ（依存と生成物）。
EXCLUDED_DIR_NAMES = {"node_modules", ".next", "out", "coverage"}
# frontend/web からの相対パスで走査しないディレクトリ（proto の生成物、テスト、静的ファイル）。
EXCLUDED_DIR_PATHS = {"src/proto", "test", "public"}

COLOR = "色"
COLUMNS = "列数"

# 既存の画面用クラスのうち、部品の適用（issue #121 など）まで残す宣言。
# (frontend/web からの相対パス, 文脈の列, プロパティ, 空白を 1 つに詰めた値) が完全に
# 一致するときだけ通す。宣言が消えたら一覧からも外す（残すと検査が失敗する）。
# 新しい例外は足さず、色は :root の変数、列数は部品か auto-fit で書く。
GLOBALS_CSS = "app/globals.css"
EXISTING_EXCEPTIONS = [
    (GLOBALS_CSS, (".cards",), "grid-template-columns", "repeat(2, minmax(0, 1fr))"),
    (GLOBALS_CSS, (".dot.live",), "box-shadow", "0 0 0 0 rgba(15, 139, 141, 0.45)"),
    (GLOBALS_CSS, ("@keyframes pulse", "0%"), "box-shadow", "0 0 0 0 rgba(15, 139, 141, 0.5)"),
    (GLOBALS_CSS, ("@keyframes pulse", "70%"), "box-shadow", "0 0 0 7px rgba(15, 139, 141, 0)"),
    (GLOBALS_CSS, ("@keyframes pulse", "100%"), "box-shadow", "0 0 0 0 rgba(15, 139, 141, 0)"),
    (GLOBALS_CSS, ("@keyframes flash", "from"), "background-color", "rgba(15, 139, 141, 0.14)"),
    (GLOBALS_CSS, (".sum-grid",), "grid-template-columns", "repeat(4, minmax(0, 1fr))"),
    (GLOBALS_CSS, (".dialog-backdrop",), "background", "rgba(31, 35, 40, 0.4)"),
    (GLOBALS_CSS, ("@media (max-width: 1280px)", ".cards"), "grid-template-columns", "repeat(2, minmax(0, 1fr))"),
    (GLOBALS_CSS, ("@media (max-width: 1280px)", ".sum-grid"), "grid-template-columns", "repeat(2, minmax(0, 1fr))"),
]

HEX_COLOR = re.compile(r"(?<![\w&-])#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![\w-])")
COLOR_FUNCTION = re.compile(r"(?<![\w-])(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(", re.IGNORECASE)
NAMED_COLORS = (
    "aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue "
    "blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk "
    "crimson cyan darkblue darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki "
    "darkmagenta darkolivegreen darkorange darkorchid darkred darksalmon darkseagreen "
    "darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink deepskyblue "
    "dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite "
    "gold goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki "
    "lavender lavenderblush lawngreen lemonchiffon lightblue lightcoral lightcyan "
    "lightgoldenrodyellow lightgray lightgreen lightgrey lightpink lightsalmon lightseagreen "
    "lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime limegreen linen "
    "magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen "
    "mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream "
    "mistyrose moccasin navajowhite navy oldlace olive olivedrab orange orangered orchid "
    "palegoldenrod palegreen paleturquoise palevioletred papayawhip peachpuff peru pink plum "
    "powderblue purple rebeccapurple red rosybrown royalblue saddlebrown salmon sandybrown "
    "seagreen seashell sienna silver skyblue slateblue slategray slategrey snow springgreen "
    "steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow "
    "yellowgreen"
).split()
COLOR_NAME = re.compile(r"(?<![\w-])(?:" + "|".join(NAMED_COLORS) + r")(?![\w-])", re.IGNORECASE)

CSS_COMMENT = re.compile(r"/\*.*?\*/", re.DOTALL)
CSS_STRING_OR_URL = re.compile(r"\"[^\"]*\"|'[^']*'|url\([^)]*\)", re.IGNORECASE)
CSS_GRID_PROPERTIES = {"grid-template-columns", "grid-template", "grid"}
CSS_COLUMN_PROPERTIES = {"columns", "column-count"}
CSS_FIXED_REPEAT = re.compile(r"repeat\(\s*\d+\s*,")
LEADING_INTEGER = re.compile(r"^\d")

TSX_BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.DOTALL)
# `https://` の // を残すため、行頭か空白の直後の // だけをコメントとみなす。
TSX_LINE_COMMENT = re.compile(r"(?:^|(?<=\s))//.*$", re.MULTILINE)
TSX_STRING = re.compile(r"\"[^\"\n]*\"|'[^'\n]*'|`[^`\n]*`")
# style の宣言として読むキー。色を持つキーと、列数を持つキー（gridTemplateColumns など）と、
# 列数のキー（columnCount と columns）を、キーと値の組として読む。
TSX_COLOR_PROPERTY = (
    r"color|background|backgroundColor|backgroundImage|border\w*|outline\w*|fill|stroke"
    r"|boxShadow|textShadow|caretColor|accentColor|textDecorationColor|columnRuleColor"
)
TSX_GRID_PROPERTY = r"gridTemplateColumns|gridTemplate|grid"
TSX_STYLE_DECLARATION = re.compile(
    r"\b(?P<property>" + TSX_COLOR_PROPERTY + "|" + TSX_GRID_PROPERTY + r"|columnCount|columns)\s*:\s*"
)
TSX_GRID_PROPERTIES = {"gridTemplateColumns", "gridTemplate", "grid"}
TSX_FIXED_REPEAT = re.compile(r"repeat\(\s*(?:\d+|\$\{[^}]*\})")


def scan(root, exceptions):
    """root（リポジトリのルート）の frontend/web を走査し、判定の結果を返す。

    exceptions は既存の例外（EXISTING_EXCEPTIONS と同じ形の組の一覧）で、完全に一致する
    宣言だけを通し、見つからない項目は missing_exceptions として返す。
    """
    web = Path(root) / WEB_DIR
    css_files, component_files = collect_files(web)
    violations = []
    matched = set()
    for relative in css_files:
        for declaration in css_declarations((web / relative).read_text(encoding="utf-8")):
            key = (relative, declaration["context"], declaration["property"], declaration["value"])
            kinds = css_violation_kinds(declaration)
            if kinds and key in exceptions:
                matched.add(key)
                continue
            violations.extend(format_css_violation(relative, declaration, kind) for kind in kinds)
    for relative in component_files:
        violations.extend(component_violations(relative, (web / relative).read_text(encoding="utf-8")))
    missing = [
        f"例外一覧の宣言が見つかりません: {path} {format_context(context)} {{ {prop}: {value} }}"
        for path, context, prop, value in exceptions
        if (path, context, prop, value) not in matched
    ]
    return {
        "css_files": css_files,
        "component_files": component_files,
        "violations": violations,
        "missing_exceptions": missing,
    }


def collect_files(web):
    css_files = []
    component_files = []
    for directory, dir_names, file_names in os.walk(web):
        base = Path(directory).relative_to(web)
        dir_names[:] = [
            name
            for name in dir_names
            if name not in EXCLUDED_DIR_NAMES and (base / name).as_posix() not in EXCLUDED_DIR_PATHS
        ]
        for name in file_names:
            relative = (base / name).as_posix()
            if name.endswith(".css"):
                css_files.append(relative)
            elif name.endswith(".tsx") and not name.endswith(".test.tsx"):
                component_files.append(relative)
    return sorted(css_files), sorted(component_files)


def strip_keeping_lines(pattern, text):
    return pattern.sub(lambda match: "\n" * match.group(0).count("\n"), text)


def css_declarations(text):
    """CSS の宣言を、文脈（入れ子の規則の前置き）と行番号つきで並べる。"""
    text = strip_keeping_lines(CSS_COMMENT, text)
    declarations = []
    contexts = []
    buffer = []
    start_line = None
    line = 1
    quote = None
    for char in text:
        if quote:
            buffer.append(char)
            if char == quote:
                quote = None
        elif char in "\"'":
            quote = char
            buffer.append(char)
        elif char == "{":
            contexts.append(" ".join("".join(buffer).split()))
            buffer, start_line = [], None
        elif char in ";}":
            declaration = parse_declaration("".join(buffer), tuple(contexts), start_line)
            if declaration:
                declarations.append(declaration)
            buffer, start_line = [], None
            if char == "}" and contexts:
                contexts.pop()
        else:
            if start_line is None and not char.isspace():
                start_line = line
            buffer.append(char)
        if char == "\n":
            line += 1
    return declarations


def parse_declaration(raw, context, line):
    text = raw.strip()
    if not text or text.startswith("@") or ":" not in text:
        return None
    prop, value = text.split(":", 1)
    return {
        "context": context,
        "property": prop.strip().lower(),
        "value": " ".join(value.split()),
        "line": line,
    }


def css_violation_kinds(declaration):
    prop = declaration["property"]
    value = declaration["value"]
    kinds = []
    is_root_variable = prop.startswith("--") and declaration["context"][-1:] == (":root",)
    if not is_root_variable and has_color(CSS_STRING_OR_URL.sub("", value)):
        kinds.append(COLOR)
    if (prop in CSS_GRID_PROPERTIES and CSS_FIXED_REPEAT.search(value)) or (
        prop in CSS_COLUMN_PROPERTIES and LEADING_INTEGER.match(value)
    ):
        kinds.append(COLUMNS)
    return kinds


def has_color(value):
    return bool(HEX_COLOR.search(value) or COLOR_FUNCTION.search(value) or COLOR_NAME.search(value))


def format_context(context):
    return " > ".join(context)


def format_css_violation(relative, declaration, kind):
    return (
        f"{relative}:{declaration['line']} {format_context(declaration['context'])} "
        f"{{ {declaration['property']}: {declaration['value']} }} [{kind}]"
    )


def component_violations(relative, text):
    text = strip_keeping_lines(TSX_BLOCK_COMMENT, text)
    text = TSX_LINE_COMMENT.sub("", text)
    return [f"{relative}:{number} {found}" for number, found in component_findings(text)]


def component_findings(text):
    """コンポーネントの違反を (行番号, 文言) で返す。

    16 進数の色は文字列リテラルから検出し、色の関数・色の名前・列数の固定は style の
    宣言の値だけから検出する。宣言は改行をまたいで読み、行番号は違反の文字列がある行を
    指す。JSX の本文や、文字列のメソッド呼び出しのような style 以外の式は調べない。
    """
    findings = []
    for literal in TSX_STRING.finditer(text):
        for match in HEX_COLOR.finditer(literal.group(0)):
            findings.append((literal.start() + match.start(), f"{match.group(0)} [{COLOR}]"))
    for declaration in TSX_STYLE_DECLARATION.finditer(text):
        findings.extend(style_declaration_findings(text, declaration))
    findings.sort(key=lambda finding: finding[0])
    return [(line_number_at(text, offset), found) for offset, found in findings]


def line_number_at(text, offset):
    return text.count("\n", 0, offset) + 1


def style_declaration_findings(text, declaration):
    property_name = declaration.group("property")
    value, is_string, value_offset = read_style_value(text, declaration.end())
    if property_name in TSX_GRID_PROPERTIES:
        return [
            (value_offset + match.start(), f"{match.group(0).strip()} [{COLUMNS}]")
            for match in TSX_FIXED_REPEAT.finditer(value)
        ]
    if property_name == "columnCount":
        return [(declaration.start(), f"columnCount: {value.strip()} [{COLUMNS}]")]
    if property_name == "columns":
        if LEADING_INTEGER.match(value.strip().lstrip("\"'`").strip()):
            return [(declaration.start(), f"columns: {value.strip()} [{COLUMNS}]")]
        return []
    findings = []
    for match in COLOR_FUNCTION.finditer(value):
        call = re.match(r"[^)]*\)?", value[match.start():]).group(0)
        findings.append((value_offset + match.start(), f"{call} [{COLOR}]"))
    if is_string:
        findings.extend(
            (value_offset + match.start(), f"{match.group(0)} [{COLOR}]")
            for match in COLOR_NAME.finditer(value)
        )
    return findings


def read_style_value(text, start):
    """style の宣言の値を読み、(値, 文字列リテラルかどうか, 値の開始位置) を返す。

    文字列の値は引用符の中身を返し、開始位置は開き引用符の次の文字にする。それ以外の
    値は、括弧の深さが 0 の位置にあるカンマ・閉じ括弧・改行までを読む。改行をまたぐ
    宣言でも、キーと値を組として読める。
    """
    if start < len(text) and text[start] in "\"'`":
        quote = text[start]
        end = start + 1
        while end < len(text):
            if text[end] == "\\":
                end += 2
                continue
            if text[end] == quote or (quote != "`" and text[end] == "\n"):
                break
            end += 1
        return text[start + 1 : end], True, start + 1
    depth = 0
    end = start
    while end < len(text):
        char = text[end]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            if depth == 0:
                break
            depth -= 1
        elif depth == 0 and char in ",\n":
            break
        end += 1
    return text[start:end], False, start


def check_frontend_style(root, error_type, exceptions=EXISTING_EXCEPTIONS):
    """frontend-style 検査の本体。違反があれば error_type（呼び出し側の GateError）を raise する。"""
    result = scan(root, exceptions)
    if not result["css_files"]:
        raise error_type(f"{WEB_DIR} に検査する CSS がありません")
    problems = result["violations"] + result["missing_exceptions"]
    if problems:
        raise error_type(
            "色の直書きまたは列数の固定があります（規則は docs/layout.md の「機械の検査」）: "
            + "; ".join(problems)
        )
    return {
        "css_files": result["css_files"],
        "component_files": len(result["component_files"]),
        "exceptions": len(exceptions),
    }
