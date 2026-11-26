#!/usr/bin/env bash
# Checks every Quint specification under specs/quint/ using the quint pinned in
# package-lock.json: manifest/README consistency, typecheck, every `run` in
# *_test.qnt via quint test, and each spec's invariant via quint run.
# Run from the repository root. CI runs it in the `quint` job of ci.yml.
# Design: docs/design/quint-ci.md.
set -uo pipefail

quint=./node_modules/.bin/quint
manifest=specs/quint/ci-specs.json

if [ ! -x "$quint" ]; then
  echo "::error::$quint not found or not executable; run 'npm ci' first"
  exit 1
fi

for cmd in jq python3; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "::error::required tool '$cmd' is not installed"
    exit 1
  fi
done

if ! python3 scripts/quint-check-manifest.py --manifest "$manifest"; then
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
  echo "::error file=$1::$2"
  return 1
}

check_spec() {
  local entry=$1 dir spec_file test_file spec_path test_path
  local spec_ir test_ir out mod name passing max
  local run_main run_inv run_steps run_samples
  local spec_runs expected_mods actual_mods ok=0
  local runs=() bad=()

  dir=$(jq -r '.dir' <<<"$entry")
  spec_file=$(jq -r '.spec' <<<"$entry")
  test_file=$(jq -r '.test' <<<"$entry")
  spec_path="specs/quint/$dir/$spec_file"
  test_path="specs/quint/$dir/$test_file"
  spec_ir="$work/${dir}.spec.ir.json"
  test_ir="$work/${dir}.test.ir.json"
  out="$work/${dir}.out"

  if ! "$quint" typecheck "$spec_path"; then
    fail "$spec_path" "quint typecheck failed"
    return 1
  fi
  if ! "$quint" typecheck "$test_path"; then
    fail "$test_path" "quint typecheck failed"
    return 1
  fi

  if ! "$quint" parse --out="$spec_ir" "$spec_path" >/dev/null; then
    fail "$spec_path" "quint parse failed"
    return 1
  fi
  spec_runs=$(jq -r '[.modules[].declarations[] | select(.qualifier == "run") | .name] | join(", ")' "$spec_ir")
  if [ -n "$spec_runs" ]; then
    fail "$spec_path" "model file defines run declaration(s) ($spec_runs); move tests to $test_path"
    return 1
  fi

  if ! "$quint" parse --out="$test_ir" "$test_path" >/dev/null; then
    fail "$test_path" "quint parse failed"
    return 1
  fi

  expected_mods=$(jq -r '.test_modules | sort | join(",")' <<<"$entry")
  actual_mods=$(jq -r '[.modules[] | select(([.declarations[] | select(.qualifier == "run")] | length) > 0) | .name] | sort | join(",")' "$test_ir")
  if [ "$actual_mods" != "$expected_mods" ]; then
    fail "$test_path" "modules with run declarations (${actual_mods:-none}) do not match ci-specs.json test_modules ($expected_mods)"
    return 1
  fi

  while IFS= read -r mod; do
    runs=()
    bad=()
    while IFS= read -r name; do
      runs+=("$name")
      case $name in
        *Test) ;;
        *) bad+=("$name") ;;
      esac
    done < <(jq -r --arg m "$mod" '.modules[] | select(.name == $m) | .declarations[] | select(.qualifier == "run") | .name' "$test_ir")

    if [ "${#runs[@]}" -eq 0 ]; then
      fail "$test_path" "module $mod defines no run; every test module needs at least one"
      ok=1
      continue
    fi
    if [ "${#bad[@]}" -gt 0 ]; then
      fail "$test_path" "quint test skips these runs in module $mod because their names do not end in Test: ${bad[*]}"
      ok=1
      continue
    fi

    if ! "$quint" test --main="$mod" "$test_path" | tee "$out"; then
      fail "$test_path" "quint test failed for module $mod"
      ok=1
      continue
    fi
    passing=$(sed -nE 's/^ *([0-9]+) passing.*/\1/p' "$out")
    if [ "$passing" != "${#runs[@]}" ]; then
      fail "$test_path" "quint test passed ${passing:-0} test(s) in module $mod but IR defines ${#runs[@]} run(s)"
      ok=1
    fi
  done < <(jq -r '.test_modules[]' <<<"$entry")

  while IFS= read -r run_cfg; do
    run_main=$(jq -r '.main' <<<"$run_cfg")
    run_inv=$(jq -r '.invariant' <<<"$run_cfg")
    run_steps=$(jq -r '.max_steps' <<<"$run_cfg")
    run_samples=$(jq -r '.max_samples' <<<"$run_cfg")

    if ! "$quint" run \
      --main="$run_main" \
      --invariant="$run_inv" \
      --max-steps="$run_steps" \
      --max-samples="$run_samples" \
      --verbosity=2 \
      "$spec_path" | tee "$out"; then
      fail "$spec_path" "quint run failed (main=$run_main, invariant=$run_inv). If an unrelated PR hit a real counterexample under a random seed: reproduce with the --seed printed above, open a 種別:バグ issue with the counterexample to fix the spec or implementation, and update this PR after the fix lands on main."
      ok=1
      continue
    fi

    max=$(sed -nE 's/^Trace length statistics: max=([0-9]+),.*/\1/p' "$out")
    if [ -z "$max" ] || [[ ! "$max" =~ ^[0-9]+$ ]]; then
      fail "$spec_path" "quint run printed no valid trace length statistics (main=$run_main)"
      ok=1
      continue
    fi
    if [ "$max" -le 1 ]; then
      fail "$spec_path" "no trace took a step (main=$run_main, max trace length $max): step is never enabled"
      ok=1
    fi
  done < <(jq -c '.runs[]' <<<"$entry")

  return "$ok"
}

failed=()
while IFS= read -r entry; do
  dir=$(jq -r '.dir' <<<"$entry")
  echo "::group::specs/quint/$dir"
  if ! check_spec "$entry"; then
    failed+=("specs/quint/$dir")
  fi
  echo '::endgroup::'
done < <(jq -c '.specs[]' "$manifest")

if [ "${#failed[@]}" -gt 0 ]; then
  for spec_dir in "${failed[@]}"; do
    echo "::error file=$spec_dir::quint check failed (see the group for $spec_dir above)"
  done
  exit 1
fi
