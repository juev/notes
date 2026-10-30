#!/usr/bin/env bash
# Installs an APK on an emulator, starts it and fails when the launch does not go well.
#
#   scripts/check-launch-android.sh <apk> <serial>
#
# Meant for the release build, which goes through R8: a class that R8 renamed or removed while
# JNA or WorkManager reach it by name shows up only at run time. The app must still be running
# after a while, must not have crashed, and the Rust core must have opened its database: the
# store is the first thing the main screen reads through JNA.
#
# The emulator needs a system image that allows `adb root` (google_apis, not google_play), since
# the data folder of a release build is not readable otherwise. The app is uninstalled first,
# so give it an emulator of its own.
set -euo pipefail

apk="${1:?usage: check-launch-android.sh <apk> <serial>}"
serial="${2:?usage: check-launch-android.sh <apk> <serial>}"
package=org.evsyukov.notes
adb=(adb -s "$serial")

# Right after boot adbd may still be restarting and refuse the call ("unable to connect for root:
# closed"): a few attempts before giving up.
for attempt in 1 2 3 4 5 6; do
    if out="$("${adb[@]}" root 2>&1)"; then break; fi
    if [ "$attempt" = 6 ]; then
        echo "launch: adb root failed: $out" >&2
        exit 1
    fi
    sleep 5
done
"${adb[@]}" wait-for-device
"${adb[@]}" uninstall "$package" >/dev/null 2>&1 || true
"${adb[@]}" install "$apk" >/dev/null
"${adb[@]}" logcat -b all -c
"${adb[@]}" shell am start -W -n "$package/.MainActivity" >/dev/null
sleep 15

fail() {
    echo "launch: $1. Log:" >&2
    "${adb[@]}" logcat -d -b crash >&2 || true
    "${adb[@]}" logcat -d -s AndroidRuntime:E >&2 || true
    exit 1
}

crashes="$("${adb[@]}" logcat -d -b crash)"
[ -z "$crashes" ] || fail "the app crashed"
"${adb[@]}" shell pidof "$package" >/dev/null || fail "the app is not running 15 s after its start"
"${adb[@]}" shell test -s "/data/data/$package/files/notes/notes.sqlite" ||
    fail "the core did not open its database"
echo "launch: the app is running 15 s after its start and the core opened its database"
