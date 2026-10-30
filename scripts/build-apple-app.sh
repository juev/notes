#!/usr/bin/env bash
# Generates the Xcode project and builds Notes.app into apple/build.
# Usage: build-apple-app.sh [Debug|Release] [extra xcodebuild settings…]
# NOTES_XCODE_ACTION=test runs the tests of the scheme (apple/Tests) instead of
# only building it.
#
# The share extension reads the same database as the app through an App Group,
# and a group needs a signing team. The certificate is the one named by
# NOTES_SIGN_IDENTITY ("Apple Development" by default), the team is taken from
# NOTES_TEAM_ID or from that certificate. Without one, or with
# NOTES_SIGN_IDENTITY=-, the app is signed ad hoc: it works, the share
# extension does not. As in Lists.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
config="${1:-Debug}"
shift || true

identity="${NOTES_SIGN_IDENTITY:-Apple Development}"
if [ "$identity" != "-" ] && [ -z "${NOTES_TEAM_ID:-}" ]; then
    NOTES_TEAM_ID="$(security find-certificate -c "$identity" -p 2>/dev/null \
        | openssl x509 -noout -subject 2>/dev/null \
        | sed -n 's/.*OU *= *\([A-Z0-9]\{10\}\).*/\1/p' || true)"
fi
if [ "$identity" != "-" ] && [ -n "${NOTES_TEAM_ID:-}" ]; then
    export NOTES_TEAM_ID NOTES_SIGN_IDENTITY="$identity"
else
    echo "apple: no signing team found, building with ad hoc signing" >&2
    export NOTES_TEAM_ID="" NOTES_SIGN_IDENTITY="-"
fi

"$root/scripts/build-apple.sh"
(cd "$root/apple" && xcodegen generate --quiet)
xcodebuild -project "$root/apple/Notes.xcodeproj" -scheme Notes -configuration "$config" \
    -derivedDataPath "$root/apple/build" -quiet "$@" \
    "${NOTES_XCODE_ACTION:-build}"
echo "apple: $root/apple/build/Build/Products/$config/Notes.app"
