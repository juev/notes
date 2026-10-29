# Notes: one core, apps on top of it. See README.md.

.PHONY: test lint deny fmt

# --locked: a lockfile that no longer matches Cargo.toml is an error here
# instead of being rewritten on the way.
test:
	cd core && cargo test --locked

lint:
	cd core && cargo fmt --check && cargo clippy --locked --all-targets --all-features -- -D warnings

# Advisories, licenses and sources of the dependencies (deny.toml).
deny:
	cargo deny --manifest-path core/Cargo.toml --config deny.toml check

fmt:
	cd core && cargo fmt
