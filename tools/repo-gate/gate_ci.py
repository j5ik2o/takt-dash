""".github/workflows/ の CI が make done と aislop のゲートを呼び、ツールチェーンを用意するかを検査する。"""
import re

from gate_aislop import AISLOP_CHECK_COMMAND, AISLOP_FAILURE_NOTE, invokes_aislop_check
from gate_common import GateError, read_workflow


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
        has_stable = False
        has_nightly_rustfmt = False
        has_node = False
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
                options = step.get("with") or {}
                if uses == "dtolnay/rust-toolchain@stable":
                    has_stable = True
                if uses == "dtolnay/rust-toolchain@nightly":
                    components = re.split(r"[\s,]+", str(options.get("components", "")))
                    has_nightly_rustfmt |= "rustfmt" in components
                if uses.startswith("actions/setup-node@"):
                    version = str(options.get("node-version", "")).strip()
                    has_node |= re.fullmatch(r"[0-9.x]+", version) is not None
                run = step.get("run")
                if not isinstance(run, str):
                    continue
                continue_on_error = step.get("continue-on-error", False)
                if continue_on_error is not False and (
                    not isinstance(continue_on_error, str)
                    or re.fullmatch(r"\s*\$\{\{\s*false\s*\}\}\s*", continue_on_error) is None
                ):
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
        problems = []
        if not has_stable:
            problems.append("Rust の stable をセットアップしていません")
        if not has_nightly_rustfmt:
            problems.append("Rust の nightly と rustfmt をセットアップしていません")
        if not has_node:
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
