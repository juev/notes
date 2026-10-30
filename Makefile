# Notes: one core, apps on top of it. See README.md.

JAVA_HOME ?= /opt/homebrew/opt/openjdk@21
ANDROID_HOME ?= $(HOME)/Library/Android/sdk
export JAVA_HOME ANDROID_HOME

.PHONY: test lint deny fmt apple apple-run apple-test apple-lint android android-test android-lint android-install

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

# The Android app (android/): the core, its Kotlin bindings and the debug APK.
android:
	./scripts/build-android.sh
	cd android && ./gradlew --quiet assembleDebug

# The tests in android/app/src/test run on the JVM, without a device.
android-test:
	./scripts/build-android.sh
	cd android && ./gradlew --quiet testDebugUnitTest

# ktlint with the settings in .editorconfig; the generated bindings are left out.
android-lint:
	cd android && ktlint 'app/src/**/*.kt' '!app/src/main/java/uniffi/**'

# The release build for a phone connected over adb.
android-install:
	NOTES_ANDROID_ABIS=arm64-v8a ./scripts/build-android.sh
	cd android && ./gradlew --quiet assembleRelease
	$(ANDROID_HOME)/platform-tools/adb install -r android/app/build/outputs/apk/release/app-release.apk
