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
  local entry=$1 dir spec_file test_file spec_path test_path spec_work
  local spec_ir test_ir out mod name passing max
  local run_main run_inv run_steps run_samples
  local spec_runs expected_mods actual_mods test_mods_raw mod_runs_raw runs_raw ok=0
  local runs=() bad=()

  if ! dir=$(jq -r '.dir' <<<"$entry") \
    || ! spec_file=$(jq -r '.spec' <<<"$entry") \
    || ! test_file=$(jq -r '.test' <<<"$entry"); then
    fail "$manifest" "failed to parse spec entry from manifest"
    return 1
  fi
  spec_path="specs/quint/$dir/$spec_file"
  test_path="specs/quint/$dir/$test_file"
  if ! spec_work=$(mktemp -d "$work/spec.XXXXXX"); then
    fail "$spec_path" "failed to create temp directory"
    return 1
  fi
  spec_ir="$spec_work/spec.ir.json"
  test_ir="$spec_work/test.ir.json"
  out="$spec_work/out"

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
  if ! spec_runs=$(jq -r '[.modules[].declarations[] | select(.qualifier == "run") | .name] | join(", ")' "$spec_ir"); then
    fail "$spec_path" "failed to inspect parsed IR"
    return 1
  fi
  if [ -n "$spec_runs" ]; then
    fail "$spec_path" "model file defines run declaration(s) ($spec_runs); move tests to $test_path"
    return 1
  fi

  if ! "$quint" parse --out="$test_ir" "$test_path" >/dev/null; then
    fail "$test_path" "quint parse failed"
    return 1
  fi

  if ! expected_mods=$(jq -r '.test_modules | sort | join(",")' <<<"$entry"); then
    fail "$test_path" "failed to read test_modules from manifest"
    return 1
  fi
  if ! actual_mods=$(jq -r '[.modules[] | select(([.declarations[] | select(.qualifier == "run")] | length) > 0) | .name] | sort | join(",")' "$test_ir"); then
    fail "$test_path" "failed to inspect test IR modules"
    return 1
  fi
  if [ "$actual_mods" != "$expected_mods" ]; then
    fail "$test_path" "modules with run declarations (${actual_mods:-none}) do not match ci-specs.json test_modules ($expected_mods)"
    return 1
  fi

  if ! test_mods_raw=$(jq -r '.test_modules[]' <<<"$entry") || [ -z "$test_mods_raw" ]; then
    fail "$test_path" "failed to read test_modules from manifest"
    return 1
  fi
  while IFS= read -r mod; do
    [ -n "$mod" ] || continue
    runs=()
    bad=()
    if ! mod_runs_raw=$(jq -r --arg m "$mod" '.modules[] | select(.name == $m) | .declarations[] | select(.qualifier == "run") | .name' "$test_ir"); then
      fail "$test_path" "failed to inspect run declarations in module $mod"
      ok=1
      continue
    fi
    if [ -n "$mod_runs_raw" ]; then
      while IFS= read -r name; do
        [ -n "$name" ] || continue
        runs+=("$name")
        case $name in
          *Test) ;;
          *) bad+=("$name") ;;
        esac
      done <<<"$mod_runs_raw"
    fi

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
  done <<<"$test_mods_raw"

  if ! runs_raw=$(jq -c '.runs[]' <<<"$entry") || [ -z "$runs_raw" ]; then
    fail "$spec_path" "failed to read runs from manifest"
    return 1
  fi
  while IFS= read -r run_cfg; do
    [ -n "$run_cfg" ] || continue
    if ! run_main=$(jq -r '.main' <<<"$run_cfg") \
      || ! run_inv=$(jq -r '.invariant' <<<"$run_cfg") \
      || ! run_steps=$(jq -r '.max_steps' <<<"$run_cfg") \
      || ! run_samples=$(jq -r '.max_samples' <<<"$run_cfg"); then
      fail "$spec_path" "failed to parse run configuration from manifest"
      ok=1
      continue
    fi

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
  done <<<"$runs_raw"

  return "$ok"
}

if ! specs_raw=$(jq -c '.specs[]' "$manifest") || [ -z "$specs_raw" ]; then
  echo "::error file=$manifest::failed to read specs from manifest"
  exit 1
fi

failed=()
checked=0
while IFS= read -r entry; do
  [ -n "$entry" ] || continue
  if ! dir=$(jq -r '.dir' <<<"$entry"); then
    echo "::error file=$manifest::failed to read spec dir from manifest entry"
    failed+=("$manifest")
    continue
  fi
  checked=$((checked + 1))
  echo "::group::specs/quint/$dir"
  if ! check_spec "$entry"; then
    failed+=("specs/quint/$dir")
  fi
  echo '::endgroup::'
done <<<"$specs_raw"

if [ "$checked" -eq 0 ]; then
  echo "::error file=$manifest::no Quint specs were checked"
  exit 1
fi

if [ "${#failed[@]}" -gt 0 ]; then
  for spec_dir in "${failed[@]}"; do
    echo "::error file=$spec_dir::quint check failed (see the group for $spec_dir above)"
  done
  exit 1
fi
