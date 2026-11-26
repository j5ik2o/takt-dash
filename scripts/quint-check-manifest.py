#!/usr/bin/env python3
"""specs/quint/ci-specs.json と specs/quint/ 配下の .qnt / README.md の整合を検証する。

- specs/quint/ 配下のすべての .qnt ファイルが ci-specs.json と 1 対 1 で対応すること
- ci-specs.json のスキーマと値が妥当であること
- 各仕様の README.md に書かれた quint test / quint run コマンドと一致すること
"""

from __future__ import annotations

import argparse
import json
import pathlib
import shlex
import sys
from typing import Any


def emit_error(message: str, file_path: str | None = None) -> None:
    if file_path:
        print(f"::error file={file_path}::{message}", file=sys.stderr)
    else:
        print(f"::error::{message}", file=sys.stderr)


def is_positive_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def is_safe_segment(name: Any) -> bool:
    return (
        isinstance(name, str)
        and bool(name)
        and "/" not in name
        and "\\" not in name
        and name not in (".", "..")
    )


def extract_fenced_commands(readme_text: str) -> list[str]:
    """Markdown のフェンスコードブロックから、バックスラッシュ継続を結合したコマンド行を返す。"""
    commands: list[str] = []
    in_fence = False
    fence_marker = ""
    pending = ""

    for raw_line in readme_text.splitlines():
        stripped = raw_line.strip()
        if not in_fence:
            if stripped.startswith("```") or stripped.startswith("~~~"):
                in_fence = True
                fence_marker = stripped[:3]
                pending = ""
            continue

        if stripped.startswith(fence_marker):
            if pending.strip():
                commands.append(pending.strip())
            in_fence = False
            fence_marker = ""
            pending = ""
            continue

        line = raw_line.rstrip()
        if line.endswith("\\"):
            pending += line[:-1] + " "
        else:
            pending += line
            if pending.strip():
                commands.append(pending.strip())
            pending = ""

    if pending.strip():
        commands.append(pending.strip())
    return commands


def parse_readme_commands(
    readme_text: str,
) -> tuple[set[tuple[str, str]], set[tuple[str, str, str, int, int]]]:
    """README.md から (test_file, main) と (spec_file, main, invariant, max_steps, max_samples) を抽出する。"""
    test_cmds: set[tuple[str, str]] = set()
    run_cmds: set[tuple[str, str, str, int, int]] = set()

    for line in extract_fenced_commands(readme_text):
        try:
            tokens = shlex.split(line, comments=True)
        except ValueError:
            continue
        if len(tokens) < 2 or tokens[0] != "quint":
            continue

        subcmd = tokens[1]
        rest = tokens[2:]
        if subcmd == "test":
            parsed = _parse_quint_test(rest)
            if parsed is not None:
                test_cmds.add(parsed)
        elif subcmd == "run":
            parsed = _parse_quint_run(rest)
            if parsed is not None:
                run_cmds.add(parsed)

    return test_cmds, run_cmds


def _read_flag_value(tokens: list[str], index: int, flag: str) -> tuple[str | None, int]:
    tok = tokens[index]
    if tok == flag and index + 1 < len(tokens):
        return tokens[index + 1], index + 2
    prefix = f"{flag}="
    if tok.startswith(prefix):
        return tok[len(prefix) :], index + 1
    return None, index


def _skip_unknown_flag(tokens: list[str], index: int) -> int:
    tok = tokens[index]
    if (
        "=" not in tok
        and index + 1 < len(tokens)
        and not tokens[index + 1].startswith("-")
        and not tokens[index + 1].endswith(".qnt")
    ):
        return index + 2
    return index + 1


def _parse_quint_test(tokens: list[str]) -> tuple[str, str] | None:
    target: str | None = None
    main: str | None = None
    i = 0
    while i < len(tokens):
        val, next_i = _read_flag_value(tokens, i, "--main")
        if val is not None:
            main, i = val, next_i
        elif tokens[i].startswith("-"):
            i = _skip_unknown_flag(tokens, i)
        else:
            if target is None and tokens[i].endswith(".qnt"):
                target = pathlib.PurePosixPath(tokens[i]).name
            i += 1
    return (target, main) if target and main else None


