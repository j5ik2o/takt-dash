.PHONY: fmt clippy test typecheck lint proto-gen proto-check coverage done e2e e2e-build e2e-prod e2e-screenshots e2e-skip-only-check prod-check workflow-inspect takt-check

BASE_URL ?= http://127.0.0.1:3000

# workflow-inspect と takt-check は任意の検査。CI と make done には入れない（CI に takt が無いため）。

# 共有 TAKT（フォーク）の takt と、確かめのスクリプトを取り出すフォークのチェックアウト。
TAKT_BIN ?= $(HOME)/local/takt-custom/bin/takt
TAKT_REPO ?= $(HOME)/Sources/github.com/j5ik2o/takt

# coverage.py と PyYAML を、システム Python を汚さずに用意する。CI の done ジョブは
# PyYAML しか導入しないため、通常の make done でも coverage 判定を実行できるよう、
# target 配下の仮想環境を使う（PEP668 の強制回避やシステムへの導入はしない）。
COVERAGE_VENV ?= target/coverage/venv
COVERAGE_PY := $(COVERAGE_VENV)/bin/python

fmt:
	cargo +nightly fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace
	npm --prefix frontend/web run test
	python3 tools/repo-gate/repo_gate_test.py
	python3 tools/test-summary/test_summary_test.py
	python3 tools/coverage/coverage_threshold_test.py
	python3 tools/coverage/coverage_wiring_test.py
	python3 .takt/scripts/notify_orca_test.py
	python3 tools/project-status/task_context_test.py
	python3 tools/prod-check/prod_log_test.py

typecheck:
	npm --prefix frontend/web run typecheck
	npm --prefix frontend/web run typecheck:test

lint:
	npm --prefix frontend/web run lint

# run の前に、呼んだシェルの PATH の takt が task_finish_hook を知る共有 TAKT かを確かめる。
# NG なら 0 以外で終わる。README「run の前に TAKT の版を確かめる」。
takt-check:
	TAKT_REPO="$(TAKT_REPO)" sh scripts/takt-check.sh

# 任意。解決できない workflow_call を takt workflow inspect に名指しさせる（doctor は同じ
# 状態でも Workflow OK と言う）。使う takt は TAKT_BIN（既定は共有 TAKT）。先に、その takt で
# takt-check を流し、NG なら inspect しない。名前を渡すと 1 件だけ見る:
# make workflow-inspect WORKFLOW=taktdash-change
workflow-inspect:
	PATH="$(dir $(TAKT_BIN)):$$PATH" TAKT_REPO="$(TAKT_REPO)" sh scripts/takt-check.sh
	TAKT_BIN="$(TAKT_BIN)" python3 tools/workflow-inspect/workflow_inspect.py $(WORKFLOW)

proto-gen:
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	rm -rf frontend/web/src/proto
	mkdir -p frontend/web/src/proto
	npm --prefix frontend/web run proto:generate

proto-check:
	@set -e; \
	tmp="$$(mktemp -d)"; \
	trap 'rm -rf "$${tmp}"' EXIT; \
	mkdir -p "$${tmp}/proto"; \
	if [ -d frontend/web/src/proto ]; then cp -R frontend/web/src/proto/. "$${tmp}/proto/"; fi; \
	$(MAKE) proto-gen; \
	diff -r "$${tmp}/proto" frontend/web/src/proto

coverage:
	@echo "== backend (cargo-llvm-cov) =="
	@cargo llvm-cov --version > /dev/null 2>&1 || cargo install cargo-llvm-cov --locked
	rustup component add llvm-tools-preview
	cargo llvm-cov -p takt-dash-backend --fail-under-lines 80 --ignore-filename-regex '(^|/)(target|tests)/|(^|/)tests\.rs$$|_tests\.rs$$|/\.cargo/registry/|/rustc/'
	@echo "== frontend/cli (cargo-llvm-cov) =="
	cargo llvm-cov -p takt-dash-cli --fail-under-lines 80 --ignore-filename-regex '(^|/)(target|tests)/|(^|/)tests\.rs$$|_tests\.rs$$|/\.cargo/registry/|/rustc/'
	@echo "== frontend/web (c8) =="
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	cd frontend/web && npx --yes c8@12.0.0 --all --src . \
		--include 'app/**/*.{ts,tsx}' --include 'src/**/*.{ts,tsx}' \
		--exclude 'src/proto/**' --exclude '**/*.test.ts' --exclude 'test/**' --exclude '**/*.d.ts' \
		--reporter text --check-coverage --lines 80 --extension .ts --extension .tsx npm run test
	@echo "== tools/repo-gate (coverage.py) =="
	mkdir -p target/coverage
	"$(COVERAGE_PY)" -m pip --version > /dev/null 2>&1 || python3 -m venv "$(COVERAGE_VENV)"
	"$(COVERAGE_PY)" -m pip install --quiet --upgrade pip
	"$(COVERAGE_PY)" -m pip install --quiet coverage pyyaml
	"$(COVERAGE_PY)" -m coverage run --data-file=target/coverage/.coverage --include='tools/repo-gate/repo_gate.py,tools/repo-gate/gate_*.py' tools/repo-gate/repo_gate_test.py
	"$(COVERAGE_PY)" -m coverage json --data-file=target/coverage/.coverage -o target/coverage/repo-gate.json
	"$(COVERAGE_PY)" tools/coverage/coverage_threshold.py --json target/coverage/repo-gate.json --layer repo-gate

done:
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	$(MAKE) proto-check
	$(MAKE) fmt
	$(MAKE) clippy
	$(MAKE) test
	$(MAKE) typecheck
	$(MAKE) lint
	$(MAKE) coverage

e2e-skip-only-check:
	! grep -RInE 'test\.(skip|only)([^A-Za-z0-9_]|$$)' e2e/tests e2e/smoke

# startStack（e2e/support/stack.ts）で backend と web を起動する Playwright の実行に共通するビルドの前提。
e2e-build:
	test -d e2e/node_modules || npm --prefix e2e ci
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	cargo build -p takt-dash-backend
	npm --prefix frontend/web run build

e2e: e2e-build
	$(MAKE) e2e-skip-only-check
	npm --prefix e2e test

# 撮影の手順（docs/layout.md の「撮影」）。make e2e とは別の入口で、e2e-build でビルドしてから撮る。
e2e-screenshots: e2e-build
	npm --prefix e2e run screenshots

e2e-prod:
	test -d e2e/node_modules || npm --prefix e2e ci
	$(MAKE) e2e-skip-only-check
	npm --prefix e2e run test:prod

# 終了コードで判定するジョブは make を経由しないこと。GNU make はレシピの
# 失敗を一律に 2 へ変えるため、prod_check.py の 1（不合格）と 2（確認
# できず）を区別できない。ジョブからは python3 tools/prod-check/prod_check.py
# を直接実行する。
prod-check:
	python3 tools/prod-check/prod_check.py --base-url $(BASE_URL) $(if $(WITH_E2E),--with-e2e,)
