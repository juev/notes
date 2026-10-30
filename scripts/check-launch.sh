#!/usr/bin/env bash
# Starts a built Notes.app and fails when the launch does not go well.
#
#   scripts/check-launch.sh <path to Notes.app> debug     # a Debug build, as `make apple` makes
#   scripts/check-launch.sh <path to Notes.app> release   # a Release build
#
# debug: the app is started with the launch probe (NOTES_DEBUG_LAUNCH_PROBE): a launch that runs
# the event loop while the model is built stops the app. The run has a data folder of its own.
#
# release: the app is started as it is and must still be running after a while. A Release build
# has no debug script and no probe, and it uses the data folder of the account it runs under:
# meant for a build machine, not for a Mac with Notes in use. As in Lists.
set -euo pipefail

app="${1:?usage: check-launch.sh <Notes.app> debug|release}"
mode="${2:?usage: check-launch.sh <Notes.app> debug|release}"
app="$(cd "$app" && pwd -P)"
binary="$app/Contents/MacOS/Notes"
[ -x "$binary" ] || { echo "launch: no app at $app" >&2; exit 2; }

work="$(mktemp -d "${TMPDIR:-/tmp}/notes-launch-XXXXXX")"
log="$work/out.log"

running() { pgrep -f "^$binary\$" >/dev/null; }

case "$mode" in
debug)
    trap 'rm -rf "$work"' EXIT
    mkdir -p "$work/data"
    open -g -j -n --env "NOTES_DEBUG_DIR=$work/data" --env NOTES_DEBUG_QUIET=1 --env NOTES_DEBUG_LAUNCH_PROBE=1 \
        --env "NOTES_DEBUG_SCRIPT=apphidden;quit" --stdout "$log" --stderr "$work/err.log" "$app"
    waited=0
    while [ "$waited" -lt 60 ]; do
        sleep 1
        waited=$((waited + 1))
        if [ "$waited" -gt 2 ] && ! running; then break; fi
    done
    if running; then
        echo "launch: the app did not quit within a minute" >&2
        exit 1
    fi
    if grep -q "debug: launch probe ran after the model was built" "$log" && grep -q "debug: app windows" "$log"; then
        echo "launch: the probe passed"
    else
        echo "launch: the app stopped at launch, or the probe did not run. Output:" >&2
        cat "$log" "$work/err.log" >&2 || true
        exit 1
    fi
    ;;
release)
    trap 'rm -rf "$work"' EXIT
    open -g -j -n --stdout "$log" --stderr "$work/err.log" "$app"
    sleep 15
    if running; then
        echo "launch: the app is running 15 s after its start"
        osascript -e 'tell application id "org.evsyukov.notes" to quit' >/dev/null 2>&1 || true
    else
        echo "launch: the app is not running 15 s after its start. Output:" >&2
        cat "$log" "$work/err.log" >&2 || true
        exit 1
    fi
    ;;
*)
    echo "launch: the mode is debug or release" >&2
    exit 2
    ;;
esac