def _parse_quint_run(tokens: list[str]) -> tuple[str, str, str, int, int] | None:
    target: str | None = None
    opts: dict[str, str] = {}
    i = 0
    while i < len(tokens):
        tok = tokens[i]
        if tok == "--witnesses":
            i += 1
            while i < len(tokens) and not tokens[i].startswith("-") and not tokens[i].endswith(".qnt"):
                i += 1
            continue
        matched = False
        for flag in ("--main", "--invariant", "--max-steps", "--max-samples"):
            val, next_i = _read_flag_value(tokens, i, flag)
            if val is not None:
                opts[flag] = val
                i = next_i
                matched = True
                break
        if matched:
            continue
        if tok.startswith("-"):
            i = _skip_unknown_flag(tokens, i)
        else:
            if target is None and tok.endswith(".qnt"):
                target = pathlib.PurePosixPath(tok).name
            i += 1

    if not target or not all(k in opts for k in ("--main", "--invariant", "--max-steps", "--max-samples")):
        return None
    try:
        steps = int(opts["--max-steps"])
        samples = int(opts["--max-samples"])
    except ValueError:
        return None
    return (target, opts["--main"], opts["--invariant"], steps, samples)


def _validate_runs(
    spec_dir: str, runs: Any, manifest_rel: str
) -> tuple[list[tuple[str, str, int, int]], int]:
    if not isinstance(runs, list) or not runs:
        emit_error(f"{spec_dir}: runs must be a non-empty list", manifest_rel)
        return [], 1

    errors = 0
    seen_keys: set[tuple[str, str]] = set()
    valid: list[tuple[str, str, int, int]] = []
    for r_idx, run_cfg in enumerate(runs):
        if not isinstance(run_cfg, dict):
            emit_error(f"{spec_dir}: runs[{r_idx}] must be an object", manifest_rel)
            errors += 1
            continue
        main = run_cfg.get("main")
        invariant = run_cfg.get("invariant")
        max_steps = run_cfg.get("max_steps")
        max_samples = run_cfg.get("max_samples")
        if not (isinstance(main, str) and main.strip()):
            emit_error(f"{spec_dir}: runs[{r_idx}].main must be a non-empty string", manifest_rel)
            errors += 1
            continue
        if not (isinstance(invariant, str) and invariant.strip()):
            emit_error(f"{spec_dir}: runs[{r_idx}].invariant must be a non-empty string", manifest_rel)
            errors += 1
            continue
        if not is_positive_int(max_steps) or not is_positive_int(max_samples):
            emit_error(
                f"{spec_dir}: runs[{r_idx}] max_steps and max_samples must be positive integers",
                manifest_rel,
            )
            errors += 1
            continue
        key = (main, invariant)
        if key in seen_keys:
            emit_error(f"{spec_dir}: duplicate run entry for main={main}, invariant={invariant}", manifest_rel)
            errors += 1
            continue
        seen_keys.add(key)
        valid.append((main, invariant, max_steps, max_samples))
    return valid, errors


def _validate_readme(
    root: pathlib.Path,
    spec_dir: str,
    spec_file: str,
    test_file: str,
    test_modules: list[str],
    valid_runs: list[tuple[str, str, int, int]],
) -> int:
    readme_rel = f"specs/quint/{spec_dir}/README.md"
    readme_path = root / readme_rel
    if not readme_path.is_file():
        emit_error(f"{spec_dir}: missing README.md", readme_rel)
        return 1

    errors = 0
    readme_tests, readme_runs = parse_readme_commands(readme_path.read_text(encoding="utf-8"))
    for mod in test_modules:
        if (test_file, mod) not in readme_tests:
            emit_error(
                f"{spec_dir}: ci-specs.json test module '{mod}' for {test_file} "
                f"does not match any `quint test` command in {readme_rel}",
                readme_rel,
            )
            errors += 1

    for main, invariant, max_steps, max_samples in valid_runs:
        expected = (spec_file, main, invariant, max_steps, max_samples)
        if expected not in readme_runs:
            emit_error(
                f"{spec_dir}: ci-specs.json run (main={main}, invariant={invariant!r}, "
                f"max_steps={max_steps}, max_samples={max_samples}) "
                f"does not match any `quint run` command in {readme_rel}",
                readme_rel,
            )
            errors += 1
    return errors


