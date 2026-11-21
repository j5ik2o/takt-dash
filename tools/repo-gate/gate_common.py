"""repo_gate の各検査が共有する補助（例外、外部コマンドの実行、宣言ファイルの読み込み）。"""
import json
import os
import subprocess
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.11 未満は対象外
    tomllib = None

GATE_TARGETS = ["fmt", "clippy", "test", "typecheck", "lint", "proto_check"]
# machine_gate の system_input の as 名と、Makefile のターゲット名の対応。
# proto-check は Makefile のターゲット名にハイフンを含むため、式から参照しやすい
# as 名（proto_check）と分けて対応づける。
GATE_MAKE_TARGETS = {"proto_check": "proto-check"}


def gate_make_target(name):
    return GATE_MAKE_TARGETS.get(name, name)


class GateError(Exception):
    """検査を実行できない理由、または満たせない条件を表す。"""


def run_command(command, cwd, timeout_seconds):
    try:
        proc = subprocess.run(
            command,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=timeout_seconds,
            env=_child_environment(),
            check=False,
        )
    except FileNotFoundError as error:
        return {"status": None, "stdout": "", "stderr": f"{command[0]} が見つかりません: {error}"}
    except subprocess.TimeoutExpired:
        return {
            "status": None,
            "stdout": "",
            "stderr": f"{' '.join(command)} が {timeout_seconds} 秒以内に終わりませんでした",
        }
    return {"status": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}


def _child_environment():
    env = dict(os.environ)
    # TAKT の command ステップは HOME を渡さない。HOME が無いと cargo と npm が
    # キャッシュや設定を読めないため、ログインユーザーのホームを補う。
    if "HOME" not in env:
        env["HOME"] = str(Path.home())
    return env


def read_text(path):
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError as error:
        raise GateError(f"{path} がありません: {error}")


def read_toml(path):
    if tomllib is None:
        raise GateError("tomllib を読み込めません。Python 3.11 以上で実行してください")
    try:
        return tomllib.loads(read_text(path))
    except tomllib.TOMLDecodeError as error:
        raise GateError(f"{path} を TOML として読めません: {error}")


def read_json(path):
    try:
        return json.loads(read_text(path))
    except ValueError as error:
        raise GateError(f"{path} を JSON として読めません: {error}")


def read_workflow(path):
    """ワークフローの YAML を読み込む。PyYAML が必要である。"""
    try:
        import yaml
    except ModuleNotFoundError as error:
        raise GateError(f"PyYAML を読み込めません: {error}")
    try:
        return yaml.safe_load(read_text(path))
    except yaml.YAMLError as error:
        raise GateError(f"{path} を YAML として読めません: {error}")
