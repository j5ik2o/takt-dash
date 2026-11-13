.PHONY: fmt clippy test typecheck lint done

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

done:
	test -d frontend/web/node_modules || npm --prefix frontend/web ci
	$(MAKE) fmt
	$(MAKE) clippy
	$(MAKE) test
	$(MAKE) typecheck
	$(MAKE) lint