def _validate_entry(
    root: pathlib.Path,
    idx: int,
    entry: Any,
    manifest_rel: str,
    seen_dirs: set[str],
    declared_files: set[str],
) -> int:
    if not isinstance(entry, dict):
        emit_error(f"specs[{idx}] must be an object", manifest_rel)
        return 1

    spec_dir = entry.get("dir")
    spec_file = entry.get("spec")
    test_file = entry.get("test")
    test_modules = entry.get("test_modules")

    if not is_safe_segment(spec_dir):
        emit_error(f"specs[{idx}].dir is invalid: {spec_dir!r}", manifest_rel)
        return 1
    if spec_dir in seen_dirs:
        emit_error(f"duplicate spec dir in manifest: {spec_dir}", manifest_rel)
        return 1
    seen_dirs.add(spec_dir)

    if not (is_safe_segment(spec_file) and spec_file.endswith(".qnt")):
        emit_error(f"{spec_dir}: invalid spec filename: {spec_file!r}", manifest_rel)
        return 1
    if not (is_safe_segment(test_file) and test_file.endswith("_test.qnt")):
        emit_error(f"{spec_dir}: invalid test filename: {test_file!r}", manifest_rel)
        return 1
    if spec_file == test_file:
        emit_error(f"{spec_dir}: spec and test must be distinct files", manifest_rel)
        return 1

    declared_files.add(f"specs/quint/{spec_dir}/{spec_file}")
    declared_files.add(f"specs/quint/{spec_dir}/{test_file}")

    if not isinstance(test_modules, list) or not test_modules or not all(
        isinstance(m, str) and m.strip() for m in test_modules
    ):
        emit_error(f"{spec_dir}: test_modules must be a non-empty list of strings", manifest_rel)
        return 1
    if len(set(test_modules)) != len(test_modules):
        emit_error(f"{spec_dir}: duplicate entries in test_modules", manifest_rel)
        return 1

    valid_runs, run_errors = _validate_runs(spec_dir, entry.get("runs"), manifest_rel)
    if run_errors:
        return run_errors
    return _validate_readme(root, spec_dir, spec_file, test_file, test_modules, valid_runs)


def validate(root: pathlib.Path, manifest_rel: str) -> int:
    manifest_path = root / manifest_rel
    specs_root = root / "specs" / "quint"

    if not specs_root.is_dir():
        emit_error("specs/quint directory does not exist")
        return 1
    if not manifest_path.is_file():
        emit_error(f"manifest not found: {manifest_rel}", manifest_rel)
        return 1

    try:
        data = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        emit_error(f"failed to parse manifest: {exc}", manifest_rel)
        return 1

    entries = data.get("specs") if isinstance(data, dict) else None
    if not isinstance(entries, list) or not entries:
        emit_error("manifest 'specs' must be a non-empty array", manifest_rel)
        return 1

    discovered = sorted(
        path.relative_to(root).as_posix()
        for path in specs_root.rglob("*.qnt")
        if path.is_file()
    )
    if not discovered:
        emit_error("No Quint spec matched specs/quint/**/*.qnt")
        return 1

    declared_files: set[str] = set()
    seen_dirs: set[str] = set()
    errors = sum(
        _validate_entry(root, idx, entry, manifest_rel, seen_dirs, declared_files)
        for idx, entry in enumerate(entries)
    )

    discovered_set = set(discovered)
    for missing in sorted(declared_files - discovered_set):
        emit_error(f"manifest references missing Quint file: {missing}", missing)
        errors += 1
    for extra in sorted(discovered_set - declared_files):
        emit_error(
            f"unregistered Quint file under specs/quint/: {extra}; "
            f"register it in {manifest_rel} with its README parameters",
            extra,
        )
        errors += 1

    return 1 if errors else 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate specs/quint/ci-specs.json against files and READMEs.")
    parser.add_argument("--root", default=".", help="Repository root directory")
    parser.add_argument("--manifest", default="specs/quint/ci-specs.json", help="Manifest path relative to root")
    args = parser.parse_args()
    return validate(pathlib.Path(args.root).resolve(), args.manifest)


if __name__ == "__main__":
    sys.exit(main())
