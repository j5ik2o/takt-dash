"""ワークスペースの構成、.gitignore、Rust のツールチェーンを検査する。"""
import json
from pathlib import Path

from gate_common import GateError, read_json, read_text, read_toml

WORKSPACE_MEMBERS = ["backend", "frontend/cli"]


def check_workspace_layout(root, run):
    cargo_toml = root / "Cargo.toml"
    manifest = read_toml(cargo_toml)
    workspace = manifest.get("workspace")
    if not isinstance(workspace, dict):
        raise GateError("Cargo.toml に [workspace] がありません")
    members = workspace.get("members")
    if not isinstance(members, list):
        raise GateError("Cargo.toml の [workspace] に members がありません")
    if sorted(members) != sorted(WORKSPACE_MEMBERS):
        raise GateError(
            f"Cargo.toml の members が {WORKSPACE_MEMBERS} ではありません: {members!r}"
        )

    metadata = run(["cargo", "metadata", "--no-deps", "--format-version", "1"], root, 60)
    if metadata["status"] != 0:
        raise GateError(f"cargo metadata が失敗しました: {metadata['stderr'].strip()}")
    try:
        payload = json.loads(metadata["stdout"])
    except ValueError as error:
        raise GateError(f"cargo metadata の出力が JSON ではありません: {error}")
    member_ids = set(payload["workspace_members"])
    kinds = {}
    paths = {}
    for package in payload["packages"]:
        if package["id"] not in member_ids:
            continue
        target_kinds = []
        for target in package["targets"]:
            target_kinds.extend(target["kind"])
        kinds[Path(package["manifest_path"]).parent.resolve().relative_to(root).as_posix()] = sorted(
            set(target_kinds)
        )
        paths[package["name"]] = package["manifest_path"]
    for member in WORKSPACE_MEMBERS:
        if member not in kinds:
            raise GateError(f"ワークスペースのメンバー {member} が cargo metadata にありません")
    if "lib" not in kinds["backend"]:
        raise GateError(f"backend が lib を持ちません: {kinds['backend']!r}")
    if "bin" not in kinds["frontend/cli"]:
        raise GateError(f"frontend/cli が bin を持ちません: {kinds['frontend/cli']!r}")

    for member in ("proto", "frontend/web"):
        if member in kinds:
            raise GateError(f"{member} がワークスペースのメンバーに含まれています")

    web_manifest = read_json(root / "frontend/web/package.json")
    dependencies = {
        **(web_manifest.get("dependencies") or {}),
        **(web_manifest.get("devDependencies") or {}),
    }
    if "next" not in dependencies:
        raise GateError("frontend/web の package.json に next への依存がありません")
    if "typescript" not in dependencies:
        raise GateError("frontend/web の package.json に typescript への依存がありません")

    proto_dir = root / "proto"
    if not proto_dir.is_dir():
        raise GateError("proto/ がありません")

    web_scripts = web_manifest.get("scripts") or {}
    for script in ("typecheck", "lint", "test"):
        if not isinstance(web_scripts.get(script), str) or not web_scripts[script].strip():
            raise GateError(f"frontend/web の package.json に {script} の script がありません")

    return {
        "members": members,
        "kinds": kinds,
        "packages": paths,
        "scripts": sorted(web_scripts),
    }


def check_gitignore(root):
    lines = [line.strip() for line in read_text(root / ".gitignore").splitlines()]
    present = {line.rstrip("/") for line in lines if line and not line.startswith("#")}
    required = ["/target", "node_modules", ".next", "next-env.d.ts", "*.sqlite"]
    missing = [pattern for pattern in required if pattern not in present]
    if missing:
        raise GateError(f".gitignore に生成物を除外する行がありません: {missing}")
    return {"required": required}


def check_toolchain(root):
    manifest = read_toml(root / "rust-toolchain.toml")
    channel = (manifest.get("toolchain") or {}).get("channel")
    if channel != "stable":
        raise GateError(f"rust-toolchain.toml の channel が stable ではありません: {channel!r}")
    return {"channel": channel}
