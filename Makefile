.PHONY: fmt clippy test typecheck lint proto-gen proto-check coverage done e2e e2e-build e2e-prod e2e-screenshots e2e-skip-only-check prod-check

BASE_URL ?= http://127.0.0.1:3000

fmt:
	cargo +nightly fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace
	npm --prefix frontend/web run test
	python3 tools/repo-gate/repo_gate_test.py
	python3 tools/test-summary/test_summary_test.py

typecheck:
	npm --prefix frontend/web run typecheck
	npm --prefix frontend/web run typecheck:test

lint:
	npm --prefix frontend/web run lint

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
	cargo llvm-cov -p takt-dash-backend --ignore-filename-regex '(^|/)(target|tests)/|(^|/)tests\.rs$$|_tests\.rs$$|/\.cargo/registry/|/rustc/'
	@echo "== frontend/cli (cargo-llvm-cov) =="
	cargo llvm-cov -p takt-dash-cli --ignore-filename-regex '(^|/)(target|tests)/|(^|/)tests\.rs$$|_tests\.rs$$|/\.cargo/registry/|/rustc/'
	@echo "== frontend/web (c8) =="
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	cd frontend/web && npx --yes c8@12.0.0 --all --src . \
		--include 'app/**/*.{ts,tsx}' --include 'src/**/*.{ts,tsx}' \
		--exclude 'src/proto/**' --exclude '**/*.test.ts' --exclude 'test/**' --exclude '**/*.d.ts' \
		--reporter text --extension .ts --extension .tsx npm run test
	@echo "== tools/repo-gate (coverage.py) =="
	mkdir -p target/coverage
	python3 -m coverage run --data-file=target/coverage/.coverage --include='tools/repo-gate/repo_gate.py,tools/repo-gate/gate_*.py' tools/repo-gate/repo_gate_test.py
	python3 -m coverage report --data-file=target/coverage/.coverage --show-missing

done:
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	$(MAKE) proto-check
	$(MAKE) fmt
	$(MAKE) clippy
	$(MAKE) test
	$(MAKE) typecheck
	$(MAKE) lint

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
