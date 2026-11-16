""".github/workflows/ の CI が make done と aislop のゲートを呼び、ツールチェーンを用意するかを検査する。"""
import re

from gate_aislop import AISLOP_CHECK_COMMAND, AISLOP_FAILURE_NOTE, invokes_aislop_check
from gate_common import GateError, read_text, read_workflow


def check_ci(root):
    workflows_dir = root / ".github/workflows"
    files = sorted(
        path
        for path in workflows_dir.glob("*")
        if path.suffix in (".yml", ".yaml") and path.is_file()
    )
    if not files:
        raise GateError(".github/workflows/ に CI の定義がありません")

    found = []
    for path in files:
        workflow = read_workflow(path)
        ordered_runs = []
        # チェックアウト順の不変条件はジョブ単位で判定する。ファイル全体で 1 回だけ
        # 初期化すると、先のジョブの checkout が後続ジョブへ累積し、aislop を含む
        # ジョブ自身に checkout が無くても合格してしまう。
        aislop_misordered_jobs = []
        aislop_ordered_jobs = []
        for job_id, job in (workflow.get("jobs") or {}).items():
            checkout_seen = False
            job_calls_aislop = False
            job_aislop_ordered = False
            for step in job.get("steps") or []:
                if not isinstance(step, dict):
                    continue
                uses = str(step.get("uses") or "")
                if uses.startswith("actions/checkout"):
                    checkout_seen = True
                run = step.get("run")
                if not isinstance(run, str):
                    continue
                ordered_runs.append(run)
                if not invokes_aislop_check(run):
                    continue
                job_calls_aislop = True
                if checkout_seen:
                    job_aislop_ordered = True
                else:
                    aislop_misordered_jobs.append(job_id)
            if job_calls_aislop:
                aislop_ordered_jobs.append(job_aislop_ordered)
        make_done = [
            line
            for run in ordered_runs
            for line in run.splitlines()
            if [token for token in line.strip().split() if token][:2] == ["make", "done"]
        ]
        if not make_done:
            continue
        aislop_called = bool(aislop_ordered_jobs)
        text = read_text(path)
        toolchains = re.findall(r"dtolnay/rust-toolchain@(stable|nightly)", text)
        components = re.findall(r"components:\s*([^\n]+)", text)
        flattened = " ".join(components)
        node_versions = re.findall(r"node-version:\s*['\"]?([0-9.x]+)", text)
        has_node_action = "actions/setup-node" in text
        problems = []
        if "stable" not in toolchains:
            problems.append("Rust の stable をセットアップしていません")
        if "nightly" not in toolchains or "rustfmt" not in flattened:
            problems.append("Rust の nightly と rustfmt をセットアップしていません")
        if not has_node_action or not node_versions:
            problems.append("Node をセットアップしていません")
        if not aislop_called:
            problems.append(
                f"aislop のゲート（node {AISLOP_CHECK_COMMAND}）を呼んでいません"
                f"（{AISLOP_FAILURE_NOTE}）"
            )
        elif aislop_misordered_jobs:
            problems.append("aislop のゲートをチェックアウトより前に呼んでいます")
        found.append({"file": str(path.relative_to(root)), "problems": problems})

    if not found:
        raise GateError(".github/workflows/ の CI が make done を呼んでいません")
    problems = [problem for item in found for problem in item["problems"]]
    if problems:
        raise GateError("; ".join(problems))
    return {"workflows": found}
