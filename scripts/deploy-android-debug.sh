#!/usr/bin/env bash
#
# One-liner: build the arm64 FamiRust libretro core from the current working
# tree, drop it into the TrophyHubAndroid debug app's jniLibs, and assemble the
# debug APK (optionally install it to an attached device).
#
#   scripts/deploy-android-debug.sh            # build core -> jniLibs -> assembleDebug
#   scripts/deploy-android-debug.sh install    # ... then adb install -r the APK
#
# Why this exists: the core (FamiRust) and the app (TrophyHubAndroid) are
# separate repos in the TrophyHub umbrella, so getting a core change onto a device
# is a fixed three-step dance. This pins it so no one rediscovers it. Only
# arm64-v8a ships (see the app's abiFilters); the two devices are arm64.
#
# FamiRust owns nes AND fds on Android (Nestopia is retired), so a bad core here
# takes both platforms with it -- run `cargo test` before deploying.
#
# Save states: the state format is versioned (nes_core::STATE_VERSION) and a core
# only accepts states at or below its own version. When a change bumps it, the
# desktop core (TrophyHubDesktop/runtime-deps/cores_windows/nescore_libretro.dll)
# has to be rebuilt in the same pass or the phone and the desktop stop agreeing.
# The version each build reports is its libretro library_version -- read it in
# the app's core-info line, or `strings` the .so.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID="$(cd "$REPO/../TrophyHubAndroid" && pwd)"
TRIPLE="aarch64-linux-android"
ABI="arm64-v8a"
SO="libnescore_libretro.so"
# FamiRust is a standalone workspace (open-source), so its target dir is local --
# not the umbrella's shared TrophyHub/target. The umbrella's .cargo/config.toml
# still supplies the NDK linker + the 16 KB max-page-size link arg.
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target}"

echo "[1/3] cargo build --release -p nes-libretro --target $TRIPLE"
( cd "$REPO" && cargo build --release -p nes-libretro --target "$TRIPLE" )

SRC="$TARGET_DIR/$TRIPLE/release/$SO"
DST="$ANDROID/app/src/main/jniLibs/$ABI/$SO"
[ -f "$SRC" ] || { echo "ERROR: built core not found at $SRC" >&2; exit 1; }
echo "[2/3] cp core -> $DST"
cp "$SRC" "$DST"
# Keep the checked-in-looking copy under out/release in step with what shipped,
# so "what is on the device" can be answered from this repo alone.
mkdir -p "$REPO/out/release"
cp "$SRC" "$REPO/out/release/libnescore_libretro.android-arm64.so"

echo "[3/3] ./gradlew :app:assembleDebug"
( cd "$ANDROID" && ./gradlew :app:assembleDebug )

APK="$ANDROID/app/build/outputs/apk/debug/app-debug.apk"
echo "APK: $APK"

# Where the built APK is dropped for a phone or tablet to pull without a cable
# (Drive for Desktop syncs it up).
#
# NOT the shared slot, on purpose. `TrophyHub-debug.apk` in that folder is the
# owner's build, and it has `TrophyHub-debug-BUILD-NOTES.txt` beside it naming
# that build's md5 and commit and telling him what to smoke. Copying over the
# APK does not update the notes, so the folder goes on describing a file that
# is no longer there, and the mismatch is silent. That happened on 2026-09-28:
# another repo replaced the owner's APK at 13:32 and the notes disagreed with
# the file for two hours, md5 included. This script used to default into the
# same slot and would have done the same thing.
#
# So this repo writes its own file, and writes a note beside it saying what it
# is. A 237 MB APK nobody can identify is the same problem one step along.
# TROPHYHUB_DEBUG_APK still overrides, and if a FamiRust build should go in
# front of the owner in the shared slot, it wants cutting WITH matching notes
# rather than copied on top of his.
DRIVE_DIR="${TROPHYHUB_DRIVE_DIR:-/g/My Drive/Trophy Hub}"
DRIVE_SLOT="${TROPHYHUB_DEBUG_APK:-$DRIVE_DIR/TrophyHub-debug-famirust.apk}"
if [ -d "$(dirname "$DRIVE_SLOT")" ]; then
  cp "$APK" "$DRIVE_SLOT"
  echo "Drive: $DRIVE_SLOT (replaced)"

  # A note beside it, so the file is self-describing. Same reasoning as the
  # owner's own notes file: the point of the hash is that somebody can check
  # the thing in the folder is the thing being described.
  NOTES="${DRIVE_SLOT%.apk}-NOTES.txt"
  {
    echo "FamiRust core smoke build, $(date '+%Y-%m-%d %H:%M')"
    echo "================================================"
    echo
    echo "Built by the FamiRust repo, NOT the owner's main debug slot."
    echo "The app code is whatever TrophyHubAndroid had checked out at build time;"
    echo "the only thing this build is FOR is the NES/FDS core inside it."
    echo
    echo "FamiRust HEAD   $(cd "$REPO" && git rev-parse --short HEAD) $(cd "$REPO" && git log -1 --format=%ad --date=short)"
    echo "core source at  $(cd "$REPO" && git log -1 --format='%h %ad' --date=short -- crates/nes-core/src crates/nes-libretro/src)"
    echo "TrophyHubAndroid $(cd "$ANDROID" && git rev-parse --short HEAD 2>/dev/null || echo '(unknown)')"
    echo "APK md5         $(md5sum "$APK" | cut -d' ' -f1)"
    echo "core .so md5    $(md5sum "$SRC" | cut -d' ' -f1)"
    echo "state version   $(strings "$SRC" 2>/dev/null | grep -m1 -E '^0\.[0-9]+\.[0-9]+$' || echo '(unread)')"
  } > "$NOTES"
  echo "Drive: $NOTES (written)"
else
  echo "note: Drive slot dir missing, skipped ($DRIVE_SLOT)"
fi

if [ "${1:-}" = "install" ]; then
  echo "adb install -r (device must be authorized for USB debugging)"
  adb install -r "$APK"
fi
