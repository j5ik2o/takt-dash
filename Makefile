.PHONY: fmt clippy test typecheck lint proto-gen proto-check done

fmt:
	cargo +nightly fmt --all -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace
	npm --prefix frontend/web run test

typecheck:
	npm --prefix frontend/web run typecheck

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

done:
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	$(MAKE) proto-check
	$(MAKE) fmt
	$(MAKE) clippy
	$(MAKE) test
	$(MAKE) typecheck
	$(MAKE) lint
