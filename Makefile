# Notes: one core, apps on top of it. See README.md.

.PHONY: test lint deny fmt apple apple-run apple-test apple-lint

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

# The macOS app (apple/): the core, its Swift bindings, the Xcode project and the build.
apple:
	./scripts/build-apple-app.sh Debug

apple-run: apple
	open apple/build/Build/Products/Debug/Notes.app

# The tests in apple/Tests run without starting the app.
apple-test:
	NOTES_XCODE_ACTION=test ./scripts/build-apple-app.sh Debug

# SwiftLint with the settings in apple/.swiftlint.yml.
apple-lint:
	cd apple && swiftlint lint --strict --quiet
